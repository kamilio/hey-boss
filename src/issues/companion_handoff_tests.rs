fn companion_reviewed_evidence() -> Vec<crate::issues::ReviewedGithubEvidence> {
    serde_json::from_value(json!([{
        "report": {
            "data": {"repository":"example/repo","number":1,
                "pull_request":{"number":1,"state":"open","head":{"sha":"head"},"base":{"ref":"main","sha":"base","repo":{"full_name":"example/repo"}},"user":{"login":"author"}},
                "conflicts":"clean","comments":[],"review_comments":[],"reviews":[{"id":1,"state":"COMMENTED","body":"No findings","user":{"login":"reviewer"}}],"timeline":[],"review_events":[],"review_threads":[],
                "review_status":{"requested_reviewers":[{"login":"reviewer"}],"requested_teams":[],"latest_reviews":[],"approved_by":[],"changes_requested_by":[],"dismissed_reviews":[],"resolved_threads":0,"unresolved_threads":0,"outdated_threads":0},
                "ci":{"head_sha":"head","merge_sha":null,"check_runs":[],"commit_statuses":[],"workflow_runs":[],"jobs":[],"summary":{"state":"pending","successful":0,"failed":0,"pending":1,"skipped":0,"unknown":0},"failures":[],"errors":[]},"errors":[]},
            "complete":true,"observed_at_ms":100,"oldest_validation_at_ms":100,"validations":[]
        },
        "policy":{"repository":"example/repo","pull_number":1,"head_sha":"head","base_branch":"main","base_sha":"base","pr_base_sha":"base","state":"not_required","strict":false,"up_to_date":true,"checks":[],"rules":[],"errors":[],"cursor":"unused","pull_request_state":"open"}
    }])).unwrap()
}

#[test]
fn companion_ready_handoff_survives_completion_and_rearms_for_new_evidence() {
    for reviewed in [false, true] {
        for late in [false, true] {
            let mut f = HandoffFixture::new(true);
            f.apply(Operation::Assign {
                reviewed_evidence: None,
                number: 1,
                target: "github".into(),
                if_version: f.issue().version,
            });
            f.job.actor.machine = "peer".into();
            f.job.machine = "peer".into();
            f.store
                .db
                .execute("DELETE FROM issue_pull_requests WHERE url LIKE '%/2'", [])
                .unwrap();
            let evidence = companion_reviewed_evidence();
            let observation = hey_gh::watcher::observe(&evidence[0].report, &evidence[0].policy);
            let mut previous = companion_reviewed_evidence();
            previous[0].policy.head_sha = "previous-head".into();
            f.store
                .record_github_observation(
                    "https://github.com/example/repo/pull/1",
                    &hey_gh::watcher::observe_required(&previous[0].policy),
                    50,
                )
                .unwrap();
            // Both paths start with a real watcher event in the launch snapshot.
            f.store
                .record_github_observation(
                    "https://github.com/example/repo/pull/1",
                    &observation,
                    100,
                )
                .unwrap();
            f.job.issue =
                super::super::subtasks::worker_issue(&f.store.db, &f.job.project.id, 1).unwrap();
            assert!(f.job.issue["github_status"]["event"].is_string());
            // Reviewed evidence can also acknowledge an event absent at launch.
            if reviewed {
                f.job.issue.as_object_mut().unwrap().remove("github_status");
            }
            edit_handoff_notes(&mut f);
            f.apply(
                serde_json::from_value(
                    json!({"action":"create","title":"Dependent","body":"","labels":[]}),
                )
                .unwrap(),
            );
            f.apply(
                serde_json::from_value(json!({"action":"set_blockers","number":2,"blockers":[1]}))
                    .unwrap(),
            );
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
            pull(&f.store, &peer, json!([]));
            peer.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,claimed_at) VALUES(?1,?2,1,?3,?4,'running',1,'start','peer',0,0,0)", params![f.job.id,f.job.project.id,serde_json::to_string(&f.job).unwrap(),f.job.actor.id]).unwrap();
            f.store
                .db
                .execute("UPDATE worker_runs SET finished_at=1", [])
                .unwrap();
            f.store.db.execute("INSERT OR REPLACE INTO fleet_state VALUES('machines',?1)", [json!([{"workers":[{"runs":[{"id":f.job.id,"project_id":f.job.project.id,"number":1,"actor_id":f.job.actor.id,"finished_at":null}]}]}]).to_string()]).unwrap();
            requirements_ready(&mut f, true).unwrap();
            f.apply(Operation::Assign {
                reviewed_evidence: reviewed.then_some(evidence),
                number: 1,
                target: "github".into(),
                if_version: f.issue().version,
            });
            pull(&f.store, &peer, json!([]));
            // Production companions can lack an older generated watcher note.
            // The exact GitHub event is present and acknowledged independently.
            assert_eq!(peer.db.execute("DELETE FROM comments WHERE author='watcher:github' AND id=(SELECT min(id) FROM comments WHERE author='watcher:github')", []).unwrap(), 1);
            peer.db.execute("DELETE FROM fleet_outbox", []).unwrap();
            event(
                &peer.db,
                &f.job.project.id,
                1,
                "watcher:github",
                "commented",
                100,
                &json!({"body":"Older watcher note arriving after the handoff"}),
            )
            .unwrap();
            peer.db.execute("DELETE FROM fleet_outbox", []).unwrap();
            assert!(handoff_version_is_current(&peer, &f.job.project.id));
            assert_eq!(
                peer.db
                    .query_row("SELECT count(*) FROM agent_steering", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0,
                "Supervisor receipts are not replicated"
            );
            if late {
                watch_event(&mut f, "late-review");
            }
            peer = Store::open(&f.root.join("peer.db")).unwrap();
            peer.worker_finish(&f.job, "completed", "Pending external review")
                .unwrap();
            assert_eq!(
                get_issue(&peer.db, &f.job.project.id, 1, true)
                    .unwrap()
                    .assignee
                    .as_deref(),
                Some("watcher:github"),
                "reviewed={reviewed}"
            );
            let changes: Vec<Value> = peer.db.prepare("SELECT seq,table_name,before_json,after_json,created_at FROM fleet_outbox ORDER BY seq").unwrap().query_map([], |r| Ok(json!({"seq":r.get::<_,i64>(0)?,"table_name":r.get::<_,String>(1)?,"before_json":r.get::<_,Option<String>>(2)?,"after_json":r.get::<_,Option<String>>(3)?,"created_at":r.get::<_,i64>(4)?}))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            let receipts = replica(
                &f.store.db,
                &json!({"replica":"accept","node":"peer","changes":changes}),
            );
            assert!(
                receipts
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|r| r["state"] == "applied"),
                "{receipts}"
            );
            pull(&f.store, &peer, receipts);
            assert_eq!(f.issue().state, if late { "open" } else { "ready" });
            assert_eq!(
                f.issue().assignee.as_deref(),
                if late { None } else { Some("watcher:github") }
            );
            assert_eq!(
                get_issue(&f.store.db, &f.job.project.id, 2, true)
                    .unwrap()
                    .state,
                if late { "blocked" } else { "open" }
            );
            assert_eq!(f.store.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE project_id=?1 AND number=1)", [&f.job.project.id], |r|r.get::<_,bool>(0)).unwrap(), late);
            if !late {
                f.store
                    .record_github_observation(
                        "https://github.com/example/repo/pull/1",
                        &observation,
                        200,
                    )
                    .unwrap();
                assert_eq!(f.issue().state, "ready");
                watch_event(&mut f, "new-review");
                assert_eq!(f.issue().state, "open");
                assert!(f.issue().assignee.is_none());
            }
        }
    }
}
