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
                    if tag == "gate" || tag == "paced-gate" || tag == "held-optional" {
                        gate.entered.notify_one();
                        gate.release.notified().await;
                    }
                    if tag == "slow-paced-optional" {
                        tokio::time::sleep(Duration::from_millis(650)).await;
                    }
                    use axum::response::IntoResponse;
                    let mut response = axum::Json(json!({"data":{"tag":tag}})).into_response();
                    if tag.starts_with("paced-") || tag.starts_with("slow-paced-") {
                        let headers = response.headers_mut();
                        headers.insert("x-ratelimit-resource", "graphql".parse().unwrap());
                        headers.insert(
                            "x-ratelimit-remaining",
                            if tag == "slow-paced-long-seed" {
                                "1000"
                            } else if tag.starts_with("slow-paced-") {
                                "2500"
                            } else {
                                "5000"
                            }
                            .parse()
                            .unwrap(),
                        );
                        headers.insert(
                            "x-ratelimit-reset",
                            (now_ms() / 1000 + 3600).to_string().parse().unwrap(),
                        );
                    }
                    response
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

#[tokio::test]
async fn optional_selector_allows_a_bounded_response_after_a_late_dispatch() {
    for caller_bound in [false, true] {
        let f = Fixture::new().await;
        let gate = f.hold().await;
        let started = tokio::time::Instant::now();
        let client = f.client.clone();
        let optional = tokio::spawn(async move {
            let read =
                optional_selector_read(read(client, "slow-paced-optional", Freshness::Revalidate));
            if caller_bound {
                READ_DEADLINE
                    .scope(started + Duration::from_millis(1900), read)
                    .await
            } else {
                read.await
            }
        });
        f.queued(2).await;
        tokio::time::sleep_until(started + Duration::from_millis(1550)).await;
        f.gate.release.notify_one();
        gate.await.unwrap().unwrap();
        let result = optional.await.unwrap();
        if caller_bound {
            assert!(matches!(result, Err(Error::Deadline)));
            assert!(started.elapsed() < Duration::from_millis(2100));
        } else {
            assert!(
                result.is_ok(),
                "a dispatched shortcut lost its response: {result:?}"
            );
            assert!(started.elapsed() < Duration::from_millis(2800));
            assert_eq!(f.client.status().network_requests, 2);
        }
    }
}

#[tokio::test]
async fn optional_selector_response_allowance_is_bounded_and_releases_its_lane() {
    let f = Fixture::new().await;
    let gate = f.hold().await;
    let client = f.client.clone();
    let started = tokio::time::Instant::now();
    let optional = tokio::spawn(optional_selector_read(read(
        client,
        "held-optional",
        Freshness::Revalidate,
    )));
    f.queued(2).await;
    tokio::time::sleep_until(started + Duration::from_millis(1550)).await;
    f.gate.release.notify_one();
    gate.await.unwrap().unwrap();
    f.gate.entered.notified().await;
    assert!(matches!(optional.await.unwrap(), Err(Error::Deadline)));
    assert!(started.elapsed() >= Duration::from_millis(2450));
    assert!(started.elapsed() < Duration::from_millis(3100));
    tokio::time::timeout(Duration::from_millis(500), async {
        while f.client.status().outstanding_requests != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expired optional transport retained its lane");
    f.gate.release.notify_one();
}

#[tokio::test]
async fn late_optional_dispatch_keeps_each_coalesced_callers_deadline() {
    for caller_bound in [false, true] {
        let f = Fixture::new().await;
        let gate = f.hold().await;
        let client = f.client.clone();
        let started = tokio::time::Instant::now();
        let optional = tokio::spawn(async move {
            let read = optional_selector_read(read(client, "held-optional", Freshness::Revalidate));
            if caller_bound {
                READ_DEADLINE
                    .scope(started + Duration::from_millis(1800), read)
                    .await
            } else {
                read.await
            }
        });
        f.queued(2).await;
        let required = tokio::spawn(read(
            f.client.clone(),
            "held-optional",
            Freshness::Revalidate,
        ));
        tokio::time::timeout(Duration::from_secs(1), async {
            while f.client.status().coalesced_requests == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep_until(started + Duration::from_millis(1550)).await;
        f.gate.release.notify_one();
        gate.await.unwrap().unwrap();
        f.gate.entered.notified().await;
        assert!(matches!(optional.await.unwrap(), Err(Error::Deadline)));
        if caller_bound {
            assert!(started.elapsed() < Duration::from_millis(1950));
        } else {
            assert!(started.elapsed() >= Duration::from_millis(2450));
        }
        assert!(
            !required.is_finished(),
            "optional expiry cancelled its required peer"
        );
        f.gate.release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), required)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(f.client.status().network_requests, 2);
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

#[tokio::test]
async fn foreground_selector_gets_a_turn_before_a_paced_background_graphql_backlog() {
    let f = Fixture::new().await;
    let core = f.hold_core().await;
    let held = tokio::spawn(read(f.client.clone(), "paced-gate", Freshness::Revalidate));
    tokio::time::timeout(Duration::from_secs(1), f.gate.entered.notified())
        .await
        .unwrap();
    let mut background = Vec::new();
    for tag in [
        "paced-a", "paced-b", "paced-c", "paced-d", "paced-e", "paced-f",
    ] {
        let client = f.client.clone();
        background.push(tokio::spawn(async move {
            INTERACTIVE_READ
                .scope(
                    Arc::new(AtomicBool::new(false)),
                    client.graphql(
                        "query Fixture($tag:String!) { viewer { login } }",
                        json!({"tag":tag}),
                        Freshness::Revalidate,
                    ),
                )
                .await
        }));
    }
    f.queued(8).await;
    let optional = tokio::spawn(optional_selector_read(read(
        f.client.clone(),
        "paced-optional",
        Freshness::Revalidate,
    )));
    f.queued(9).await;
    f.gate.release.notify_one();
    held.await.unwrap().unwrap();
    let response = optional.await.unwrap();
    for task in background {
        task.await.unwrap().unwrap();
    }
    f.gate.core_release.notify_one();
    core.await.unwrap().unwrap();
    let calls = f.gate.calls.lock().unwrap();
    let position = calls.iter().position(|tag| tag == "paced-optional");
    assert!(
        response.is_ok() && position.is_some_and(|index| index <= 2),
        "A foreground selector must get its normal foreground turn, while required background work continues: response={response:?}, calls={calls:?}"
    );
    assert_eq!(calls.len(), 8);
}

#[tokio::test]
async fn optional_graphql_borrows_its_turn_only_for_a_congested_rest_fallback_and_repays_it() {
    for fallback in ["busy", "busy-long", "queued", "exhausted", "free"] {
        let f = Fixture::new().await;
        let core = match fallback {
            "busy" | "busy-long" => Some(f.hold_core().await),
            "queued" => {
                f.client
                    .get("core-reserve/1000", Freshness::Revalidate)
                    .await
                    .unwrap();
                let client = f.client.clone();
                let core = tokio::spawn(async move {
                    client.get("core-reserve/2000", Freshness::Revalidate).await
                });
                f.queued(1).await;
                Some(core)
            }
            "exhausted" => {
                f.client
                    .get("core-reserve/0", Freshness::Revalidate)
                    .await
                    .unwrap();
                None
            }
            _ => None,
        };
        read(
            f.client.clone(),
            if fallback == "busy-long" {
                "slow-paced-long-seed"
            } else {
                "slow-paced-seed"
            },
            Freshness::Revalidate,
        )
        .await
        .unwrap();
        let started = tokio::time::Instant::now();
        let response = optional_selector_read(read(
            f.client.clone(),
            "slow-paced-optional",
            Freshness::Revalidate,
        ))
        .await;
        if fallback == "free" {
            assert!(
                response.is_ok(),
                "an ordinarily paced dispatch must retain its bounded response allowance: {response:?}"
            );
            assert!(
                started.elapsed() >= Duration::from_millis(1900),
                "an available REST fallback must not borrow its paced turn"
            );
        } else {
            assert!(
                response.is_ok(),
                "{fallback}: dispatch must leave response time inside the two-second budget: {response:?}"
            );
            assert!(
                started.elapsed() < Duration::from_millis(1400),
                "{fallback}: shortcut waited for its paced slot"
            );
            read(f.client.clone(), "after-loan", Freshness::Revalidate)
                .await
                .unwrap();
            assert!(
                started.elapsed() >= Duration::from_millis(2700),
                "{fallback}: completing the optional read erased its pacing debt"
            );
        }
        f.gate.core_release.notify_one();
        if let Some(core) = core {
            core.await.unwrap().unwrap();
        }
    }
}
