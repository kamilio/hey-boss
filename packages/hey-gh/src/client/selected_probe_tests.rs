use super::*;
use axum::response::IntoResponse;

struct Fixture {
    client: Client,
    calls: Arc<Mutex<Vec<(String, tokio::time::Instant)>>>,
    task: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    async fn new(changed: bool, remaining: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let live = Arc::new(AtomicBool::new(false));
        let reset = now_ms() / 1000 + 3600;
        let router = axum::Router::new().fallback({
            let calls = calls.clone();
            let live = live.clone();
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
                let calls = calls.clone();
                let live = live.clone();
                async move {
                    calls
                        .lock()
                        .unwrap()
                        .push((uri.path().to_owned(), tokio::time::Instant::now()));
                    let mut response = if !changed && headers.contains_key("if-none-match") {
                        axum::http::StatusCode::NOT_MODIFIED.into_response()
                    } else {
                        axum::Json(serde_json::json!({"ok":true})).into_response()
                    };
                    let headers = response.headers_mut();
                    headers.insert("etag", "\"synthetic\"".parse().unwrap());
                    if live.load(Ordering::Relaxed) {
                        headers.insert("x-ratelimit-resource", "core".parse().unwrap());
                        headers.insert(
                            "x-ratelimit-remaining",
                            remaining.to_string().parse().unwrap(),
                        );
                        headers.insert("x-ratelimit-reset", reset.to_string().parse().unwrap());
                    }
                    response
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::with_token(
            Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::ZERO,
                queue_timeout: Duration::from_secs(5),
                ..Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        for path in ["first", "second", "third"] {
            client.get(path, Freshness::Revalidate).await.unwrap();
        }
        live.store(true, Ordering::Relaxed);
        client.get("seed", Freshness::Revalidate).await.unwrap();
        calls.lock().unwrap().clear();
        Self {
            client,
            calls,
            task,
            _dir: dir,
        }
    }

    async fn read(&self, path: &str) -> Result<Response> {
        INTERACTIVE_READ
            .scope(
                foreground_priority(),
                self.client.get(path, Freshness::Revalidate),
            )
            .await
    }
}

#[tokio::test]
async fn selected_foreground_validator_can_revalidate_before_its_soft_slot() {
    let f = Fixture::new(false, 5000).await;
    for path in ["first", "second", "third"] {
        let response = tokio::time::timeout(Duration::from_millis(300), f.read(path))
            .await
            .expect("First-in-line validator waited for soft pacing")
            .unwrap();
        assert!(matches!(response.source, Source::Revalidated));
    }
    assert_eq!(f.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn changed_selected_probe_cannot_renew_its_own_borrowing_allowance() {
    let f = Fixture::new(true, 5000).await;
    let start = tokio::time::Instant::now();
    tokio::time::timeout(Duration::from_millis(300), f.read("first"))
        .await
        .expect("Selected validator could not borrow its wait")
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), f.read("second"))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(150), f.read("third"))
            .await
            .is_err()
    );
    assert_eq!(
        f.calls.lock().unwrap().len(),
        1,
        "Changed probes kept borrowing without repaying"
    );
    f.read("second").await.unwrap();
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "Borrowed interval was not repaid"
    );
}

#[tokio::test]
async fn selected_probe_preserves_exhaustion_and_quota_reserves() {
    for remaining in [0, 100, 101] {
        let f = Fixture::new(false, remaining).await;
        let result = tokio::time::timeout(Duration::from_millis(100), f.read("first")).await;
        assert!(!matches!(result, Ok(Ok(_))));
        assert!(
            f.calls.lock().unwrap().is_empty(),
            "Selected probe bypassed a quota gate"
        );
    }
}
