use super::*;

#[tokio::test]
async fn daemon_read_budget_releases_orphaned_work_but_preserves_coalescers() {
    for surviving_caller in [false, true] {
        let h = Harness::new().await;
        let c = h.client();
        let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server =
            tokio::spawn(async move { axum::serve(listener, api.router()).await.unwrap() });
        let gate = tokio::spawn({
            let c = c.clone();
            async move { c.get("slow", Freshness::Revalidate).await }
        });
        until(|| h.calls().len() == 1).await;
        let read = tokio::spawn(async move {
            reqwest::Client::new()
                .get(format!("{origin}/v1/prs/acme/demo/7/metadata?refresh=true"))
                .header("x-hey-gh-read-timeout-ms", "100")
                .send()
                .await
                .unwrap()
        });
        until(|| c.status().outstanding_requests == 2).await;
        let survivor = surviving_caller.then(|| {
            tokio::spawn({
                let c = c.clone();
                async move { c.pull_request("acme/demo", 7, Freshness::Revalidate).await }
            })
        });
        if surviving_caller {
            until(|| c.status().coalesced_requests == 1).await;
        }
        let response = tokio::time::timeout(Duration::from_secs(1), read)
            .await
            .expect("daemon ignored the caller budget")
            .unwrap();
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(response.json::<Value>().await.unwrap()["code"], "deadline");
        h.mock.release.notify_one();
        gate.await.unwrap().unwrap();
        if let Some(survivor) = survivor {
            survivor.await.unwrap().unwrap();
        }
        until(|| c.status().outstanding_requests == 0).await;
        assert_eq!(
            h.calls()
                .iter()
                .filter(|call| call.path == "/repos/acme/demo/pulls/7")
                .count(),
            usize::from(surviving_caller)
        );
        server.abort();
    }
}

#[tokio::test]
async fn daemon_rejects_invalid_read_budgets_without_upstream_work() {
    let h = Harness::new().await;
    let api = hey_gh::api::Api::new(h.client()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, api.router()).await.unwrap() });
    for value in ["0", "-1", "invalid", "3600001", "18446744073709551616"] {
        let response = reqwest::Client::new()
            .get(format!("{origin}/v1/prs/acme/demo/7?refresh=true"))
            .header("x-hey-gh-read-timeout-ms", value)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{value}");
    }
    assert!(h.calls().is_empty());
    server.abort();
}

#[tokio::test]
async fn sdk_shrinks_one_budget_and_never_sends_after_expiry() {
    let budgets = Arc::new(Mutex::new(Vec::<u64>::new()));
    let captured = budgets.clone();
    let router=Router::new().route("/v1/prs/acme/demo/7/metadata",axum::routing::get(move |headers:HeaderMap| {
        let captured=captured.clone();
        async move {
            captured.lock().unwrap().push(headers["x-hey-gh-read-timeout-ms"].to_str().unwrap().parse().unwrap());
            axum::Json(json!({"data":{"number":7},"validated_at_ms":123,"fetched_at_ms":123,"source":"cache"}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sdk = hey_gh::ApiClient::new(
        format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap()
    .with_read_deadline(tokio::time::Instant::now() + Duration::from_secs(2));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    sdk.pull_request("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    sdk.clone()
        .with_read_deadline(tokio::time::Instant::now() + Duration::from_secs(10))
        .pull_request("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    let sent = budgets.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert!(sent[0] <= 2000 && sent[1] < sent[0]);
    assert!(matches!(
        sdk.with_read_deadline(tokio::time::Instant::now() - Duration::from_secs(1))
            .pull_request("acme/demo", 7, Freshness::CachedOnly)
            .await,
        Err(Error::Deadline)
    ));
    assert_eq!(budgets.lock().unwrap().len(), 2);
    server.abort();
}

#[tokio::test]
async fn active_request_keeps_its_attempt_when_a_longer_caller_joins() {
    let h = Harness::new().await;
    h.mode("issue72-stall-metadata");
    let c = h.client();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, api.router()).await.unwrap() });
    let short = tokio::spawn(async move {
        reqwest::Client::new()
            .get(format!("{origin}/v1/prs/acme/demo/7/metadata?refresh=true"))
            .header("x-hey-gh-read-timeout-ms", "100")
            .send()
            .await
            .unwrap()
    });
    until(|| h.calls().len() == 1).await;
    let survivor = tokio::spawn({
        let c = c.clone();
        async move { c.pull_request("acme/demo", 7, Freshness::Revalidate).await }
    });
    until(|| c.status().coalesced_requests == 1).await;
    assert_eq!(short.await.unwrap().status(), StatusCode::GATEWAY_TIMEOUT);
    h.mock.release.notify_waiters();
    tokio::time::timeout(Duration::from_millis(500), survivor)
        .await
        .expect("a shorter caller forced the surviving reader to retry its active request")
        .unwrap()
        .unwrap();
    assert_eq!(
        h.calls().len(),
        1,
        "the shared network attempt must not be restarted"
    );
    server.abort();
}
