use super::*;

#[tokio::test]
async fn admitted_background_collections_finish_after_admission_closes() {
    drain(true).await;
}

#[tokio::test]
async fn explicit_refresh_keeps_its_original_total_deadline() {
    drain(false).await;
}

#[tokio::test]
async fn stopping_a_draining_watch_preserves_an_independent_coalesced_reader() {
    let h = Harness::new().await;
    h.mode("account-large-cycle-drain-cancel");
    h.phase(2);
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(2);
    config.queue_timeout = Duration::from_secs(5);
    config.request_timeout = Duration::from_secs(5);
    config.queue_capacity = 16;
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let started = tokio::time::Instant::now();
    api.watch_account(60).await.unwrap();
    until(|| {
        h.calls()
            .iter()
            .any(|call| call.path == "/repos/acme/watch00/pulls/7")
    })
    .await;
    let coalesced = c.status().coalesced_requests;
    let reader = tokio::spawn({
        let c = c.clone();
        async move {
            c.pull_request("acme/watch00", 7, Freshness::Revalidate)
                .await
        }
    });
    until(|| c.status().coalesced_requests > coalesced).await;
    tokio::time::sleep_until(started + Duration::from_millis(2200)).await;
    assert!(
        c.account_refresh_cycle(true).await.unwrap().is_none(),
        "the admitted collection was cancelled at the scan boundary"
    );
    api.stop().await;
    assert!(
        !reader.is_finished(),
        "watch cancellation stopped an independent reader"
    );
    h.mock.release.notify_waiters();
    let response = tokio::time::timeout(Duration::from_secs(2), reader)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.data["node_id"], "PR_acme/watch00_7");
    assert_eq!(
        h.calls()
            .iter()
            .filter(|call| call.path == "/repos/acme/watch00/pulls/7")
            .count(),
        1
    );
    assert!(
        !h.calls()
            .iter()
            .any(|call| call.path.starts_with("/repos/acme/watch01/"))
    );
}

async fn drain(background: bool) {
    let h = Harness::new().await;
    h.mode("account-large-cycle-drain");
    h.phase(2);
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(2);
    config.queue_timeout = Duration::from_secs(5);
    config.request_timeout = Duration::from_secs(5);
    // One collection at a time makes the second PR start late in the cycle.
    config.queue_capacity = 16;
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let before = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    if background {
        let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
        api.watch_account(60).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while c.account_refresh_cycle(true).await.unwrap().is_none()
                || c.account_refresh_cycle(false).await.unwrap().is_none()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        api.stop().await;
    } else {
        c.refresh_pr_status(Freshness::Revalidate, true)
            .await
            .unwrap();
    }
    let cycle = c.account_refresh_cycle(true).await.unwrap().unwrap();
    assert_eq!(cycle.total, 25);
    assert_eq!(
        cycle.attempted, 2,
        "admission swept past its deadline: {cycle:?}"
    );
    assert_eq!(cycle.succeeded, if background { 2 } else { 1 }, "{cycle:?}");
    assert_eq!(
        cycle.interrupted,
        if background { 0 } else { 1 },
        "{cycle:?}"
    );
    assert_eq!(cycle.failed, 0);
    assert_eq!(cycle.deferred, 23);
    assert!(cycle.cycle_budget_exhausted);
    assert!(cycle.finished_at_ms - cycle.started_at_ms < 4000);
    let after = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let untouched = |page: &hey_gh::PrStatusPage| {
        page.pull_requests
            .iter()
            .find(|row| row["repository"]["nameWithOwner"] == "acme/watch23")
            .unwrap()
            .clone()
    };
    assert_eq!(
        untouched(&before),
        untouched(&after),
        "deferred evidence changed"
    );
    assert!(
        !h.calls()
            .iter()
            .any(|call| call.path.starts_with("/repos/acme/watch01/"))
    );
    if background {
        let details = c.account_refresh_cycle(false).await.unwrap().unwrap();
        assert_eq!(details.attempted, 2, "{details:?}");
        assert_eq!(details.succeeded, 2, "{details:?}");
        assert_eq!(details.interrupted, 0, "{details:?}");
        assert_eq!(details.deferred, 23, "{details:?}");
        let row = after
            .pull_requests
            .iter()
            .find(|row| row["repository"]["nameWithOwner"] == "acme/watch00")
            .unwrap();
        assert_eq!(row["ci"]["summary"]["state"], "success");
        assert!(row["sourceErrors"]["ci"].is_null());
    }
}
