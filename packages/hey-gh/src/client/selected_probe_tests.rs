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

#[tokio::test]
async fn a_free_ordinary_validation_releases_its_reserved_slot_for_cold_work() {
    for changed in [false, true] {
        let f = Fixture::new(changed, 5000).await;
        // Background first validations take their ordinary paced turn. Only
        // the response tells us whether that reserved charge was actually free.
        f.client.get("first", Freshness::Revalidate).await.unwrap();
        let cold = tokio::time::timeout(Duration::from_millis(350), f.read("cold")).await;
        if changed {
            assert!(
                cold.is_err(),
                "a charged validation must retain its interval"
            );
        } else {
            assert!(
                matches!(cold, Ok(Ok(_))),
                "a confirmed 304 kept delaying cold work: {cold:?}"
            );
        }
    }
}

#[tokio::test]
async fn proven_validator_uses_borrowed_window_before_unproven_completion() {
    mixed_validation_window(false).await;
}

#[tokio::test]
async fn changed_proven_validator_still_repays_and_preserves_the_owed_turn() {
    mixed_validation_window(true).await;
}

async fn mixed_validation_window(proven_changes: bool) {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let live = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(tokio::sync::Notify::new());
    let reset = now_ms() / 1000 + 3600;
    let router = axum::Router::new().fallback({
        let calls = calls.clone();
        let live = live.clone();
        let gate = gate.clone();
        move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
            let calls = calls.clone();
            let live = live.clone();
            let gate = gate.clone();
            async move {
                calls
                    .lock()
                    .unwrap()
                    .push((uri.path().to_owned(), tokio::time::Instant::now()));
                if uri.path() == "/gate" {
                    gate.notified().await;
                }
                let live = live.load(Ordering::Relaxed);
                let changed = live && (uri.path().ends_with("/pulls/1") || proven_changes);
                let mut response = if !changed && headers.contains_key("if-none-match") {
                    axum::http::StatusCode::NOT_MODIFIED.into_response()
                } else {
                    axum::Json(serde_json::json!({"ok":true})).into_response()
                };
                let headers = response.headers_mut();
                headers.insert("etag", "\"synthetic\"".parse().unwrap());
                if live {
                    headers.insert("x-ratelimit-resource", "core".parse().unwrap());
                    headers.insert("x-ratelimit-remaining", "5000".parse().unwrap());
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
            min_spacing: Duration::from_millis(10),
            queue_timeout: Duration::from_secs(10),
            ..Config::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let fixture = Fixture {
        client,
        calls,
        task,
        _dir: dir,
    };
    let last_proven = if proven_changes { 2 } else { 4 };
    for (number, reads) in std::iter::once((1, 1)).chain((2..=last_proven).map(|n| (n, 2))) {
        for _ in 0..reads {
            fixture
                .client
                .get(
                    &format!("repos/acme/demo/pulls/{number}"),
                    Freshness::Revalidate,
                )
                .await
                .unwrap();
        }
    }
    // The third foreground turn owes the next ordinary turn to background.
    for path in ["burst/1", "burst/2"] {
        fixture.read(path).await.unwrap();
    }
    fixture.calls.lock().unwrap().clear();
    live.store(true, Ordering::Relaxed);
    let first = tokio::spawn({
        let client = fixture.client.clone();
        async move {
            INTERACTIVE_READ
                .scope(
                    foreground_priority(),
                    client.get("gate", Freshness::Revalidate),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.calls.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut tasks = Vec::new();
    for (number, foreground, completing) in [(99, false, false), (1, true, true)]
        .into_iter()
        .chain((2..=last_proven).map(|n| (n, true, false)))
    {
        let client = fixture.client.clone();
        tasks.push(tokio::spawn(async move {
            let path = format!("repos/acme/demo/pulls/{number}");
            let read = INTERACTIVE_READ.scope(
                Arc::new(AtomicBool::new(foreground)),
                client.get(&path, Freshness::Revalidate),
            );
            if completing {
                COMPLETION_VALIDATION.scope((), read).await
            } else {
                read.await
            }
        }));
        tokio::time::timeout(Duration::from_secs(3), async {
            while fixture.client.status().outstanding_requests != tasks.len() + 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    let released = tokio::time::Instant::now();
    gate.notify_one();
    first.await.unwrap().unwrap();
    let deadline = released + Duration::from_millis(300);
    let mut promptly = true;
    for mut proven in tasks.split_off(2) {
        match tokio::time::timeout_at(deadline, &mut proven).await {
            Ok(value) => {
                value.unwrap().unwrap();
            }
            Err(_) => {
                promptly = false;
                proven.await.unwrap().unwrap();
            }
        }
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(
        calls[1].0, "/repos/acme/demo/pulls/2",
        "an unproven probe consumed the borrowed window before a previously unchanged validator"
    );
    assert!(
        promptly,
        "known validator waited behind another probe's charged debt"
    );
    if !proven_changes {
        assert_eq!(calls[2].0, "/repos/acme/demo/pulls/3");
        assert_eq!(calls[3].0, "/repos/acme/demo/pulls/4");
    }
    if proven_changes {
        assert_eq!(
            calls[2].0, "/repos/acme/demo/pulls/99",
            "changed validator renewed borrowing before the owed turn"
        );
        assert!(
            calls[2].1.duration_since(released) >= Duration::from_secs(1),
            "changed probe's pacing debt was erased"
        );
    }
}
