use super::ci_app_selectors::app_config;
use super::ci_app_selectors::expire_metadata;
use super::*;

#[tokio::test]
async fn personal_report_seed_overlaps_nonselector_app_changes_with_final_validation() {
    for change in ["mergeability", "updated", "head", "lifecycle"] {
        let overlaps = matches!(change, "mergeability" | "updated");
        let h = Harness::new().await;
        h.phase(2);
        h.mode("account");
        let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
        let warm = c
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert!(warm.complete);
        let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
        db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',0,'$.data.title','unvalidated personal seed','$.etag','expired-seed','$.last_modified',null) WHERE key LIKE '%/pulls/7' OR key LIKE '%/issues/7/comments%'", []).unwrap();
        let (scope, key, raw): (String, String, String) = db.query_row(
            "SELECT scope,key,response FROM cache WHERE key LIKE '%/pulls/7#installation-ci-pr'", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        let mut app: Value = serde_json::from_str(&raw).unwrap();
        app["validated_at_ms"] = json!(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        );
        app["etag"] = json!("expired-app");
        app["last_modified"] = Value::Null;
        if change == "mergeability" {
            app["data"]["mergeable"] = Value::Null;
            app["data"]["mergeable_state"] = json!("unknown");
        } else if change == "updated" {
            app["data"]["updated_at"] = json!("2026-10-06T00:00:00Z");
        } else if change == "head" {
            app["data"]["head"]["sha"] = json!(NEW_HEAD);
        } else {
            app["data"]["state"] = json!("closed");
        }
        db.execute(
            "UPDATE cache SET response=?1 WHERE scope=?2 AND key=?3",
            rusqlite::params![app.to_string(), scope, key],
        )
        .unwrap();
        // A collection seed must not become a usable generic cached response.
        assert!(matches!(
            c.pull_request("acme/demo", 7, Freshness::CachedOnly).await,
            Err(Error::CacheMiss)
        ));
        h.mode("issue72-stall-metadata");
        let before = h.calls().len();
        let reader = c.clone();
        let read = tokio::spawn(async move {
            reader
                .pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
                .await
        });
        let started = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if h.calls()[before..]
                    .iter()
                    .any(|c| c.path.ends_with("/issues/7/comments"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        let pending = !read.is_finished();
        h.mode("account");
        h.mock.release.notify_waiters();
        assert_eq!(
            started.is_ok(),
            overlaps,
            "{change}: only nonselector changes may overlap personal validation"
        );
        assert!(pending, "{change}: a seed cannot bypass final validation");
        let report = tokio::time::timeout(Duration::from_secs(3), read)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(report.complete, "{change}: {:?}", report.data.errors);
        assert_ne!(
            report.data.pull_request["title"],
            "unvalidated personal seed"
        );
        assert!(report.validations.iter().all(|v| v.validated_at_ms > 0));
        let calls = h.calls();
        assert!(
            calls[before..]
                .iter()
                .any(|c| c.path.ends_with("/pulls/7") && c.token == "Bearer synthetic-token")
        );
        assert!(
            calls[before..]
                .iter()
                .filter(|c| c.path.ends_with("/comments")
                    || c.path.ends_with("/reviews")
                    || c.path.ends_with("/timeline"))
                .all(|c| c.token == "Bearer synthetic-token")
        );
    }
}

#[tokio::test]
async fn ci_rest_cold_metadata_uses_app_despite_personal_quota_exhaustion() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("ci-rest-personal-quota");
    let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    assert!(matches!(
        c.pull_request("acme/demo", 7, Freshness::Revalidate).await,
        Err(Error::RateLimited { .. })
    ));
    let before = h.calls().len();
    let ci = c
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(ci.complete, "{:?}", ci.data.errors);
    assert!(
        h.calls()[before..]
            .iter()
            .filter(|c| c.path.ends_with("/pulls/7"))
            .all(|c| c.token == "Bearer synthetic-app-token")
    );
    assert!(c.status().rate_limits.contains_key("core"));
    assert!(matches!(
        c.pull_request("acme/demo", 7, Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
}

#[tokio::test]
async fn ci_rest_refresh_and_ambiguous_mergeability_use_app_metadata() {
    for phase in [2, 3, 5] {
        let h = Harness::new().await;
        h.phase(phase);
        h.mode("ci-rest-valid");
        let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
        for policy in [Freshness::Revalidate, Freshness::MaxAge(Duration::ZERO)] {
            let before = h.calls().len();
            assert!(c.ci_for_pr("acme/demo", 7, policy).await.unwrap().complete);
            let reads = h.calls()[before..].to_vec();
            let metadata: Vec<_> = reads
                .iter()
                .filter(|c| c.path.ends_with("/pulls/7"))
                .collect();
            assert!(!metadata.is_empty());
            assert!(
                metadata
                    .iter()
                    .all(|c| c.token == "Bearer synthetic-app-token"),
                "phase {phase}: {metadata:?}"
            );
        }
    }
}

#[tokio::test]
async fn ci_rest_app_cache_does_not_satisfy_generic_personal_metadata() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("ci-rest-valid");
    let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    assert!(matches!(
        c.pull_request("acme/demo", 7, Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    let before = h.calls().len();
    c.pull_request("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(h.calls().len(), before + 1);
    assert_eq!(h.calls().last().unwrap().token, "Bearer synthetic-token");
}

#[tokio::test]
async fn ci_rest_stale_conflicting_or_unknown_metadata_uses_app_without_selectors() {
    for phase in [3, 5] {
        let h = Harness::new().await;
        h.phase(phase);
        h.mode("ci-rest-valid");
        let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        expire_metadata(&h);
        let before = h.calls().len();
        assert!(
            c.ci_for_pr("acme/demo", 7, Freshness::default())
                .await
                .unwrap()
                .complete
        );
        let reads = h.calls()[before..].to_vec();
        assert!(!reads.is_empty());
        assert!(
            reads
                .iter()
                .all(|c| c.path.ends_with("/pulls/7") && c.token == "Bearer synthetic-app-token"),
            "{reads:?}"
        );
    }
}

#[tokio::test]
async fn ci_rest_changed_selectors_revalidate_with_app() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("ci-rest-valid");
    let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    expire_metadata(&h);
    h.mode("ci-point-base");
    let before = h.calls().len();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    let reads = h.calls()[before..].to_vec();
    assert!(reads.iter().any(|c| c.path == "/graphql"));
    assert!(reads.iter().any(|c| c.path.ends_with("/pulls/7")));
    assert!(
        reads
            .iter()
            .all(|c| c.token == "Bearer synthetic-app-token"),
        "{reads:?}"
    );
}

#[tokio::test]
async fn ci_rest_fresh_personal_metadata_can_seed_ci_without_another_metadata_request() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("ci-rest-valid");
    let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    let personal = c
        .pull_request("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let before = h.calls().len();
    let ci = c
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(ci.complete);
    assert!(
        ci.validations
            .iter()
            .any(|v| v.resource.ends_with("/pulls/7")
                && v.validated_at_ms == personal.validated_at_ms
                && matches!(v.source, Source::Cache))
    );
    assert!(
        h.calls()[before..]
            .iter()
            .all(|c| !c.path.ends_with("/pulls/7"))
    );
}

#[tokio::test]
async fn ci_rest_newest_provider_preserves_full_closed_payload_offline_after_restart() {
    for latest_app in [false, true] {
        let h = Harness::new().await;
        h.phase(2);
        h.mode("ci-rest-valid");
        let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        c.pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
        let recent = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 1;
        for app in [false, true] {
            let pattern = if app {
                "%/pulls/7#installation-ci-pr"
            } else {
                "%/pulls/7"
            };
            let latest = app == latest_app;
            assert_eq!(db.execute(
                "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1,'$.data.state',?2,'$.data.extra',json(?3)) WHERE key LIKE ?4",
                rusqlite::params![if latest {recent} else {0}, if latest {"closed"} else {"open"}, json!({"raw":{"retained":latest}}).to_string(), pattern],
            ).unwrap(), 1);
        }
        let before = h.calls().len();
        let restarted = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
        for freshness in [Freshness::CachedOnly, Freshness::default()] {
            let ci = restarted
                .ci_for_pr("acme/demo", 7, freshness)
                .await
                .unwrap();
            assert!(ci.complete, "{:?}", ci.data.errors);
            assert!(
                ci.validations
                    .iter()
                    .filter(|v| v.resource.ends_with("/pulls/7"))
                    .all(|v| v.validated_at_ms == recent)
            );
            let raw: String = db
                .query_row(
                    "SELECT data FROM snapshots WHERE resource LIKE 'metadata://%/acme/demo/7'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let data: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(data["pull_request"]["state"], "closed");
            assert_eq!(
                data["pull_request"]["extra"],
                json!({"raw":{"retained":true}})
            );
        }
        assert_eq!(
            h.calls().len(),
            before,
            "cached reads must not mint or fetch"
        );
    }
}

#[tokio::test]
async fn ci_rest_app_errors_do_not_fallback_even_with_a_stale_personal_cache() {
    for mode in ["ci-rest-denied", "ci-rest-app-quota"] {
        let h = Harness::new().await;
        h.phase(3);
        h.mode("ci-rest-valid");
        let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
        c.pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        expire_metadata(&h);
        h.mode(mode);
        let before = h.calls().len();
        assert!(
            c.ci_for_pr("acme/demo", 7, Freshness::default())
                .await
                .is_err()
        );
        let reads = h.calls()[before..].to_vec();
        assert!(reads.iter().any(|c| c.path.ends_with("/pulls/7")));
        assert!(
            reads
                .iter()
                .filter(|c| c.path.starts_with("/repos/"))
                .all(|c| c.token == "Bearer synthetic-app-token")
        );
        h.mode("ci-rest-valid");
        c.pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert_eq!(h.calls().last().unwrap().token, "Bearer synthetic-token");
    }
}

#[tokio::test]
async fn ci_rest_renews_app_tokens_and_mint_failures_remain_errors() {
    for mode in ["ci-rest-renew", "ci-point-mint-denied"] {
        let h = Harness::new().await;
        h.phase(2);
        h.mode(mode);
        let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
        let result = c.ci_for_pr("acme/demo", 7, Freshness::default()).await;
        if mode == "ci-rest-renew" {
            assert!(result.unwrap().complete);
            assert_eq!(
                h.calls()
                    .iter()
                    .filter(|c| c.path.contains("access_tokens"))
                    .count(),
                2
            );
        } else {
            assert!(result.is_err());
            assert!(h.calls().iter().all(|c| c.path.contains("access_tokens")));
        }
        assert!(
            h.calls()
                .iter()
                .filter(|c| c.path.starts_with("/repos/"))
                .all(|c| c.token == "Bearer synthetic-app-token")
        );
    }
}

#[tokio::test]
async fn ci_rest_app_and_personal_inflight_requests_are_isolated() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("ci-rest-stalled");
    let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    let ci_client = c.clone();
    let ci = tokio::spawn(async move {
        ci_client
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
    });
    let personal_client = c.clone();
    let personal = tokio::spawn(async move {
        personal_client
            .pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while c.status().outstanding_requests != 2
            || !h.calls().iter().any(|c| c.path.ends_with("/pulls/7"))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("provider-specific in-flight requests");
    h.mock.release.notify_waiters();
    tokio::time::timeout(Duration::from_secs(2), async {
        while h
            .calls()
            .iter()
            .filter(|c| c.path.ends_with("/pulls/7"))
            .count()
            < 2
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    h.mock.release.notify_waiters();
    assert!(ci.await.unwrap().unwrap().complete);
    personal.await.unwrap().unwrap();
    let reads = h.calls();
    for token in ["Bearer synthetic-token", "Bearer synthetic-app-token"] {
        assert!(
            reads
                .iter()
                .any(|c| c.path.ends_with("/pulls/7") && c.token == token)
        );
    }
}

#[tokio::test]
async fn ci_rest_deadline_does_not_start_personal_fallback() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("ci-rest-stalled");
    let c = Client::with_token(
        Config {
            report_timeout: Duration::from_millis(150),
            ..app_config(&h)
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        c.ci_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(Error::Deadline)), "{result:?}");
    let reads = h.calls();
    assert!(reads.iter().any(|c| c.path.ends_with("/pulls/7")));
    assert!(
        reads
            .iter()
            .filter(|c| c.path.starts_with("/repos/"))
            .all(|c| c.token == "Bearer synthetic-app-token")
    );
}

#[tokio::test]
async fn ci_rest_retired_identity_cannot_reuse_either_provider_cache() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("account-ci-selectors");
    let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    c.pull_request("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    h.mode("account-new-identity");
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    let before = h.calls().len();
    assert!(matches!(
        c.ci_for_pr("acme/demo", 7, Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    assert!(matches!(
        c.pull_request("acme/demo", 7, Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    assert_eq!(h.calls().len(), before);
    assert!(
        c.ci_for_pr("ACME/DEMO", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    assert!(
        h.calls()[before..]
            .iter()
            .filter(|c| c.path.ends_with("/pulls/7"))
            .all(|c| c.token == "Bearer synthetic-app-token")
    );
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let raw: String = db
        .query_row(
            "SELECT data FROM snapshots WHERE resource LIKE 'metadata://%/acme/demo/7'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let data: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(data["pull_request"]["node_id"], "PR_acme/demo_new_7");
}

#[tokio::test]
async fn ci_rest_repository_generation_invalidates_inflight_metadata() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("ci-rest-valid");
    let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    h.mode("ci-rest-stalled");
    let before = h.calls().len();
    let slow_client = c.clone();
    let slow = tokio::spawn(async move {
        slow_client
            .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !h.calls()[before..]
            .iter()
            .any(|c| c.path.ends_with("/pulls/7"))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute("INSERT INTO repository_generation(scope,repository,generation) SELECT DISTINCT scope,'acme/demo',1 FROM cache WHERE true ON CONFLICT(scope,repository) DO UPDATE SET generation=generation+1", []).unwrap();
    h.mode("ci-rest-valid");
    h.mock.release.notify_waiters();
    let result = tokio::time::timeout(Duration::from_secs(2), slow)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(Error::Invalid(ref message)) if message == "PR entity changed while collecting evidence"),
        "{result:?}"
    );
    let after = h.calls().len();
    assert!(matches!(
        c.ci_for_pr("acme/demo", 7, Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    assert_eq!(h.calls().len(), after);
}

#[tokio::test]
async fn ci_rest_newer_app_metadata_cannot_be_undone_by_a_personal_full_report() {
    for mode in ["account-head-change", "account-state-version"] {
        let h = Harness::new().await;
        h.phase(2);
        h.mode("account");
        let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
        let initial = c
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert!(initial.complete);
        tokio::time::sleep(Duration::from_millis(2)).await;
        h.mode(mode);
        assert!(
            c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
                .await
                .unwrap()
                .complete
        );
        let before = h.calls().len();
        let report = c
            .pr_report("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(report.complete);
        if mode == "account-head-change" {
            assert_eq!(report.data.pull_request["head"]["sha"], NEW_HEAD);
        } else {
            assert_eq!(report.data.pull_request["state"], "closed");
        }
        let reads = h.calls()[before..].to_vec();
        let metadata: Vec<_> = reads
            .iter()
            .filter(|c| c.path.ends_with("/pulls/7"))
            .collect();
        assert!(
            !metadata.is_empty(),
            "outdated personal payload must be validated"
        );
        assert!(metadata.iter().all(|c| c.token == "Bearer synthetic-token"));
    }
}

#[tokio::test]
async fn ci_rest_newer_lifecycle_makes_old_personal_cache_unavailable_without_network() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("account");
    let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    assert!(
        c.pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    tokio::time::sleep(Duration::from_millis(2)).await;
    h.mode("account-state-version");
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let before = h.calls().len();
    assert!(matches!(
        c.pull_request("acme/demo", 7, Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    assert!(matches!(
        c.pr_report("acme/demo", 7, Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    assert_eq!(h.calls().len(), before);
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let raw: String = db
        .query_row(
            "SELECT data FROM snapshots WHERE resource LIKE 'metadata://%/acme/demo/7'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let data: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(data["pull_request"]["state"], "closed");
}
