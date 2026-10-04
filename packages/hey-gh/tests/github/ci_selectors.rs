use super::*;

async fn seeded() -> (Harness, Client) {
    let h = Harness::new().await;
    h.mode("account-ci-selectors");
    h.phase(2);
    let c = h.client();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    rusqlite::Connection::open(h.config().cache_path).unwrap().execute(
        "UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE key LIKE '%/pulls/7'", [],
    ).unwrap();
    (h, c)
}

#[tokio::test]
async fn point_selectors_validate_ci_without_spending_rest_metadata_quota() {
    for mode in ["ci-point-valid", "ci-point-case"] {
        let (h, c) = seeded().await;
        h.mode(mode);
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        assert!(report.complete);
        assert_eq!(report.data.head_sha, HEAD);
        assert_eq!(report.data.merge_sha.as_deref(), Some(MERGE));
        assert_eq!(report.data.summary.state, "success");
        let calls = h.calls();
        let reads = &calls[before..];
        assert!(
            reads.iter().all(|call| !call.path.ends_with("/pulls/7")),
            "{reads:?}"
        );
        assert_eq!(
            reads.iter().filter(|call| call.path == "/graphql").count(),
            1
        );
        assert!(
            reads
                .iter()
                .all(|call| call.token == "Bearer synthetic-token")
        );
        let cached = c
            .pull_request("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert_eq!(
            cached.validated_at_ms, 0,
            "selector confirmation cannot refresh the REST body"
        );
        assert!(report.validations.iter().all(|v| v.validated_at_ms > 0));
        assert!(
            report
                .validations
                .iter()
                .any(|v| v.resource.ends_with("/graphql"))
        );
    }
}

#[tokio::test]
async fn point_selectors_fall_back_for_changed_or_ambiguous_evidence() {
    for case in [
        "head",
        "base",
        "id",
        "number",
        "repository",
        "merge",
        "null-merge",
        "parents",
        "closed",
        "merged",
        "unknown",
    ] {
        let (h, c) = seeded().await;
        h.mode(&format!("ci-point-{case}"));
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        assert!(report.complete, "{case}");
        let reads = h.calls()[before..].to_vec();
        assert!(reads.iter().any(|call| call.path == "/graphql"), "{case}");
        assert!(
            reads.iter().any(|call| call.path.ends_with("/pulls/7")),
            "{case}"
        );
        assert!(
            report
                .validations
                .iter()
                .any(|v| v.resource.ends_with("/pulls/7") && v.validated_at_ms > 0),
            "{case}"
        );
    }
}

#[tokio::test]
async fn point_selectors_preserve_forced_and_offline_reads() {
    for freshness in [
        Freshness::Revalidate,
        Freshness::MaxAge(Duration::ZERO),
        Freshness::CachedOnly,
    ] {
        let (h, c) = seeded().await;
        h.mode("ci-point-valid");
        let before = h.calls().len();
        assert!(
            c.ci_for_pr("acme/demo", 7, freshness)
                .await
                .unwrap()
                .complete
        );
        let reads = h.calls()[before..].to_vec();
        assert!(reads.iter().all(|call| call.path != "/graphql"));
        if matches!(freshness, Freshness::CachedOnly) {
            assert!(reads.is_empty());
        }
    }
}

#[tokio::test]
async fn point_selectors_preserve_access_denials() {
    let (h, c) = seeded().await;
    h.mode("ci-point-denied");
    let before = h.calls().len();
    let result = c
        .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await;
    assert!(
        matches!(
            result,
            Err(Error::GraphQL {
                access_denied: true,
                ..
            })
        ),
        "{result:?}"
    );
    assert!(
        h.calls()[before..]
            .iter()
            .all(|call| !call.path.ends_with("/pulls/7"))
    );
}

#[tokio::test]
async fn point_selectors_timeout_leaves_rest_fallback_available() {
    let (h, c) = seeded().await;
    h.mode("ci-point-stalled");
    let before = h.calls().len();
    let report = tokio::time::timeout(
        Duration::from_secs(4),
        c.ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30))),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(report.complete);
    assert!(
        h.calls()[before..]
            .iter()
            .any(|call| call.path == "/graphql")
    );
    assert!(
        h.calls()[before..]
            .iter()
            .any(|call| call.path.ends_with("/pulls/7"))
    );
}

#[tokio::test]
async fn point_selectors_recheck_expired_final_evidence() {
    for changed in [false, true] {
        let (h, c) = seeded().await;
        h.mode("ci-point-valid");
        let warm = c
            .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        let old = warm.observed_at_ms - 20_000;
        rusqlite::Connection::open(h.config().cache_path).unwrap().execute(
            "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/graphql#%'", [old],
        ).unwrap();
        if changed {
            h.mode("ci-point-head");
        }
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        assert!(report.complete);
        let reads = h.calls()[before..].to_vec();
        assert_eq!(
            reads.iter().filter(|call| call.path == "/graphql").count(),
            1,
            "final 15-second bound must be rechecked"
        );
        assert_eq!(
            reads.iter().any(|call| call.path.ends_with("/pulls/7")),
            changed
        );
        assert!(report.validations.iter().any(|v| v.validated_at_ms > old));
    }
}

#[tokio::test]
async fn point_selectors_do_not_certify_a_full_rest_report() {
    let (h, c) = seeded().await;
    h.mode("ci-point-valid");
    rusqlite::Connection::open(h.config().cache_path).unwrap().execute(
        "UPDATE cache SET response=json_set(response,'$.data.title','old title','$.etag',NULL) WHERE key LIKE '%/pulls/7'", [],
    ).unwrap();
    let before = h.calls().len();
    let report = c
        .pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert!(report.complete);
    assert_eq!(report.data.pull_request["title"], "A PR");
    let reads = h.calls()[before..].to_vec();
    assert!(reads.iter().any(|call| call.path == "/graphql"
        && call.body["query"].as_str().unwrap().contains("CiSelectors")));
    assert_eq!(
        reads
            .iter()
            .filter(|call| call.path.ends_with("/pulls/7"))
            .count(),
        1
    );
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.ends_with("/pulls/7") && v.validated_at_ms > 0)
    );
}

#[tokio::test]
async fn point_selectors_reject_future_cached_validation_clocks() {
    let (h, c) = seeded().await;
    h.mode("ci-point-valid");
    let warm = c
        .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    rusqlite::Connection::open(h.config().cache_path).unwrap().execute(
        "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/graphql#%'", [warm.observed_at_ms + 60_000],
    ).unwrap();
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert!(report.complete);
    let reads = h.calls()[before..].to_vec();
    assert!(reads.iter().any(|call| call.path.ends_with("/pulls/7")));
}
