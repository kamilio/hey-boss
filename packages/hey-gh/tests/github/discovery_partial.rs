use super::*;

fn age_discovery(h: &Harness) {
    ci_discovery_status::edit_discovery(h, |r| {
        r["validated_at_ms"] = json!(1);
        if let Some(clocks) = r["data"]["validatedAtByPr"].as_object_mut() {
            for clock in clocks.values_mut() {
                *clock = json!(1);
            }
        }
    });
}

fn memo(h: &Harness) -> String {
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .query_row(
            "SELECT response FROM cache WHERE key='account-discovery-complete:v1'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn denied_discovery_nodes_retain_other_ci_proofs_and_continue_without_certifying_the_roster()
{
    for app in [false, true] {
        let (h, c) = ci_discovery_checks::seeded(app).await;
        age_discovery(&h);
        let old_memo = memo(&h);
        h.mode("account-ci-selectors-partial");
        let before = h.calls().len();
        let error = c
            .all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::GraphQL {
                access_denied: true,
                ..
            }
        ));
        let calls = h.calls()[before..].to_vec();
        assert!(
            calls
                .iter()
                .any(|c| c.body["variables"]["after"] == "PR-next"),
            "an unrelated denied node must not prevent collecting later pages"
        );
        assert!(calls.iter().all(|c| c.token == "Bearer synthetic-token"));
        assert_eq!(
            memo(&h),
            old_memo,
            "partial evidence cannot renew or replace the complete roster"
        );
        let failure: String = rusqlite::Connection::open(h.config().cache_path)
            .unwrap()
            .query_row("SELECT last_error FROM discovery_health LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(failure.contains("Synthetic repository access denied"));
        let before = h.calls().len();
        let ci = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(ci.complete, "{:?}", ci.data.errors);
        assert!(ci.data.check_runs.is_empty());
        assert_eq!(
            h.calls().len(),
            before,
            "allowed node proofs must avoid additional reads"
        );
        let proofs: Vec<_> = ci
            .validations
            .iter()
            .filter(|v| v.resource.contains("#check-runs:"))
            .collect();
        assert_eq!(proofs.len(), 2);
        assert!(proofs.iter().all(|v| v.validated_at_ms > 1));
        drop(c);
        let c = Client::with_token(
            if app {
                ci_app_selectors::app_config(&h)
            } else {
                h.config()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let offline = c
            .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(offline.complete, "{:?}", offline.data.errors);
        assert_eq!(
            h.calls().len(),
            before,
            "offline restart must not fetch or mint"
        );
        for proof in proofs {
            assert!(offline.validations.iter().any(
                |v| v.resource == proof.resource && v.validated_at_ms == proof.validated_at_ms
            ));
        }
        assert_eq!(memo(&h), old_memo);
    }
}

#[tokio::test]
async fn partial_discovery_never_reuses_a_node_with_a_denied_field_or_unscoped_error() {
    for case in ["field", "root", "identity", "http", "expired", "future"] {
        let (h, c) = ci_discovery_checks::seeded(false).await;
        age_discovery(&h);
        h.mode(&format!("account-ci-selectors-partial-{case}"));
        assert!(
            c.all_my_open_pull_requests(Freshness::Revalidate)
                .await
                .is_err()
        );
        if matches!(case, "expired" | "future") {
            rusqlite::Connection::open(h.config().cache_path).unwrap().execute(
                "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%#partial-discovery-v1%'",
                [if case=="expired" {1_i64} else {i64::MAX}],
            ).unwrap();
        }
        let before = h.calls().len();
        let ci = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(ci.complete, "{case}: {:?}", ci.data.errors);
        assert_eq!(
            h.calls()[before..]
                .iter()
                .filter(|c| c.path.ends_with("/check-runs"))
                .count(),
            2,
            "{case}"
        );
        assert!(
            ci.validations
                .iter()
                .all(|v| !v.resource.contains("#check-runs:")),
            "{case}"
        );
    }
}

#[tokio::test]
async fn partial_pages_preserve_generic_graphql_errors_and_yield_to_a_new_complete_page() {
    let (h, c) = ci_discovery_checks::seeded(false).await;
    age_discovery(&h);
    h.mode("account-ci-selectors-partial");
    let before = h.calls().len();
    assert!(
        c.all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .is_err()
    );
    let call = h.calls()[before..]
        .iter()
        .find(|c| {
            c.body["query"]
                .as_str()
                .is_some_and(|q| q.contains("query MyOpenPullRequests("))
                && c.body["variables"]["after"].is_null()
        })
        .unwrap()
        .clone();
    let query = call.body["query"].as_str().unwrap();
    let variables = call.body["variables"].clone();
    assert!(matches!(
        c.graphql(query, variables.clone(), Freshness::Revalidate)
            .await,
        Err(Error::GraphQL {
            access_denied: true,
            ..
        })
    ));
    let cached = c
        .graphql(query, variables, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(
        cached.validated_at_ms, 1,
        "generic reads retain the old successful representation"
    );
    assert!(cached.data.get("errors").is_none());
    tokio::time::sleep(Duration::from_millis(3)).await;
    h.mode("account-ci-selectors");
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    assert_eq!(
        h.calls()[before..]
            .iter()
            .filter(|c| c.path.ends_with("/check-runs"))
            .count(),
        2,
        "newer full pages must defeat older advisory counts"
    );
    let error: Option<String> = rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .query_row("SELECT last_error FROM discovery_health LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(
        error.is_none(),
        "a complete successful scan may clear discovery failure"
    );
}

#[tokio::test]
async fn discovery_deadline_retains_permitted_evidence_without_publishing_a_partial_roster() {
    let (h, c) = ci_discovery_checks::seeded(false).await;
    age_discovery(&h);
    let original = memo(&h);
    drop(c);
    let c = Client::with_token(
        Config {
            report_timeout: Duration::from_millis(300),
            ..h.config()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    h.mode("account-ci-selectors-partial-slow");
    assert!(matches!(
        c.all_my_open_pull_requests(Freshness::Revalidate).await,
        Err(Error::Deadline)
    ));
    assert_eq!(memo(&h), original);
    let before = h.calls().len();
    let cached = c
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(cached.complete, "{:?}", cached.data.errors);
    assert!(cached.data.check_runs.is_empty());
    assert!(
        cached
            .validations
            .iter()
            .any(|v| v.resource.starts_with("my-open-prs://")
                && v.resource.contains("#check-runs:")
                && v.validated_at_ms > 1)
    );
    assert_eq!(h.calls().len(), before);
    assert_eq!(memo(&h), original);
}

#[tokio::test]
async fn excluded_nodes_still_count_toward_the_discovery_byte_budget() {
    let (h, c) = ci_discovery_checks::seeded(false).await;
    drop(c);
    let c = Client::with_token(
        Config {
            max_collection_bytes: 4096,
            ..h.config()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    h.mode("account-ci-selectors-partial-large");
    let before = h.calls().len();
    let error = c
        .all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Invalid(ref m) if m.contains("collection limit")),
        "{error:?}"
    );
    assert!(
        !h.calls()[before..]
            .iter()
            .any(|c| c.body["variables"]["after"] == "PR-next")
    );
}
