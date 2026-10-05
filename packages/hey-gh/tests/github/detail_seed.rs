use super::*;

#[tokio::test]
async fn detail_seed_starts_sources_without_publishing_expired_metadata() {
    detail_seed("valid").await;
}

#[tokio::test]
async fn detail_seed_requires_usable_cached_identity() {
    for case in ["cold", "malformed"] {
        detail_seed(case).await;
    }
}

#[tokio::test]
async fn closed_detail_seed_starts_sources_without_publishing_stale_lifecycle() {
    detail_seed("closed").await;
}

#[tokio::test]
async fn detail_seed_cannot_clear_a_final_metadata_access_denial() {
    detail_seed("denied").await;
}

async fn detail_seed(case: &str) {
    let h = Harness::new().await;
    h.mode("account");
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(2);
    config.queue_timeout = Duration::from_secs(5);
    config.request_timeout = Duration::from_secs(5);
    config.max_attempts = 1;
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let previous = c.account_refresh_cycle(false).await.unwrap().unwrap();
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    // Preserve snapshots and the roster, but expire upstream representations.
    db.execute(
        "UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE key LIKE 'http%'",
        [],
    )
    .unwrap();
    db.execute(
        "UPDATE cache SET response=json_set(response,'$.data.title','Expired seed title','$.etag','old-representation','$.last_modified',null) WHERE key LIKE '%/acme/demo/pulls/7'",
        [],
    ).unwrap();
    match case {
        "cold" => {
            db.execute("DELETE FROM cache WHERE key LIKE '%/acme/demo/pulls/7'", [])
                .unwrap();
        }
        "closed" => {
            db.execute("UPDATE cache SET response=json_set(response,'$.data.state','closed') WHERE key LIKE '%/acme/demo/pulls/7'", []).unwrap();
        }
        "malformed" => {
            db.execute("UPDATE cache SET response=json_set(response,'$.data.head.sha','invalid') WHERE key LIKE '%/acme/demo/pulls/7'", []).unwrap();
        }
        _ => {}
    }
    h.phase(4);
    h.mode(if case == "denied" {
        "account-detail-seed-denied"
    } else {
        "account-detail-seed-stall"
    });
    let before = h.calls().len();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    api.watch_account(60).await.unwrap();
    let cycle = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(cycle) = c.account_refresh_cycle(false).await.unwrap()
                && cycle.started_at_ms > previous.started_at_ms
            {
                break cycle;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    api.stop().await;
    let page = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let row = &page.pull_requests[0];
    assert!(
        row["sourceErrors"]["details"].is_string(),
        "stale metadata cannot certify completed details: {row}"
    );
    assert_ne!(
        row["title"], "Expired seed title",
        "a seed must not overwrite observed metadata"
    );
    assert!(
        h.calls()[before..]
            .iter()
            .any(|call| call.path == "/repos/acme/demo/pulls/7"),
        "final metadata must still be requested"
    );
    assert_eq!(
        row["reviewThreads"][0]["isResolved"],
        matches!(case, "valid" | "closed" | "denied"),
        "independent sources should start only with a usable seed: {case}"
    );
    h.mode("account");
    h.mock.release.notify_waiters();
    until(|| c.status().outstanding_requests == 0).await;
    if case != "valid" {
        return;
    }
    // A later successful final validation must clear the interrupted detail health.
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if c.account_refresh_cycle(false)
                .await
                .unwrap()
                .is_some_and(|next| next.started_at_ms > cycle.started_at_ms)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    api.stop().await;
    let page = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let row = &page.pull_requests[0];
    assert!(row["sourceErrors"]["details"].is_null(), "{row}");
    assert_eq!(row["title"], "A PR");
    assert_eq!(row["reviewThreads"][0]["isResolved"], true);
}
