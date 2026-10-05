use super::*;
use ci_selectors::edit_selector_cache;

#[tokio::test]
async fn upgrading_selector_query_keeps_legacy_empty_status_evidence_offline() {
    use sha2::Digest;
    const LEGACY: &str = r#"query CiSelectors($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      id number state merged mergeable headRefOid baseRefOid
      repository { nameWithOwner }
      commits(last: 1) { nodes { commit { oid status { id } } } }
      potentialMergeCommit { oid status { id } parents(first: 2) { totalCount nodes { oid } } }
    }
  }
}"#;
    for (app, counted) in [(false, false), (true, false), (false, true), (true, true)] {
        let h = Harness::new().await;
        h.mode("account-ci-selectors");
        h.phase(2);
        let config = if app {
            ci_app_selectors::app_config(&h)
        } else {
            h.config()
        };
        let c = Client::with_token(config.clone(), "synthetic-token".into()).unwrap();
        assert!(
            c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
                .await
                .unwrap()
                .complete
        );
        ci_app_selectors::expire_metadata(&h);
        h.mode("ci-point-empty-status");
        assert!(
            c.ci_for_pr("acme/demo", 7, Freshness::default())
                .await
                .unwrap()
                .complete
        );
        let mut body = h
            .calls()
            .iter()
            .find(|c| c.path == "/graphql")
            .unwrap()
            .body
            .clone();
        let current = format!("{:x}", sha2::Sha256::digest(body.to_string().as_bytes()));
        body["query"] = json!(if counted {
            LEGACY.replace(
                "status { id }",
                "status { id } statusCheckRollup { contexts(first: 1) { checkRunCount } }",
            )
        } else {
            LEGACY.to_owned()
        });
        let legacy = format!("{:x}", sha2::Sha256::digest(body.to_string().as_bytes()));
        let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
        assert_eq!(
            db.execute(
                "UPDATE cache SET key=replace(key,?1,?2) WHERE key LIKE ?3",
                [&current, &legacy, &format!("%/graphql#{current}%")]
            )
            .unwrap(),
            1
        );
        db.execute(
            "DELETE FROM cache WHERE key LIKE '%/commits/%/status?%'",
            [],
        )
        .unwrap();
        drop(c);
        let c = Client::with_token(config, "synthetic-token".into()).unwrap();
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(report.complete, "app={app}: {:?}", report.data.errors);
        assert_eq!(
            report
                .validations
                .iter()
                .filter(|v| v.resource.contains("#commit-statuses:"))
                .count(),
            2
        );
        assert_eq!(h.calls().len(), before);
    }
}

async fn empty_checks_seed() -> (Harness, Client) {
    let (h, c) = ci_selectors::seeded().await;
    h.mode("ci-point-empty-checks");
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap()
            .complete
    );
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute(
            "DELETE FROM cache WHERE key LIKE '%/commits/%/check-runs?%'",
            [],
        )
        .unwrap();
    (h, c)
}

#[tokio::test]
async fn empty_check_rollups_avoid_rest_without_an_extra_query_and_survive_restart() {
    for offline in [false, true] {
        let (h, c) = empty_checks_seed().await;
        drop(c);
        let before = h.calls().len();
        let report = h
            .client()
            .ci_for_pr(
                "acme/demo",
                7,
                if offline {
                    Freshness::CachedOnly
                } else {
                    Freshness::MaxAge(Duration::from_secs(30))
                },
            )
            .await
            .unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        assert!(report.data.check_runs.is_empty());
        assert_eq!(
            h.calls().len(),
            before,
            "rollups already prove both check lists empty"
        );
        assert_eq!(
            report
                .validations
                .iter()
                .filter(|v| v.resource.contains("#check-runs:") && v.validated_at_ms > 0)
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn empty_check_rollups_preserve_explicit_refresh() {
    let (h, c) = empty_checks_seed().await;
    let before = h.calls().len();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    assert_eq!(
        h.calls()[before..]
            .iter()
            .filter(|c| c.path.ends_with("/check-runs"))
            .count(),
        2
    );
}

#[tokio::test]
async fn empty_check_rollups_require_complete_identity_and_explicit_counts() {
    for case in [
        "missing",
        "contexts-missing",
        "count-missing",
        "negative",
        "fraction",
        "string",
        "nonempty",
        "oid",
        "duplicate",
        "head-ref",
        "merge-oid",
        "id",
        "number",
        "repository",
        "zero",
        "future",
    ] {
        let (h, c) = empty_checks_seed().await;
        edit_selector_cache(&h, |r| {
            let pr = &mut r["data"]["data"]["repository"]["pullRequest"];
            let commit = &mut pr["commits"]["nodes"][0]["commit"];
            match case {
                "missing" => {
                    commit.as_object_mut().unwrap().remove("statusCheckRollup");
                }
                "contexts-missing" => commit["statusCheckRollup"] = json!({"state":"SUCCESS"}),
                "count-missing" => commit["statusCheckRollup"] = json!({"contexts":{}}),
                "negative" => {
                    commit["statusCheckRollup"] = json!({"contexts":{"checkRunCount":-1}})
                }
                "fraction" => {
                    commit["statusCheckRollup"] = json!({"contexts":{"checkRunCount":0.5}})
                }
                "string" => commit["statusCheckRollup"] = json!({"contexts":{"checkRunCount":"0"}}),
                "nonempty" => commit["statusCheckRollup"] = json!({"contexts":{"checkRunCount":1}}),
                "oid" => commit["oid"] = json!(NEW_HEAD),
                "duplicate" => {
                    let other = pr["commits"]["nodes"][0].clone();
                    pr["commits"]["nodes"].as_array_mut().unwrap().push(other);
                }
                "head-ref" => pr["headRefOid"] = json!(NEW_HEAD),
                "merge-oid" => pr["potentialMergeCommit"]["oid"] = json!(NEW_HEAD),
                "id" => pr["id"] = json!("replacement"),
                "number" => pr["number"] = json!(8),
                "repository" => pr["repository"]["nameWithOwner"] = json!("acme/other"),
                "zero" => r["validated_at_ms"] = json!(0),
                "future" => r["validated_at_ms"] = json!(u64::MAX),
                _ => unreachable!(),
            }
        });
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(!report.complete, "{case}");
        assert!(
            report
                .data
                .errors
                .iter()
                .all(|e| e.source.starts_with("check_runs:")),
            "{case}"
        );
        assert_eq!(
            report.data.errors.len(),
            if ["id", "number", "repository", "zero", "future"].contains(&case) {
                2
            } else {
                1
            },
            "{case}"
        );
        assert_eq!(h.calls().len(), before, "{case}");
    }
}

#[tokio::test]
async fn newer_rest_checks_defeat_empty_rollups() {
    let (h, c) = empty_checks_seed().await;
    edit_selector_cache(&h, |r| {
        r["validated_at_ms"] = json!(r["validated_at_ms"].as_u64().unwrap() - 1)
    });
    h.mode("ci-point-valid");
    for sha in [HEAD, MERGE] {
        c.get(
            &format!("repos/acme/demo/commits/{sha}/check-runs?filter=latest&per_page=100"),
            Freshness::Revalidate,
        )
        .await
        .unwrap();
    }
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(report.complete);
    assert!(!report.data.check_runs.is_empty());
    assert!(
        report
            .validations
            .iter()
            .all(|v| !v.resource.contains("#check-runs:"))
    );
    assert_eq!(h.calls().len(), before);
}

#[tokio::test]
async fn newer_nonempty_rollups_invalidate_empty_rest_and_disagreements_remain_errors() {
    for outcome in ["offline", "online", "disagree", "same-clock"] {
        let (h, c) = empty_checks_seed().await;
        for sha in [HEAD, MERGE] {
            c.get(
                &format!("repos/acme/demo/commits/{sha}/check-runs?filter=latest&per_page=100"),
                Freshness::Revalidate,
            )
            .await
            .unwrap();
        }
        // Deterministic ordering without advancing the evidence into the future.
        let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
        db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',1) WHERE key LIKE '%/commits/%/check-runs?%'", []).unwrap();
        edit_selector_cache(&h, |r| {
            if outcome == "same-clock" {
                db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/commits/%/check-runs?%'", [r["validated_at_ms"].as_u64().unwrap()]).unwrap();
            }
            let pr = &mut r["data"]["data"]["repository"]["pullRequest"];
            pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"] =
                json!({"contexts":{"checkRunCount":1}});
            pr["potentialMergeCommit"]["statusCheckRollup"] =
                json!({"contexts":{"checkRunCount":1}});
        });
        if outcome == "online" {
            h.mode("ci-point-valid");
        }
        let before = h.calls().len();
        let report = c
            .ci_for_pr(
                "acme/demo",
                7,
                if matches!(outcome, "offline" | "same-clock") {
                    Freshness::CachedOnly
                } else {
                    Freshness::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(report.complete, outcome == "online", "{outcome}");
        if matches!(outcome, "offline" | "same-clock") {
            assert_eq!(h.calls().len(), before);
        } else {
            assert_eq!(
                h.calls()[before..]
                    .iter()
                    .filter(|c| c.path.ends_with("/check-runs"))
                    .count(),
                2
            );
        }
        if outcome == "disagree" {
            assert!(
                report
                    .data
                    .errors
                    .iter()
                    .all(|e| e.message.contains("disagree"))
            );
        }
    }
}

#[tokio::test]
async fn existing_discovery_null_rollup_can_supply_head_checks_without_new_fields_or_queries() {
    let (h, c) = ci_selectors::seeded().await;
    h.mode("account-ci-selectors-empty-checks");
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute(
            "DELETE FROM cache WHERE key LIKE ?1",
            [format!("%/commits/{HEAD}/check-runs?%")],
        )
        .unwrap();
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    assert_eq!(h.calls().len(), before);
    assert_eq!(
        report
            .validations
            .iter()
            .filter(|v| v.resource.starts_with("my-open-prs://")
                && v.resource.ends_with(&format!("#check-runs:{HEAD}")))
            .count(),
        1
    );
}

#[tokio::test]
async fn empty_check_rollups_preserve_freshness_and_original_clocks() {
    for offline in [false, true] {
        let (h, c) = empty_checks_seed().await;
        edit_selector_cache(&h, |r| r["validated_at_ms"] = json!(1));
        c.pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        let before = h.calls().len();
        let report = c
            .ci_for_pr(
                "acme/demo",
                7,
                if offline {
                    Freshness::CachedOnly
                } else {
                    Freshness::default()
                },
            )
            .await
            .unwrap();
        assert!(report.complete);
        if offline {
            assert_eq!(h.calls().len(), before);
            assert_eq!(
                report
                    .validations
                    .iter()
                    .filter(|v| v.resource.contains("#check-runs:"))
                    .map(|v| v.validated_at_ms)
                    .collect::<Vec<_>>(),
                [1, 1]
            );
        } else {
            assert_eq!(
                h.calls()[before..]
                    .iter()
                    .filter(|c| c.path.ends_with("/check-runs"))
                    .count(),
                2
            );
            assert!(
                report
                    .validations
                    .iter()
                    .all(|v| !v.resource.contains("#check-runs:"))
            );
        }
    }
}

#[tokio::test]
async fn app_empty_check_rollups_use_installation_query_and_work_offline_without_minting() {
    let h = Harness::new().await;
    h.mode("account-ci-selectors");
    h.phase(2);
    let c = Client::with_token(ci_app_selectors::app_config(&h), "synthetic-token".into()).unwrap();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    ci_app_selectors::expire_metadata(&h);
    h.mode("ci-point-empty-checks");
    let before = h.calls().len();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    let queries = h.calls()[before..]
        .iter()
        .filter(|c| c.path == "/graphql")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].token, "Bearer synthetic-app-token");
    assert!(
        queries[0].body["query"]
            .as_str()
            .unwrap()
            .contains("checkRunCount")
    );
    drop(c);
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute(
            "DELETE FROM cache WHERE key LIKE '%/commits/%/check-runs?%'",
            [],
        )
        .unwrap();
    let c = Client::with_token(ci_app_selectors::app_config(&h), "synthetic-token".into()).unwrap();
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(report.complete);
    assert!(report.data.check_runs.is_empty());
    assert_eq!(h.calls().len(), before);
}
