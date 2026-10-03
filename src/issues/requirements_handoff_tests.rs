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

// Resident workers predating preservation receipts compare these exact values.
fn handoff_version_is_current(store: &Store, project: &str) -> bool {
    store.db.query_row("SELECT i.version=json_extract(e.data,'$.requirements_handoff.version') FROM issues i JOIN events e ON e.project_id=i.project_id AND e.issue_number=i.number WHERE i.project_id=?1 AND i.number=1 AND e.action IN ('assigned','claimed','ready','unassigned','closed','reopened','blocked','deleted','restored') ORDER BY e.id DESC LIMIT 1", [project], |r| r.get::<_,Option<bool>>(0)).unwrap().unwrap_or(false)
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
fn requirements_handoff_rejects_external_transfer_and_can_be_explicitly_renewed() {
    let mut f = HandoffFixture::new(true);
    edit_handoff_notes(&mut f);
    requirements_ready(&mut f, true).unwrap();
    let mut external = f.job.actor.clone();
    external.id = "human:boss".into();
    f.store
        .execute(&Request {
            version: 1,
            project: f.job.project.clone(),
            project_override: None,
            actor: Some(external),
            request_id: None,
            operation: Operation::Comment {
                number: 1,
                body: "Parent advanced; restack".into(),
                allow_long_comment: false,
            },
        })
        .unwrap();
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
fn requirements_handoff_owner_comments_preserve_exact_guarded_completion() {
    for before_assignment in [false, true] {
        let mut f = HandoffFixture::new(true);
        edit_handoff_notes(&mut f);
        requirements_ready(&mut f, true).unwrap();
        if !before_assignment {
            f.apply(Operation::Assign {
                reviewed_evidence: None,
                number: 1,
                target: "github".into(),
                if_version: f.issue().version,
            });
        }
        f.apply(Operation::Comment {
            number: 1,
            body: "Published delivery details".into(),
            allow_long_comment: false,
        });
        if before_assignment {
            f.apply(Operation::Assign {
                reviewed_evidence: None,
                number: 1,
                target: "github".into(),
                if_version: f.issue().version,
            });
        }
        assert!(handoff_version_is_current(&f.store, &f.job.project.id));
        f.store
            .worker_finish(&f.job, "completed", "Delivered")
            .unwrap();
        assert_eq!(f.state(), "completed");
        assert_eq!(f.issue().state, "ready");
        assert_eq!(f.issue().assignee.as_deref(), Some("watcher:github"));
    }
}

#[test]
fn requirements_handoff_preservation_cannot_cover_a_gap_or_changed_binding() {
    for change in ["gap", "run", "actor", "digest", "source", "external_row"] {
        let mut f = HandoffFixture::new(true);
        requirements_ready(&mut f, true).unwrap();
        f.apply(Operation::Assign {
            reviewed_evidence: None,
            number: 1,
            target: "github".into(),
            if_version: f.issue().version,
        });
        if change == "gap" {
            f.store
                .db
                .execute("UPDATE issues SET version=version+1", [])
                .unwrap();
        }
        f.apply(Operation::Comment {
            number: 1,
            body: "Published delivery details".into(),
            allow_long_comment: false,
        });
        // Exercise receipts retained from the earlier, version-advancing writer.
        // Zero-delta audit notes cannot authorize a revision advance at all.
        f.store.db.execute_batch("UPDATE events SET data=json_set(data,'$.version',json_extract(data,'$.previous_version')+1) WHERE action='requirements_preserved'; UPDATE issues SET version=version+1;").unwrap();
        match change {
            "run" => {
                f.store.db.execute("UPDATE events SET data=json_set(data,'$.acknowledgement.run','another-run') WHERE action='requirements_preserved'", []).unwrap();
            }
            "actor" => {
                f.store.db.execute("UPDATE events SET actor='human:boss' WHERE action='requirements_preserved'", []).unwrap();
            }
            "digest" => {
                f.store.db.execute("UPDATE events SET data=json_set(data,'$.acknowledgement.sha256','changed') WHERE action='requirements_preserved'", []).unwrap();
            }
            "source" => {
                f.store.db.execute("UPDATE events SET data=json_set(data,'$.source','prose') WHERE action='requirements_preserved'", []).unwrap();
            }
            // A split replica batch can deliver the comment before its event
            // and version bump. Do not consume an obsolete acknowledgement.
            "external_row" => {
                f.store.db.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?1,1,'human:boss','Parent advanced; restack',1)", [&f.job.project.id]).unwrap();
            }
            _ => {}
        }
        f.store
            .worker_finish(&f.job, "completed", "Delivered")
            .unwrap();
        assert_eq!(f.state(), "blocked", "{change}");
        assert_ne!(
            f.issue().assignee.as_deref(),
            Some("watcher:github"),
            "{change}"
        );
    }
}

#[test]
fn requirements_handoff_two_store_completion_preserves_only_unchanged_work() {
    for case in [
        "control_edited",
        "control_unchanged",
        "delayed_notes",
        "final_notes",
        "external_comment",
        "external_delayed",
        "external_rows_first",
        "external_events_first",
        "requirements",
        "owner",
        "watcher",
        "approval",
        "unexplained_version",
        "version_collision",
    ] {
        let mut f = HandoffFixture::new(true);
        f.job.actor.machine = "peer".into();
        f.job.machine = "peer".into();
        if case != "control_unchanged" {
            edit_handoff_notes(&mut f);
        }
        let mut peer = Store::open(&f.root.join("peer.db")).unwrap();
        let replica = crate::fleet::test_replica;
        replica(
            &f.store.db,
            &json!({"replica":"capture","role":"controller","node":"main"}),
        );
        replica(
            &peer.db,
            &json!({"replica":"capture","role":"agent","node":"peer"}),
        );
        f.store
            .db
            .execute(
                "INSERT INTO fleet_allocations VALUES(?1,1,'peer')",
                [&f.job.project.id],
            )
            .unwrap();
        let pull = |main: &Store, peer: &Store, receipts: Value| {
            let payload = replica(&main.db, &json!({"replica":"snapshot","node":"peer"}));
            replica(
                &peer.db,
                &json!({"replica":"pull","node":"peer","payload":payload,"receipts":receipts}),
            );
        };
        let project_id = f.job.project.id.clone();
        let sync = |main: &Store, peer: &Store| {
            let changes: Vec<Value> = peer.db.prepare("SELECT seq,table_name,before_json,after_json,created_at FROM fleet_outbox ORDER BY seq").unwrap().query_map([], |r| Ok(json!({"seq":r.get::<_,i64>(0)?,"table_name":r.get::<_,String>(1)?,"before_json":r.get::<_,Option<String>>(2)?,"after_json":r.get::<_,Option<String>>(3)?,"created_at":r.get::<_,i64>(4)?}))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            if matches!(case, "external_rows_first" | "external_events_first") {
                let table = if case == "external_rows_first" {
                    "comments"
                } else {
                    "events"
                };
                let first: Vec<_> = changes
                    .iter()
                    .filter(|c| c["table_name"] == table || c["table_name"] == "agents")
                    .cloned()
                    .collect();
                let cursor: i64 = main
                    .db
                    .query_row("SELECT coalesce(max(seq),0) FROM fleet_outbox", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                let request = json!({"replica":"accept","node":"peer","changes":first});
                let result = replica(&main.db, &request);
                assert!(
                    result
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|r| r["state"] == "applied"),
                    "{case}: {result}"
                );
                assert!(
                    !handoff_version_is_current(main, &project_id),
                    "Legacy reader must reject the first split batch: {case}"
                );
                let first_delta: String = main.db.query_row("SELECT table_name FROM fleet_outbox WHERE seq>?1 AND table_name IN ('issues','comments','events') ORDER BY seq LIMIT 1", [cursor], |r| r.get(0)).unwrap();
                assert_eq!(
                    first_delta, "issues",
                    "Canonical pulls must invalidate before exposing history"
                );
                let version = get_issue(&main.db, &project_id, 1, true).unwrap().version;
                assert_eq!(replica(&main.db, &request), result);
                assert_eq!(
                    get_issue(&main.db, &project_id, 1, true).unwrap().version,
                    version
                );
            }
            let request = json!({"replica":"accept","node":"peer","changes":changes});
            let receipts = replica(&main.db, &request);
            assert!(
                receipts
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|r| r["state"] == "applied"),
                "{case}: {receipts}"
            );
            assert_eq!(
                replica(&main.db, &request),
                receipts,
                "Idempotent replay: {case}"
            );
            pull(main, peer, receipts);
            assert_eq!(
                peer.db
                    .query_row("SELECT count(*) FROM fleet_outbox", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        };
        pull(&f.store, &peer, json!([]));
        peer.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,claimed_at) VALUES(?1,?2,1,?3,?4,'running',1,'start','peer',0,0,0)", params![f.job.id,f.job.project.id,serde_json::to_string(&f.job).unwrap(),f.job.actor.id]).unwrap();
        // The supervisor sees the companion's reservation through its real fleet shape.
        f.store
            .db
            .execute("UPDATE worker_runs SET finished_at=1", [])
            .unwrap();
        f.store.db.execute("INSERT OR REPLACE INTO fleet_state VALUES('machines',?1)", [json!([{"workers":[{"runs":[{"id":f.job.id,"project_id":f.job.project.id,"number":1,"actor_id":f.job.actor.id,"finished_at":null}]}]}]).to_string()]).unwrap();
        let job = f.job.clone();
        let comment = |store: &mut Store, external: bool| {
            let mut actor = job.actor.clone();
            if external {
                actor.id = "human:reviewer".into();
            }
            store
                .execute(&Request {
                    version: 1,
                    project: job.project.clone(),
                    project_override: None,
                    actor: Some(actor),
                    request_id: None,
                    operation: Operation::Comment {
                        number: 1,
                        body: if external {
                            "Parent advanced; restack"
                        } else {
                            "Published delivery details"
                        }
                        .into(),
                        allow_long_comment: false,
                    },
                })
                .unwrap();
        };
        let delayed = matches!(
            case,
            "delayed_notes" | "external_delayed" | "external_rows_first" | "external_events_first"
        );
        if delayed {
            comment(&mut peer, case != "delayed_notes");
        }
        requirements_ready(&mut f, true).unwrap();
        f.apply(Operation::Assign {
            reviewed_evidence: None,
            number: 1,
            target: "github".into(),
            if_version: f.issue().version,
        });
        if !delayed {
            pull(&f.store, &peer, json!([]));
        }
        if matches!(case, "final_notes" | "version_collision") {
            comment(&mut peer, false);
        }
        if case == "external_comment" {
            comment(&mut f.store, true);
        }
        match case {
            "requirements" => {
                f.store
                    .db
                    .execute(
                        "UPDATE issues SET body='New requirements',version=version+1",
                        [],
                    )
                    .unwrap();
            }
            "owner" => {
                f.store
                    .db
                    .execute(
                        "UPDATE issues SET assignee='human:boss',version=version+1",
                        [],
                    )
                    .unwrap();
            }
            "unexplained_version" | "version_collision" => {
                f.store
                    .db
                    .execute("UPDATE issues SET version=version+1", [])
                    .unwrap();
            }
            "watcher" => watch_event(&mut f, "new-evidence"),
            _ => {}
        }
        sync(&f.store, &peer);
        peer = Store::open(&f.root.join("peer.db")).unwrap();
        if matches!(
            case,
            "control_edited" | "control_unchanged" | "delayed_notes" | "final_notes"
        ) {
            assert!(
                handoff_version_is_current(&f.store, &f.job.project.id),
                "Supervisor legacy reader: {case}"
            );
            assert!(
                handoff_version_is_current(&peer, &f.job.project.id),
                "Companion legacy reader: {case}"
            );
        }
        peer.worker_finish(
            &f.job,
            "completed",
            if case == "approval" {
                "Codex needs input or approval: billing"
            } else {
                "Delivered"
            },
        )
        .unwrap();
        let issue = get_issue(&peer.db, &f.job.project.id, 1, true).unwrap();
        let parked = matches!(
            case,
            "control_edited" | "control_unchanged" | "delayed_notes" | "final_notes"
        );
        assert_eq!(
            issue.assignee.as_deref() == Some("watcher:github"),
            parked,
            "{case}"
        );
        if parked {
            assert_eq!(issue.state, "ready", "{case}");
            let (state, retry): (String, Option<i64>) = peer
                .db
                .query_row("SELECT state,retry_at FROM worker_runs", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert_eq!(state, "completed", "{case}");
            assert_eq!(retry, None, "{case}");
            sync(&f.store, &peer);
            assert_eq!(f.issue().state, "ready", "{case}");
            assert_eq!(
                f.issue().assignee.as_deref(),
                Some("watcher:github"),
                "{case}"
            );
            assert_eq!(
                f.store
                    .db
                    .query_row("SELECT count(*) FROM fleet_allocations", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            peer.worker_finish(&f.job, "completed", "Duplicate callback")
                .unwrap();
            assert_eq!(
                peer.db
                    .query_row("SELECT count(*) FROM fleet_outbox", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        if case == "owner" {
            assert_eq!(issue.assignee.as_deref(), Some("human:boss"));
        }
    }
}

#[test]
fn requirements_handoff_suppressed_external_history_does_not_invalidate() {
    let mut f = HandoffFixture::new(true);
    requirements_ready(&mut f, true).unwrap();
    let replica = crate::fleet::test_replica;
    replica(
        &f.store.db,
        &json!({"replica":"capture","role":"controller","node":"main"}),
    );
    f.store.db.execute_batch("CREATE TRIGGER suppress_fixture_note BEFORE INSERT ON comments WHEN NEW.body='Obsolete fixture note' BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    let before = f.issue().version;
    let cursor: i64 = f
        .store
        .db
        .query_row("SELECT coalesce(max(seq),0) FROM fleet_outbox", [], |r| {
            r.get(0)
        })
        .unwrap();
    let row = json!({"id":901,"project_id":f.job.project.id,"issue_number":1,"author":"human:boss","body":"Obsolete fixture note","created_at":1});
    let request = json!({"replica":"accept","node":"peer","changes":[{"seq":1,"table_name":"comments","before_json":null,"after_json":row.to_string(),"created_at":1}]});
    let result = replica(&f.store.db, &request);
    assert_eq!(result[0]["state"], "applied", "{result}");
    assert!(result[0]["suppressed"].is_string());
    assert_eq!(replica(&f.store.db, &request), result);
    assert_eq!(f.issue().version, before);
    assert!(handoff_version_is_current(&f.store, &f.job.project.id));
    assert_eq!(
        f.store
            .db
            .query_row("SELECT coalesce(max(seq),0) FROM fleet_outbox", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        cursor
    );
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
