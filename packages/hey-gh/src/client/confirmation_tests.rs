use super::*;
use serde_json::json;

async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn personal_graphql_confirmations_take_completion_turns_without_starving_other_reads() {
    for coalesced in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(tokio::sync::Notify::new());
        let router = axum::Router::new().fallback({
            let calls = calls.clone();
            let release = release.clone();
            move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                let calls = calls.clone();
                let release = release.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer synthetic-token");
                    let id = body["variables"]["id"].as_str().unwrap().to_owned();
                    calls.lock().unwrap().push(id.clone());
                    if id == "gate" {
                        release.notified().await;
                    }
                    axum::Json(json!({"data":{"id":id}}))
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
                queue_timeout: Duration::from_secs(10),
                ..Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let read = |id: String, foreground: bool, completion: bool| {
            let client = client.clone();
            tokio::spawn(async move {
                let query = "query($id: ID!) { node(id: $id) { id } }";
                let future = INTERACTIVE_READ.scope(
                    Arc::new(AtomicBool::new(foreground)),
                    client.graphql(query, json!({"id":id}), Freshness::Revalidate),
                );
                if completion {
                    COMPLETION_VALIDATION.scope((), future).await
                } else {
                    future.await
                }
            })
        };
        let mut tasks = vec![read("gate".into(), true, false)];
        until(|| calls.lock().unwrap().len() == 1).await;
        for n in 0..6 {
            tasks.push(read(format!("ordinary-{n}"), true, false));
            until(|| client.status().outstanding_requests == tasks.len()).await;
        }
        tasks.push(read("background".into(), false, false));
        until(|| client.status().outstanding_requests == tasks.len()).await;
        if coalesced {
            tasks.push(read("confirmation-0".into(), false, false));
            until(|| client.status().outstanding_requests == tasks.len()).await;
        }
        for n in 0..2 {
            tasks.push(read(format!("confirmation-{n}"), true, true));
            until(|| {
                client.status().outstanding_requests == tasks.len() - usize::from(coalesced)
                    && client.status().coalesced_requests == u64::from(coalesced)
            })
            .await;
        }
        release.notify_one();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        server.abort();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 10, "coalescing must not duplicate requests");
        assert_eq!(
            &calls[..5],
            [
                "gate",
                "confirmation-0",
                "ordinary-0",
                "background",
                "confirmation-1"
            ],
            "coalesced={coalesced}: {calls:?}"
        );
    }
}

#[tokio::test]
async fn shorter_http_read_deadlines_reach_the_shared_scheduler() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    let router = axum::Router::new().fallback({
        let calls = calls.clone();
        let release = release.clone();
        move |uri: axum::http::Uri| {
            let calls = calls.clone();
            let release = release.clone();
            async move {
                let number = uri
                    .path()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                calls.lock().unwrap().push(number);
                if number == 1 {
                    release.notified().await;
                }
                if number == 2 {
                    tokio::time::sleep(Duration::from_millis(2500)).await;
                }
                axum::Json(json!({"number":number}))
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let github = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
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
    let api = crate::api::Api::new(client.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sdk = crate::ApiClient::new(
        format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let daemon = tokio::spawn(async move { axum::serve(listener, api.router()).await.unwrap() });
    let start = tokio::time::Instant::now();
    let read = |number, seconds| {
        let sdk = sdk
            .clone()
            .with_read_deadline(start + Duration::from_secs(seconds));
        tokio::spawn(async move {
            sdk.pull_request("acme/demo", number, Freshness::Revalidate)
                .await
        })
    };
    let mut tasks = vec![read(1, 25)];
    until(|| calls.lock().unwrap().len() == 1).await;
    for (number, seconds) in [(2, 20), (3, 21), (4, 2), (5, 4)] {
        tasks.push(read(number, seconds));
        until(|| client.status().outstanding_requests == tasks.len()).await;
    }
    // A longer HTTP coalescer cannot erase the short caller's urgency.
    tasks.push(read(4, 24));
    until(|| client.status().coalesced_requests == 1).await;
    release.notify_one();
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    daemon.abort();
    github.abort();
    assert!(results.iter().all(Result::is_ok), "{results:?}");
    assert_eq!(
        *calls.lock().unwrap(),
        [1, 4, 2, 5, 3],
        "Short deadlines must get bounded turns while FIFO still progresses"
    );
}

#[tokio::test]
async fn shorter_read_deadlines_get_bounded_turns_without_starving_older_reads() {
    for (completion, scenario) in [false, true].into_iter().flat_map(|completion| {
        ["normal", "extended", "cancelled", "cancelled_shared"].map(|s| (completion, s))
    }) {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(tokio::sync::Notify::new());
        let router = axum::Router::new().fallback({
            let calls = calls.clone();
            let release = release.clone();
            move |uri: axum::http::Uri| {
                let calls = calls.clone();
                let release = release.clone();
                async move {
                    let path = uri.path().to_owned();
                    calls.lock().unwrap().push(path.clone());
                    if path.ends_with("/1") {
                        release.notified().await;
                    }
                    if path.ends_with("/2") {
                        tokio::time::sleep(Duration::from_millis(2500)).await;
                    }
                    axum::Json(json!({"ok":true}))
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
                queue_timeout: Duration::from_secs(30),
                ..Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let start = tokio::time::Instant::now();
        let read = |number, seconds| {
            let client = client.clone();
            tokio::spawn(async move {
                let path = format!("repos/acme/demo/pulls/{number}");
                let read = READ_DEADLINE.scope(
                    start + Duration::from_secs(seconds),
                    INTERACTIVE_READ.scope(
                        foreground_priority(),
                        client.get(&path, Freshness::Revalidate),
                    ),
                );
                tokio::time::timeout_at(start + Duration::from_secs(seconds), async {
                    if completion {
                        COMPLETION_VALIDATION.scope((), read).await
                    } else {
                        read.await
                    }
                })
                .await
                .unwrap_or(Err(Error::Deadline))
            })
        };
        let mut tasks = vec![read(1, 25)];
        until(|| calls.lock().unwrap().len() == 1).await;
        // Long watcher lifetimes must not make both shorter CLI reads wait.
        // A stream of short reads must still yield every other turn to FIFO.
        for (number, seconds) in [(2, 20), (3, 21), (4, 2), (5, 4)] {
            tasks.push(read(number, seconds));
            until(|| client.status().outstanding_requests == tasks.len()).await;
        }
        if matches!(scenario, "extended" | "cancelled_shared") {
            // A long-lived coalescer must not erase the short caller's urgency.
            tasks.push(read(4, 24));
            until(|| client.status().coalesced_requests == 1).await;
        }
        if scenario.starts_with("cancelled") {
            let cancelled = tasks.remove(3);
            cancelled.abort();
            assert!(cancelled.await.unwrap_err().is_cancelled());
            until(|| client.status().outstanding_requests == tasks.len()).await;
        }
        release.notify_one();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        server.abort();
        let order: Vec<_> = calls
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.rsplit('/').next().unwrap().to_owned())
            .collect();
        let expected: &[&str] = match scenario {
            "cancelled_shared" => &["1", "5", "2", "3", "4"],
            "cancelled" => &["1", "5", "2", "3"],
            _ => &["1", "4", "2", "5", "3"],
        };
        assert_eq!(order, expected, "completion={completion}, {scenario}");
    }
}

#[tokio::test]
async fn borrowed_validation_slots_honor_short_deadlines_and_then_yield_to_fifo() {
    use axum::response::IntoResponse;
    for proven in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let live = Arc::new(AtomicBool::new(false));
        let release = Arc::new(tokio::sync::Notify::new());
        let reset = now_ms() / 1000 + 3600;
        let router = axum::Router::new().fallback({
            let calls = calls.clone();
            let live = live.clone();
            let release = release.clone();
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
                let calls = calls.clone();
                let live = live.clone();
                let release = release.clone();
                async move {
                    let number = uri
                        .path()
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap();
                    let live = live.load(Ordering::Relaxed);
                    if live {
                        calls.lock().unwrap().push(number);
                        if number == 1 {
                            release.notified().await;
                        }
                        if number == 2 {
                            tokio::time::sleep(Duration::from_millis(1100)).await;
                        }
                    }
                    let mut response = if headers.contains_key("if-none-match") {
                        axum::http::StatusCode::NOT_MODIFIED.into_response()
                    } else {
                        axum::Json(json!({"number":number})).into_response()
                    };
                    let headers = response.headers_mut();
                    headers.insert("etag", "\"synthetic\"".parse().unwrap());
                    if live {
                        headers.insert("x-ratelimit-resource", "core".parse().unwrap());
                        headers.insert("x-ratelimit-remaining", "1000".parse().unwrap());
                        headers.insert("x-ratelimit-reset", reset.to_string().parse().unwrap());
                    }
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
        for number in 2..=5 {
            for _ in 0..if proven { 2 } else { 1 } {
                client
                    .get(
                        &format!("repos/acme/demo/pulls/{number}"),
                        Freshness::Revalidate,
                    )
                    .await
                    .unwrap();
            }
        }
        if proven {
            client
                .get("repos/acme/demo/pulls/6", Freshness::Revalidate)
                .await
                .unwrap();
        }
        // Three foreground turns owe the next paced turn to the background.
        for number in [100, 101] {
            INTERACTIVE_READ
                .scope(
                    foreground_priority(),
                    client.get(
                        &format!("repos/acme/demo/pulls/{number}"),
                        Freshness::Revalidate,
                    ),
                )
                .await
                .unwrap();
        }
        live.store(true, Ordering::Relaxed);
        let start = tokio::time::Instant::now();
        let read = |number, seconds, foreground, completing| {
            let client = client.clone();
            tokio::spawn(async move {
                let path = format!("repos/acme/demo/pulls/{number}");
                let read = READ_DEADLINE.scope(
                    start + Duration::from_secs(seconds),
                    INTERACTIVE_READ.scope(
                        Arc::new(AtomicBool::new(foreground)),
                        client.get(&path, Freshness::Revalidate),
                    ),
                );
                if completing {
                    COMPLETION_VALIDATION.scope((), read).await
                } else {
                    read.await
                }
            })
        };
        let mut tasks = vec![read(1, 20, true, false)];
        until(|| calls.lock().unwrap().len() == 1).await;
        for (number, seconds, foreground, completing) in [
            (99, 20, false, false),
            (2, 15, true, !proven),
            (3, 16, true, !proven),
            (4, 1, true, !proven),
            (5, 3, true, !proven),
        ] {
            tasks.push(read(number, seconds, foreground, completing));
            until(|| client.status().outstanding_requests == tasks.len()).await;
            if number == 99 && proven {
                // Prefer the proven validators before this unproven completion,
                // but retain deadline/FIFO ordering within those validators.
                tasks.push(read(6, 15, true, true));
                until(|| client.status().outstanding_requests == tasks.len()).await;
            }
        }
        if proven {
            // A longer coalescing read must not erase the urgent borrower's
            // deadline, even though it extends the shared request's lifetime.
            tasks.push(read(4, 10, true, false));
            until(|| client.status().coalesced_requests == 1).await;
        }
        release.notify_one();
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap());
        }
        server.abort();
        assert!(
            results.iter().all(Result::is_ok),
            "proven={proven}: {results:?}"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            if proven {
                vec![1, 4, 2, 5, 3, 6, 99]
            } else {
                vec![1, 4, 2, 5, 3, 99]
            },
            "short deadlines must borrow first, then repay FIFO; proven={proven}"
        );
    }
}
