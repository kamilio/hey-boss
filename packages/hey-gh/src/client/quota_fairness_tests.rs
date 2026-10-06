use super::*;
use axum::{extract::State, http::Uri};
use serde_json::json;
use tokio::sync::Notify;

#[derive(Default)]
struct Gate {
    entered: Notify,
    release: Notify,
    calls: Mutex<Vec<u64>>,
}

async fn read(client: Client, number: u64, app: bool, completion: bool, foreground: bool) {
    let url = client
        .rest_url(&format!("repos/acme/demo/pulls/{number}"))
        .unwrap()
        .to_string();
    let request = client.request_versioned(url, None, Freshness::Revalidate, None, app);
    INTERACTIVE_READ
        .scope(Arc::new(AtomicBool::new(foreground)), async {
            if completion {
                COMPLETION_VALIDATION.scope((), request).await
            } else {
                request.await
            }
            .unwrap();
        })
        .await;
}

#[tokio::test]
async fn completion_stream_cannot_monopolize_another_providers_dispatch_slots() {
    for cold_app in [false, true] {
        let gate = Arc::new(Gate::default());
        let router = axum::Router::new().fallback(|State(gate): State<Arc<Gate>>, uri: Uri| async move {
            let number: u64 = uri.path().rsplit('/').next().unwrap().parse().unwrap();
            gate.calls.lock().unwrap().push(number);
            if number == 1 {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            axum::Json(json!({"node_id":format!("PR_{number}"),"number":number,"state":"open","merged":false}))
        }).with_state(gate.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let app = crate::AppInstallation::new(
            "fixture".into(),
            42,
            vec!["acme/demo".into()],
            include_str!("../../tests/fixtures/github-app-test-key.pem"),
        )
        .unwrap();
        app.accept(&serde_json::to_vec(&json!({"token":"synthetic-installation-token","expires_at":"2099-01-01T00:00:00Z"})).unwrap()).unwrap();
        let client = Client::with_token(
            Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::from_millis(10),
                installation: Some(app),
                ..Config::default()
            },
            "synthetic-personal-token".into(),
        )
        .unwrap();
        let mut tasks = vec![tokio::spawn(read(client.clone(), 1, cold_app, false, true))];
        tokio::time::timeout(Duration::from_secs(2), gate.entered.notified())
            .await
            .unwrap();
        // Hold the shared core socket until both quotas have queued work.
        // The busy quota has both foreground and background completions, so
        // its own 3:1 alternation cannot provide fairness to the other quota.
        for number in 2..14 {
            tasks.push(tokio::spawn(read(
                client.clone(),
                number,
                !cold_app,
                true,
                number % 4 != 0,
            )));
        }
        tasks.push(tokio::spawn(read(
            client.clone(),
            99,
            cold_app,
            false,
            true,
        )));
        tokio::time::timeout(Duration::from_secs(2), async {
            while client.status().outstanding_requests != 14 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        gate.release.notify_one();
        for task in tasks {
            task.await.unwrap();
        }
        server.abort();
        let calls = gate.calls.lock().unwrap();
        let cold = calls.iter().position(|n| *n == 99).unwrap();
        assert!(
            cold <= 2,
            "A ready cold read must get a turn before the peer completion backlog drains (cold_app={cold_app}): {calls:?}"
        );
    }
}

#[tokio::test]
async fn detail_reads_cannot_spend_another_lanes_dispatch_turn() {
    // A provider's details keep making progress on their separate socket while
    // its cold CI waits behind the other provider's core stream.
    let (entered, mut entries) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let router = axum::Router::new().fallback({
        let release = release.clone();
        move |uri: Uri| {
            let entered = entered.clone();
            let release = release.clone();
            async move {
                if !uri.path().ends_with("/comments") {
                    let number: u64 = uri.path().rsplit('/').next().unwrap().parse().unwrap();
                    entered.send(number).unwrap();
                    if number != 99 {
                        release.acquire().await.unwrap().forget();
                    }
                }
                axum::Json(json!([]))
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let app = crate::AppInstallation::new(
        "fixture".into(),
        42,
        vec!["acme/demo".into()],
        include_str!("../../tests/fixtures/github-app-test-key.pem"),
    )
    .unwrap();
    app.accept(
        &serde_json::to_vec(
            &json!({"token":"synthetic-installation-token","expires_at":"2099-01-01T00:00:00Z"}),
        )
        .unwrap(),
    )
    .unwrap();
    let client = Client::with_token(
        Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: dir.path().join("cache.sqlite"),
            min_spacing: Duration::from_millis(10),
            installation: Some(app),
            ..Config::default()
        },
        "synthetic-personal-token".into(),
    )
    .unwrap();
    let mut tasks = vec![tokio::spawn(read(client.clone(), 1, true, true, true))];
    assert_eq!(entries.recv().await, Some(1));
    for number in 2..8 {
        tasks.push(tokio::spawn(read(client.clone(), number, true, true, true)));
    }
    tasks.push(tokio::spawn(read(client.clone(), 99, false, false, true)));
    tokio::time::timeout(Duration::from_secs(2), async {
        while client.status().outstanding_requests != 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut calls = vec![1];
    loop {
        let detail = format!("{url}repos/acme/demo/issues/{}/comments", calls.len());
        INTERACTIVE_READ
            .scope(
                Arc::new(AtomicBool::new(true)),
                client.request_versioned(detail, None, Freshness::Revalidate, None, false),
            )
            .await
            .unwrap();
        release.add_permits(1);
        let number = tokio::time::timeout(Duration::from_secs(2), entries.recv())
            .await
            .unwrap()
            .unwrap();
        calls.push(number);
        if number == 99 {
            break;
        }
    }
    release.add_permits(8);
    for task in tasks {
        task.await.unwrap();
    }
    server.abort();
    assert!(
        calls.len() <= 3,
        "Details must not erase a cold CI read's turn at its shared socket: {calls:?}"
    );
}
