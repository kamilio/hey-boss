// Exercise the supported request path with a real saved worker attempt.
fn requirements_ready(f: &mut HandoffFixture, acknowledge: bool) -> Result<Value> {
    let guard = ready::snapshot(&f.store.db, &f.job.project.id, &f.issue()).unwrap();
    f.store.execute(&Request {
        version: 1,
        project: f.job.project.clone(),
        project_override: None,
        actor: Some(f.job.actor.clone()),
        request_id: Some(format!("ready-{}", guard.if_version)),
        operation: serde_json::from_value(json!({
            "action":"ready", "number":1, "force":false, "guard":guard,
            "acknowledge_requirements":acknowledge
        }))
        .unwrap(),
    })
}

fn edit_handoff_notes(f: &mut HandoffFixture) {
    f.apply(
        serde_json::from_value(json!({
            "action":"edit", "number":1, "body":"Published handoff notes\n\nRequirements",
            "add_labels":[], "remove_labels":[]
        }))
        .unwrap(),
    );
}

#[test]
fn requirements_handoff_reconciles_only_an_explicit_current_snapshot() {
    for outcome in ["completed", "blocked", "interrupted", "failed"] {
        for change in [
            "none",
            "unchanged",
            "unacknowledged",
            "body",
            "external",
            "version",
            "run",
            "actor",
            "artifact",
            "deleted",
            "owner",
            "watcher",
        ] {
            let mut f = HandoffFixture::new(true);
            if change != "unchanged" {
                edit_handoff_notes(&mut f);
            }
            requirements_ready(&mut f, change != "unacknowledged").unwrap();
            assert_eq!(
                f.store.worker_cancelled(&f.job).unwrap(),
                change == "unacknowledged",
                "{outcome}/{change}"
            );
            f.apply(Operation::Assign {
                reviewed_evidence: None,
                number: 1,
                target: "github".into(),
                if_version: f.issue().version,
            });
            match change {
                "body" => {
                    f.store
                        .db
                        .execute(
                            "UPDATE issues SET body='Later requirements',version=version+1",
                            [],
                        )
                        .unwrap();
                }
                "external" => {
                    let mut request = Request {
                        version:1, project:f.job.project.clone(), project_override:None,
                        actor:Some(f.job.actor.clone()), request_id:None,
                        operation:serde_json::from_value(json!({"action":"edit","number":1,"body":"External requirements","add_labels":[],"remove_labels":[]})).unwrap(),
                    };
                    request.actor.as_mut().unwrap().id = "human:boss".into();
                    f.store.execute(&request).unwrap();
                }
                "version" => {
                    f.store
                        .db
                        .execute("UPDATE issues SET version=version+1", [])
                        .unwrap();
                }
                "run" => {
                    f.store.db.execute("UPDATE events SET data=json_set(data,'$.requirements_handoff.run','foreign-run') WHERE action='assigned'", []).unwrap();
                }
                "actor" => {
                    f.store
                        .db
                        .execute(
                            "UPDATE events SET actor='human:boss' WHERE action='assigned'",
                            [],
                        )
                        .unwrap();
                }
                "artifact" => {
                    f.store
                        .db
                        .execute("UPDATE issues SET labels='[\"task:plan\"]'", [])
                        .unwrap();
                }
                "deleted" => {
                    f.store
                        .db
                        .execute("UPDATE issues SET deleted_at=1,assignee=NULL", [])
                        .unwrap();
                }
                "owner" => {
                    f.store
                        .db
                        .execute("UPDATE issues SET assignee='human:boss'", [])
                        .unwrap();
                }
                "watcher" => watch_event(&mut f, "new-evidence"),
                _ => {}
            }
            f.store = Store::open(&f.root.join("issues.db")).unwrap();
            f.store
                .worker_finish(&f.job, outcome, "Published work; explicit handoff")
                .unwrap();
            let issue = get_issue(&f.store.db, &f.job.project.id, 1, true).unwrap();
            let parked = matches!(change, "none" | "unchanged");
            assert_eq!(
                issue.assignee.as_deref() == Some("watcher:github"),
                parked,
                "{outcome}/{change}"
            );
            if parked {
                assert_eq!(f.issue().state, "ready");
                assert_eq!(f.state(), outcome);
                let retry: Option<i64> = f
                    .store
                    .db
                    .query_row("SELECT retry_at FROM worker_runs", [], |r| r.get(0))
                    .unwrap();
                assert!(retry.is_none());
                f.store
                    .worker_finish(&f.job, outcome, "Duplicate callback")
                    .unwrap();
                assert_eq!(f.issue().state, "ready");
            } else if outcome == "completed" && change != "watcher" {
                assert_eq!(f.state(), "blocked", "{change}");
            }
            assert_ne!(issue.state, "closed", "Delivery never resolves the issue");
        }
    }
}

#[test]
fn requirements_handoff_rejects_stale_transfer_and_can_be_explicitly_renewed() {
    let mut f = HandoffFixture::new(true);
    edit_handoff_notes(&mut f);
    requirements_ready(&mut f, true).unwrap();
    f.apply(Operation::Comment {
        number: 1,
        body: "Later handoff detail".into(),
        allow_long_comment: false,
    });
    let before = f.issue();
    let request = Request {
        version: 1,
        project: f.job.project.clone(),
        project_override: None,
        actor: Some(f.job.actor.clone()),
        request_id: None,
        operation: Operation::Assign {
            reviewed_evidence: None,
            number: 1,
            target: "github".into(),
            if_version: before.version,
        },
    };
    assert!(
        f.store
            .execute(&request)
            .unwrap_err()
            .message
            .contains("acknowledgement changed")
    );
    assert_eq!(f.issue().version, before.version);
    assert_eq!(f.issue().assignee, before.assignee);
    requirements_ready(&mut f, true).unwrap();
    f.apply(Operation::Assign {
        reviewed_evidence: None,
        number: 1,
        target: "github".into(),
        if_version: f.issue().version,
    });
    f.store
        .worker_finish(&f.job, "completed", "Renewed handoff")
        .unwrap();
    assert_eq!(f.issue().state, "ready");
    assert_eq!(f.issue().assignee.as_deref(), Some("watcher:github"));
}

#[test]
fn requirements_handoff_requires_guards_and_an_active_owner() {
    for mode in ["unguarded", "stale", "foreign", "unclaimed", "finished"] {
        let mut f = HandoffFixture::new(true);
        edit_handoff_notes(&mut f);
        let guard = ready::snapshot(&f.store.db, &f.job.project.id, &f.issue()).unwrap();
        let mut request = Request {
            version:1, project:f.job.project.clone(), project_override:None,
            actor:Some(f.job.actor.clone()), request_id:Some("ack-guard".into()),
            operation:serde_json::from_value(json!({"action":"ready","number":1,"force":false,"guard":if mode == "unguarded" {Value::Null} else {json!(guard)},"acknowledge_requirements":true})).unwrap(),
        };
        match mode {
            "stale" => {
                f.store
                    .db
                    .execute("UPDATE issues SET version=version+1", [])
                    .unwrap();
            }
            "foreign" => {
                request.actor.as_mut().unwrap().id = "human:boss".into();
            }
            "unclaimed" => {
                f.store
                    .db
                    .execute("UPDATE worker_runs SET claimed_at=NULL", [])
                    .unwrap();
            }
            "finished" => {
                f.store
                    .db
                    .execute("UPDATE worker_runs SET finished_at=1", [])
                    .unwrap();
            }
            _ => {}
        }
        let before = f.issue();
        assert!(f.store.execute(&request).is_err(), "{mode}");
        assert_eq!(f.issue().version, before.version);
        assert_eq!(f.issue().assignee, before.assignee);
        assert_eq!(f.issue().state, "open");
    }
}

#[test]
fn requirements_handoff_uses_companion_run_identity_and_survives_replica_restart() {
    let mut f = HandoffFixture::new(true);
    edit_handoff_notes(&mut f);
    // Model the supervisor's view: this attempt lives on a companion.
    f.store
        .db
        .execute("UPDATE worker_runs SET finished_at=1", [])
        .unwrap();
    f.store
        .db
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS fleet_state(key TEXT PRIMARY KEY,value TEXT NOT NULL)",
        )
        .unwrap();
    f.store.db.execute("INSERT OR REPLACE INTO fleet_state VALUES('machines',?1)", [json!([{"workers":[{"runs":[{"id":f.job.id,"project_id":f.job.project.id,"number":1,"actor_id":f.job.actor.id,"finished_at":null}]}]}]).to_string()]).unwrap();
    requirements_ready(&mut f, true).unwrap();
    f.apply(Operation::Assign {
        reviewed_evidence: None,
        number: 1,
        target: "github".into(),
        if_version: f.issue().version,
    });
    // The same event metadata is consumed on the companion's active run.
    f.store
        .db
        .execute("UPDATE worker_runs SET finished_at=NULL", [])
        .unwrap();
    f.store = Store::open(&f.root.join("issues.db")).unwrap();
    f.store
        .worker_finish(&f.job, "completed", "Companion delivery")
        .unwrap();
    assert_eq!(f.state(), "completed");
    assert_eq!(f.issue().state, "ready");
    assert_eq!(f.issue().assignee.as_deref(), Some("watcher:github"));
}

#[test]
fn requirements_handoff_cannot_acknowledge_a_changed_artifact_mode() {
    let mut f = HandoffFixture::new(true);
    edit_handoff_notes(&mut f);
    f.store
        .db
        .execute(
            "UPDATE issues SET labels='[\"task:plan\"]',version=version+1",
            [],
        )
        .unwrap();
    requirements_ready(&mut f, true).unwrap();
    f.apply(Operation::Assign {
        reviewed_evidence: None,
        number: 1,
        target: "github".into(),
        if_version: f.issue().version,
    });
    f.store
        .worker_finish(&f.job, "completed", "Changed task mode")
        .unwrap();
    assert_eq!(f.state(), "blocked");
    assert!(f.issue().assignee.is_none());
    assert_eq!(f.issue().state, "open");
}
