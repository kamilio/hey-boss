use super::*;

async fn server(c: &Client) -> (hey_gh::ApiClient, JoinHandle<()>) {
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sdk = hey_gh::ApiClient::new(
        format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, api.router()).await.unwrap() });
    (sdk, server)
}

#[tokio::test]
async fn foreground_metadata_promotes_shared_work_without_duplicate_reads() {
    let h = Harness::new().await;
    let c = h.client();
    let client = c.clone();
    let gate = tokio::spawn(async move { client.get("slow", Freshness::Revalidate).await });
    until(|| h.calls().len() == 1).await;
    let mut queued = Vec::new();
    for n in 0..8 {
        let client = c.clone();
        queued.push(tokio::spawn(async move {
            client
                .get(&format!("queued/{n}"), Freshness::Revalidate)
                .await
        }));
    }
    until(|| c.status().outstanding_requests == 9).await;
    let client = c.clone();
    queued.push(tokio::spawn(async move {
        client
            .get("repos/acme/demo/pulls/7", Freshness::Revalidate)
            .await
    }));
    until(|| c.status().outstanding_requests == 10).await;
    let (sdk, server) = server(&c).await;
    let read = tokio::spawn(async move {
        sdk.background()
            .foreground()
            .pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    until(|| c.status().coalesced_requests == 1).await;
    h.mock.release.notify_one();
    gate.await.unwrap().unwrap();
    let observed = read.await.unwrap().unwrap();
    for job in queued {
        job.await.unwrap().unwrap();
    }
    let calls = h.calls();
    assert_eq!(
        calls[1].path, "/repos/acme/demo/pulls/7",
        "active watchers must not lose metadata priority at the HTTP boundary"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.path == "/repos/acme/demo/pulls/7")
            .count(),
        1
    );
    assert_eq!(observed.data["head"]["sha"], HEAD);
    assert!(
        calls
            .iter()
            .all(|call| call.token == "Bearer synthetic-token")
    );
    assert!(c.watches().await.unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn metadata_deadline_bounds_new_jobs_without_cancelling_longer_shared_reads() {
    for shared in [false, true] {
        let h = Harness::new().await;
        let c = Client::with_token(
            Config {
                queue_timeout: Duration::from_secs(60),
                request_timeout: Duration::from_secs(60),
                ..h.config()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let client = c.clone();
        let gate = tokio::spawn(async move { client.get("slow", Freshness::Revalidate).await });
        until(|| h.calls().len() == 1).await;
        let longer = shared.then(|| {
            let client = c.clone();
            tokio::spawn(async move {
                client
                    .get("repos/acme/demo/pulls/7", Freshness::Revalidate)
                    .await
            })
        });
        if shared {
            until(|| c.status().outstanding_requests == 2).await;
        }
        let (sdk, server) = server(&c).await;
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(18),
            sdk.foreground()
                .pull_request("acme/demo", 7, Freshness::Revalidate),
        )
        .await;
        assert!(matches!(result, Ok(Err(Error::Deadline))), "{result:?}");
        assert!(started.elapsed() >= Duration::from_secs(14));
        if !shared {
            until(|| c.status().outstanding_requests == 1).await;
        }
        assert_eq!(c.status().outstanding_requests, if shared { 2 } else { 1 });
        assert_eq!(h.calls().len(), 1, "expired metadata must not dispatch");
        h.mock.release.notify_one();
        gate.await.unwrap().unwrap();
        if let Some(longer) = longer {
            assert_eq!(longer.await.unwrap().unwrap().data["head"]["sha"], HEAD);
        }
        until(|| c.status().outstanding_requests == 0).await;
        assert_eq!(h.calls().len(), if shared { 2 } else { 1 });
        server.abort();
    }
}

#[tokio::test]
async fn foreground_metadata_deadline_releases_an_active_socket() {
    let h = Harness::new().await;
    h.mode("issue72-stall-metadata");
    let c = Client::with_token(
        Config {
            queue_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(60),
            ..h.config()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let (sdk, server) = server(&c).await;
    let result = tokio::time::timeout(
        Duration::from_secs(18),
        sdk.pull_request("acme/demo", 7, Freshness::Revalidate),
    )
    .await;
    assert!(matches!(result, Ok(Err(Error::Deadline))), "{result:?}");
    until(|| c.status().outstanding_requests == 0).await;
    assert_eq!(c.status().active_requests, 0);
    assert_eq!(h.calls().len(), 1, "expired metadata must not retry");
    h.mock.release.notify_waiters();
    server.abort();
}
