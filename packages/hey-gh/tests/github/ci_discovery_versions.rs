use super::*;

fn forget_point_proof(h: &Harness) {
    rusqlite::Connection::open(h.config().cache_path).unwrap().execute(
        "DELETE FROM cache WHERE key LIKE '%/graphql#%' AND json_extract(response,'$.data.data.repository.pullRequest.number')=7", [],
    ).unwrap();
}

fn discovery_clock(h: &Harness) -> u64 {
    let raw: String = rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .query_row(
            "SELECT response FROM cache WHERE key='account-discovery-complete:v1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str::<Value>(&raw).unwrap()["data"]["validatedAtByPr"]["acme/demo/7"]
        .as_u64()
        .unwrap()
}

fn edit_discovery(h: &Harness, at: u64, edit: impl Fn(&mut Value)) {
    ci_discovery_status::edit_discovery(h, |r| {
        r["validated_at_ms"] = json!(at);
        if let Some(clocks) = r["data"]["validatedAtByPr"].as_object_mut() {
            for clock in clocks.values_mut() {
                *clock = json!(at);
            }
        }
        let nodes = if r["data"]["pulls"].is_array() {
            r["data"]["pulls"].as_array_mut().unwrap()
        } else {
            r["data"]["data"]["viewer"]["pullRequests"]["nodes"]
                .as_array_mut()
                .unwrap()
        };
        for node in nodes
            .iter_mut()
            .filter(|n| n["repository"]["nameWithOwner"] == "acme/demo")
        {
            edit(node);
        }
    });
}

#[tokio::test]
async fn discovery_status_versions_reuse_head_rest_without_extra_queries_or_auth_changes() {
    for app in [false, true] {
        let (h, c) = ci_status_versions::seeded(app).await;
        let at = ci_status_versions::proof_clock(&h);
        ci_status_versions::age_statuses(&h, at - 60_000);
        forget_point_proof(&h);
        h.mode("account-ci-selectors-status-versions");
        let discovery_start = h.calls().len();
        c.all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .unwrap();
        let discovery = h.calls()[discovery_start..].to_vec();
        assert!(
            discovery
                .iter()
                .all(|c| c.path == "/graphql" && c.token == "Bearer synthetic-token")
        );
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        let calls = h.calls()[before..].to_vec();
        assert!(
            !calls
                .iter()
                .any(|c| c.path.contains(HEAD) && c.path.ends_with("/status")),
            "{calls:?}"
        );
        assert!(!calls.iter().any(|c| c.path == "/graphql"), "{calls:?}");
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.path.contains(MERGE) && c.path.ends_with("/status"))
                .count(),
            1
        );
        assert!(report.validations.iter().any(|v| {
            v.resource.starts_with("my-open-prs://")
                && v.resource
                    .ends_with(&format!("#commit-status-versions:{HEAD}"))
        }));
        assert!(
            report
                .data
                .commit_statuses
                .iter()
                .all(|s| s["retained_extra"]["synthetic"] == "raw")
        );
        let query = discovery
            .iter()
            .find(|c| {
                c.body["query"]
                    .as_str()
                    .is_some_and(|q| q.contains("query MyOpenPullRequests("))
            })
            .unwrap()
            .body["query"]
            .as_str()
            .unwrap();
        assert!(query.contains("statusContextCount"));
        assert_eq!(
            query
                .matches("updatedAt context state description targetUrl")
                .count(),
            1,
            "only the head roster is expanded"
        );
    }
}

#[tokio::test]
async fn discovery_status_versions_newer_roster_overrides_an_older_point_proof() {
    for app in [false, true] {
        let (h, c) = ci_status_versions::seeded(app).await;
        let at = ci_status_versions::proof_clock(&h);
        ci_selectors::edit_selector_cache(&h, |r| r["validated_at_ms"] = json!(at - 1000));
        ci_status_versions::age_statuses(&h, at - 900);
        h.phase(3);
        h.mode("account-ci-selectors-status-versions");
        c.all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .unwrap();
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        assert_eq!(report.data.summary.state, "failure");
        let head = report
            .data
            .commit_statuses
            .iter()
            .find(|s| s["observed_sha"] == HEAD)
            .unwrap();
        assert_eq!(head["id"], 602);
        let calls = h.calls()[before..].to_vec();
        let statuses: Vec<_> = calls
            .iter()
            .filter(|c| c.path.ends_with("/status"))
            .collect();
        assert_eq!(statuses.len(), 1, "{calls:?}");
        assert!(statuses[0].path.contains(HEAD));
        assert_eq!(
            statuses[0].token,
            if app {
                "Bearer synthetic-app-token"
            } else {
                "Bearer synthetic-token"
            }
        );
    }
}

#[tokio::test]
async fn discovery_status_versions_preserve_offline_clocks_restart_and_forced_refresh() {
    for app in [false, true] {
        for refresh in [false, true] {
            let (h, c) = ci_status_versions::seeded(app).await;
            ci_status_versions::age_statuses(&h, 1);
            forget_point_proof(&h);
            h.mode("account-ci-selectors-status-versions");
            c.all_my_open_pull_requests(Freshness::Revalidate)
                .await
                .unwrap();
            let at = discovery_clock(&h);
            drop(c);
            let config = if app {
                ci_app_selectors::app_config(&h)
            } else {
                h.config()
            };
            let c = Client::with_token(config, "synthetic-token".into()).unwrap();
            let before = h.calls().len();
            let r = c
                .ci_for_pr(
                    "acme/demo",
                    7,
                    if refresh {
                        Freshness::Revalidate
                    } else {
                        Freshness::CachedOnly
                    },
                )
                .await
                .unwrap();
            assert!(r.complete, "{:?}", r.data.errors);
            if refresh {
                assert_eq!(
                    h.calls()[before..]
                        .iter()
                        .filter(|c| c.path.ends_with("/status"))
                        .count(),
                    2
                );
                assert!(
                    !r.validations
                        .iter()
                        .any(|v| v.resource.contains("#commit-status-versions:"))
                );
            } else {
                assert_eq!(h.calls().len(), before);
                assert!(r.validations.iter().any(|v| {
                    v.resource
                        .ends_with(&format!("#commit-status-versions:{HEAD}"))
                        && v.validated_at_ms == at
                }));
                assert!(
                    r.validations
                        .iter()
                        .any(|v| v.resource.contains(&format!("/{HEAD}/status?"))
                            && v.validated_at_ms == 1)
                );
            }
        }
    }
}

#[tokio::test]
async fn discovery_status_versions_select_proofs_by_clock_and_reject_conflicting_or_malformed_newer_rosters()
 {
    for case in [
        "older",
        "equal-conflict",
        "malformed-newer",
        "presence-only",
    ] {
        let (h, c) = ci_status_versions::seeded(false).await;
        h.mode("account-ci-selectors-status-versions");
        h.phase(3);
        c.all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .unwrap();
        let at = discovery_clock(&h);
        ci_status_versions::age_statuses(&h, at - 60_000);
        ci_selectors::edit_selector_cache(&h, |r| {
            r["validated_at_ms"] = json!(if case == "malformed-newer" || case == "presence-only" {
                at - 1000
            } else {
                at
            })
        });
        edit_discovery(&h, if case == "older" { at - 1000 } else { at }, |n| {
            let commit = &mut n["commits"]["nodes"][0]["commit"];
            if case == "malformed-newer" {
                commit["statusCheckRollup"]["contexts"]["statusContextCount"] = json!(2);
            }
            if case == "presence-only" {
                commit["status"] = json!({"id":"S_head"});
                commit["statusCheckRollup"]["contexts"]
                    .as_object_mut()
                    .unwrap()
                    .remove("statusContextCount");
            }
        });
        let before = h.calls().len();
        let r = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(r.complete, "{case}: {:?}", r.data.errors);
        let reuse = matches!(case, "older" | "presence-only");
        assert_eq!(
            h.calls()[before..]
                .iter()
                .filter(|c| c.path.contains(HEAD) && c.path.ends_with("/status"))
                .count(),
            usize::from(!reuse),
            "{case}"
        );
        assert_eq!(
            r.validations
                .iter()
                .filter(|v| v
                    .resource
                    .ends_with(&format!("#commit-status-versions:{HEAD}")))
                .count(),
            usize::from(reuse),
            "{case}"
        );
        if reuse {
            assert_eq!(
                r.data
                    .commit_statuses
                    .iter()
                    .find(|s| s["observed_sha"] == HEAD)
                    .unwrap()["state"],
                "success"
            );
        }
    }
}

#[tokio::test]
async fn discovery_status_versions_use_newer_partial_page_without_renewing_the_roster() {
    let (h, c) = ci_status_versions::seeded(false).await;
    let at = ci_status_versions::proof_clock(&h);
    ci_status_versions::age_statuses(&h, at - 60_000);
    forget_point_proof(&h);
    h.mode("account-ci-selectors-status-versions");
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute(r#"UPDATE cache SET response=json_set(response,'$.data.validatedAtByPr."acme/demo/7"',1) WHERE key='account-discovery-complete:v1'"#,[]).unwrap();
    let memo: String = db
        .query_row(
            "SELECT response FROM cache WHERE key='account-discovery-complete:v1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let before = h.calls().len();
    let r = c
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(r.complete, "{:?}", r.data.errors);
    assert!(
        !h.calls()[before..]
            .iter()
            .any(|c| c.path.contains(HEAD) && c.path.ends_with("/status"))
    );
    assert!(r.validations.iter().any(|v| {
        v.resource
            .ends_with(&format!("#commit-status-versions:{HEAD}"))
            && v.validated_at_ms > 1
    }));
    assert_eq!(
        db.query_row(
            "SELECT response FROM cache WHERE key='account-discovery-complete:v1'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        memo
    );
}

#[tokio::test]
async fn discovery_status_versions_query_upgrade_preserves_legacy_offline_scan_and_page_hints() {
    use sha2::Digest;
    for (app, previous) in [(false, false), (false, true), (true, false), (true, true)] {
        let (h, c) = ci_status_versions::seeded(app).await;
        forget_point_proof(&h);
        h.mode("account-ci-selectors-empty");
        let before = h.calls().len();
        c.all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .unwrap();
        let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
        for call in h.calls()[before..].iter().filter(|c| {
            c.body["query"]
                .as_str()
                .is_some_and(|q| q.contains("query MyOpenPullRequests("))
        }) {
            let mut body = call.body.clone();
            let current = format!("{:x}", sha2::Sha256::digest(body.to_string().as_bytes()));
            let query = body["query"]
                .as_str()
                .unwrap()
                .replace(
                    " statusCheckRollup { contexts(first: 1) { checkRunCount } }",
                    "",
                )
                .replace("checkRunCount statusContextCount", "statusContextCount");
            body["query"] = json!(if previous {
                query
            } else {
                query
                    .replace(
                        " contexts { id updatedAt context state description targetUrl }",
                        "",
                    )
                    .replace(" contexts(first: 1) { statusContextCount }", "")
            });
            let legacy = format!("{:x}", sha2::Sha256::digest(body.to_string().as_bytes()));
            assert_ne!(current, legacy);
            assert_eq!(
                db.execute(
                    "UPDATE cache SET key=replace(key,?1,?2) WHERE key LIKE ?3",
                    [&current, &legacy, &format!("%/graphql#{current}%")]
                )
                .unwrap(),
                1
            );
        }
        let (scope, memo): (String, String) = db
            .query_row(
                "SELECT scope,response FROM cache WHERE key='account-discovery-complete:v1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        db.execute(
            "DELETE FROM cache WHERE key='account-discovery-complete:v1'",
            [],
        )
        .unwrap();
        let before = h.calls().len();
        assert_eq!(
            c.all_my_open_pull_requests(Freshness::CachedOnly)
                .await
                .unwrap()
                .len(),
            2
        );
        db.execute(
            "INSERT INTO cache(scope,key,response) VALUES (?1,'account-discovery-complete:v1',?2)",
            [scope, memo],
        )
        .unwrap();
        db.execute(r#"UPDATE cache SET response=json_set(response,'$.data.validatedAtByPr."acme/demo/7"',1,'$.data.pulls[0].commits.nodes[0].commit.status',json('{"id":"old"}')) WHERE key='account-discovery-complete:v1'"#,[]).unwrap();
        db.execute(
            "DELETE FROM cache WHERE key LIKE '%/commits/%/status?%'",
            [],
        )
        .unwrap();
        drop(c);
        let config = if app {
            ci_app_selectors::app_config(&h)
        } else {
            h.config()
        };
        let c = Client::with_token(config, "synthetic-token".into()).unwrap();
        let r = c
            .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(r.complete, "{:?}", r.data.errors);
        assert!(r.data.commit_statuses.is_empty());
        assert!(r.validations.iter().any(|v| {
            v.resource.ends_with(&format!("#commit-statuses:{HEAD}")) && v.validated_at_ms > 1
        }));
        assert_eq!(h.calls().len(), before);
    }
}
