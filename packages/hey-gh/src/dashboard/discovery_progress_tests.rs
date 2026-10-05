use super::*;
use axum::{Json, Router, extract::State, routing::post};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering as AtomicOrdering},
};

struct Server {
    calls: AtomicUsize,
    mode: AtomicUsize,
}

async fn page(State(state): State<Arc<Server>>, Json(body): Json<Value>) -> Json<Value> {
    state.calls.fetch_add(1, AtomicOrdering::SeqCst);
    if body["query"].as_str().unwrap().contains("Boundary") {
        return Json(
            json!({"data":{"viewer":{"pullRequests":{"totalCount":5,"nodes":[{"id":"PR_5"}]}}}}),
        );
    }
    let index = body["variables"]["after"]
        .as_str()
        .unwrap_or("0")
        .parse::<u64>()
        .unwrap();
    if index == 2 && state.mode.load(AtomicOrdering::SeqCst) == 1 {
        std::future::pending::<()>().await;
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    if index == 2 && state.mode.load(AtomicOrdering::SeqCst) == 2 {
        return Json(json!({"errors":[{"type":"FORBIDDEN","message":"access denied"}]}));
    }
    let number = index + 1;
    Json(json!({"data":{"viewer":{"pullRequests":{
        "totalCount":5,
        "nodes":[{"id":format!("PR_{number}"),"number":number,"state":"OPEN","repository":{"nameWithOwner":"acme/demo"}}],
        "pageInfo":{"hasNextPage":number<5,"endCursor":number.to_string()}
    }}}}))
}

struct Harness {
    _dir: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
    state: Arc<Server>,
    client: Client,
}

impl Harness {
    async fn new() -> Self {
        let state = Arc::new(Server {
            calls: AtomicUsize::new(0),
            mode: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/graphql", post(page))
            .with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            crate::Config {
                cache_path: dir.path().join("cache.sqlite"),
                rest_url: base.parse().unwrap(),
                graphql_url: format!("{base}graphql").parse().unwrap(),
                report_timeout: Duration::from_millis(250),
                queue_timeout: Duration::from_secs(2),
                ..crate::Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        Self {
            _dir: dir,
            task,
            state,
            client,
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort()
    }
}

#[tokio::test]
async fn background_scan_keeps_completed_pages_beyond_one_interactive_budget() {
    let h = Harness::new().await;
    let rows = h
        .client
        .refresh_background_discovery(Freshness::Revalidate)
        .await
        .expect("healthy background pages must not restart at the interactive deadline");
    assert_eq!(rows.len(), 5);
    assert_eq!(
        h.state.calls.load(AtomicOrdering::SeqCst),
        6,
        "each page is fetched once"
    );
    let cached = h.client.derived(DISCOVERY_CACHE).await.unwrap().unwrap();
    let clocks = cached.data["validatedAtByPr"].as_object().unwrap();
    let oldest = clocks.values().map(|v| v.as_u64().unwrap()).min().unwrap();
    let newest = clocks.values().map(|v| v.as_u64().unwrap()).max().unwrap();
    assert!(
        newest - oldest >= 250,
        "source clocks must retain the slow scan's age"
    );
    assert!(cached.data["validatedAtMs"].as_u64().unwrap() <= oldest);
    assert_eq!(
        h.client
            .all_my_open_pull_requests(Freshness::CachedOnly)
            .await
            .unwrap(),
        rows
    );
    assert_eq!(h.state.calls.load(AtomicOrdering::SeqCst), 6);
}

#[tokio::test]
async fn background_progress_does_not_remove_interactive_or_stalled_page_deadlines() {
    let h = Harness::new().await;
    let good = h
        .client
        .refresh_background_discovery(Freshness::Revalidate)
        .await
        .unwrap();
    for (background, mode) in [(false, 0), (true, 2), (true, 1)] {
        h.state.mode.store(mode, AtomicOrdering::SeqCst);
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            if background {
                h.client
                    .refresh_background_discovery(Freshness::Revalidate)
                    .await
            } else {
                h.client
                    .all_my_open_pull_requests(Freshness::Revalidate)
                    .await
            }
        })
        .await
        .expect("a stalled page must remain bounded");
        if mode == 2 {
            assert!(matches!(result, Err(Error::GraphQL { .. })), "{result:?}");
        } else {
            assert!(matches!(result, Err(Error::Deadline)));
        }
        let health = h.client.discovery_health().await.unwrap().unwrap();
        assert!(health.last_error.is_some());
        let before = h.state.calls.load(AtomicOrdering::SeqCst);
        assert_eq!(
            h.client
                .all_my_open_pull_requests(Freshness::CachedOnly)
                .await
                .unwrap(),
            good
        );
        assert_eq!(h.state.calls.load(AtomicOrdering::SeqCst), before);
    }
}
