use super::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Copy)]
enum Mode {
    Complete,
    Pages,
    DeniedEvents,
    MissingEvents,
}

fn connection(nodes: Value, next: Option<&str>) -> Value {
    json!({"nodes":nodes,"pageInfo":{"hasNextPage":next.is_some(),"endCursor":next}})
}

fn reply(mode: Mode, operation: &str, after: &Value) -> Value {
    let pages = matches!(mode, Mode::Pages);
    let first = after.is_null();
    let comment = |id| json!({"id":id,"body":"comment".repeat(40)});
    let thread = json!({"id":if first {"T1"} else {"T2"},"isResolved":false,"isOutdated":false,
        "comments":connection(json!([comment(if first {"C1"} else {"C3"})]), (pages && first).then_some("C-next"))});
    let threads = connection(json!([thread]), (pages && first).then_some("T-next"));
    let events = connection(
        json!([{"__typename":"ReviewRequestedEvent","id":if first {"E1"} else {"E2"},
        "createdAt":"2026-10-05T00:00:00Z","requestedReviewer":{"__typename":"User","login":"reviewer"}}]),
        (pages && first).then_some("E-next"),
    );
    if operation == "Comments" {
        return json!({"data":{"node":{"comments":connection(json!([comment("C2")]),None)}}});
    }
    let mut pr = serde_json::Map::new();
    if operation != "ReviewEvents" {
        pr.insert("reviewThreads".into(), threads);
    }
    if operation != "Threads"
        && !(operation == "ReviewActivity" && matches!(mode, Mode::MissingEvents))
    {
        pr.insert("timelineItems".into(), events);
    }
    let mut result = json!({"data":{"repository":{"pullRequest":pr}}});
    if matches!(mode, Mode::DeniedEvents) && operation != "Threads" {
        result["data"]["repository"]["pullRequest"]["timelineItems"] = Value::Null;
        result["errors"] = json!([{"type":"FORBIDDEN","message":"events denied"}]);
    }
    result
}

struct Fixture {
    client: Client,
    mode: Arc<Mutex<Mode>>,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new(mode: Mode, limit: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mode = Arc::new(Mutex::new(mode));
        let router = axum::Router::new().route(
            "/graphql",
            axum::routing::post({
                let requests = requests.clone();
                let mode = mode.clone();
                move |axum::Json(body): axum::Json<Value>| {
                    let requests = requests.clone();
                    let mode = mode.clone();
                    async move {
                        let query = body["query"].as_str().unwrap();
                        let operation = ["ReviewActivity", "ReviewEvents", "Comments", "Threads"]
                            .into_iter()
                            .find(|name| query.contains(&format!("query {name}(")))
                            .unwrap();
                        requests
                            .lock()
                            .unwrap()
                            .push((operation.into(), body["variables"].clone()));
                        axum::Json(reply(
                            *mode.lock().unwrap(),
                            operation,
                            &body["variables"]["after"],
                        ))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::with_token(
            crate::Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::ZERO,
                max_collection_bytes: limit,
                ..crate::Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        Self {
            client,
            mode,
            requests,
            server,
            _dir: dir,
        }
    }

    async fn collect(&self, freshness: Freshness) -> (Result<Vec<Value>>, Result<Vec<Value>>) {
        let first = review_activity::FirstPage::new(&self.client, "acme/repo", 1, freshness);
        tokio::join!(
            self.client
                .review_threads("acme/repo", 1, freshness, &first),
            self.client.review_events("acme/repo", 1, freshness, &first)
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn review_threads_and_events_share_one_first_page_request() {
    let f = Fixture::new(Mode::Complete, 1 << 20).await;
    let (threads, events) = f.collect(Freshness::Revalidate).await;
    assert_eq!(threads.unwrap()[0]["id"], "T1");
    assert_eq!(events.unwrap()[0]["id"], "E1");
    assert_eq!(f.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn shared_review_pages_preserve_each_sources_collection_limit_and_pagination() {
    let size = |operation, after| reply(Mode::Pages, operation, &after).to_string().len();
    let thread_bytes = size("Threads", Value::Null)
        + size("Threads", json!("T-next"))
        + size("Comments", json!("C-next"));
    let event_bytes = size("ReviewEvents", Value::Null) + size("ReviewEvents", json!("E-next"));
    let limit = thread_bytes.max(event_bytes);
    assert!(size("ReviewActivity", Value::Null) < limit);
    let f = Fixture::new(Mode::Pages, limit).await;
    let (threads, events) = f.collect(Freshness::Revalidate).await;
    let threads = threads.expect("unrelated event bytes consumed the threads' pagination budget");
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0]["comments"]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(events.unwrap().len(), 2);
    let requests = f.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    for (operation, cursor) in [
        ("Threads", "T-next"),
        ("Comments", "C-next"),
        ("ReviewEvents", "E-next"),
    ] {
        assert!(
            requests
                .iter()
                .any(|(op, v)| op == operation && v["after"] == cursor)
        );
    }
}

#[tokio::test]
async fn oversized_combined_pages_fall_back_without_relaxing_source_limits() {
    let limit = ["Threads", "ReviewEvents"]
        .into_iter()
        .map(|operation| {
            reply(Mode::Complete, operation, &Value::Null)
                .to_string()
                .len()
        })
        .max()
        .unwrap();
    assert!(
        reply(Mode::Complete, "ReviewActivity", &Value::Null)
            .to_string()
            .len()
            > limit
    );
    let f = Fixture::new(Mode::Complete, limit).await;
    let (threads, events) = f.collect(Freshness::Revalidate).await;
    assert!(threads.is_ok() && events.is_ok());
    assert_eq!(f.requests.lock().unwrap().len(), 3);
    *f.mode.lock().unwrap() = Mode::Pages;
    let (threads, _) = f.collect(Freshness::Revalidate).await;
    assert!(matches!(threads, Err(Error::Invalid(ref message)) if message.contains("collection")));
}

#[tokio::test]
async fn a_denied_review_source_does_not_hide_the_independent_source() {
    let f = Fixture::new(Mode::DeniedEvents, 1 << 20).await;
    let (threads, events) = f.collect(Freshness::Revalidate).await;
    assert_eq!(threads.unwrap()[0]["id"], "T1");
    assert!(matches!(
        events,
        Err(Error::GraphQL {
            access_denied: true,
            ..
        })
    ));
    assert_eq!(f.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn combined_denial_cannot_recover_from_older_successful_source_caches() {
    let f = Fixture::new(Mode::Complete, 1 << 20).await;
    for query in [THREAD_QUERY, REVIEW_EVENTS_QUERY] {
        f.client
            .graphql(
                query,
                json!({"owner":"acme","repo":"repo","number":1,"after":null}),
                Freshness::Revalidate,
            )
            .await
            .unwrap();
    }
    *f.mode.lock().unwrap() = Mode::DeniedEvents;
    let (threads, events) = f.collect(Freshness::MaxAge(Duration::from_secs(60))).await;
    assert_eq!(threads.unwrap()[0]["id"], "T1");
    assert!(
        matches!(
            events,
            Err(Error::GraphQL {
                access_denied: true,
                ..
            })
        ),
        "a new denial was hidden by the old successful event cache: {events:?}"
    );
    assert_eq!(f.requests.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn a_missing_combined_connection_uses_only_its_original_query() {
    let f = Fixture::new(Mode::MissingEvents, 1 << 20).await;
    let (threads, events) = f.collect(Freshness::Revalidate).await;
    assert_eq!(threads.unwrap()[0]["id"], "T1");
    assert_eq!(events.unwrap()[0]["id"], "E1");
    let requests = f.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|(op, _)| op.as_str())
            .collect::<Vec<_>>(),
        ["ReviewActivity", "ReviewEvents"]
    );
}

#[tokio::test]
async fn legacy_offline_review_pages_remain_available_without_new_requests() {
    let f = Fixture::new(Mode::Complete, 1 << 20).await;
    for query in [THREAD_QUERY, REVIEW_EVENTS_QUERY] {
        f.client
            .graphql(
                query,
                json!({"owner":"acme","repo":"repo","number":1,"after":null}),
                Freshness::Revalidate,
            )
            .await
            .unwrap();
    }
    let (threads, events) = f.collect(Freshness::CachedOnly).await;
    assert_eq!(threads.unwrap()[0]["id"], "T1");
    assert_eq!(events.unwrap()[0]["id"], "E1");
    assert_eq!(f.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn delayed_review_sources_share_only_their_own_collection() {
    let f = Fixture::new(Mode::Complete, 1 << 20).await;
    for (repository, number) in [
        ("acme/repo", 1),
        ("acme/repo", 1),
        ("acme/repo", 2),
        ("acme/other", 1),
    ] {
        let first =
            review_activity::FirstPage::new(&f.client, repository, number, Freshness::Revalidate);
        f.client
            .review_threads(repository, number, Freshness::Revalidate, &first)
            .await
            .unwrap();
        f.client
            .review_events(repository, number, Freshness::Revalidate, &first)
            .await
            .unwrap();
    }
    assert_eq!(f.requests.lock().unwrap().len(), 4);
    let (threads, events) = f.collect(Freshness::CachedOnly).await;
    assert!(threads.is_ok() && events.is_ok());
    assert_eq!(f.requests.lock().unwrap().len(), 4);
}
