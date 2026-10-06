use super::*;
use axum::response::IntoResponse;
use serde_json::json;

struct Fixture {
    client: Client,
    calls: Arc<Mutex<Vec<(String, tokio::time::Instant)>>>,
    release: Arc<tokio::sync::Notify>,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(remaining: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(tokio::sync::Notify::new());
        let reset = now_ms() / 1000 + 3600;
        let router = axum::Router::new().fallback({
            let calls = calls.clone();
            let release = release.clone();
            move |axum::Json(body): axum::Json<Value>| {
                let calls = calls.clone();
                let release = release.clone();
                async move {
                    let id = body["variables"]["id"].as_str().unwrap().to_owned();
                    calls
                        .lock()
                        .unwrap()
                        .push((id.clone(), tokio::time::Instant::now()));
                    if id == "hold" {
                        release.notified().await;
                    }
                    let mut response = axum::Json(json!({"data":{"id":id}})).into_response();
                    if id == "invalid-304" {
                        *response.status_mut() = axum::http::StatusCode::NOT_MODIFIED;
                    }
                    let headers = response.headers_mut();
                    headers.insert("x-ratelimit-resource", "graphql".parse().unwrap());
                    headers.insert(
                        "x-ratelimit-remaining",
                        remaining.to_string().parse().unwrap(),
                    );
                    headers.insert("x-ratelimit-reset", reset.to_string().parse().unwrap());
                    response
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::with_token(
            Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::ZERO,
                queue_timeout: Duration::from_secs(20),
                ..Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let fixture = Self {
            client,
            calls,
            release,
            server,
            _dir: dir,
        };
        fixture.read("seed", false).await.unwrap();
        fixture.calls.lock().unwrap().clear();
        fixture
    }

    async fn read(&self, id: &str, foreground: bool) -> Result<Response> {
        read(&self.client, id, foreground).await
    }

    async fn queued(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.client.status().outstanding_requests != count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

async fn read(client: &Client, id: &str, foreground: bool) -> Result<Response> {
    INTERACTIVE_READ
        .scope(
            Arc::new(AtomicBool::new(foreground)),
            client.graphql(
                "query($id: ID!) { node(id: $id) { id } }",
                json!({"id":id}),
                Freshness::Revalidate,
            ),
        )
        .await
}

#[tokio::test]
async fn foreground_graphql_can_use_its_reserved_soft_slot_early_once() {
    let f = Fixture::new(5000).await;
    let started = tokio::time::Instant::now();
    tokio::time::timeout(Duration::from_millis(300), f.read("first", true))
        .await
        .expect("foreground GraphQL waited for its soft quota slot")
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), f.read("cancelled", true))
            .await
            .is_err()
    );
    assert_eq!(
        f.calls.lock().unwrap().len(),
        1,
        "a second read borrowed unpaid capacity"
    );
    f.read("second", true).await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "cancelled wait erased borrowed debt"
    );
}

#[tokio::test]
async fn background_and_optional_graphql_keep_their_pacing() {
    let f = Fixture::new(5000).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), f.read("background", false))
            .await
            .is_err()
    );
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        optional_selector_read(f.read("optional", true)),
    )
    .await;
    assert!(!matches!(result, Ok(Ok(_))));
    assert!(f.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn foreground_graphql_keeps_exhaustion_and_reserve_gates() {
    for remaining in [0, 100, 101] {
        let f = Fixture::new(remaining).await;
        assert!(!matches!(
            tokio::time::timeout(Duration::from_millis(100), f.read("held", true)).await,
            Ok(Ok(_))
        ));
        assert!(f.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn borrowed_graphql_turns_do_not_skip_background_fairness() {
    let f = Fixture::new(5000).await;
    let mut reads = Vec::new();
    for (index, (id, foreground)) in [
        ("hold", true),
        ("first", true),
        ("second", true),
        ("third", true),
        ("background", false),
    ]
    .into_iter()
    .enumerate()
    {
        let client = f.client.clone();
        reads.push(tokio::spawn(
            async move { read(&client, id, foreground).await },
        ));
        f.queued(index + 1).await;
    }
    f.release.notify_one();
    for read in reads {
        read.await.unwrap().unwrap();
    }
    let calls = f.calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["hold", "first", "second", "background", "third"]
    );
}

#[tokio::test]
async fn cancelled_graphql_caller_cannot_erase_inflight_debt() {
    let f = Fixture::new(5000).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), f.read("hold", true))
            .await
            .is_err()
    );
    assert_eq!(
        f.calls.lock().unwrap().len(),
        1,
        "first request did not borrow"
    );
    f.release.notify_one();
    f.queued(0).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), f.read("next", true))
            .await
            .is_err()
    );
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_graphql_304_does_not_grant_free_borrowing() {
    let f = Fixture::new(5000).await;
    assert!(matches!(
        tokio::time::timeout(Duration::from_millis(300), f.read("invalid-304", true)).await,
        Ok(Err(Error::Invalid(_)))
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), f.read("next", true))
            .await
            .is_err()
    );
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}
