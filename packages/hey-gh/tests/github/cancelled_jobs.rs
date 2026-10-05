use super::*;

#[tokio::test]
async fn cancelled_empty_jobs_reuse_settled_versions_but_refresh_and_reruns_refetch() {
    let h = Harness::new().await;
    h.mode("cancelled-empty-settled");
    h.phase(2);
    let client = h.client();
    let policy = Freshness::MaxAge(Duration::ZERO);
    let count = || {
        h.calls()
            .iter()
            .filter(|c| c.path.ends_with("/jobs"))
            .count()
    };
    for _ in 0..2 {
        let report = client.ci_for_pr("acme/demo", 7, policy).await.unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        assert!(report.data.jobs.is_empty());
        assert_ne!(report.data.summary.state, "success");
    }
    assert_eq!(
        count(),
        1,
        "a settled cancelled empty attempt needs only one jobs request"
    );
    assert!(
        client
            .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    assert_eq!(
        count(),
        2,
        "explicit refresh still validates empty attempts"
    );
    h.phase(7);
    let rerun = client.ci_for_pr("acme/demo", 7, policy).await.unwrap();
    assert!(rerun.complete, "{:?}", rerun.data.errors);
    assert_eq!(rerun.data.workflow_runs[0]["run_attempt"], 3);
    assert!(!rerun.data.jobs.is_empty());
    assert_eq!(
        count(),
        3,
        "a new attempt cannot reuse the cancelled attempt's empty jobs"
    );
}

#[tokio::test]
async fn cancelled_empty_jobs_reuse_requires_a_settled_cancellation() {
    for mode in [
        "cancelled-empty-recent",
        "cancelled-empty-future",
        "cancelled-empty-invalid",
        "cancelled-empty-success",
    ] {
        let h = Harness::new().await;
        h.mode(mode);
        h.phase(2);
        let client = h.client();
        for _ in 0..2 {
            let report = client
                .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::ZERO))
                .await
                .unwrap();
            assert!(report.complete, "{mode}: {:?}", report.data.errors);
            if mode != "cancelled-empty-success" {
                assert_ne!(report.data.summary.state, "success");
            }
        }
        assert_eq!(
            h.calls()
                .iter()
                .filter(|c| c.path.ends_with("/jobs"))
                .count(),
            2,
            "{mode}"
        );
    }
}
