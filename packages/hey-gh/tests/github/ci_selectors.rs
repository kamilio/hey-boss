use super::*;

async fn empty_status_seed() -> (Harness, Client) {
    let (h, c) = seeded().await;
    h.mode("ci-point-empty-status");
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap()
            .complete
    );
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute(
            "DELETE FROM cache WHERE key LIKE '%/commits/%/status?%'",
            [],
        )
        .unwrap();
    (h, c)
}

#[tokio::test]
async fn empty_status_selectors_avoid_two_rest_reads_without_an_extra_query() {
    let (h, c) = empty_status_seed().await;
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert!(report.complete);
    assert!(report.data.commit_statuses.is_empty());
    let reads = h.calls()[before..].to_vec();
    assert!(
        reads.is_empty(),
        "fresh selector evidence should satisfy both empty status sources: {reads:?}"
    );
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.contains("#commit-statuses:") && v.validated_at_ms > 0)
    );
}

#[tokio::test]
async fn empty_status_selectors_remain_available_offline_after_restart() {
    let (h, c) = empty_status_seed().await;
    let before = h.calls().len();
    drop(c);
    let report = h
        .client()
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    assert!(report.data.commit_statuses.is_empty());
    assert_eq!(h.calls().len(), before);
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.contains("#commit-statuses:") && v.validated_at_ms > 0)
    );
}

#[tokio::test]
async fn empty_status_selectors_preserve_explicit_refresh() {
    let (h, c) = empty_status_seed().await;
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(report.complete);
    assert_eq!(
        h.calls()[before..]
            .iter()
            .filter(|call| call.path.ends_with("/status"))
            .count(),
        2
    );
}

fn edit_selector_cache(h: &Harness, edit: impl FnOnce(&mut Value)) {
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let (key, raw): (String, String) = db
        .query_row(
            "SELECT key,response FROM cache WHERE key LIKE '%/graphql#%'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let mut response: Value = serde_json::from_str(&raw).unwrap();
    edit(&mut response);
    db.execute(
        "UPDATE cache SET response=?1 WHERE key=?2",
        [response.to_string(), key],
    )
    .unwrap();
}

#[tokio::test]
async fn empty_status_selectors_reject_missing_nonempty_or_mismatched_evidence() {
    for case in [
        "head-missing",
        "head-nonempty",
        "head-oid",
        "head-null",
        "head-duplicate",
        "head-ref",
        "merge-missing",
        "merge-nonempty",
        "merge-oid",
        "merge-null",
        "id",
        "number",
        "repository",
        "zero",
        "future",
    ] {
        let (h, c) = empty_status_seed().await;
        edit_selector_cache(&h, |response| {
            let pr = &mut response["data"]["data"]["repository"]["pullRequest"];
            match case {
                "head-missing" => {
                    pr["commits"]["nodes"][0]["commit"]
                        .as_object_mut()
                        .unwrap()
                        .remove("status");
                }
                "head-nonempty" => {
                    pr["commits"]["nodes"][0]["commit"]["status"] = json!({"id":"S_head"})
                }
                "head-oid" => pr["commits"]["nodes"][0]["commit"]["oid"] = json!(NEW_HEAD),
                "head-null" => pr["commits"]["nodes"][0]["commit"] = Value::Null,
                "head-duplicate" => {
                    let duplicate = pr["commits"]["nodes"][0].clone();
                    pr["commits"]["nodes"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                "head-ref" => pr["headRefOid"] = json!(NEW_HEAD),
                "merge-missing" => {
                    pr["potentialMergeCommit"]
                        .as_object_mut()
                        .unwrap()
                        .remove("status");
                }
                "merge-nonempty" => pr["potentialMergeCommit"]["status"] = json!({"id":"S_merge"}),
                "merge-oid" => pr["potentialMergeCommit"]["oid"] = json!(NEW_HEAD),
                "merge-null" => pr["potentialMergeCommit"] = Value::Null,
                "id" => pr["id"] = json!("PR_replaced"),
                "number" => pr["number"] = json!(8),
                "repository" => pr["repository"]["nameWithOwner"] = json!("acme/other"),
                "zero" => response["validated_at_ms"] = json!(0),
                "future" => response["validated_at_ms"] = json!(u64::MAX),
                _ => unreachable!(),
            }
        });
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(!report.complete, "{case}");
        assert_eq!(
            report.data.errors.len(),
            if case.starts_with("head-") || case.starts_with("merge-") {
                1
            } else {
                2
            },
            "{case}"
        );
        assert!(
            report
                .data
                .errors
                .iter()
                .all(|e| e.source.starts_with("commit_statuses:")),
            "{case}"
        );
        assert_eq!(h.calls().len(), before, "{case}");
    }
}

#[tokio::test]
async fn empty_status_selectors_do_not_hide_newer_rest_statuses() {
    for freshness in [
        Freshness::CachedOnly,
        Freshness::MaxAge(Duration::from_secs(30)),
    ] {
        let (h, c) = empty_status_seed().await;
        edit_selector_cache(&h, |response| {
            response["validated_at_ms"] = json!(response["validated_at_ms"].as_u64().unwrap() - 1);
        });
        h.mode("ci-point-valid");
        for sha in [HEAD, MERGE] {
            c.get(
                &format!("repos/acme/demo/commits/{sha}/status?per_page=100"),
                Freshness::Revalidate,
            )
            .await
            .unwrap();
        }
        let before = h.calls().len();
        let report = c.ci_for_pr("acme/demo", 7, freshness).await.unwrap();
        assert!(report.complete);
        assert_eq!(
            report.data.commit_statuses.len(),
            1,
            "REST status IDs retain their deduplication"
        );
        assert_eq!(report.data.commit_statuses[0]["id"], 6);
        assert!(
            report
                .validations
                .iter()
                .all(|v| !v.resource.contains("#commit-statuses:"))
        );
        assert_eq!(h.calls().len(), before);
    }
}

#[tokio::test]
async fn empty_status_selectors_do_not_extend_freshness_or_validation_clocks() {
    let (h, c) = empty_status_seed().await;
    edit_selector_cache(&h, |response| response["validated_at_ms"] = json!(1));
    // Fresh REST PR metadata prevents a new optional selector query. Stale
    // empty evidence must not satisfy a current status read on its own.
    c.pull_request("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert!(report.complete);
    assert_eq!(
        h.calls()[before..]
            .iter()
            .filter(|call| call.path.ends_with("/status"))
            .count(),
        2
    );
    assert!(
        report
            .validations
            .iter()
            .all(|v| !v.resource.contains("#commit-statuses:"))
    );

    let (h, c) = empty_status_seed().await;
    edit_selector_cache(&h, |response| response["validated_at_ms"] = json!(1));
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(report.complete);
    let stamps: Vec<_> = report
        .validations
        .iter()
        .filter(|v| v.resource.contains("#commit-statuses:"))
        .map(|v| v.validated_at_ms)
        .collect();
    assert_eq!(
        stamps,
        [1, 1],
        "offline reads retain the evidence's original age"
    );
}

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
