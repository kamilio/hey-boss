use super::*;

async fn called(f: &Fixture, suffix: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !f
            .mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.ends_with(suffix))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn review_api_has_explicit_coverage_and_preserves_full_snapshot_and_history() {
    let f = Fixture::new().await;
    assert!(
        f.client
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let full_key = format!("pr://{}/acme/demo/7", f.client.hostname());
    let before = f.client.stored_snapshot(&full_key).await.unwrap();
    let status_key = format!("pr-status://{}/acme/demo/7", f.client.hostname());
    let status = json!({"pullRequest":{"id":"PR_7","number":7,"state":"OPEN","headRefOid":"a".repeat(40),"repository":{"nameWithOwner":"acme/demo"},"complete":false,"sourceErrors":{"details":"timeline unavailable"}}});
    f.client.observe(&status_key, &status).await.unwrap();
    let cursor = f.client.bootstrap().await.unwrap().cursor;
    f.mock.deny_timeline.store(true, Ordering::Relaxed);
    f.mock.metadata.lock().unwrap()["title"] = json!("New review metadata");
    let api = crate::api::Api::new(f.client.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sdk = crate::ApiClient::new(
        format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, api.router()).await.unwrap() });
    let report = sdk
        .pr_review_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    server.abort();
    assert!(report.complete);
    assert_eq!(report.data.pull_request["title"], "New review metadata");
    let wire = serde_json::to_value(report).unwrap();
    assert!(wire.get("cursor").is_none());
    assert!(wire["data"].get("timeline").is_none());
    assert!(wire["data"].get("review_events").is_none());
    assert_eq!(f.client.stored_snapshot(&full_key).await.unwrap(), before);
    assert_eq!(
        f.client.stored_snapshot(&status_key).await.unwrap(),
        Some(status)
    );
    assert_eq!(f.client.bootstrap().await.unwrap().cursor, cursor);
    let full = f
        .client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(
        !full.complete,
        "A full report must still require timeline evidence"
    );
    assert!(full.data.errors.iter().any(|e| e.source == "timeline"));
}

#[tokio::test]
async fn review_source_failures_stay_explicit() {
    for (path, source) in [
        ("/repos/acme/demo/issues/7/comments", "comments"),
        ("/repos/acme/demo/pulls/7/comments", "review_comments"),
        ("/repos/acme/demo/pulls/7/reviews", "reviews"),
        ("/graphql", "review_threads"),
    ] {
        let f = Fixture::new().await;
        *f.mock.deny_path.lock().unwrap() = Some(path.into());
        let report = f
            .client
            .pr_review_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert!(!report.complete, "{source}");
        assert!(
            report.data.errors.iter().any(|e| e.source == source),
            "{source}: {:?}",
            report.data.errors
        );
    }
}

#[tokio::test]
async fn review_metadata_and_ci_failures_cannot_certify_completion() {
    let f = Fixture::new().await;
    f.mock.deny_metadata.store(true, Ordering::Relaxed);
    assert!(
        f.client
            .pr_review_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .is_err()
    );
    f.mock.deny_metadata.store(false, Ordering::Relaxed);
    *f.mock.deny_path.lock().unwrap() = Some(format!(
        "/repos/acme/demo/commits/{}/check-runs",
        "a".repeat(40)
    ));
    let report = f
        .client
        .pr_review_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(!report.complete);
    assert!(!report.data.ci.errors.is_empty());
}

#[tokio::test]
async fn review_only_read_retries_a_changed_head_with_matching_ci() {
    let f = Fixture::new().await;
    f.mock.pause_reviews.store(true, Ordering::Relaxed);
    let client = f.client.clone();
    let task = tokio::spawn(async move {
        client
            .pr_review_report("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    called(&f, "/reviews").await;
    f.mock.metadata.lock().unwrap()["head"]["sha"] = json!("c".repeat(40));
    f.mock.pause_reviews.store(false, Ordering::Relaxed);
    f.mock.reviews_release.notify_one();
    let report = task.await.unwrap().unwrap();
    assert!(report.complete);
    assert_eq!(report.data.pull_request["head"]["sha"], "c".repeat(40));
    assert_eq!(report.data.ci.head_sha, "c".repeat(40));
    assert!(
        !f.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.ends_with("/timeline"))
    );
}

#[tokio::test]
async fn review_only_offline_reads_never_fetch_and_revalidation_refreshes_consumed_sources() {
    let f = Fixture::new().await;
    let warm = f
        .client
        .pr_review_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(warm.complete);
    let calls = f.mock.calls.lock().unwrap().len();
    let cached = f
        .client
        .pr_review_report("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(cached.complete);
    assert_eq!(f.mock.calls.lock().unwrap().len(), calls);
    let started = now_ms();
    let refreshed = f
        .client
        .pr_review_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(refreshed.complete);
    assert!(
        refreshed
            .validations
            .iter()
            .all(|v| v.validated_at_ms >= started)
    );
    for suffix in [
        "/issues/7/comments?per_page=100",
        "/pulls/7/comments?per_page=100",
        "/pulls/7/reviews?per_page=100",
        "/pulls/7",
        "/graphql",
    ] {
        assert!(
            refreshed
                .validations
                .iter()
                .any(|v| v.resource.ends_with(suffix)),
            "{suffix}"
        );
    }
    assert!(
        refreshed
            .validations
            .iter()
            .all(|v| !v.resource.contains("/timeline"))
    );
}

#[tokio::test]
async fn review_read_does_not_wait_for_a_full_reports_blocked_timeline() {
    let f = Fixture::new().await;
    // Isolate the report lock from the synthetic server's single detail lane:
    // an unrelated active socket may still legitimately hold an uncached read.
    assert!(
        f.client
            .pr_review_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    f.mock.pause_timeline.store(true, Ordering::Relaxed);
    let client = f.client.clone();
    let full = tokio::spawn(async move {
        client
            .pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
    });
    called(&f, "/timeline").await;
    let review = tokio::time::timeout(
        Duration::from_secs(2),
        f.client
            .pr_review_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30))),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(review.complete);
    assert!(!full.is_finished());
    f.mock.pause_timeline.store(false, Ordering::Relaxed);
    f.mock.timeline_release.notify_one();
    assert!(full.await.unwrap().unwrap().complete);
}
