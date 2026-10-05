use super::*;

#[tokio::test]
async fn a_matching_rollup_cannot_hide_later_workflow_checks() {
    let f = Fixture::new().await;
    f.seed_ci_proof().await;
    {
        let mut data = f.data.lock().unwrap();
        let commit = &mut data.ci_graph["data"]["repository"]["merge"];
        // GitHub's rollup omitted later workflow-generated checks on a real
        // merge commit. Its matching roster is not evidence of completeness.
        commit["statusCheckRollup"] = json!({"contexts":{"totalCount":1,"checkRunCount":1,"statusContextCount":0,"pageInfo":{"hasNextPage":false},"nodes":commit["checkSuites"]["nodes"][0]["checkRuns"]["nodes"]}});
        commit["checkSuites"] = json!({"totalCount":2,"pageInfo":{"hasNextPage":true},"nodes":[]});
    }
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "satisfied");
    assert_eq!(
        f.ci_rest_calls(),
        4,
        "a matching rollup hid incomplete suite evidence"
    );
}

fn commit(sha: &str) -> Value {
    json!({"__typename":"Commit","oid":sha,"status":null,"checkSuites":{
        "totalCount":1,"pageInfo":{"hasNextPage":false},"nodes":[{
            "id":format!("CS_{}",sha.as_bytes()[0]),"databaseId":sha.as_bytes()[0],
            "checkRuns":{"totalCount":1,"pageInfo":{"hasNextPage":false},"nodes":[{
                "__typename":"CheckRun","id":format!("CR_{}",sha.as_bytes()[0]),"databaseId":sha.as_bytes()[0],
                "name":"tests","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":null,"startedAt":null,"completedAt":null,
                "checkSuite":{"app":{"databaseId":1},"commit":{"oid":sha}}
            }]}
        }]
    }})
}

impl Fixture {
    async fn seed_ci_proof(&self) -> u64 {
        let old = self.seed().await;
        self.data.lock().unwrap().ci_graph = json!({"data":{"repository":{
            "id":"R_demo","nameWithOwner":"acme/demo","head":commit(HEAD),"merge":commit(MERGE)
        }}});
        let db = rusqlite::Connection::open(self.dir.path().join("cache.sqlite")).unwrap();
        assert_eq!(db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/commits/%'", [old]).unwrap(), 4);
        old
    }

    fn ci_proof_calls(&self) -> usize {
        self.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(_, body)| {
                body["query"]
                    .as_str()
                    .is_some_and(|q| q.contains("RequiredPolicyCi"))
            })
            .count()
    }

    fn ci_rest_calls(&self) -> usize {
        self.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(path, _)| path.contains("/commits/"))
            .count()
    }
}

#[tokio::test]
async fn one_complete_ci_proof_avoids_stalled_rest_and_keeps_raw_cache_clocks() {
    for installation in [false, true] {
        let f = Fixture::with_installation(installation).await;
        let old = f.seed_ci_proof().await;
        f.data.lock().unwrap().stall_checks = true;
        let report = tokio::time::timeout(
            Duration::from_secs(2),
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::default()),
        )
        .await
        .expect("unchanged CI still waited for the four REST reads")
        .unwrap();
        assert_eq!(report.state, "satisfied", "{:?}", report.errors);
        assert_eq!(f.ci_proof_calls(), 1);
        assert_eq!(f.ci_rest_calls(), 0);
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        let clock:u64=db.query_row("SELECT MAX(json_extract(response,'$.validated_at_ms')) FROM cache WHERE key LIKE '%/commits/%'",[],|r|r.get(0)).unwrap();
        assert_eq!(
            clock, old,
            "a narrow proof cannot refresh the full REST cache"
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM snapshots WHERE resource LIKE 'ci://%'",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        let data = f.data.lock().unwrap();
        for ((_, body), token) in data.calls.iter().zip(&data.tokens) {
            let app = installation
                && body["query"]
                    .as_str()
                    .is_some_and(|q| q.contains("RequiredPolicyCi"));
            assert_eq!(
                token,
                if app {
                    "Bearer synthetic-app-token"
                } else {
                    "Bearer synthetic-token"
                }
            );
        }
    }
}

#[tokio::test]
async fn malformed_changed_or_truncated_ci_proofs_require_fresh_rest() {
    let cases = [
        ("/id", json!(null)),
        ("/nameWithOwner", json!("acme/other")),
        ("/head/oid", json!(MERGE)),
        ("/merge", Value::Null),
        ("/head/checkSuites", Value::Null),
        ("/head/checkSuites/nodes/0/checkRuns/totalCount", json!(2)),
        ("/head/checkSuites/totalCount", json!(0)),
        ("/head/checkSuites/nodes/0/id", json!("")),
        ("/head/checkSuites/nodes/0/databaseId", json!(0)),
        ("/head/status", json!({"contexts":null})),
        ("/head/checkSuites/pageInfo/hasNextPage", json!(true)),
        (
            "/head/checkSuites/nodes/0/checkRuns/pageInfo/hasNextPage",
            json!(true),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/id",
            json!("other"),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/databaseId",
            json!(0),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/name",
            json!("other"),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/status",
            json!("IN_PROGRESS"),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/conclusion",
            json!("FAILURE"),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/startedAt",
            json!("2026-10-01T00:00:00Z"),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/completedAt",
            json!("2026-10-01T01:00:00Z"),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/detailsUrl",
            json!("https://example.test/new"),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/checkSuite/app/databaseId",
            json!(2),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/checkSuite/commit/oid",
            json!(MERGE),
        ),
        (
            "/head/checkSuites/nodes/0/checkRuns/nodes/0/__typename",
            json!("Other"),
        ),
    ];
    for (pointer, value) in cases {
        let f = Fixture::new().await;
        f.seed_ci_proof().await;
        *f.data.lock().unwrap().ci_graph["data"]["repository"]
            .pointer_mut(pointer)
            .unwrap() = value;
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied", "{pointer}: {:?}", report.errors);
        assert_eq!(f.ci_proof_calls(), 1, "{pointer}");
        assert_eq!(f.ci_rest_calls(), 4, "{pointer}");
    }
}

#[tokio::test]
async fn later_suites_preserve_cancelled_failures_and_pending_reruns() {
    for (status, conclusion, expected) in [
        ("completed", "cancelled", "failure"),
        ("completed", "failure", "failure"),
        ("in_progress", "", "pending"),
        ("completed", "success", "satisfied"),
    ] {
        let f = Fixture::new().await;
        f.seed_ci_proof().await;
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        let id = u64::from(MERGE.as_bytes()[0]) + 100;
        let run = json!({"id":id,"node_id":"CR_rerun","name":"tests","app":{"id":1},"head_sha":MERGE,"status":status,"conclusion":if conclusion.is_empty(){Value::Null}else{json!(conclusion)},"started_at":null,"completed_at":null,"details_url":null});
        db.execute("UPDATE cache SET response=json_set(response,'$.data.total_count',2,'$.data.check_runs[#]',json(?1)) WHERE key LIKE ?2",rusqlite::params![run.to_string(),format!("%/commits/{MERGE}/check-runs%")]).unwrap();
        {
            let mut data = f.data.lock().unwrap();
            let suites = &mut data.ci_graph["data"]["repository"]["merge"]["checkSuites"];
            let mut later = suites["nodes"][0].clone();
            later["id"] = json!("CS_rerun");
            later["databaseId"] = json!(id);
            let check = &mut later["checkRuns"]["nodes"][0];
            check["id"] = json!("CR_rerun");
            check["databaseId"] = json!(id);
            check["status"] = json!(status.to_ascii_uppercase());
            check["conclusion"] = if conclusion.is_empty() {
                Value::Null
            } else {
                json!(conclusion.to_ascii_uppercase())
            };
            suites["nodes"].as_array_mut().unwrap().push(later);
            // Empty suites are real: cancelled workflows and installed apps
            // can leave them alongside suites containing check runs.
            suites["nodes"].as_array_mut().unwrap().push(json!({"id":"CS_empty","databaseId":999,"checkRuns":{"totalCount":0,"pageInfo":{"hasNextPage":false},"nodes":[]}}));
            suites["totalCount"] = json!(3);
            data.stall_checks = true;
        }
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(
            report.state, expected,
            "{status}/{conclusion}: {:?}",
            report.errors
        );
        assert_eq!(f.ci_rest_calls(), 0);
    }
}

#[tokio::test]
async fn unseen_later_check_forces_rest_even_when_rollup_matches() {
    let f = Fixture::new().await;
    f.seed_ci_proof().await;
    {
        let mut data = f.data.lock().unwrap();
        let c = &mut data.ci_graph["data"]["repository"]["merge"];
        let original = c["checkSuites"]["nodes"][0]["checkRuns"]["nodes"][0].clone();
        c["statusCheckRollup"] = json!({"contexts":{"totalCount":1,"checkRunCount":1,"statusContextCount":0,"pageInfo":{"hasNextPage":false},"nodes":[original]}});
        let mut later = c["checkSuites"]["nodes"][0].clone();
        later["id"] = json!("CS_later");
        later["databaseId"] = json!(500);
        later["checkRuns"]["nodes"][0]["id"] = json!("CR_later");
        later["checkRuns"]["nodes"][0]["databaseId"] = json!(500);
        later["checkRuns"]["nodes"][0]["conclusion"] = json!("FAILURE");
        c["checkSuites"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(later);
        c["checkSuites"]["totalCount"] = json!(2);
        data.check_conclusion = "failure";
    }
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "failure");
    assert_eq!(f.ci_rest_calls(), 4);
}

#[tokio::test]
async fn duplicate_ci_nodes_and_unavailable_cache_never_supply_proofs() {
    for mode in [
        "duplicate",
        "missing",
        "pagination",
        "wrong_sha",
        "malformed_id",
        "expired",
    ] {
        let f = Fixture::new().await;
        let old = f.seed_ci_proof().await;
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        match mode {
            "duplicate" => {
                let mut data = f.data.lock().unwrap();
                let c = &mut data.ci_graph["data"]["repository"]["head"]["checkSuites"]["nodes"][0]
                    ["checkRuns"];
                c["nodes"] = json!([c["nodes"][0], c["nodes"][0]]);
                c["totalCount"] = json!(2);
            }
            "missing" => {
                db.execute(
                    "DELETE FROM cache WHERE key LIKE '%/commits/%/check-runs%'",
                    [],
                )
                .unwrap();
            }
            "pagination" => {
                db.execute("UPDATE cache SET response=json_set(response,'$.link','<http://example.test/next>; rel=\"next\"') WHERE key LIKE '%/commits/%'",[]).unwrap();
            }
            "wrong_sha" => {
                db.execute("UPDATE cache SET response=json_set(response,'$.data.sha','wrong') WHERE key LIKE '%/commits/%/status%'",[]).unwrap();
            }
            "malformed_id" => {
                db.execute("UPDATE cache SET response=json_set(response,'$.data.check_runs[0].node_id','') WHERE key LIKE '%/check-runs%'",[]).unwrap();
            }
            "expired" => {
                db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/commits/%'",[old-86_400_000]).unwrap();
            }
            _ => unreachable!(),
        }
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied", "{mode}: {:?}", report.errors);
        assert_eq!(
            f.ci_proof_calls(),
            usize::from(mode == "duplicate"),
            "{mode}"
        );
        assert!(f.ci_rest_calls() > 0, "{mode}");
    }
}

#[tokio::test]
async fn changed_ci_proof_cannot_reuse_an_old_peer_refresh() {
    let f = Fixture::new().await;
    f.seed_ci_proof().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    {
        let mut data = f.data.lock().unwrap();
        data.ci_graph["data"]["repository"]["head"]["checkSuites"]["nodes"][0]["checkRuns"]["nodes"]
            [0]["conclusion"] = json!("FAILURE");
        data.ci_graph_gate = Some(gate.clone());
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
        while f.ci_proof_calls() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.client
        .get(
            &format!("repos/acme/demo/commits/{HEAD}/check-runs?filter=latest&per_page=100"),
            Freshness::Revalidate,
        )
        .await
        .unwrap();
    f.data.lock().unwrap().check_conclusion = "failure";
    gate.notify_one();
    let report = read.await.unwrap().unwrap();
    assert_eq!(report.state, "failure");
    assert_eq!(
        f.ci_rest_calls(),
        5,
        "fresh REST must replace the peer's old successful data"
    );
}

#[tokio::test]
async fn explicit_refresh_and_offline_policy_keep_rest_semantics() {
    for freshness in [Freshness::Revalidate, Freshness::CachedOnly] {
        let f = Fixture::new().await;
        f.seed_ci_proof().await;
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, freshness)
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied");
        assert_eq!(f.ci_proof_calls(), 0);
        assert_eq!(
            f.ci_rest_calls(),
            if matches!(freshness, Freshness::Revalidate) {
                4
            } else {
                0
            }
        );
    }
}

#[tokio::test]
async fn ci_graph_access_errors_propagate_without_rest_bypass() {
    let f = Fixture::new().await;
    f.seed_ci_proof().await;
    f.data.lock().unwrap().ci_graph = json!({"errors":[{"type":"FORBIDDEN","message":"denied"}]});
    let result = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await;
    assert!(matches!(
        result,
        Err(hey_gh::Error::GraphQL {
            access_denied: true,
            ..
        })
    ));
    assert_eq!(f.ci_rest_calls(), 0);
}

#[tokio::test]
async fn complete_empty_ci_rosters_prove_missing_required_checks() {
    let f = Fixture::new().await;
    f.seed_ci_proof().await;
    let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.data.check_runs',json('[]'),'$.data.total_count',0) WHERE key LIKE '%/check-runs%'",[]).unwrap();
    {
        let mut data = f.data.lock().unwrap();
        for key in ["head", "merge"] {
            data.ci_graph["data"]["repository"][key]["checkSuites"] =
                json!({"totalCount":0,"pageInfo":{"hasNextPage":false},"nodes":[]});
        }
        data.stall_checks = true;
    }
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "missing");
    assert_eq!(f.ci_rest_calls(), 0);
}

#[tokio::test]
async fn commit_status_versions_are_checked_alongside_check_runs() {
    for changed in [false, true] {
        let f = Fixture::new().await;
        {
            let mut data = f.data.lock().unwrap();
            data.status_state = Some("success");
            data.rules[0]["parameters"]["required_status_checks"]
                .as_array_mut()
                .unwrap()
                .push(json!({"context":"deploy","integration_id":null}));
        }
        f.seed_ci_proof().await;
        {
            let mut data = f.data.lock().unwrap();
            for (key, sha) in [("head", HEAD), ("merge", MERGE)] {
                data.ci_graph["data"]["repository"][key]["status"] = json!({"contexts":[{"id":format!("S_{}",sha.as_bytes()[0]),"context":"deploy","state":if changed {"FAILURE"} else {"SUCCESS"},"updatedAt":"2026-10-01T00:00:00Z","targetUrl":null}]});
            }
            if changed {
                data.status_state = Some("failure");
            } else {
                data.stall_checks = true;
            }
        }
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, if changed { "failure" } else { "satisfied" });
        assert_eq!(f.ci_rest_calls(), if changed { 4 } else { 0 });
    }
}

#[tokio::test]
async fn ci_proof_clocks_cannot_be_future_dated_or_older_than_rest() {
    for future in [false, true] {
        let f = Fixture::new().await;
        let old = f.seed_ci_proof().await;
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        let now = old + 120_000;
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        assert_eq!(db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE json_type(response,'$.data.data.repository.head')='object'",[if future {now+60_000} else {now-10_000}]).unwrap(),1);
        if !future {
            db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/commits/%/check-runs%'",[now-5_000]).unwrap();
        }
        f.data.lock().unwrap().calls.clear();
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied");
        assert_eq!(
            f.ci_proof_calls(),
            0,
            "the invalid cached proof was the one inspected"
        );
        assert_eq!(f.ci_rest_calls(), 4);
    }
}

#[tokio::test]
async fn stalled_optional_ci_proof_leaves_a_rest_fallback_budget() {
    let f = Fixture::new().await;
    f.seed_ci_proof().await;
    f.data.lock().unwrap().ci_graph_gate = Some(Arc::new(tokio::sync::Notify::new()));
    let report = tokio::time::timeout(
        // The client itself has a five-second report deadline. Both this CI
        // shortcut and the final personal selector can need bounded fallbacks.
        Duration::from_secs(6),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(report.state, "satisfied");
    assert_eq!(f.ci_rest_calls(), 4);
}
