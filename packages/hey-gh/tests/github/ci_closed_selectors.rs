use super::*;

async fn seeded(app: bool) -> (Harness, Client) {
    let h = Harness::new().await;
    h.mode("ci-point-closed-valid");
    h.phase(2);
    let c = Client::with_token(
        if app {
            ci_app_selectors::app_config(&h)
        } else {
            h.config()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    ci_app_selectors::expire_metadata(&h);
    (h, c)
}

#[tokio::test]
async fn closed_selectors_validate_explicit_test_merge_and_reuse_empty_status_proofs() {
    for app in [false, true] {
        let (h, c) = seeded(app).await;
        h.mode("ci-point-closed-empty-status");
        rusqlite::Connection::open(h.config().cache_path)
            .unwrap()
            .execute(
                "DELETE FROM cache WHERE key LIKE '%/commits/%/status?%'",
                [],
            )
            .unwrap();
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        assert_eq!(report.data.head_sha, HEAD);
        assert_eq!(report.data.merge_sha.as_deref(), Some(MERGE));
        assert!(
            !report.data.check_runs.is_empty(),
            "Real checks must remain visible"
        );
        assert!(report.data.commit_statuses.is_empty());
        let reads = h.calls()[before..].to_vec();
        assert_eq!(reads.iter().filter(|v| v.path == "/graphql").count(), 1);
        assert!(
            reads.iter().all(|v| !v.path.ends_with("/pulls/7")),
            "{reads:?}"
        );
        assert!(reads.iter().all(|v| v.token
            == if app {
                "Bearer synthetic-app-token"
            } else {
                "Bearer synthetic-token"
            }));
        assert!(report.validations.iter().all(|v| v.validated_at_ms > 0));
        // CI starts REST sources alongside the selector read; a status read
        // already dispatched may finish first. Once evidence is cached, both
        // empty sources must work without their REST cache or another query.
        rusqlite::Connection::open(h.config().cache_path)
            .unwrap()
            .execute(
                "DELETE FROM cache WHERE key LIKE '%/commits/%/status?%'",
                [],
            )
            .unwrap();
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        assert!(report.complete);
        assert!(report.data.commit_statuses.is_empty());
        assert_eq!(h.calls().len(), before);
        assert_eq!(
            report
                .validations
                .iter()
                .filter(|v| v.resource.contains("#commit-statuses:"))
                .count(),
            2
        );
        let at: u64 = rusqlite::Connection::open(h.config().cache_path).unwrap().query_row(
            "SELECT json_extract(response,'$.validated_at_ms') FROM cache WHERE key LIKE '%/pulls/7' OR key LIKE '%/pulls/7#%'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(at, 0, "Selector evidence cannot refresh the REST payload");
    }
}

#[tokio::test]
async fn closed_selectors_skip_merged_or_unknown_rest_seeds() {
    for edit in [
        "'$.data.merged',json('true')",
        "'$.data.mergeable',NULL",
        "'$.data.merge_commit_sha',NULL",
        "'$.data.merged',NULL",
    ] {
        let (h, c) = seeded(false).await;
        rusqlite::Connection::open(h.config().cache_path)
            .unwrap()
            .execute(
                &format!(
                    "UPDATE cache SET response=json_set(response,{edit}) WHERE key LIKE '%/pulls/7'"
                ),
                [],
            )
            .unwrap();
        h.mode("ci-point-closed-valid");
        let before = h.calls().len();
        assert!(
            c.ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
                .await
                .unwrap()
                .complete
        );
        let reads = h.calls()[before..].to_vec();
        assert!(
            reads.iter().all(|v| v.path != "/graphql"),
            "{edit}: {reads:?}"
        );
        assert!(reads.iter().any(|v| v.path.ends_with("/pulls/7")), "{edit}");
    }
}

#[tokio::test]
async fn closed_selectors_keep_rest_for_ambiguous_changed_or_reopened_prs() {
    for case in [
        "null-merge",
        "merge",
        "parents",
        "head",
        "base",
        "id",
        "number",
        "repository",
        "merged",
        "unknown",
        "reopened",
    ] {
        let (h, c) = seeded(false).await;
        h.mode(&format!("ci-point-closed-{case}"));
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        assert!(report.complete, "{case}");
        assert_eq!(
            report.data.merge_sha.as_deref(),
            Some(MERGE),
            "Null GraphQL merge is not proof of REST merge absence: {case}"
        );
        let reads = h.calls()[before..].to_vec();
        assert!(reads.iter().any(|v| v.path == "/graphql"), "{case}");
        assert!(reads.iter().any(|v| v.path.ends_with("/pulls/7")), "{case}");
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
async fn closed_selectors_do_not_certify_full_metadata_or_explicit_refresh() {
    for full in [false, true] {
        let (h, c) = seeded(false).await;
        let before = h.calls().len();
        if full {
            let report = c
                .pr_report("acme/demo", 7, Freshness::default())
                .await
                .unwrap();
            assert!(report.complete);
            assert_eq!(report.data.pull_request["state"], "closed");
            assert_eq!(report.data.pull_request["merged"], false);
        } else {
            assert!(
                c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
                    .await
                    .unwrap()
                    .complete
            );
        }
        assert!(
            h.calls()[before..]
                .iter()
                .any(|v| v.path.ends_with("/pulls/7"))
        );
    }
}
