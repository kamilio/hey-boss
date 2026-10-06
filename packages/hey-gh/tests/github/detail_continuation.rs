use super::*;

#[tokio::test]
async fn collected_details_resume_after_cycle_expiry_without_replaying_fresh_sources() {
    detail_continuation("fresh").await;
}

#[tokio::test]
async fn unfinished_details_keep_the_ordinary_rotation_after_cycle_expiry() {
    detail_continuation("partial").await;
}

#[tokio::test]
async fn a_stalled_final_detail_validation_cannot_claim_a_continuation_before_cycle_expiry() {
    detail_continuation("early_stall").await;
}

#[tokio::test]
async fn resumed_details_revalidate_expired_sources() {
    detail_continuation("expired").await;
}

async fn detail_continuation(case: &str) {
    let resumable = matches!(case, "fresh" | "expired");
    let cycle_seconds = if case == "early_stall" { 8 } else { 3 };
    let h = Harness::new().await;
    h.mode(if case == "partial" {
        "account-large-detail-partial"
    } else {
        "account-large-detail-finish"
    });
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(cycle_seconds);
    config.queue_timeout = Duration::from_secs(5);
    config.request_timeout = Duration::from_secs(5);
    config.max_attempts = 1;
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    assert!(
        c.pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    // Exercise a genuinely required final validation. A still-fresh personal
    // observation can now complete details without the stalled REST request.
    db.execute(
        "UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE key LIKE '%/pulls/7'",
        [],
    ).unwrap();
    if case == "partial" {
        db.execute(
            "DELETE FROM cache WHERE key LIKE '%/issues/7/timeline%'",
            [],
        )
        .unwrap();
    }
    h.phase(2);
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    api.watch_account(60).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(cycle_seconds + 3), async {
        loop {
            if let Some(cycle) = c.account_refresh_cycle(false).await.unwrap() {
                break cycle;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    api.stop().await;
    h.mock.release.notify_waiters();
    until(|| c.status().outstanding_requests == 0).await;
    assert!(first.interrupted > 0);
    let schedule: String = db
        .query_row(
            "SELECT response FROM cache WHERE key='account-status-schedule:details'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let schedule: Value = serde_json::from_str(&schedule).unwrap();
    let resume = schedule["data"]["resume"].as_array().unwrap();
    assert_eq!(
        resume.contains(&json!(["acme/demo", 7])),
        resumable,
        "a collected PR should retain a completion turn, but unfinished sources must not jump the rotation: {schedule}"
    );
    assert_ne!(
        schedule["data"]["next"],
        json!(["acme/demo", 7]),
        "ordinary work must advance too"
    );
    let before = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        before.pull_requests[0]["sourceErrors"]["details"].is_string(),
        "continuation is not successful validation"
    );
    if !resumable {
        return;
    }

    let calls_before = h.calls().len();
    if case == "expired" {
        db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE key LIKE '%/issues/7/%' OR key LIKE '%/pulls/7/%'", []).unwrap();
    }
    h.phase(3);
    // Reopen the client and persisted watch: continuation must survive restart.
    drop(c);
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(3);
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            if c.account_refresh_cycle(false)
                .await
                .unwrap()
                .is_some_and(|cycle| cycle.started_at_ms > first.started_at_ms)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    api.stop().await;
    let after = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        after.pull_requests[0]["sourceErrors"]["details"].is_null(),
        "{:?}",
        after.pull_requests[0]["sourceErrors"]
    );
    let calls = h.calls();
    let reads = &calls[calls_before..];
    assert!(
        reads
            .iter()
            .any(|call| call.path == "/repos/acme/demo/pulls/7"),
        "final metadata still requires real validation"
    );
    let detail_reads = reads
        .iter()
        .filter(|call| {
            call.path.starts_with("/repos/acme/demo/issues/")
                || call.path.starts_with("/repos/acme/demo/pulls/7/")
                    && (call.path.ends_with("/comments") || call.path.ends_with("/reviews"))
        })
        .count();
    if case == "expired" {
        assert!(
            detail_reads >= 4,
            "expired source collections must be validated again: {reads:?}"
        );
    } else {
        assert_eq!(
            detail_reads, 0,
            "fresh completed REST collections were replayed: {reads:?}"
        );
    }
}
