use super::*;
use ci_selectors::edit_selector_cache;

fn edit_status_cache(h: &Harness, sha: &str, edit: impl FnOnce(&mut Value)) {
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let (key, raw): (String, String) = db
        .query_row(
            "SELECT key,response FROM cache WHERE key LIKE ?1",
            [format!("%/commits/{sha}/status?per_page=100%")],
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

pub(super) fn proof_clock(h: &Harness) -> u64 {
    let mut at = 0;
    edit_selector_cache(h, |r| at = r["validated_at_ms"].as_u64().unwrap());
    at
}

pub(super) fn age_statuses(h: &Harness, at: u64) {
    for sha in [HEAD, MERGE] {
        edit_status_cache(h, sha, |r| r["validated_at_ms"] = json!(at));
    }
}

fn version_count(report: &hey_gh::CiObservation) -> usize {
    report
        .validations
        .iter()
        .filter(|v| v.resource.contains("#commit-status-versions:"))
        .count()
}

pub(super) async fn seeded(app: bool) -> (Harness, Client) {
    let h = Harness::new().await;
    h.mode("ci-point-status-versions");
    h.phase(2);
    let config = if app {
        ci_app_selectors::app_config(&h)
    } else {
        h.config()
    };
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    ci_app_selectors::expire_metadata(&h);
    // Metadata and CI collect concurrently. Populate the existing selector
    // proof before asserting reuse; the shortcut must not wait on a query.
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    (h, c)
}

#[tokio::test]
async fn unchanged_status_versions_reuse_stale_rest_payloads_without_extra_queries() {
    for app in [false, true] {
        let (h, c) = seeded(app).await;
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 60_000;
        rusqlite::Connection::open(h.config().cache_path).unwrap().execute(
            "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/commits/%/status?%'", [at],
        ).unwrap();
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        assert_eq!(report.data.commit_statuses.len(), 2);
        assert!(
            report
                .data
                .commit_statuses
                .iter()
                .all(|s| s["retained_extra"]["synthetic"] == "raw")
        );
        let calls = h.calls()[before..].to_vec();
        assert_eq!(
            calls.iter().filter(|c| c.path.ends_with("/status")).count(),
            0
        );
        assert_eq!(calls.iter().filter(|c| c.path == "/graphql").count(), 0);
        assert_eq!(
            report
                .validations
                .iter()
                .filter(|v| v.resource.contains("#commit-status-versions:"))
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn newer_status_version_invalidates_a_fresh_but_superseded_rest_list() {
    let (h, c) = seeded(false).await;
    h.phase(3);
    let body = h
        .calls()
        .into_iter()
        .find(|c| c.path == "/graphql")
        .unwrap()
        .body;
    c.graphql(
        body["query"].as_str().unwrap(),
        body["variables"].clone(),
        Freshness::Revalidate,
    )
    .await
    .unwrap();
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    assert_eq!(report.data.summary.state, "failure");
    assert!(
        report
            .data
            .commit_statuses
            .iter()
            .all(|s| s["state"] == "failure")
    );
    assert_eq!(
        h.calls()[before..]
            .iter()
            .filter(|c| c.path.ends_with("/status"))
            .count(),
        2
    );
}

#[tokio::test]
async fn status_versions_preserve_refresh_offline_restart_and_provider_routes() {
    for app in [false, true] {
        for refresh in [false, true] {
            let (h, c) = seeded(app).await;
            age_statuses(&h, 1);
            drop(c);
            let config = if app {
                ci_app_selectors::app_config(&h)
            } else {
                h.config()
            };
            let c = Client::with_token(config, "synthetic-token".into()).unwrap();
            let before = h.calls().len();
            let report = c
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
            assert!(report.complete, "{:?}", report.data.errors);
            if refresh {
                assert_eq!(version_count(&report), 0);
                let calls = h.calls()[before..].to_vec();
                let statuses: Vec<_> = calls
                    .iter()
                    .filter(|c| c.path.ends_with("/status"))
                    .collect();
                assert_eq!(statuses.len(), 2);
                assert!(statuses.iter().all(|c| c.token
                    == if app {
                        "Bearer synthetic-app-token"
                    } else {
                        "Bearer synthetic-token"
                    }));
            } else {
                assert_eq!(h.calls().len(), before);
                assert_eq!(version_count(&report), 2);
                assert_eq!(
                    report
                        .validations
                        .iter()
                        .filter(|v| v.resource.contains("/status?") && v.validated_at_ms == 1)
                        .count(),
                    2
                );
                assert!(report.oldest_validation_at_ms <= 1);
            }
        }
    }
}

#[tokio::test]
async fn malformed_status_versions_never_certify_stale_rest() {
    for case in [
        "count",
        "missing-count",
        "empty",
        "duplicate",
        "id",
        "state",
        "time",
        "missing-description",
        "bad-url",
        "head-sha",
        "head-roster",
        "rest-sha",
        "stale-proof",
        "future-proof",
    ] {
        let (h, c) = seeded(false).await;
        let at = proof_clock(&h);
        age_statuses(&h, at - 60_000);
        edit_selector_cache(&h, |r| {
            if case == "stale-proof" {
                r["validated_at_ms"] = json!(1);
            }
            if case == "future-proof" {
                r["validated_at_ms"] = json!(at + 60_000);
            }
            let pr = &mut r["data"]["data"]["repository"]["pullRequest"];
            if case == "head-roster" {
                let duplicate = pr["commits"]["nodes"][0].clone();
                pr["commits"]["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            for path in ["/commits/nodes/0/commit", "/potentialMergeCommit"] {
                let commit = pr.pointer_mut(path).unwrap();
                match case {
                    "count" => {
                        commit["statusCheckRollup"]["contexts"]["statusContextCount"] = json!(2)
                    }
                    "missing-count" => {
                        commit["statusCheckRollup"]["contexts"]
                            .as_object_mut()
                            .unwrap()
                            .remove("statusContextCount");
                    }
                    "empty" => commit["status"]["contexts"] = json!([]),
                    "duplicate" => {
                        let duplicate = commit["status"]["contexts"][0].clone();
                        commit["status"]["contexts"]
                            .as_array_mut()
                            .unwrap()
                            .push(duplicate);
                        commit["statusCheckRollup"]["contexts"]["statusContextCount"] = json!(2);
                    }
                    "id" => commit["status"]["contexts"][0]["id"] = json!(""),
                    "state" => commit["status"]["contexts"][0]["state"] = json!("NEUTRAL"),
                    "time" => commit["status"]["contexts"][0]["updatedAt"] = json!("invalid"),
                    "missing-description" => {
                        commit["status"]["contexts"][0]
                            .as_object_mut()
                            .unwrap()
                            .remove("description");
                    }
                    "bad-url" => commit["status"]["contexts"][0]["targetUrl"] = json!(false),
                    "head-sha" => commit["oid"] = json!(NEW_HEAD),
                    _ => {}
                }
            }
        });
        if case == "rest-sha" {
            for sha in [HEAD, MERGE] {
                edit_status_cache(&h, sha, |r| r["data"]["sha"] = json!(NEW_HEAD));
            }
        }
        // Keep metadata fresh separately so invalid proofs do not trigger a
        // replacement selector query during this fallback test.
        c.pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(report.complete, "{case}: {:?}", report.data.errors);
        let expected = if case == "head-roster" { 1 } else { 0 };
        assert_eq!(version_count(&report), expected, "{case}");
        assert_eq!(
            h.calls()[before..]
                .iter()
                .filter(|c| c.path.ends_with("/status"))
                .count(),
            2 - expected,
            "{case}"
        );
    }
}

#[tokio::test]
async fn status_version_mutations_invalidate_fresh_cached_payloads_offline() {
    for case in [
        "node",
        "numeric",
        "time",
        "time-submillis",
        "state",
        "context",
        "description",
        "url",
        "removed",
        "added",
    ] {
        let (h, c) = seeded(false).await;
        let at = proof_clock(&h);
        age_statuses(&h, at - 1000);
        edit_status_cache(&h, HEAD, |r| {
            let status = &mut r["data"]["statuses"][0];
            match case {
                "node" => status["node_id"] = json!("old-id"),
                "numeric" => status["id"] = json!(0),
                "time" => status["updated_at"] = json!("2026-09-19T00:00:01Z"),
                "time-submillis" => status["updated_at"] = json!("2026-09-19T00:00:00.000001Z"),
                "state" => status["state"] = json!("pending"),
                "context" => status["context"] = json!("different"),
                "description" => status["description"] = json!("different"),
                "url" => status["target_url"] = Value::Null,
                "removed" => r["data"]["statuses"] = json!([]),
                "added" => {
                    let duplicate = status.clone();
                    r["data"]["statuses"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                _ => unreachable!(),
            }
        });
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(!report.complete, "{case}");
        assert_eq!(h.calls().len(), before, "{case}");
        assert_eq!(version_count(&report), 1, "{case}");
        assert!(
            report
                .data
                .errors
                .iter()
                .any(|e| e.source.starts_with("commit_statuses:")),
            "{case}: {:?}",
            report.data.errors
        );
    }
}

#[tokio::test]
async fn newer_rest_and_maximum_raw_payload_age_defeat_status_versions() {
    for case in ["newer-rest", "same-clock", "old-raw"] {
        let (h, c) = seeded(false).await;
        let at = proof_clock(&h);
        let rest_at = match case {
            "newer-rest" => at + 1,
            "same-clock" => at,
            _ => 1,
        };
        age_statuses(&h, rest_at);
        if case == "newer-rest" {
            for sha in [HEAD, MERGE] {
                edit_status_cache(&h, sha, |r| {
                    r["data"]["statuses"][0]["state"] = json!("failure")
                });
            }
        }
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(report.complete, "{case}: {:?}", report.data.errors);
        assert_eq!(
            version_count(&report),
            if case == "same-clock" { 2 } else { 0 },
            "{case}"
        );
        assert_eq!(
            h.calls()[before..]
                .iter()
                .filter(|c| c.path.ends_with("/status"))
                .count(),
            if case == "old-raw" { 2 } else { 0 },
            "{case}"
        );
        if case == "newer-rest" {
            assert_eq!(report.data.summary.state, "failure");
        }
    }
}

fn second_status_page(h: &Harness, at: u64, case: &str) {
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let (scope, key, raw): (String, String, String) = db
        .query_row(
            "SELECT scope,key,response FROM cache WHERE key LIKE ?1",
            [format!("%/commits/{HEAD}/status?per_page=100%")],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let mut first: Value = serde_json::from_str(&raw).unwrap();
    let mut second = first.clone();
    first["data"]["total_count"] = json!(2);
    first["link"] = json!(format!(
        "<{}repos/acme/demo/commits/{HEAD}/status?per_page=100&page=2>; rel=\"next\"",
        h.url
    ));
    second["data"]["total_count"] = json!(2);
    second["data"]["statuses"][0]["id"] = json!(602);
    second["data"]["statuses"][0]["node_id"] = json!("SC_head_extra");
    second["data"]["statuses"][0]["context"] = json!("extra");
    if case == "newer-page" {
        second["validated_at_ms"] = json!(at + 1);
        second["data"]["statuses"][0]["state"] = json!("failure");
    }
    if case == "zero-clock" {
        second["validated_at_ms"] = json!(0);
    }
    if case == "duplicate-node" {
        second["data"]["statuses"][0]["node_id"] = json!("SC_head_1");
    }
    if case == "duplicate-id" {
        second["data"]["statuses"][0]["id"] = json!(601);
    }
    if case == "byte-limit" {
        second["data"]["statuses"][0]["padding"] = json!("x".repeat(8000));
    }
    if case == "cycle" {
        second["link"] = first["link"].clone();
    }
    db.execute(
        "UPDATE cache SET response=?1 WHERE scope=?2 AND key=?3",
        [first.to_string(), scope.clone(), key.clone()],
    )
    .unwrap();
    if case != "missing-page" {
        db.execute(
            "INSERT INTO cache(scope,key,response) VALUES (?1,?2,?3)",
            [
                scope,
                key.replace("?per_page=100", "?per_page=100&page=2"),
                second.to_string(),
            ],
        )
        .unwrap();
    }
    edit_selector_cache(h, |r| {
        let commit =
            &mut r["data"]["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"];
        let first = commit["status"]["contexts"][0].clone();
        let mut extra = first.clone();
        extra["id"] = json!("SC_head_extra");
        extra["context"] = json!("extra");
        // Proving a roster must not depend on its order or reorder raw REST.
        commit["status"]["contexts"] = json!([extra, first]);
        commit["statusCheckRollup"]["contexts"]["statusContextCount"] = json!(2);
    });
}

#[tokio::test]
async fn status_version_reuse_requires_complete_bounded_pages_and_every_page_clock() {
    for case in [
        "matching",
        "missing-page",
        "newer-page",
        "zero-clock",
        "duplicate-node",
        "duplicate-id",
        "byte-limit",
        "cycle",
    ] {
        let (h, c) = seeded(false).await;
        let at = proof_clock(&h);
        age_statuses(&h, at - 60_000);
        second_status_page(&h, at, case);
        drop(c);
        let c = Client::with_token(
            Config {
                max_collection_bytes: 4096,
                ..h.config()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let before = h.calls().len();
        let report = c
            .ci_for_pr(
                "acme/demo",
                7,
                if case == "matching" {
                    Freshness::default()
                } else {
                    Freshness::CachedOnly
                },
            )
            .await
            .unwrap();
        assert_eq!(h.calls().len(), before, "{case}");
        assert_eq!(
            report.complete,
            matches!(case, "matching" | "newer-page" | "zero-clock"),
            "{case}: {:?}",
            report.data.errors
        );
        assert_eq!(
            version_count(&report),
            if case == "matching" { 2 } else { 1 },
            "{case}"
        );
        if case == "matching" {
            let head: Vec<_> = report
                .data
                .commit_statuses
                .iter()
                .filter(|s| s["observed_sha"] == HEAD)
                .map(|s| s["context"].as_str().unwrap())
                .collect();
            assert_eq!(head, ["deploy", "extra"]);
            assert!(
                !report
                    .validations
                    .iter()
                    .any(|v| v.resource.contains("/status?")),
                "unused REST page clocks must not be published"
            );
        }
        if case == "newer-page" {
            assert_eq!(report.data.summary.state, "failure");
        }
        if case == "byte-limit" {
            assert!(
                report
                    .data
                    .errors
                    .iter()
                    .any(|e| e.message.contains("byte limit"))
            );
        }
        if case == "cycle" {
            assert!(
                report
                    .data
                    .errors
                    .iter()
                    .any(|e| e.message.contains("cycle"))
            );
        }
    }
}

#[tokio::test]
async fn newer_discovery_can_remove_statuses_but_cannot_renew_version_proofs() {
    for case in ["absent", "present", "expired-proof"] {
        let (h, c) = seeded(false).await;
        let at = proof_clock(&h)
            - if case == "expired-proof" {
                60_000
            } else {
                1000
            };
        edit_selector_cache(&h, |r| r["validated_at_ms"] = json!(at));
        age_statuses(&h, at - 60_000);
        h.mode(if case == "absent" {
            "account-ci-selectors-empty"
        } else {
            "account-ci-selectors-nonempty"
        });
        c.all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .unwrap();
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(report.complete, "{case}: {:?}", report.data.errors);
        assert_eq!(
            version_count(&report),
            if case == "present" { 2 } else { 0 },
            "{case}"
        );
        if case == "absent" {
            assert!(report.data.commit_statuses.is_empty());
        }
        if case == "present" {
            assert!(
                report
                    .validations
                    .iter()
                    .filter(|v| v.resource.contains("#commit-status-versions:"))
                    .all(|v| v.validated_at_ms == at)
            );
        }
        assert_eq!(
            h.calls()[before..]
                .iter()
                .filter(|c| c.path.ends_with("/status"))
                .count(),
            if case == "expired-proof" { 2 } else { 0 },
            "{case}"
        );
    }
}
