use super::*;
use axum::{Json, Router, extract::State, routing::post};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) fn fixture(path: &Path, expires_at: u64) {
    save(
        path,
        &Tokens {
            access_token: "sk-ant-oat01-synthetic-old".into(),
            refresh_token: "synthetic-refresh-old".into(),
            expires_at,
        },
    )
    .unwrap();
}

#[test]
fn encrypted_store_is_private_authenticated_and_never_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claude.json");
    fixture(&path, now() + 3600);
    let content = fs::read_to_string(&path).unwrap();
    assert!(!content.contains("synthetic"));
    assert_eq!(load(&path).unwrap().refresh_token, "synthetic-refresh-old");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&path, &path.with_extension("key")] {
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        }
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(load(&link).is_err());
    }
    let mut value: Value = serde_json::from_str(&content).unwrap();
    let raw = value["encrypted"]
        .as_str()
        .unwrap()
        .strip_prefix("v1:")
        .unwrap();
    let mut bytes = STANDARD.decode(raw).unwrap();
    bytes[12] ^= 1;
    value["encrypted"] = json!(format!("v1:{}", STANDARD.encode(bytes)));
    atomic_write(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(load(&path).is_err());
}

#[test]
fn oauth_url_uses_pkce_profile_scope_and_random_state() {
    let url = authorization_url("expected-state", "test-verifier").unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(params["state"], "expected-state");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(
        params["code_challenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(b"test-verifier"))
    );
    assert!(
        params["scope"]
            .split_whitespace()
            .any(|s| s == "user:profile")
    );
    assert!(
        params["scope"]
            .split_whitespace()
            .any(|s| s == "user:inference")
    );
    assert!(!url.as_str().contains("test-verifier"));
}

#[tokio::test]
async fn oauth_callback_rejects_wrong_state_without_consuming_login() {
    let (sender, mut receiver) = tokio::sync::oneshot::channel();
    let state = Arc::new(CallbackState {
        state: "correct".into(),
        sender: Mutex::new(Some(sender)),
    });
    let (status, _) = callback(
        State(state.clone()),
        axum::extract::Query(Callback {
            state: Some("wrong".into()),
            code: Some("code".into()),
            error: None,
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(receiver.try_recv().is_err());
    let (status, _) = callback(
        State(state),
        axum::extract::Query(Callback {
            state: Some("correct".into()),
            code: Some("code".into()),
            error: None,
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(receiver.await.unwrap().unwrap(), "code");
}

async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    (
        url,
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
    )
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn concurrent_managers_rotate_once_and_persist_rotated_refresh_token() {
    let count = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route("/token", post(|State(count): State<Arc<AtomicUsize>>, Json(body): Json<Value>| async move {
        count.fetch_add(1, Ordering::SeqCst);
        assert_eq!(body["refresh_token"], "synthetic-refresh-old");
        assert_eq!(body["grant_type"], "refresh_token");
        tokio::time::sleep(Duration::from_millis(50)).await;
        Json(json!({"access_token":"sk-ant-oat01-synthetic-new","refresh_token":"synthetic-refresh-new","expires_in":3600}))
    })).with_state(count.clone());
    let (url, server) = serve(app).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claude.json");
    fixture(&path, 1);
    let mut jobs = Vec::new();
    for _ in 0..8 {
        let (path, url) = (path.clone(), url.clone());
        // Independent managers must also synchronize via the file lock.
        jobs.push(tokio::spawn(async move {
            TokenManager::default()
                .obtain(&path, &client(), None, &url)
                .await
                .unwrap()
        }));
    }
    for job in jobs {
        assert_eq!(job.await.unwrap(), "sk-ant-oat01-synthetic-new");
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(load(&path).unwrap().refresh_token, "synthetic-refresh-new");
    // Another process already refreshed the token rejected by an in-flight request.
    assert_eq!(
        TokenManager::default()
            .obtain(
                &path,
                &client(),
                Some("sk-ant-oat01-synthetic-old".into()),
                &url
            )
            .await
            .unwrap(),
        "sk-ant-oat01-synthetic-new"
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn refresh_survives_caller_cancellation() {
    let called = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let pair = (called.clone(), release.clone());
    let app = Router::new().route("/token", post(move || {
        let (called, release) = pair.clone();
        async move { called.notify_one(); release.notified().await; Json(json!({"access_token":"sk-ant-oat01-synthetic-new","refresh_token":"rotated","expires_in":3600})) }
    }));
    let (url, server) = serve(app).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claude.json");
    fixture(&path, 1);
    let owned = path.clone();
    let job = tokio::spawn(async move {
        TokenManager::default()
            .obtain(&owned, &client(), None, &url)
            .await
    });
    called.notified().await;
    job.abort();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if load(&path).unwrap().refresh_token == "rotated" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    server.abort();
}

#[tokio::test]
async fn refresh_errors_are_redacted_and_backed_off_without_damaging_store() {
    let count = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/token",
            post(|State(count): State<Arc<AtomicUsize>>| async move {
                count.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    "synthetic-refresh-old private server error",
                )
            }),
        )
        .with_state(count.clone());
    let (url, server) = serve(app).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claude.json");
    fixture(&path, 1);
    let manager = TokenManager::default();
    for _ in 0..2 {
        let error = manager
            .obtain(&path, &client(), None, &url)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("synthetic-refresh-old"));
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(load(&path).unwrap().refresh_token, "synthetic-refresh-old");
    server.abort();
}
