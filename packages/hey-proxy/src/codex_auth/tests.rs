use super::*;
use axum::{Json, Router, extract::State, routing::post};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) fn fixture(path: &Path, expires_at: u64) {
    save(
        path,
        &Tokens {
            access_token: "codex-oat-synthetic-old".into(),
            refresh_token: "codex-refresh-old".into(),
            refresh_error: None,
            expires_at,
            account_id: Some("acct_synthetic_123".into()),
        },
    )
    .unwrap();
}

#[test]
fn encrypted_store_and_jwt_account_metadata_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex.json");
    fixture(&path, now() + 3600);
    let content = fs::read_to_string(&path).unwrap();
    assert!(!content.contains("synthetic"));
    let loaded = load(&path).unwrap();
    assert_eq!(loaded.refresh_token, "codex-refresh-old");
    assert_eq!(loaded.account_id.as_deref(), Some("acct_synthetic_123"));

    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({
            "exp": 1890000000u64,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "org_workspace_42"
            }
        }))
        .unwrap(),
    );
    let jwt = format!("{header}.{payload}.sig");
    let (exp, account_id) = jwt_metadata(&jwt);
    assert_eq!(exp, Some(1890000000));
    assert_eq!(account_id.as_deref(), Some("org_workspace_42"));
}

#[test]
fn oauth_url_includes_codex_pkce_parameters() {
    let url = authorization_url(
        DEFAULT_ISSUER,
        "http://localhost:1455/auth/callback",
        "expected-state",
        "test-verifier",
    )
    .unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(params["client_id"], CLIENT_ID);
    assert_eq!(params["state"], "expected-state");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(
        params["code_challenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(b"test-verifier"))
    );
    assert_eq!(params["codex_cli_simplified_flow"], "true");
    assert_eq!(params["id_token_add_organizations"], "true");
    assert!(!url.as_str().contains("test-verifier"));
}

#[tokio::test]
async fn concurrent_refresh_rotates_once_and_preserves_account_id() {
    let count = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/oauth/token",
            post(
                |State(count): State<Arc<AtomicUsize>>, Json(body): Json<Value>| async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(body["refresh_token"], "codex-refresh-old");
                    assert_eq!(body["grant_type"], "refresh_token");
                    assert_eq!(body["client_id"], CLIENT_ID);
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    Json(json!({
                        "access_token": "codex-oat-synthetic-new",
                        "refresh_token": "codex-refresh-new",
                        "expires_in": 3600
                    }))
                },
            ),
        )
        .with_state(count.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/oauth/token", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex.json");
    fixture(&path, 1);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut jobs = Vec::new();
    for _ in 0..6 {
        let (path, url, client) = (path.clone(), url.clone(), client.clone());
        jobs.push(tokio::spawn(async move {
            TokenManager::default()
                .credentials(&path, &client, &url)
                .await
                .unwrap()
        }));
    }
    for job in jobs {
        let creds = job.await.unwrap();
        assert_eq!(creds.access_token, "codex-oat-synthetic-new");
        assert_eq!(creds.account_id.as_deref(), Some("acct_synthetic_123"));
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(load(&path).unwrap().refresh_token, "codex-refresh-new");
    server.abort();
}

async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/oauth/token", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, server)
}

fn fresh_response() -> Json<Value> {
    Json(
        json!({"access_token":"codex-oat-synthetic-new","refresh_token":"codex-refresh-new","expires_in":3600}),
    )
}

#[tokio::test]
async fn rejected_refresh_is_redacted_and_not_replayed_after_restart() {
    for (status, code, expected) in [
        (401, "refresh_token_expired", RefreshError::Expired),
        (401, "refresh_token_reused", RefreshError::Reused),
        (401, "refresh_token_invalidated", RefreshError::Revoked),
        (400, "invalid_grant", RefreshError::Invalid),
        (403, "invalid_client", RefreshError::Rejected),
        (401, "codex-refresh-old", RefreshError::Rejected),
    ] {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let app = Router::new().route("/oauth/token", post(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                (axum::http::StatusCode::from_u16(status).unwrap(),
                 Json(json!({"error":{"code":code,"message":"codex-refresh-old private server detail"}})))
            }
        }));
        let (url, server) = serve(app).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codex.json");
        fixture(&path, 1);
        for _ in 0..2 {
            let error = TokenManager::default()
                .credentials(&path, &reqwest::Client::new(), &url)
                .await
                .unwrap_err();
            assert_eq!(error.downcast_ref::<RefreshError>(), Some(&expected));
            assert!(!format!("{error:#?}").contains("codex-refresh-old"));
            assert!(!format!("{error:#}").contains("private server detail"));
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let tokens = load(&path).unwrap();
        assert_eq!(tokens.refresh_token, "codex-refresh-old");
        assert_eq!(tokens.refresh_error, Some(expected));
        server.abort();
    }
}

#[tokio::test]
async fn uncertain_refresh_is_not_replayed_and_new_login_recovers() {
    use axum::{body::Body, response::IntoResponse};
    for outcome in ["invalid_json", "lost_body", "server_error"] {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let (url, server) = serve(Router::new().route(
            "/oauth/token",
            post(move || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n > 0 {
                        return fresh_response().into_response();
                    }
                    match outcome {
                        "invalid_json" => "{\"access_token\":\"private".into_response(),
                        "lost_body" => axum::response::Response::new(Body::from_stream(
                            futures_util::stream::iter([
                                Ok::<_, std::io::Error>("{\"access_token\":\"private"),
                                Err(std::io::Error::other("synthetic lost response")),
                            ]),
                        )),
                        _ => (
                            axum::http::StatusCode::SERVICE_UNAVAILABLE,
                            "private server failure",
                        )
                            .into_response(),
                    }
                }
            }),
        ))
        .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codex.json");
        fixture(&path, 1);
        let manager = TokenManager::default();
        for manager in [&manager, &TokenManager::default()] {
            let error = manager
                .credentials(&path, &reqwest::Client::new(), &url)
                .await
                .unwrap_err();
            assert_eq!(
                error.downcast_ref::<RefreshError>(),
                Some(&RefreshError::OutcomeUnknown),
                "{outcome}: {error}"
            );
            assert!(!error.to_string().contains("private"));
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // A fresh login replaces the stored state and must work in the same process.
        fixture(&path, 1);
        assert_eq!(
            manager
                .credentials(&path, &reqwest::Client::new(), &url)
                .await
                .unwrap()
                .access_token,
            "codex-oat-synthetic-new"
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(load(&path).unwrap().refresh_error, None);
        server.abort();
    }
}

#[tokio::test]
async fn connection_failure_does_not_poison_refresh_across_restart() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let url = format!("http://{address}/oauth/token");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex.json");
    fixture(&path, 1);
    let error = TokenManager::default()
        .credentials(&path, &reqwest::Client::new(), &url)
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<RefreshError>(),
        Some(&RefreshError::Unavailable)
    );
    assert_eq!(load(&path).unwrap().refresh_error, None);
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/oauth/token", post(|| async { fresh_response() })),
        )
        .await
        .unwrap();
    });
    assert!(
        TokenManager::default()
            .credentials(&path, &reqwest::Client::new(), &url)
            .await
            .is_ok()
    );
    server.abort();
}

#[tokio::test]
async fn rate_limit_backs_off_without_requiring_new_login() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let (url, server) = serve(Router::new().route(
        "/oauth/token",
        post(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { axum::http::StatusCode::TOO_MANY_REQUESTS }
        }),
    ))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex.json");
    fixture(&path, 1);
    let manager = TokenManager::default();
    for _ in 0..2 {
        let error = manager
            .credentials(&path, &reqwest::Client::new(), &url)
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<RefreshError>(),
            Some(&RefreshError::Unavailable)
        );
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(load(&path).unwrap().refresh_error, None);
    server.abort();
}

#[tokio::test]
async fn pending_refresh_blocks_replay_after_process_exit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex.json");
    fixture(&path, 1);
    let mut tokens = load(&path).unwrap();
    tokens.refresh_error = Some(RefreshError::OutcomeUnknown);
    save(&path, &tokens).unwrap();
    let error = TokenManager::default()
        .credentials(
            &path,
            &reqwest::Client::new(),
            "http://127.0.0.1:1/oauth/token",
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<RefreshError>(),
        Some(&RefreshError::OutcomeUnknown)
    );
}

#[tokio::test]
async fn refresh_survives_caller_cancellation_and_waiters_reuse_result() {
    let called = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let pair = (called.clone(), release.clone());
    let (url, server) = serve(Router::new().route(
        "/oauth/token",
        post(move || {
            let (called, release) = pair.clone();
            async move {
                called.notify_one();
                release.notified().await;
                fresh_response()
            }
        }),
    ))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex.json");
    fixture(&path, 1);
    let (owned, endpoint) = (path.clone(), url.clone());
    let job = tokio::spawn(async move {
        TokenManager::default()
            .credentials(&owned, &reqwest::Client::new(), &endpoint)
            .await
    });
    called.notified().await;
    assert_eq!(
        load(&path).unwrap().refresh_error,
        Some(RefreshError::OutcomeUnknown)
    );
    job.abort();
    release.notify_one();
    let credentials = tokio::time::timeout(
        Duration::from_secs(2),
        TokenManager::default().credentials(&path, &reqwest::Client::new(), &url),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(credentials.access_token, "codex-oat-synthetic-new");
    assert_eq!(load(&path).unwrap().refresh_error, None);
    server.abort();
}
