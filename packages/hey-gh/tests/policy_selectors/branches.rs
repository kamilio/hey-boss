use super::*;

const NEXT: &str = "dddddddddddddddddddddddddddddddddddddddd";

async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

fn branch_graph() -> Value {
    json!({"data":{"repository":{"id":"R_demo","nameWithOwner":"acme/demo","ref":{
        "id":"REF_main","name":"main","prefix":"refs/heads/",
        "target":{"__typename":"Commit","oid":BASE},"branchProtectionRule":null,"refUpdateRule":null,
        "rules":{"totalCount":6,"nodes":[{"id":"RULE_1"}]}
    }}}})
}

impl Fixture {
    async fn seed_branch(&self) -> u64 {
        {
            let mut data = self.data.lock().unwrap();
            data.branch["protected"] = json!(true);
            data.branch_graph = branch_graph();
        }
        let old = self.seed().await;
        let db = rusqlite::Connection::open(self.dir.path().join("cache.sqlite")).unwrap();
        db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/pulls/7'", [old+120_000]).unwrap();
        assert_eq!(db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/branches/main' AND key NOT LIKE '%/rules/%'",[old]).unwrap(),1);
        old
    }

    fn branch_calls(&self) -> usize {
        self.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(path, _)| path == "/repos/acme/demo/branches/main")
            .count()
    }

    fn branch_queries(&self) -> usize {
        self.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(_, body)| {
                body["query"]
                    .as_str()
                    .is_some_and(|q| q.contains("RequiredPolicyBranch"))
            })
            .count()
    }
}

#[tokio::test]
async fn ruleset_branch_projection_avoids_stalled_rest_without_refreshing_its_payload() {
    let f = Fixture::new().await;
    let old = f.seed_branch().await;
    f.data.lock().unwrap().stall_branch = true;
    let report = tokio::time::timeout(
        Duration::from_secs(2),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .expect("a ruleset-only branch waited on unrelated REST fields")
    .unwrap();
    assert_eq!(report.state, "satisfied", "{:?}", report.errors);
    assert_eq!(report.policy_sha.as_deref(), Some(BASE));
    assert_eq!(f.branch_calls(), 0);
    assert_eq!(
        f.branch_queries(),
        1,
        "collection and final confirmation share fresh proof"
    );
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.ends_with("/graphql") && v.validated_at_ms > old)
    );
    let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
    let clock:u64=db.query_row("SELECT json_extract(response,'$.validated_at_ms') FROM cache WHERE key LIKE '%/branches/main' AND key NOT LIKE '%/rules/%'",[],|r|r.get(0)).unwrap();
    assert_eq!(
        clock, old,
        "a branch projection is not a fresh full REST payload"
    );
    assert!(
        f.data
            .lock()
            .unwrap()
            .tokens
            .iter()
            .all(|t| t == "Bearer synthetic-token")
    );
}

#[tokio::test]
async fn ambiguous_branch_proof_revalidates_rest_even_after_a_peer_cached_it() {
    let f = Fixture::new().await;
    f.seed_branch().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    {
        let mut data = f.data.lock().unwrap();
        data.branch_graph["data"]["repository"]["ref"]["branchProtectionRule"] =
            json!({"id":"CLASSIC"});
        data.branch_graph_gate = Some(gate.clone());
    }
    let read = tokio::spawn({
        let client = f.client.clone();
        async move {
            client
                .required_checks_for_pr("acme/demo", 7, Freshness::default())
                .await
        }
    });
    until(|| f.branch_queries() == 1).await;
    f.client
        .get("repos/acme/demo/branches/main", Freshness::Revalidate)
        .await
        .unwrap();
    f.data.lock().unwrap().branch["commit"]["sha"] = json!(NEXT);
    gate.notify_one();
    let report = read.await.unwrap().unwrap();
    assert_eq!(report.policy_sha.as_deref(), Some(NEXT));
    assert_eq!(
        f.branch_calls(),
        2,
        "older peer metadata cannot override ambiguous/newer evidence"
    );
}

#[tokio::test]
async fn projected_branch_tip_can_advance_without_a_full_rest_payload() {
    let f = Fixture::new().await;
    f.seed_branch().await;
    {
        let mut data = f.data.lock().unwrap();
        data.stall_branch = true;
        data.branch_graph["data"]["repository"]["ref"]["target"]["oid"] = json!(NEXT);
    }
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "satisfied");
    assert_eq!(report.policy_sha.as_deref(), Some(NEXT));
    assert_eq!(report.base_sha.as_deref(), Some(NEXT));
    assert_eq!(report.pr_base_sha.as_deref(), Some(BASE));
    assert_eq!(f.branch_calls(), 0);
}

#[tokio::test]
async fn unsupported_or_incomplete_branch_proofs_fall_back_to_rest() {
    let mut cases = Vec::new();
    for (pointer, value) in [
        ("/data/repository/id", json!("")),
        ("/data/repository/nameWithOwner", json!("acme/other")),
        ("/data/repository/ref/id", Value::Null),
        ("/data/repository/ref/name", json!("Main")),
        ("/data/repository/ref/prefix", json!("refs/tags/")),
        ("/data/repository/ref/target/__typename", json!("Tag")),
        ("/data/repository/ref/target/oid", json!("invalid")),
        (
            "/data/repository/ref/branchProtectionRule",
            json!({"id":"CLASSIC"}),
        ),
        (
            "/data/repository/ref/refUpdateRule",
            json!({"pattern":"main"}),
        ),
        ("/data/repository/ref/rules/totalCount", json!(0)),
        ("/data/repository/ref/rules/totalCount", json!(-1)),
        ("/data/repository/ref/rules/nodes", json!([])),
        ("/data/repository/ref/rules/nodes", json!([{"id":""}])),
    ] {
        let mut value_graph = branch_graph();
        *value_graph.pointer_mut(pointer).unwrap() = value;
        cases.push(value_graph);
    }
    for field in ["branchProtectionRule", "refUpdateRule", "rules"] {
        let mut value = branch_graph();
        value["data"]["repository"]["ref"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        cases.push(value);
    }
    for (index, graph) in cases.into_iter().enumerate() {
        let f = Fixture::new().await;
        f.seed_branch().await;
        f.data.lock().unwrap().branch_graph = graph;
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(
            report.state, "satisfied",
            "case {index}: {:?}",
            report.errors
        );
        assert_eq!(report.policy_sha.as_deref(), Some(BASE));
        assert_eq!(f.branch_queries(), 1, "case {index}");
        assert_eq!(f.branch_calls(), 1, "case {index}");
    }
}

#[tokio::test]
async fn branch_projection_preserves_explicit_refresh_offline_and_personal_auth() {
    for freshness in [
        Freshness::CachedOnly,
        Freshness::Revalidate,
        Freshness::default(),
    ] {
        let f = Fixture::with_installation(true).await;
        f.seed_branch().await;
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, freshness)
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied", "{:?}", report.errors);
        let data = f.data.lock().unwrap();
        if matches!(freshness, Freshness::CachedOnly) {
            assert!(data.calls.is_empty());
        } else if matches!(freshness, Freshness::Revalidate) {
            assert!(data.calls.iter().all(|(_, body)| {
                !body["query"]
                    .as_str()
                    .is_some_and(|q| q.contains("RequiredPolicyBranch"))
            }));
        } else {
            let tokens: Vec<_> = data
                .calls
                .iter()
                .zip(&data.tokens)
                .filter(|((_, body), _)| {
                    body["query"]
                        .as_str()
                        .is_some_and(|q| q.contains("RequiredPolicyBranch"))
                })
                .map(|(_, token)| token.as_str())
                .collect();
            assert_eq!(tokens, ["Bearer synthetic-token"]);
        }
    }
}

#[tokio::test]
async fn branch_graphql_access_denial_is_explicit_and_never_bypassed() {
    let f = Fixture::new().await;
    f.seed_branch().await;
    f.data.lock().unwrap().branch_graph =
        json!({"errors":[{"type":"FORBIDDEN","message":"Synthetic branch access denied"}]});
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "unknown");
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.message.contains("Synthetic branch access denied"))
    );
    assert_eq!(f.branch_calls(), 0);
}

#[tokio::test]
async fn branch_changes_during_collection_require_a_new_report_attempt() {
    let f = Fixture::new().await;
    {
        let mut data = f.data.lock().unwrap();
        data.rest["head"]["sha"] = json!(NEXT);
        let pr = &mut data.graph["data"]["repository"]["pullRequest"];
        pr["headRefOid"] = json!(NEXT);
        pr["potentialMergeCommit"]["parents"]["nodes"][1]["oid"] = json!(NEXT);
    }
    f.seed_branch().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    f.data.lock().unwrap().checks_gate = Some(gate.clone());
    let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',1) WHERE key LIKE '%/check-runs?%'",[]).unwrap();
    let read = tokio::spawn({
        let client = f.client.clone();
        async move {
            client
                .required_checks_for_pr("acme/demo", 7, Freshness::default())
                .await
        }
    });
    until(|| {
        let data = f.data.lock().unwrap();
        data.calls
            .iter()
            .any(|(path, _)| path == &format!("/repos/acme/demo/commits/{NEXT}/check-runs"))
    })
    .await;
    {
        let mut data = f.data.lock().unwrap();
        data.branch_graph["data"]["repository"]["ref"]["target"]["oid"] = json!(NEXT);
        data.branch["commit"]["sha"] = json!(NEXT);
    }
    assert_eq!(db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',1) WHERE json_extract(response,'$.data.data.repository.ref.id')='REF_main'",[]).unwrap(),1);
    gate.notify_one();
    let report = read.await.unwrap().unwrap();
    assert_eq!(report.state, "satisfied", "{:?}", report.errors);
    assert_eq!(report.policy_sha.as_deref(), Some(NEXT));
    assert_eq!(
        f.branch_queries(),
        2,
        "final confirmation must refresh the expired proof"
    );
    assert_eq!(
        f.branch_calls(),
        2,
        "a changed branch restarts with explicit REST reads"
    );
}

#[tokio::test]
async fn stalled_branch_graphql_leaves_time_for_rest_fallback() {
    let f = Fixture::new().await;
    f.seed_branch().await;
    f.data.lock().unwrap().stall_graph = true;
    let report = tokio::time::timeout(
        Duration::from_secs(4),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(report.state, "satisfied", "{:?}", report.errors);
    assert_eq!(f.branch_calls(), 1);
}

#[tokio::test]
async fn branch_projection_refreshes_expired_proofs_and_rejects_future_clocks() {
    for future in [false, true] {
        let f = Fixture::new().await;
        let old = f.seed_branch().await;
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        // Both the REST seed and older GraphQL proof are expired, or the
        // proof has a future clock. Neither cached proof can validate the seed.
        let now = old + 120_000;
        let seed = now - 1000;
        let proof = if future { now + 60_000 } else { now - 2000 };
        db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/branches/main' AND key NOT LIKE '%/rules/%'",[seed]).unwrap();
        db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE json_extract(response,'$.data.data.repository.ref.id')='REF_main'",[proof]).unwrap();
        f.data.lock().unwrap().calls.clear();
        // A short freshness bound forces the stale seed through validation.
        // A future cache timestamp can otherwise appear fresh via saturation.
        f.client
            .required_checks_for_pr(
                "acme/demo",
                7,
                Freshness::MaxAge(Duration::from_millis(500)),
            )
            .await
            .unwrap();
        if future {
            assert!(
                f.branch_calls() > 0,
                "future GraphQL clocks cannot supply branch freshness"
            );
        } else {
            // An expired proof is refreshed online, rather than consuming the
            // older snapshot. The newly returned version is usable.
            assert_eq!(f.branch_queries(), 1);
        }
    }
}

#[tokio::test]
async fn branch_projection_preserves_ruleset_access_and_shape_errors() {
    for denied in [false, true] {
        let f = Fixture::new().await;
        f.seed_branch().await;
        {
            let mut data = f.data.lock().unwrap();
            data.deny_rules = denied;
            data.rules = json!([{"type":"required_status_checks","parameters":{}}]);
        }
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',1) WHERE key LIKE '%/rules/branches/%'", []).unwrap();
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "unknown", "denied={denied}");
        assert!(!report.errors.is_empty());
        assert_eq!(f.branch_queries(), 1);
        assert_eq!(f.branch_calls(), 0);
    }
}

#[tokio::test]
async fn newly_enabled_classic_protection_requires_explicit_policy_evidence() {
    let f = Fixture::new().await;
    f.seed_branch().await;
    {
        let mut data = f.data.lock().unwrap();
        data.branch_graph["data"]["repository"]["ref"]["branchProtectionRule"] =
            json!({"id":"CLASSIC"});
        data.branch["protection"] = json!({"enabled":true});
    }
    // The fixture has no valid classic required-check policy. Discovering new
    // protection must not reuse the previous proof that it was disabled.
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "unknown");
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.message.contains("malformed required-check policy"))
    );
    assert!(
        f.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(path, _)| path.ends_with("/protection/required_status_checks"))
    );
}
