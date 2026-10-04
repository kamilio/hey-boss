fn evidence() -> Value {
    json!([{
        "report":{
            "data":{"repository":"o/r","number":1,
                "pull_request":{"number":1,"state":"open","head":{"sha":"head"},"base":{"ref":"main","sha":"base","repo":{"full_name":"o/r"}},"user":{"login":"author"}},
                "conflicts":"clean","comments":[],"review_comments":[],
                "reviews":[{"id":1,"state":"COMMENTED","body":"No actionable findings","user":{"login":"reviewer"}}],
                "timeline":[],"review_events":[],"review_threads":[],
                "review_status":{"requested_reviewers":[],"requested_teams":[],"latest_reviews":[],"approved_by":[],"changes_requested_by":[],"dismissed_reviews":[],"resolved_threads":0,"unresolved_threads":0,"outdated_threads":0},
                "ci":{"head_sha":"head","merge_sha":null,"check_runs":[{"id":1,"name":"optional","status":"completed","conclusion":"failure"}],"commit_statuses":[],"workflow_runs":[],"jobs":[],"summary":{"state":"failure","successful":0,"failed":1,"pending":0,"skipped":0,"unknown":0},"failures":[],"errors":[]},"errors":[]},
            "complete":true,"observed_at_ms":100,"oldest_validation_at_ms":100,"validations":[]
        },
        "policy":{"repository":"o/r","pull_number":1,"head_sha":"head","base_branch":"main","base_sha":"base","pr_base_sha":"base","state":"not_required","strict":false,"up_to_date":true,"checks":[],"rules":[],"errors":[],"cursor":"unused","pull_request_state":"open"}
    }])
}

fn observe(value: &Value) -> hey_gh::watcher::Observation {
    hey_gh::watcher::observe(
        &serde_json::from_value(value[0]["report"].clone()).unwrap(),
        &serde_json::from_value(value[0]["policy"].clone()).unwrap(),
    )
}

#[test]
fn reviewed_handoff_seeds_cold_watcher_and_survives_restart() {
    let mut f = Fixture::new();
    f.call(json!({"action":"claim","number":1,"force":false}))
        .unwrap();
    f.call(json!({"action":"ready","number":1,"force":false}))
        .unwrap();
    let version = get_issue(&f.store.db, "named:test", 1, false)
        .unwrap()
        .version;
    f.call(json!({"action":"assign","number":1,"target":"github","if_version":version,"reviewed_evidence":evidence()})).unwrap();
    f.store = Store::open(&f.root.join("issues.db")).unwrap();
    f.store
        .record_github_error("https://github.com/o/r/pull/1", "github", "rate limited")
        .unwrap();
    f.store
        .record_github_observation("https://github.com/o/r/pull/1", &observe(&evidence()), 200)
        .unwrap();
    let view = f.call(json!({"action":"view","number":1})).unwrap();
    assert_eq!(view["issue"]["state"], "ready");
    assert_eq!(view["issue"]["assignment"]["waiting"], true);
    assert_eq!(view["comments"].as_array().unwrap().len(), 0);
}

fn reviewed_handoff(f: &mut Fixture, snapshots: Value) -> Result<Value> {
    let version = get_issue(&f.store.db, "named:test", 1, false)?.version;
    f.call(json!({"action":"assign","number":1,"target":"github","if_version":version,"reviewed_evidence":snapshots}))
}

fn ready_fixture() -> Fixture {
    let mut f = Fixture::new();
    f.call(json!({"action":"claim","number":1,"force":false}))
        .unwrap();
    f.call(json!({"action":"ready","number":1,"force":false}))
        .unwrap();
    f
}

fn green_policy_evidence() -> Value {
    let mut value = evidence();
    value[0]["report"]["data"]["ci"]["check_runs"] = json!([{"id":1,"name":"test","app":{"id":15368},"head_sha":"head","status":"completed","conclusion":"success"}]);
    value[0]["report"]["data"]["ci"]["summary"] =
        json!({"state":"success","successful":1,"failed":0,"pending":0,"skipped":0,"unknown":0});
    value[0]["policy"]["state"] = json!("satisfied");
    value[0]["policy"]["policy_sha"] = json!("base");
    value[0]["policy"]["checks"] =
        json!([{"context":"test","app_id":15368,"state":"satisfied","sha":"head","url":null}]);
    value
}

#[test]
fn equivalent_policy_refresh_preserves_ready_and_dependency_until_real_failure() {
    let mut f = ready_fixture();
    let mut value = green_policy_evidence();
    reviewed_handoff(&mut f, value.clone()).unwrap();
    f.call(json!({"action":"create","title":"Dependent","body":"","labels":[]}))
        .unwrap();
    f.call(json!({"action":"set_blockers","number":2,"blockers":[1]}))
        .unwrap();
    let before = f.call(json!({"action":"view","number":1})).unwrap();
    let child = get_issue(&f.store.db, "named:test", 2, false).unwrap();
    assert_eq!(child.state, "open");
    let url = "https://github.com/o/r/pull/1";
    value[0]["policy"]["base_sha"] = json!("advanced-main");
    value[0]["policy"]["policy_sha"] = json!("advanced-main");
    for checked_at in [200, 300] {
        f.store
            .record_github_observation(url, &observe(&value), checked_at)
            .unwrap();
        f.store = Store::open(&f.root.join("issues.db")).unwrap();
        let after = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(after["issue"]["state"], "ready");
        assert_eq!(after["issue"]["version"], before["issue"]["version"]);
        assert_eq!(after["issue"]["assignee"], "watcher:github");
        assert_eq!(after["comments"], before["comments"]);
        assert_eq!(
            get_issue(&f.store.db, "named:test", 2, false)
                .unwrap()
                .version,
            child.version
        );
        assert_eq!(
            after["issue"]["github_status"]["prs"][url]["evidence"]["policy_sha"],
            "advanced-main"
        );
    }
    value[0]["policy"]["state"] = json!("failure");
    value[0]["policy"]["checks"][0]["state"] = json!("failure");
    value[0]["report"]["data"]["ci"]["check_runs"][0]["id"] = json!(2);
    value[0]["report"]["data"]["ci"]["check_runs"][0]["conclusion"] = json!("failure");
    f.store
        .record_github_observation(url, &observe(&value), 400)
        .unwrap();
    let failed = f.call(json!({"action":"view","number":1})).unwrap();
    assert_eq!(failed["issue"]["state"], "open");
    assert!(failed["issue"]["assignee"].is_null());
    assert_eq!(
        get_issue(&f.store.db, "named:test", 2, false)
            .unwrap()
            .state,
        "blocked"
    );
    f.store
        .record_github_observation(url, &observe(&value), 500)
        .unwrap();
    let duplicate = f.call(json!({"action":"view","number":1})).unwrap();
    assert_eq!(duplicate["comments"], failed["comments"]);
    assert_eq!(duplicate["issue"]["version"], failed["issue"]["version"]);
}

#[test]
fn incomplete_policy_refresh_does_not_create_ready_or_hide_errors() {
    let mut f = Fixture::new();
    f.assign("github").unwrap();
    let url = "https://github.com/o/r/pull/1";
    let mut value = green_policy_evidence();
    value[0]["report"]["complete"] = json!(false);
    value[0]["policy"]["base_sha"] = json!("advanced-main");
    f.store
        .record_github_error(url, "reviews", "unavailable")
        .unwrap();
    let policy = serde_json::from_value(value[0]["policy"].clone()).unwrap();
    f.store
        .record_github_observation(url, &hey_gh::watcher::observe_required(&policy), 200)
        .unwrap();
    let view = f.call(json!({"action":"view","number":1})).unwrap();
    assert_eq!(view["issue"]["state"], "open");
    assert_eq!(
        view["issue"]["github_status"]["prs"][url]["error"],
        "unavailable"
    );
    assert!(reviewed_handoff(&mut f, value).is_err());
}

#[test]
fn equivalent_refresh_keeps_real_readiness_regressions_actionable() {
    for kind in ["missing", "conflict", "outdated", "app", "head", "review"] {
        let mut f = ready_fixture();
        let mut value = green_policy_evidence();
        if kind == "outdated" {
            value[0]["policy"]["strict"] = json!(true);
        }
        reviewed_handoff(&mut f, value.clone()).unwrap();
        value[0]["policy"]["base_sha"] = json!("advanced-main");
        value[0]["policy"]["policy_sha"] = json!("advanced-main");
        match kind {
            "missing" => {
                value[0]["policy"]["state"] = json!("missing");
                value[0]["policy"]["checks"][0]["state"] = json!("missing");
                value[0]["report"]["data"]["ci"]["check_runs"] = json!([]);
            }
            "conflict" => value[0]["report"]["data"]["conflicts"] = json!("conflicting"),
            "outdated" => {
                value[0]["policy"]["up_to_date"] = json!(false);
            }
            "app" => value[0]["policy"]["checks"][0]["app_id"] = json!(2),
            "head" => {
                value[0]["policy"]["head_sha"] = json!("new-head");
                value[0]["report"]["data"]["ci"]["head_sha"] = json!("new-head");
                value[0]["report"]["data"]["pull_request"]["head"]["sha"] = json!("new-head");
            }
            "review" => value[0]["report"]["data"]["reviews"][0]["body"] = json!("Fix the race"),
            _ => unreachable!(),
        }
        f.store
            .record_github_observation("https://github.com/o/r/pull/1", &observe(&value), 200)
            .unwrap();
        let view = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(view["issue"]["state"], "open", "{kind}");
        assert!(view["issue"]["assignee"].is_null(), "{kind}");
    }
}

#[test]
fn semantic_policy_upgrade_recognizes_only_exact_legacy_evidence() {
    for changed in [false, true] {
        let mut f = ready_fixture();
        let mut value = green_policy_evidence();
        reviewed_handoff(&mut f, value.clone()).unwrap();
        let legacy = observe(&value).evidence["legacy_policy_fingerprint"]
            .as_str()
            .unwrap()
            .to_owned();
        f.store
            .db
            .execute(
                "UPDATE issue_github_signals SET signal=?1 WHERE signal LIKE 'policy:v2:%'",
                [&legacy],
            )
            .unwrap();
        if changed {
            value[0]["policy"]["strict"] = json!(true);
        }
        f.store
            .record_github_observation("https://github.com/o/r/pull/1", &observe(&value), 200)
            .unwrap();
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .state,
            if changed { "open" } else { "ready" }
        );
    }
}

fn changed_evidence(kind: &str) -> Value {
    let mut value = evidence();
    match kind {
        "review" => value[0]["report"]["data"]["reviews"][0]["body"] = json!("Please fix the race"),
        "thread" => {
            value[0]["report"]["data"]["review_threads"] = json!([{"id":"thread1","isResolved":false,"isOutdated":false,"comments":{"nodes":[{"id":"comment1","body":"New finding","author":{"login":"reviewer"}}]}}])
        }
        "head" => {
            value[0]["report"]["data"]["pull_request"]["head"]["sha"] = json!("head2");
            value[0]["report"]["data"]["ci"]["head_sha"] = json!("head2");
            value[0]["policy"]["head_sha"] = json!("head2");
        }
        "rerun" => value[0]["report"]["data"]["ci"]["check_runs"][0]["id"] = json!(2),
        "policy" => value[0]["policy"]["strict"] = json!(true),
        "required_failure" => {
            value[0]["policy"]["state"] = json!("failure");
            value[0]["policy"]["checks"] = json!([{"context":"optional","app_id":null,"state":"failure","sha":"head","url":null}]);
        }
        _ => unreachable!(),
    }
    value
}

#[test]
fn reviewed_handoff_new_evidence_wakes_once_in_both_race_orders() {
    for kind in [
        "review",
        "thread",
        "head",
        "rerun",
        "policy",
        "required_failure",
    ] {
        for before in [false, true] {
            let mut f = ready_fixture();
            if before {
                f.assign("github").unwrap();
                f.store
                    .record_github_observation(
                        "https://github.com/o/r/pull/1",
                        &observe(&changed_evidence(kind)),
                        200,
                    )
                    .unwrap();
                let old = f.call(json!({"action":"view","number":1})).unwrap();
                let error = reviewed_handoff(&mut f, evidence()).unwrap_err();
                assert_eq!(error.code, "conflict", "{kind}");
                assert_eq!(
                    old["issue"],
                    f.call(json!({"action":"view","number":1})).unwrap()["issue"]
                );
            } else {
                reviewed_handoff(&mut f, evidence()).unwrap();
                f.store
                    .record_github_observation(
                        "https://github.com/o/r/pull/1",
                        &observe(&changed_evidence(kind)),
                        200,
                    )
                    .unwrap();
            }
            let first = f.call(json!({"action":"view","number":1})).unwrap();
            assert_eq!(first["issue"]["state"], "open", "{kind}");
            assert_eq!(first["issue"]["assignment"]["waiting"], false, "{kind}");
            f.store = Store::open(&f.root.join("issues.db")).unwrap();
            f.store
                .record_github_observation(
                    "https://github.com/o/r/pull/1",
                    &observe(&changed_evidence(kind)),
                    300,
                )
                .unwrap();
            let second = f.call(json!({"action":"view","number":1})).unwrap();
            assert_eq!(
                first["issue"]["version"], second["issue"]["version"],
                "{kind}"
            );
            assert_eq!(first["comments"], second["comments"], "{kind}");
        }
    }
}

#[test]
fn reviewed_handoff_rejects_incomplete_mismatched_and_missing_snapshots_atomically() {
    for kind in [
        "report",
        "quota",
        "pending",
        "head",
        "base",
        "missing",
        "duplicate",
        "foreign",
    ] {
        let mut f = ready_fixture();
        let mut value = evidence();
        match kind {
            "report" => value[0]["report"]["complete"] = json!(false),
            "quota" => {
                value[0]["policy"]["errors"] =
                    json!([{"source":"rulesets","message":"rate limited"}])
            }
            "pending" => value[0]["report"]["data"]["ci"]["summary"]["pending"] = json!(1),
            "head" => value[0]["policy"]["head_sha"] = json!("other"),
            "base" => value[0]["policy"]["pr_base_sha"] = json!("other"),
            "missing" => value = json!([]),
            "duplicate" => value.as_array_mut().unwrap().push(evidence()[0].clone()),
            "foreign" => value[0]["report"]["data"]["repository"] = json!("other/repo"),
            _ => unreachable!(),
        }
        let old = f.call(json!({"action":"view","number":1})).unwrap();
        assert!(reviewed_handoff(&mut f, value).is_err(), "{kind}");
        assert_eq!(
            old["issue"],
            f.call(json!({"action":"view","number":1})).unwrap()["issue"],
            "{kind}"
        );
        assert_eq!(
            f.store
                .db
                .query_row("SELECT count(*) FROM issue_github_signals", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{kind}"
        );
    }
}

#[test]
fn reviewed_handoff_idempotent_retry_does_not_consume_a_later_event() {
    let mut f = ready_fixture();
    f.request.request_id = Some("reviewed-handoff-retry".into());
    let original = reviewed_handoff(&mut f, evidence()).unwrap();
    let operation = f.request.operation.clone();
    f.store
        .record_github_observation(
            "https://github.com/o/r/pull/1",
            &observe(&changed_evidence("review")),
            200,
        )
        .unwrap();
    f.request.operation = operation;
    assert_eq!(f.store.execute(&f.request).unwrap(), original);
    f.request.request_id = None;
    let view = f.call(json!({"action":"view","number":1})).unwrap();
    assert_eq!(view["issue"]["state"], "open");
    assert_eq!(view["issue"]["assignment"]["waiting"], false);
}

#[test]
fn reviewed_handoff_preserves_ownership_and_foreign_reservations() {
    for foreign in [false, true] {
        let mut f = ready_fixture();
        if foreign {
            f.request.actor.as_mut().unwrap().id = "human:other".into();
        } else {
            f.store.db.execute("INSERT INTO fleet_allocations(project_id,issue_number,node) VALUES('named:test',1,'other-machine')", []).unwrap();
        }
        let before = f.call(json!({"action":"view","number":1})).unwrap();
        assert!(reviewed_handoff(&mut f, evidence()).is_err());
        assert_eq!(
            before["issue"],
            f.call(json!({"action":"view","number":1})).unwrap()["issue"]
        );
        assert_eq!(
            f.store
                .db
                .query_row("SELECT count(*) FROM issue_github_signals", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn reviewed_handoff_supersedes_only_provably_older_evidence() {
    for fresh in [false, true] {
        let mut f = ready_fixture();
        f.assign("github").unwrap();
        f.store
            .record_github_observation(
                "https://github.com/o/r/pull/1",
                &observe(&changed_evidence("review")),
                50,
            )
            .unwrap();
        let mut reviewed = evidence();
        reviewed[0]["report"]["oldest_validation_at_ms"] = json!(if fresh { 100 } else { 25 });
        reviewed[0]["policy"]["oldest_validation_at_ms"] = json!(100);
        assert_eq!(reviewed_handoff(&mut f, reviewed).is_ok(), fresh);
    }
}

#[test]
fn reviewed_handoff_managed_attempt_acknowledges_only_its_snapshot() {
    for late in [false, true] {
        let mut f = Fixture::new();
        f.call(json!({"action":"claim","number":1,"force":false}))
            .unwrap();
        let job = crate::issues::worker::Job {
            session_ref: None,
            id: "review-run".into(),
            worker_id: String::new(),
            resume_session: None,
            project: f.request.project.clone(),
            issue: serde_json::to_value(get_issue(&f.store.db, "named:test", 1, false).unwrap())
                .unwrap(),
            comments: vec![],
            config: crate::issues::worker::ProjectConfig::default(),
            actor: f.request.actor.clone().unwrap(),
            owner_pid: 1,
            owner_start: "start".into(),
            machine: f.request.actor.as_ref().unwrap().machine.clone(),
        };
        f.store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,claimed_at) VALUES(?1,'named:test',1,?2,?3,'running',1,'start',?4,0,0,0)",params![job.id,serde_json::to_string(&job).unwrap(),job.actor.id,job.machine]).unwrap();
        f.assign("github").unwrap();
        f.store
            .record_github_observation("https://github.com/o/r/pull/1", &observe(&evidence()), 100)
            .unwrap();
        queue_steering(&f.store.db, &job.id).unwrap();
        f.call(json!({"action":"ready","number":1,"force":false}))
            .unwrap();
        reviewed_handoff(&mut f, evidence()).unwrap();
        if late {
            f.store
                .record_github_observation(
                    "https://github.com/o/r/pull/1",
                    &observe(&changed_evidence("thread")),
                    200,
                )
                .unwrap();
        }
        f.store = Store::open(&f.root.join("issues.db")).unwrap();
        assert_eq!(release_worker(&f.store.db, &job, "blocked").unwrap(), !late);
        let view = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(view["issue"]["state"], if late { "open" } else { "ready" });
        assert_eq!(view["issue"]["assignment"]["waiting"], !late);
    }
}

#[test]
fn reviewed_handoff_rechecks_observation_after_acquiring_writer() {
    let mut f = ready_fixture();
    f.assign("github").unwrap();
    let url = "https://github.com/o/r/pull/1";
    let observation = observe(&evidence());
    // Model the watcher preflight on a separate connection before handoff.
    let observer = Store::open(&f.root.join("issues.db")).unwrap();
    assert!(observation_updates(&observer.db, url, &observation, 200).unwrap()[0].wake);
    reviewed_handoff(&mut f, evidence()).unwrap();
    // The write phase must reread durable acknowledgements, including after a
    // separate connection commits them between preflight and the writer lock.
    let tx = observer.db.unchecked_transaction().unwrap();
    assert!(
        observation_updates(&tx, url, &observation, 200)
            .unwrap()
            .iter()
            .all(|update| !update.wake)
    );
    tx.commit().unwrap();
}

#[test]
fn reviewed_handoff_partial_quota_recovery_and_policy_changes_are_exact() {
    let mut f = ready_fixture();
    reviewed_handoff(&mut f, evidence()).unwrap();
    let url = "https://github.com/o/r/pull/1";
    let mut policy: hey_gh::RequiredChecksReport =
        serde_json::from_value(evidence()[0]["policy"].clone()).unwrap();
    f.store
        .record_github_error(url, "reviews", "quota exhausted")
        .unwrap();
    f.store
        .record_github_observation(url, &hey_gh::watcher::observe_required(&policy), 200)
        .unwrap();
    f.store = Store::open(&f.root.join("issues.db")).unwrap();
    f.store
        .record_github_observation(url, &observe(&evidence()), 300)
        .unwrap();
    assert_eq!(
        get_issue(&f.store.db, "named:test", 1, false)
            .unwrap()
            .state,
        "ready"
    );
    policy.strict = true;
    f.store
        .record_github_observation(url, &hey_gh::watcher::observe_required(&policy), 400)
        .unwrap();
    let changed = f.call(json!({"action":"view","number":1})).unwrap();
    assert_eq!(changed["issue"]["state"], "open");
    f.store
        .record_github_observation(url, &observe(&changed_evidence("policy")), 500)
        .unwrap();
    let full = f.call(json!({"action":"view","number":1})).unwrap();
    assert_eq!(changed["comments"], full["comments"]);
    assert_eq!(changed["issue"]["version"], full["issue"]["version"]);
}

#[test]
fn reviewed_handoff_checks_signals_even_when_display_evidence_is_identical() {
    let mut f = ready_fixture();
    f.assign("github").unwrap();
    let original = observe(&evidence());
    f.store
        .record_github_observation("https://github.com/o/r/pull/1", &original, 100)
        .unwrap();
    let mut changed = observe(&changed_evidence("review"));
    // A changed review can fall outside the bounded display window.
    changed.evidence = original.evidence;
    f.store
        .record_github_observation("https://github.com/o/r/pull/1", &changed, 200)
        .unwrap();
    assert!(reviewed_handoff(&mut f, evidence()).is_err());
}

#[test]
fn reviewed_handoff_requires_all_prs_before_seeding_any_acknowledgements() {
    let mut f = ready_fixture();
    f.call(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/2","purpose":"fix"})).unwrap();
    assert!(reviewed_handoff(&mut f, evidence()).is_err());
    assert_eq!(
        f.store
            .db
            .query_row("SELECT count(*) FROM issue_github_signals", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
