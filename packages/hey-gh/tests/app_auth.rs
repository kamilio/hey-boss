use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
};
use hey_gh::{AppInstallation, Client, Config, Freshness};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
struct Mock(Arc<Mutex<Vec<(String, String, Value)>>>, Arc<Mutex<String>>);

async fn handler(
    State(mock): State<Mock>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let path = uri.path().to_owned();
    let token = headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    mock.0.lock().unwrap().push((path.clone(), token, body));
    let mode = mock.1.lock().unwrap().clone();
    if path == "/app/installations/42/access_tokens" {
        if mode == "mint-denied" {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(json!({"message":"synthetic-private-material"})),
            )
                .into_response();
        }
        if mode == "mint-limited" {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "30")],
                axum::Json(json!({"message":"rate limit"})),
            )
                .into_response();
        }
        let count = mock
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.0.contains("access_tokens"))
            .count();
        return (StatusCode::CREATED, axum::Json(
            json!({"token":format!("synthetic-installation-token-{count}"), "expires_at":"2099-01-01T00:00:00Z"}),
        ))
        .into_response();
    }
    if (path == "/user" && mode.is_empty())
        || (path.starts_with("/repos/") && mode == "installation-primary")
    {
        return (
            StatusCode::FORBIDDEN,
            [
                ("x-ratelimit-resource", "core"),
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "4070908800"),
            ],
            axum::Json(json!({"message":"API rate limit exceeded"})),
        )
            .into_response();
    }
    if path.starts_with("/repos/") {
        if mode == "secondary" {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "30")],
                axum::Json(json!({"message":"rate limit"})),
            )
                .into_response();
        }
        if mode == "unauthorized" || mode == "denied" {
            return (
                if mode == "unauthorized" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                },
                axum::Json(json!({"message":"access denied"})),
            )
                .into_response();
        }
    }
    axum::Json(json!({"ok":true})).into_response()
}

fn app() -> AppInstallation {
    AppInstallation::new(
        "test-client-id".into(),
        42,
        vec!["acme/demo".into()],
        include_str!("fixtures/github-app-test-key.pem"),
    )
    .unwrap()
}

#[tokio::test]
async fn installation_reads_have_separate_primary_quota_and_keep_user_discovery() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(handler).with_state(mock.clone()),
        )
        .into_future(),
    );
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        rest_url: base.parse().unwrap(),
        graphql_url: format!("{base}graphql").parse().unwrap(),
        cache_path: dir.path().join("cache.sqlite"),
        queue_timeout: Duration::from_secs(2),
        min_spacing: Duration::ZERO,
        installation: Some(app()),
        ..Config::default()
    };
    let client = Client::with_token(config.clone(), "synthetic-user-token".into()).unwrap();
    assert!(client.get("user", Freshness::Revalidate).await.is_err());
    client
        .get("repos/Acme/Demo/pulls/7", Freshness::Revalidate)
        .await
        .unwrap();
    client
        .get("repos/acme/demo/actions/runs", Freshness::Revalidate)
        .await
        .unwrap();
    client
        .graphql(
            "query { viewer { login } }",
            json!({}),
            Freshness::Revalidate,
        )
        .await
        .unwrap();
    assert!(
        client
            .get("repos/acme/other/pulls/7", Freshness::Revalidate)
            .await
            .is_err()
    );
    let calls = mock.0.lock().unwrap().clone();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.0.contains("access_tokens"))
            .count(),
        1
    );
    assert!(
        calls
            .iter()
            .filter(|c| c.0.starts_with("/repos/"))
            .all(|c| c.1 == "Bearer synthetic-installation-token-1")
    );
    assert_eq!(
        calls.iter().find(|c| c.0 == "/graphql").unwrap().1,
        "Bearer synthetic-user-token"
    );
    assert_eq!(
        calls
            .iter()
            .find(|c| c.0.contains("access_tokens"))
            .unwrap()
            .2["repositories"],
        json!(["demo"])
    );
    // A new provider has no token. Cached-only reads must not mint one, and
    // installation identity must survive process restarts/token renewal.
    let restarted = Client::with_token(
        Config {
            installation: Some(app()),
            ..config
        },
        "synthetic-user-token".into(),
    )
    .unwrap();
    restarted
        .get("repos/acme/demo/pulls/7", Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(mock.0.lock().unwrap().len(), calls.len());
    server.abort();
}

async fn fixture(
    mode: &str,
) -> (
    Mock,
    tempfile::TempDir,
    Config,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let mock = Mock::default();
    *mock.1.lock().unwrap() = mode.into();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(handler).with_state(mock.clone()),
        )
        .into_future(),
    );
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        rest_url: base.parse().unwrap(),
        graphql_url: format!("{base}graphql").parse().unwrap(),
        cache_path: dir.path().join("cache.sqlite"),
        min_spacing: Duration::ZERO,
        queue_timeout: Duration::from_secs(2),
        installation: Some(app()),
        ..Config::default()
    };
    (mock, dir, config, server)
}

#[tokio::test]
async fn installation_exhaustion_leaves_user_core_available() {
    let (mock, _dir, config, server) = fixture("installation-primary").await;
    let client = Client::with_token(config, "synthetic-user-token".into()).unwrap();
    assert!(
        client
            .get("repos/acme/demo/pulls/7", Freshness::Revalidate)
            .await
            .is_err()
    );
    client.get("user", Freshness::Revalidate).await.unwrap();
    assert_eq!(
        mock.0.lock().unwrap().last().unwrap().1,
        "Bearer synthetic-user-token"
    );
    server.abort();
}

#[tokio::test]
async fn classic_policy_and_unclassified_rest_reads_keep_the_user_permissions() {
    let (mock, _dir, config, server) = fixture("ok").await;
    let client = Client::with_token(config, "synthetic-user-token".into()).unwrap();
    for path in [
        "repos/acme/demo/branches/main/protection/required_status_checks",
        "repos/acme/demo/branches/main/protection",
        "repos/acme/demo/hooks",
    ] {
        client.get(path, Freshness::Revalidate).await.unwrap();
        assert_eq!(
            mock.0.lock().unwrap().last().unwrap().1,
            "Bearer synthetic-user-token",
            "{path} needs the user's existing permission set"
        );
    }
    assert_eq!(
        mock.0.lock().unwrap().len(),
        3,
        "administrative reads must not mint an installation token"
    );
    client
        .get("repos/acme/demo/rules/branches/main", Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(
        mock.0.lock().unwrap().last().unwrap().1,
        "Bearer synthetic-installation-token-1"
    );
    server.abort();
}

#[tokio::test]
async fn secondary_throttles_pause_both_credentials_including_token_exchange() {
    for mode in ["secondary", "mint-limited"] {
        let (mock, _dir, config, server) = fixture(mode).await;
        let client = Client::with_token(config, "synthetic-user-token".into()).unwrap();
        assert!(matches!(
            client
                .get("repos/acme/demo/pulls/7", Freshness::Revalidate)
                .await,
            Err(hey_gh::Error::RateLimited { .. })
        ));
        let calls = mock.0.lock().unwrap().len();
        assert!(matches!(
            client.get("user", Freshness::Revalidate).await,
            Err(hey_gh::Error::RateLimited { .. })
        ));
        assert_eq!(mock.0.lock().unwrap().len(), calls);
        server.abort();
    }
}

#[tokio::test]
async fn installation_denials_never_fall_back_to_user_and_auth_failures_are_bounded() {
    for (mode, exchanges) in [("denied", 1), ("unauthorized", 2), ("mint-denied", 1)] {
        let (mock, _dir, config, server) = fixture(mode).await;
        let client = Client::with_token(config, "synthetic-user-token".into()).unwrap();
        let error = client
            .get("repos/acme/demo/pulls/7", Freshness::Revalidate)
            .await
            .err()
            .unwrap();
        assert!(!error.to_string().contains("synthetic-private-material"));
        if mode == "mint-denied" {
            assert!(
                client
                    .get("repos/acme/demo/pulls/8", Freshness::Revalidate)
                    .await
                    .is_err()
            );
        }
        let calls = mock.0.lock().unwrap().clone();
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.0.contains("access_tokens"))
                .count(),
            exchanges
        );
        assert!(calls.iter().all(|c| c.1 != "Bearer synthetic-user-token"));
        server.abort();
    }
}

#[tokio::test]
async fn concurrent_reads_share_one_exchange_and_renewal_keeps_cached_evidence() {
    let (mock, _dir, config, server) = fixture("ok").await;
    let client = Client::with_token(config.clone(), "synthetic-user-token".into()).unwrap();
    let (one, two) = tokio::join!(
        client.get("repos/acme/demo/pulls/7", Freshness::Revalidate),
        client.get("repos/acme/demo/pulls/8", Freshness::Revalidate)
    );
    one.unwrap();
    two.unwrap();
    assert_eq!(
        mock.0
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.0.contains("access_tokens"))
            .count(),
        1
    );
    // A fresh provider renews credentials while sharing the stable disk scope.
    let renewed = Client::with_token(
        Config {
            installation: Some(app()),
            ..config.clone()
        },
        "synthetic-user-token".into(),
    )
    .unwrap();
    renewed
        .get("repos/acme/demo/pulls/7", Freshness::CachedOnly)
        .await
        .unwrap();
    renewed
        .get("repos/acme/demo/pulls/9", Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(
        mock.0.lock().unwrap().last().unwrap().1,
        "Bearer synthetic-installation-token-2"
    );
    client
        .get("repos/acme/demo/pulls/9", Freshness::CachedOnly)
        .await
        .unwrap();
    for path in [
        config.cache_path.clone(),
        config.cache_path.with_extension("sqlite-wal"),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            assert!(
                !bytes
                    .windows(b"synthetic-installation-token".len())
                    .any(|w| w == b"synthetic-installation-token")
            );
            assert!(
                !bytes
                    .windows(b"PRIVATE KEY".len())
                    .any(|w| w == b"PRIVATE KEY")
            );
        }
    }
    server.abort();
}
