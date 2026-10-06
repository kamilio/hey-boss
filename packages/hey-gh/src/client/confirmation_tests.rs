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
