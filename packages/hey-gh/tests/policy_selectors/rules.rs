use super::*;

fn rest_rules() -> Value {
    json!([{"type":"required_status_checks","ruleset_source_type":"Repository","ruleset_source":"acme/demo","ruleset_id":42,
        "parameters":{"strict_required_status_checks_policy":false,"do_not_enforce_on_create":false,
            "required_status_checks":[{"context":"tests","integration_id":1}]}}])
}

fn graph_rules() -> Value {
    json!({"totalCount":1,"pageInfo":{"hasNextPage":false},"nodes":[{
        "id":"RULE_1","type":"REQUIRED_STATUS_CHECKS",
        "repositoryRuleset":{"id":"RULESET_42","databaseId":42,"source":{"__typename":"Repository","nameWithOwner":"acme/demo"}},
        "parameters":{"__typename":"RequiredStatusChecksParameters","strictRequiredStatusChecksPolicy":false,"doNotEnforceOnCreate":false,
            "requiredStatusChecks":[{"context":"tests","integrationId":1}]}
    }]})
}

impl Fixture {
    fn stale_rules(&self, clock: u64) {
        let db = rusqlite::Connection::open(self.dir.path().join("cache.sqlite")).unwrap();
        assert_eq!(db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE 'http%/rules/branches/main'",[clock]).unwrap(),1);
    }

    fn graph_calls(&self) -> usize {
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

    async fn seed_rules(&self) -> u64 {
        self.data.lock().unwrap().rules = rest_rules();
        let old = self.seed_branch().await;
        self.data.lock().unwrap().branch_graph["data"]["repository"]["ref"]["rules"] =
            graph_rules();
        let db = rusqlite::Connection::open(self.dir.path().join("cache.sqlite")).unwrap();
        assert_eq!(db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/rules/branches/main'",[old]).unwrap(),1);
        old
    }

    fn rules_calls(&self) -> usize {
        self.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(path, _)| path == "/repos/acme/demo/rules/branches/main")
            .count()
    }
}

#[tokio::test]
async fn incomplete_changed_or_malformed_rule_proofs_revalidate_rest() {
    let cases = [
        ("/totalCount", json!(2)),
        ("/pageInfo/hasNextPage", json!(true)),
        ("/pageInfo", Value::Null),
        ("/nodes/0/id", json!("")),
        ("/nodes/0/type", json!("UNKNOWN_RULE")),
        ("/nodes/0/repositoryRuleset/id", Value::Null),
        ("/nodes/0/repositoryRuleset/databaseId", json!(43)),
        (
            "/nodes/0/repositoryRuleset/source/nameWithOwner",
            json!("acme/other"),
        ),
        (
            "/nodes/0/repositoryRuleset/source/__typename",
            json!("Enterprise"),
        ),
        ("/nodes/0/parameters/__typename", json!("OtherParameters")),
        (
            "/nodes/0/parameters/strictRequiredStatusChecksPolicy",
            json!(true),
        ),
        ("/nodes/0/parameters/doNotEnforceOnCreate", json!(true)),
        (
            "/nodes/0/parameters/requiredStatusChecks/0/context",
            json!("new check"),
        ),
        (
            "/nodes/0/parameters/requiredStatusChecks/0/integrationId",
            Value::Null,
        ),
        (
            "/nodes/0/parameters/requiredStatusChecks",
            json!([{"context":"tests"}]),
        ),
    ];
    for (pointer, value) in cases {
        let f = Fixture::new().await;
        f.seed_rules().await;
        *f.data.lock().unwrap().branch_graph["data"]["repository"]["ref"]["rules"]
            .pointer_mut(pointer)
            .unwrap() = value;
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied", "{pointer}: {:?}", report.errors);
        assert_eq!(f.rules_calls(), 1, "{pointer}");
    }
}

#[tokio::test]
async fn changed_rules_ignore_a_peer_cache_written_before_the_proof() {
    let f = Fixture::new().await;
    f.seed_rules().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    {
        let mut data = f.data.lock().unwrap();
        data.branch_graph["data"]["repository"]["ref"]["rules"]["nodes"][0]["parameters"]["requiredStatusChecks"]
            [0]["context"] = json!("new check");
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
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.graph_calls() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    f.client
        .get("repos/acme/demo/rules/branches/main", Freshness::Revalidate)
        .await
        .unwrap();
    f.data.lock().unwrap().rules[0]["parameters"]["required_status_checks"][0]["context"] =
        json!("new check");
    gate.notify_one();
    let report = read.await.unwrap().unwrap();
    assert_eq!(report.state, "missing");
    assert_eq!(
        report.rules[0]["parameters"]["required_status_checks"][0]["context"],
        "new check"
    );
    assert_eq!(
        f.rules_calls(),
        2,
        "peer cache must not suppress explicit revalidation"
    );
}

#[tokio::test]
async fn rule_proof_denial_and_previous_rest_denial_remain_unknown() {
    for rest_denial in [false, true] {
        let f = Fixture::new().await;
        let old = f.seed_rules().await;
        if rest_denial {
            f.data.lock().unwrap().deny_rules = true;
            let report = f
                .client
                .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
                .await
                .unwrap();
            assert_eq!(report.state, "unknown");
            f.stale_rules(old);
        } else {
            f.data.lock().unwrap().branch_graph =
                json!({"errors":[{"type":"FORBIDDEN","message":"Synthetic rule access denied"}]});
        }
        f.data.lock().unwrap().calls.clear();
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "unknown");
        assert!(report.errors.iter().any(|error| error.source == "rulesets"));
        assert_eq!(
            f.rules_calls(),
            0,
            "a denial is not an alternate-endpoint fallback"
        );
        if rest_denial {
            assert_eq!(
                f.graph_calls(),
                0,
                "cached denial takes precedence over the shortcut"
            );
        }
    }
}

#[tokio::test]
async fn complete_empty_rules_prove_absence_without_claiming_branch_is_unprotected() {
    let f = Fixture::new().await;
    f.seed_rules().await;
    f.data.lock().unwrap().rules = json!([]);
    f.client
        .get("repos/acme/demo/rules/branches/main", Freshness::Revalidate)
        .await
        .unwrap();
    f.stale_rules(1);
    {
        let mut data = f.data.lock().unwrap();
        data.branch_graph["data"]["repository"]["ref"]["rules"] =
            json!({"totalCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}});
        data.stall_rules = true;
        data.calls.clear();
    }
    let report = tokio::time::timeout(
        Duration::from_secs(2),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(report.state, "not_required", "{:?}", report.errors);
    assert_eq!(f.rules_calls(), 0);
    assert!(
        f.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(path, _)| path == "/repos/acme/demo/branches/main"),
        "zero rules alone is not a protected=false branch proof"
    );
}

#[tokio::test]
async fn rules_shortcut_preserves_explicit_refresh_offline_and_personal_auth() {
    for freshness in [
        Freshness::CachedOnly,
        Freshness::Revalidate,
        Freshness::default(),
    ] {
        let f = Fixture::with_installation(true).await;
        f.seed_rules().await;
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
            assert_eq!(
                data.calls
                    .iter()
                    .filter(|(path, _)| path == "/graphql")
                    .count(),
                0
            );
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
async fn unknown_rest_fields_or_pagination_do_not_gain_graphql_freshness() {
    for pagination in [false, true] {
        let f = Fixture::new().await;
        f.seed_rules().await;
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        if pagination {
            db.execute("UPDATE cache SET response=json_set(response,'$.link','<http://invalid.example/page>; rel=\"next\"') WHERE key LIKE '%/rules/branches/main'",[]).unwrap();
        } else {
            db.execute("UPDATE cache SET response=json_set(response,'$.data[0].future_policy',true) WHERE key LIKE '%/rules/branches/main'",[]).unwrap();
        }
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied");
        assert_eq!(f.rules_calls(), 1);
    }
}

#[tokio::test]
async fn rules_future_proof_clock_forces_rest_revalidation() {
    let f = Fixture::new().await;
    f.seed_rules().await;
    f.client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE json_extract(response,'$.data.data.repository.ref.id')='REF_main'",[u64::MAX/2]).unwrap();
    f.data.lock().unwrap().calls.clear();
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "satisfied");
    assert_eq!(f.rules_calls(), 1);
}

#[tokio::test]
async fn organization_and_repository_rules_match_in_any_order_without_losing_duplicates() {
    for duplicate_id in [false, true] {
        let f = Fixture::new().await;
        f.seed_rules().await;
        let mut rest = rest_rules().as_array().unwrap().clone();
        rest.push(json!({"type":"deletion","ruleset_source_type":"Organization","ruleset_source":"acme","ruleset_id":21}));
        rest.push(rest[1].clone());
        let mut graph = graph_rules();
        let extra = json!({"id":"RULE_2","type":"DELETION","parameters":null,
            "repositoryRuleset":{"id":"RULESET_21","databaseId":21,"source":{"__typename":"Organization","login":"acme"}}});
        graph["nodes"].as_array_mut().unwrap().push(extra.clone());
        let mut third = extra;
        third["id"] = json!(if duplicate_id { "RULE_2" } else { "RULE_3" });
        graph["nodes"].as_array_mut().unwrap().push(third);
        graph["nodes"].as_array_mut().unwrap().reverse();
        graph["totalCount"] = json!(3);
        {
            let mut data = f.data.lock().unwrap();
            data.rules = Value::Array(rest.clone());
            data.branch_graph["data"]["repository"]["ref"]["rules"] = graph;
        }
        f.client
            .get("repos/acme/demo/rules/branches/main", Freshness::Revalidate)
            .await
            .unwrap();
        f.stale_rules(1);
        f.data.lock().unwrap().calls.clear();
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied", "{:?}", report.errors);
        assert_eq!(report.rules, rest);
        assert_eq!(f.rules_calls(), usize::from(duplicate_id));
    }
}

#[tokio::test]
async fn interrupted_rule_shortcut_leaves_a_rest_fallback_budget() {
    let f = Fixture::new().await;
    f.seed_rules().await;
    f.data.lock().unwrap().stall_graph = true;
    let report = tokio::time::timeout(
        Duration::from_secs(4),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(report.state, "satisfied");
    assert_eq!(f.rules_calls(), 1);
}

#[tokio::test]
async fn a_rest_denial_observed_during_graphql_cannot_be_overwritten_by_success() {
    let f = Fixture::new().await;
    f.seed_rules().await;
    let coalesced = f.client.status().coalesced_requests;
    let gate = Arc::new(tokio::sync::Notify::new());
    f.data.lock().unwrap().branch_graph_gate = Some(gate.clone());
    let read = tokio::spawn({
        let client = f.client.clone();
        async move {
            client
                .required_checks_for_pr("acme/demo", 7, Freshness::default())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.graph_calls() == 0 || f.client.status().coalesced_requests == coalesced {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let key = format!(
        "policy-error://{}/repos/acme/demo/rules/branches/main",
        Config::default().hostname
    );
    assert_eq!(db.execute("INSERT INTO cache(scope,key,response) SELECT scope,?1,json_set(response,'$.data',json(?2),'$.validated_at_ms',?3) FROM cache WHERE key LIKE 'http%/rules/branches/main'",rusqlite::params![key,json!({"status":403,"message":"Synthetic peer policy denial"}).to_string(),stamp]).unwrap(),1);
    gate.notify_one();
    let report = read.await.unwrap().unwrap();
    assert_eq!(report.state, "unknown");
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.message.contains("Synthetic peer policy denial"))
    );
    assert_eq!(f.rules_calls(), 0);
}

#[tokio::test]
async fn one_complete_branch_proof_validates_rules_without_waiting_for_rest() {
    let f = Fixture::new().await;
    let old = f.seed_rules().await;
    {
        let mut data = f.data.lock().unwrap();
        data.stall_branch = true;
        data.stall_rules = true;
    }
    let report = tokio::time::timeout(
        Duration::from_secs(2),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .expect("unchanged complete rules waited on the REST queue")
    .unwrap();
    assert_eq!(report.state, "satisfied", "{:?}", report.errors);
    assert_eq!(report.rules, rest_rules().as_array().unwrap().clone());
    assert_eq!(f.rules_calls(), 0);
    assert_eq!(
        f.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(path, _)| path == "/graphql")
            .count(),
        1,
        "branch and rules share one complete query"
    );
    let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
    let clock:u64=db.query_row("SELECT json_extract(response,'$.validated_at_ms') FROM cache WHERE key LIKE '%/rules/branches/main'",[],|r|r.get(0)).unwrap();
    assert_eq!(
        clock, old,
        "internal proof does not replace or refresh REST cache"
    );
}
