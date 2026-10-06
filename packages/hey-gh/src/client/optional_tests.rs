use super::*;
use serde_json::{Value, json};
use tokio::sync::Notify;

#[derive(Default)]
struct Gate {
    entered: Notify,
    release: Notify,
    core_entered: Notify,
    core_release: Notify,
    calls: Mutex<Vec<String>>,
}

struct Fixture {
    client: Client,
    gate: Arc<Gate>,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(Gate::default());
        let router = axum::Router::new()
            .route(
                "/core-held",
                axum::routing::get(
                    |axum::extract::State(gate): axum::extract::State<Arc<Gate>>| async move {
                        gate.core_entered.notify_one();
                        gate.core_release.notified().await;
                        axum::Json(json!({"ok":true}))
                    },
                ),
            )
            .route(
                "/core-reserve/{remaining}",
                axum::routing::get(
                    |axum::extract::Path(remaining): axum::extract::Path<u64>| async move {
                        use axum::response::IntoResponse;
                        let mut response = axum::Json(json!({"ok":true})).into_response();
                        let headers = response.headers_mut();
                        headers.insert("x-ratelimit-resource", "core".parse().unwrap());
                        headers.insert(
                            "x-ratelimit-remaining",
                            remaining.to_string().parse().unwrap(),
                        );
                        headers.insert(
                            "x-ratelimit-reset",
                            (now_ms() / 1000 + 3600).to_string().parse().unwrap(),
                        );
                        response
                    },
                ),
            )
            .fallback(
                |axum::extract::State(gate): axum::extract::State<Arc<Gate>>,
                 axum::Json(body): axum::Json<Value>| async move {
                    let tag = body["variables"]["tag"].as_str().unwrap().to_owned();
                    gate.calls.lock().unwrap().push(tag.clone());
                    if tag == "gate" {
                        gate.entered.notify_one();
                        gate.release.notified().await;
                    }
                    axum::Json(json!({"data":{"tag":tag}}))
                },
            )
            .with_state(gate.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::with_token(
            Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::ZERO,
                ..Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        Self {
            client,
            gate,
            server,
            _dir: dir,
        }
    }

    async fn queued(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while self.client.status().outstanding_requests != count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn hold(&self) -> tokio::task::JoinHandle<Result<Response>> {
        let task = tokio::spawn(read(self.client.clone(), "gate", Freshness::Revalidate));
        tokio::time::timeout(Duration::from_secs(1), self.gate.entered.notified())
            .await
            .unwrap();
        task
    }

    async fn hold_core(&self) -> tokio::task::JoinHandle<Result<Response>> {
        let client = self.client.clone();
        let task =
            tokio::spawn(async move { client.get("core-held", Freshness::Revalidate).await });
        tokio::time::timeout(Duration::from_secs(1), self.gate.core_entered.notified())
            .await
            .unwrap();
        task
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn read(client: Client, tag: &'static str, freshness: Freshness) -> Result<Response> {
    INTERACTIVE_READ
        .scope(
            foreground_priority(),
            client.graphql(
                "query Fixture($tag:String!) { viewer { login } }",
                json!({"tag":tag}),
                freshness,
            ),
        )
        .await
}

#[tokio::test]
async fn optional_completion_yields_promptly_to_a_queued_required_graphql_read() {
    let f = Fixture::new().await;
    let held = f.hold().await;
    let required = tokio::spawn(read(f.client.clone(), "required", Freshness::Revalidate));
    f.queued(2).await;
    let optional = tokio::time::timeout(
        Duration::from_millis(500),
        optional_selector_read(COMPLETION_VALIDATION.scope(
            (),
            read(f.client.clone(), "optional", Freshness::Revalidate),
        )),
    )
    .await;
    f.gate.release.notify_one();
    held.await.unwrap().unwrap();
    required.await.unwrap().unwrap();
    assert!(
        matches!(optional, Ok(Err(Error::Deadline))),
        "optional shortcut must yield before its two-second deadline: {optional:?}"
    );
    assert_eq!(*f.gate.calls.lock().unwrap(), ["gate", "required"]);
}

#[tokio::test]
async fn a_required_coalescer_keeps_an_optional_graphql_job_required() {
    let f = Fixture::new().await;
    let held = f.hold().await;
    let optional = tokio::spawn(optional_selector_read(
        COMPLETION_VALIDATION.scope((), read(f.client.clone(), "shared", Freshness::Revalidate)),
    ));
    f.queued(2).await;
    let required = tokio::spawn(read(f.client.clone(), "shared", Freshness::Revalidate));
    tokio::time::timeout(Duration::from_secs(1), async {
        while f.client.status().coalesced_requests == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let other = tokio::spawn(read(f.client.clone(), "other", Freshness::Revalidate));
    f.queued(3).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !optional.is_finished(),
        "required coalescing must prevent optional-only deferral"
    );
    f.gate.release.notify_one();
    held.await.unwrap().unwrap();
    required.await.unwrap().unwrap();
    other.await.unwrap().unwrap();
    optional.await.unwrap().unwrap();
    let calls = f.gate.calls.lock().unwrap();
    assert_eq!(
        calls.iter().filter(|tag| tag.as_str() == "shared").count(),
        1
    );
}

#[tokio::test]
async fn optional_graphql_still_runs_when_uncontended_and_reuses_fresh_cache() {
    let f = Fixture::new().await;
    optional_selector_read(read(f.client.clone(), "optional", Freshness::Revalidate))
        .await
        .unwrap();
    let held = f.hold().await;
    let required = tokio::spawn(read(f.client.clone(), "required", Freshness::Revalidate));
    f.queued(2).await;
    let cached = optional_selector_read(read(
        f.client.clone(),
        "optional",
        Freshness::MaxAge(Duration::from_secs(30)),
    ))
    .await
    .unwrap();
    assert!(matches!(cached.source, Source::Cache));
    f.gate.release.notify_one();
    held.await.unwrap().unwrap();
    required.await.unwrap().unwrap();
    assert_eq!(
        *f.gate.calls.lock().unwrap(),
        ["optional", "gate", "required"]
    );
}

#[tokio::test]
async fn optional_graphql_keeps_its_route_when_rest_quota_cannot_accept_the_fallback() {
    for remaining in [0, 100] {
        let f = Fixture::new().await;
        f.client
            .get(&format!("core-reserve/{remaining}"), Freshness::Revalidate)
            .await
            .unwrap();
        let held = f.hold().await;
        let required = tokio::spawn(read(f.client.clone(), "required", Freshness::Revalidate));
        f.queued(2).await;
        let optional = tokio::spawn(optional_selector_read(COMPLETION_VALIDATION.scope(
            (),
            read(f.client.clone(), "optional", Freshness::Revalidate),
        )));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let premature = optional.is_finished();
        f.gate.release.notify_one();
        held.await.unwrap().unwrap();
        required.await.unwrap().unwrap();
        let response = optional.await.unwrap();
        assert!(
            !premature && response.is_ok(),
            "do not send a usable GraphQL read into exhausted REST quota: {response:?}"
        );
        assert!(
            f.gate
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|tag| tag == "optional")
        );
    }
}

#[tokio::test]
async fn optional_selector_waits_behind_required_graphql_when_rest_socket_is_busy() {
    let f = Fixture::new().await;
    let core = f.hold_core().await;
    let held = f.hold().await;
    let required = tokio::spawn(read(f.client.clone(), "required", Freshness::Revalidate));
    f.queued(3).await;
    let optional = tokio::spawn(optional_selector_read(COMPLETION_VALIDATION.scope(
        (),
        read(f.client.clone(), "optional", Freshness::Revalidate),
    )));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let premature = optional.is_finished();
    f.gate.release.notify_one();
    held.await.unwrap().unwrap();
    required.await.unwrap().unwrap();
    let response = optional.await.unwrap();
    f.gate.core_release.notify_one();
    core.await.unwrap().unwrap();
    assert!(
        !premature && response.is_ok(),
        "The GraphQL shortcut can complete before its busy REST fallback: {response:?}"
    );
    assert_eq!(
        *f.gate.calls.lock().unwrap(),
        ["gate", "required", "optional"]
    );
}

#[tokio::test]
async fn busy_rest_does_not_extend_the_optional_deadline_or_cancel_required_work() {
    let f = Fixture::new().await;
    let core = f.hold_core().await;
    let held = f.hold().await;
    let required = tokio::spawn(read(f.client.clone(), "required", Freshness::Revalidate));
    f.queued(3).await;
    let optional = tokio::time::timeout(
        Duration::from_secs(3),
        optional_selector_read(read(f.client.clone(), "optional", Freshness::Revalidate)),
    )
    .await
    .expect("busy fallback must not extend the shortcut's two-second budget");
    assert!(matches!(optional, Err(Error::Deadline)));
    assert!(!held.is_finished() && !required.is_finished() && !core.is_finished());
    f.queued(3).await;
    f.gate.release.notify_one();
    f.gate.core_release.notify_one();
    held.await.unwrap().unwrap();
    required.await.unwrap().unwrap();
    core.await.unwrap().unwrap();
    assert_eq!(*f.gate.calls.lock().unwrap(), ["gate", "required"]);
}

#[tokio::test]
async fn queued_paced_rest_work_keeps_the_graphql_shortcut_alive_with_a_free_socket() {
    let f = Fixture::new().await;
    f.client
        .get("core-reserve/1000", Freshness::Revalidate)
        .await
        .unwrap();
    let core = tokio::spawn({
        let client = f.client.clone();
        async move { client.get("core-reserve/2000", Freshness::Revalidate).await }
    });
    let held = f.hold().await;
    let required = tokio::spawn(read(f.client.clone(), "required", Freshness::Revalidate));
    f.queued(3).await;
    assert_eq!(f.client.status().active_requests, 1);
    let optional = tokio::spawn(optional_selector_read(COMPLETION_VALIDATION.scope(
        (),
        read(f.client.clone(), "optional", Freshness::Revalidate),
    )));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let premature = optional.is_finished();
    f.gate.release.notify_one();
    held.await.unwrap().unwrap();
    required.await.unwrap().unwrap();
    let response = optional.await.unwrap();
    let rest_still_waiting = !core.is_finished();
    core.await.unwrap().unwrap();
    assert!(
        !premature && rest_still_waiting && response.is_ok(),
        "A free socket does not make the paced REST backlog a faster fallback: {response:?}"
    );
    assert_eq!(
        *f.gate.calls.lock().unwrap(),
        ["gate", "required", "optional"]
    );
}
