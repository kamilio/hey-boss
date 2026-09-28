use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};

struct Fixture {
    root: std::path::PathBuf,
    store: Store,
}
impl Fixture {
    fn draft(&mut self) {
        self.run(json!({"action":"configure_project","drafts_enabled":true}));
        self.run(
            json!({"action":"edit","number":1,"draft":true,"add_labels":[],"remove_labels":[]}),
        );
    }
    fn draft_handoff(&mut self) -> Request {
        let mut request = self.handoff();
        let mut op = serde_json::to_value(&request.operation).unwrap();
        op["keep_draft"] = json!(true);
        request.operation = serde_json::from_value(op).unwrap();
        request
    }
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hey-boss-ready-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        let mut f = Self { root, store };
        f.run(json!({"action":"configure_project","prs_enabled":true,"subtask_scheduling":"explicit"}));
        f.run(json!({"action":"create","title":"Source","body":"","labels":[]}));
        f.run(json!({"action":"add_pull_request","number":1,"url":"https://github.com/example/repo/pull/1"}));
        f
    }
    fn request(op: Value) -> Request {
        serde_json::from_value(json!({"version":1,"project":{"id":"named:Ready QA","name":"Ready QA"},"actor":{"id":"codex:owner","kind":"codex","session_id":"owner","machine":"local","host":"local","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},"operation":op})).unwrap()
    }
    fn run(&mut self, op: Value) -> Value {
        self.store.execute(&Self::request(op)).unwrap()
    }
    fn view(&mut self) -> Value {
        self.run(json!({"action":"view","number":1}))
    }
    fn handoff(&mut self) -> Request {
        let guard = self.view()["ready_guard"].clone();
        assert!(
            guard.is_object(),
            "View must expose an atomic Ready snapshot"
        );
        let mut request =
            Self::request(json!({"action":"ready","number":1,"force":true,"guard":guard}));
        request.request_id = Some("ready-once".into());
        request
    }
    fn sql(&self, sql: &str) {
        rusqlite::Connection::open(self.root.join("issues.db"))
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
    fn reject(&mut self, request: &Request, message: &str) {
        let before = self.view();
        let err = self.store.execute(request).unwrap_err();
        assert!(err.message.contains(message), "{}", err.message);
        assert_eq!(
            self.view(),
            before,
            "Rejected handoff must preserve all state"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn ready_guard_handoff_is_atomic_idempotent_and_unblocks_dependents() {
    let mut f = Fixture::new();
    f.run(json!({"action":"create","title":"Dependent","body":"","labels":[]}));
    f.run(json!({"action":"set_blockers","number":2,"blockers":[1],"force":false}));
    f.run(json!({"action":"claim","number":1,"force":false}));
    let request = f.handoff();
    let response = f.store.execute(&request).unwrap();
    assert_eq!(response["issue"]["state"], "ready");
    assert_eq!(response["issue"]["assignee"], "human:boss");
    assert_eq!(response["issue"]["closed_at"], Value::Null);
    assert_eq!(
        f.run(json!({"action":"view","number":2}))["issue"]["state"],
        "open"
    );
    assert_eq!(f.store.execute(&request).unwrap(), response);
    f.run(json!({"action":"reopen","number":1}));
    assert_eq!(f.store.execute(&request).unwrap(), response);
    assert_eq!(
        f.view()["issue"]["state"],
        "open",
        "Replay must not redo a handoff"
    );
    let mut reused = request.clone();
    let mut changed = serde_json::to_value(&request.operation).unwrap();
    changed["force"] = json!(false);
    reused.operation = serde_json::from_value(changed).unwrap();
    f.reject(&reused, "different operation");
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM worker_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn ready_guard_rejects_version_owner_and_reservation_races_even_with_force() {
    for (sql, message) in [
        (
            "UPDATE issues SET version=version+1 WHERE number=1",
            "version",
        ),
        (
            "UPDATE issues SET assignee='codex:owner' WHERE number=1",
            "assignee",
        ),
        (
            "INSERT INTO fleet_allocations VALUES('named:Ready QA',1,'foreign')",
            "reservation",
        ),
        (
            "INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,reservation_expires,job,machine,owner_pid,owner_start) VALUES('new-run','named:Ready QA',1,'codex:foreign','reserved',0,0,9999999999999,'{}','local',1,'test')",
            "reservation",
        ),
    ] {
        let mut f = Fixture::new();
        let request = f.handoff();
        f.sql(sql);
        f.reject(&request, message);
    }
}

#[test]
fn ready_never_releases_foreign_worker_even_with_matching_guard_or_force() {
    let mut f = Fixture::new();
    f.sql("INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,claimed_at,job,machine,owner_pid,owner_start) VALUES('live','named:Ready QA',1,'codex:foreign','running',0,0,1,'{}','local',1,'test')");
    let request = f.handoff();
    f.reject(&request, "worker");
    f.reject(
        &Fixture::request(json!({"action":"ready","number":1,"force":true})),
        "worker",
    );
}

#[test]
fn ready_manual_hold_requires_explicit_guard_and_preserves_dependencies() {
    let mut f = Fixture::new();
    f.run(json!({"action":"block","number":1,"force":false}));
    f.reject(
        &Fixture::request(json!({"action":"ready","number":1,"force":true})),
        "manual hold",
    );
    let mut request = f.handoff();
    let mut op = serde_json::to_value(&request.operation).unwrap();
    op["clear_manual_hold"] = json!(true);
    request.operation = serde_json::from_value(op).unwrap();
    let ready = f.store.execute(&request).unwrap();
    assert_eq!(ready["issue"]["state"], "ready");
    assert_eq!(ready["issue"]["manual_blocked"], false);
    assert_eq!(f.store.execute(&request).unwrap(), ready);

    let mut f = Fixture::new();
    f.run(json!({"action":"create","title":"Unfinished","body":"","labels":[]}));
    f.run(json!({"action":"set_blockers","number":1,"blockers":[2],"force":false}));
    f.run(json!({"action":"block","number":1,"force":false}));
    let mut request = f.handoff();
    let mut op = serde_json::to_value(&request.operation).unwrap();
    op["clear_manual_hold"] = json!(true);
    request.operation = serde_json::from_value(op).unwrap();
    f.reject(&request, "dependencies");
}

#[test]
fn ready_claim_identity_changes_without_issue_version_are_rejected() {
    for change in [
        "UPDATE worker_runs SET id='replacement' WHERE id='mine'",
        "UPDATE worker_runs SET claimed_at=2 WHERE id='mine'",
        "UPDATE fleet_allocation_deadlines SET expires_at=expires_at+1",
    ] {
        let mut f = Fixture::new();
        f.run(json!({"action":"claim","number":1,"force":false}));
        f.sql("INSERT INTO fleet_allocations VALUES('named:Ready QA',1,'local'); INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,claimed_at,job,machine,owner_pid,owner_start) VALUES('mine','named:Ready QA',1,'codex:owner','running',0,0,1,'{}','local',1,'test')");
        let request = f.handoff();
        f.sql(change);
        f.reject(&request, "reservation");
    }
}

#[test]
fn ready_guard_preserves_own_running_attempt_and_unknown_ci() {
    let mut f = Fixture::new();
    f.run(json!({"action":"claim","number":1,"force":false}));
    f.sql("INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,claimed_at,job,machine,owner_pid,owner_start) VALUES('mine','named:Ready QA',1,'codex:owner','running',0,0,1,'{}','local',1,'test')");
    let request = f.handoff();
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    let run = || {
        db.query_row(
            "SELECT state,claimed_at,finished_at,updated_at FROM worker_runs WHERE id='mine'",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap()
    };
    let before = run();
    let response = f.store.execute(&request).unwrap();
    assert_eq!(run(), before);
    assert_eq!(response["issue"]["pull_requests"][0]["status"], "unknown");
}

#[test]
fn ready_supervisor_guards_and_companion_refusal_prevent_offline_replay() {
    let mut f = Fixture::new();
    let request = f.handoff();
    assert_eq!(
        f.store.execute_supervisor(&request).unwrap()["issue"]["state"],
        "ready"
    );
    let mut f = Fixture::new();
    f.sql("UPDATE fleet_meta SET role='agent',node='local'");
    f.reject(
        &Fixture::request(json!({"action":"ready","number":1,"force":true})),
        "supervisor",
    );
}

#[test]
fn ready_guard_authorizes_exact_manual_owner_without_claiming_work() {
    let mut f = Fixture::new();
    f.run(json!({"action":"claim","number":1,"force":false}));
    let mut request = f.handoff();
    request.actor.as_mut().unwrap().id = "codex:reviewer".into();
    let response = f.store.execute_supervisor(&request).unwrap();
    assert_eq!(response["issue"]["state"], "ready");
    let events = f.run(json!({"action":"history","number":1,"limit":100,"offset":0}));
    assert!(
        !events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["actor"] == "codex:reviewer" && e["action"] == "claimed")
    );
}

#[test]
fn labels_preserve_ready_through_reconciliation_but_invalidate_old_guards() {
    let mut f = Fixture::new();
    let request = f.handoff();
    f.store.execute(&request).unwrap();
    let mut stale = f.handoff();
    stale.request_id = Some("before-label-edit".into());
    let before = f.view()["issue"].clone();
    let mut batch = Fixture::request(json!({"action":"batch","edits":[{
        "number":1,"if_version":before["version"],"expected_assignee":"human:boss",
        "add_labels":["rework needed"],"remove_labels":["PR ready"],"assignment":"keep"
    }]}));
    batch.request_id = Some("label-only".into());
    assert_eq!(f.store.execute(&batch).unwrap()["applied"], true);
    // Creating a dependent runs graph reconciliation after the label-only batch.
    f.run(json!({"action":"create","title":"Dependent","body":"","labels":[]}));
    f.run(json!({"action":"set_blockers","number":2,"blockers":[1],"force":false}));
    let after = f.view()["issue"].clone();
    assert_eq!(after["state"], "ready");
    assert_eq!(after["assignee"], "human:boss");
    assert_eq!(after["labels"], json!(["rework needed"]));
    assert_eq!(
        after["version"].as_i64(),
        Some(before["version"].as_i64().unwrap() + 1)
    );
    assert_eq!(
        f.run(json!({"action":"view","number":2}))["issue"]["state"],
        "open"
    );
    f.reject(&stale, "version");
    // A separate lifecycle write, not a label, pauses dependent pickups.
    f.run(json!({"action":"reopen","number":1}));
    assert_eq!(
        f.run(json!({"action":"view","number":2}))["issue"]["state"],
        "blocked"
    );
}

#[test]
fn source_handoff_preserves_separate_repair_claim_and_labels() {
    let mut f = Fixture::new();
    f.run(
        json!({"action":"create","title":"Separate repair","body":"","labels":["rework needed"]}),
    );
    f.run(json!({"action":"create","title":"Dependent","body":"","labels":[]}));
    f.run(json!({"action":"set_blockers","number":3,"blockers":[1],"force":false}));
    let mut claim = Fixture::request(json!({"action":"claim","number":2,"force":false}));
    claim.actor.as_mut().unwrap().id = "codex:repairer".into();
    f.store.execute(&claim).unwrap();
    f.sql("INSERT INTO fleet_allocations VALUES('named:Ready QA',2,'foreign'); INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,claimed_at,job,machine,owner_pid,owner_start) VALUES('repair','named:Ready QA',2,'codex:repairer','running',0,0,1,'{}','foreign',1,'test')");
    let repair = f.run(json!({"action":"view","number":2}));
    let request = f.handoff();
    assert_eq!(
        f.store.execute(&request).unwrap()["issue"]["state"],
        "ready"
    );
    assert_eq!(f.run(json!({"action":"view","number":2})), repair);
    assert_eq!(
        f.run(json!({"action":"view","number":3}))["issue"]["state"],
        "open"
    );
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT state,claimed_at,finished_at FROM worker_runs WHERE id='repair'",
            [],
            |r| Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?
            ))
        )
        .unwrap(),
        ("running".into(), 1, None)
    );
    assert_eq!(
        db.query_row(
            "SELECT node FROM fleet_allocations WHERE issue_number=2",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "foreign"
    );
}

#[test]
fn ready_cli_exposes_guards_and_rejects_incomplete_snapshots() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["issue", "ready", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for flag in [
        "--if-version",
        "--expected-assignee",
        "--expected-reservation",
        "--clear-manual-hold",
        "--keep-draft",
    ] {
        assert!(help.contains(flag), "{help}");
    }
    for args in [
        vec!["--if-version", "1"],
        vec!["--expected-assignee", "unassigned"],
        vec!["--clear-manual-hold"],
        vec!["--keep-draft"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(["issue", "ready", "1"])
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("required"));
    }
}

#[test]
fn draft_handoff_keeps_no_worker_scope_and_unblocks_only_eligible_dependents() {
    let mut f = Fixture::new();
    f.draft();
    for (title, draft) in [("Runnable dependent", false), ("Draft dependent", true)] {
        let n = f.run(json!({"action":"create","title":title,"body":"","labels":[],"draft":draft}))
            ["issue"]["number"]
            .clone();
        f.run(json!({"action":"set_blockers","number":n,"blockers":[1],"force":false}));
    }
    let request = f.draft_handoff();
    let response = f.store.execute_supervisor(&request).unwrap();
    assert_eq!(response["issue"]["state"], "ready");
    assert_eq!(response["issue"]["draft"], true);
    assert_eq!(response["issue"]["assignee"], "human:boss");
    assert_eq!(response["issue"]["closed_at"], Value::Null);
    assert_eq!(response["issue"]["pull_requests"][0]["status"], "unknown");
    assert_eq!(f.store.execute_supervisor(&request).unwrap(), response);
    assert_eq!(
        f.run(json!({"action":"view","number":2}))["issue"]["state"],
        "open"
    );
    assert_eq!(
        f.run(json!({"action":"view","number":3}))["issue"]["draft"],
        true
    );
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    for table in ["worker_runs", "fleet_allocations"] {
        let sql = format!("SELECT count(*) FROM {table} WHERE issue_number IN (1,3)");
        assert_eq!(
            db.query_row(&sql, [], |r| r.get::<_, i64>(0)).unwrap(),
            0,
            "{table}"
        );
    }
    assert_eq!(db.query_row("SELECT count(*) FROM issue_pickup_ready r JOIN issues i USING(project_id,number) WHERE i.draft=0 AND i.number IN (1,3)", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM issue_pickup_ready WHERE number=2",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(db.query_row("SELECT count(*) FROM events WHERE issue_number=1 AND action='ready' AND json_extract(data,'$.kept_draft')=1", [], |r| r.get::<_,i64>(0)).unwrap(), 1);
    f.reject(
        &Fixture::request(json!({"action":"claim","number":1,"force":true})),
        "Undraft",
    );
    f.run(json!({"action":"reopen","number":1}));
    assert_eq!(f.view()["issue"]["draft"], true);
    assert_eq!(
        f.run(json!({"action":"view","number":2}))["issue"]["state"],
        "blocked"
    );
    f.reject(
        &Fixture::request(json!({"action":"claim","number":1,"force":true})),
        "Undraft",
    );
}

#[test]
fn draft_handoff_requires_explicit_guard_and_does_not_change_ordinary_drafts() {
    let mut f = Fixture::new();
    f.draft();
    let ordinary = f.handoff();
    f.reject(&ordinary, "--keep-draft");
    f.reject(
        &Fixture::request(json!({"action":"ready","number":1,"force":true,"keep_draft":true})),
        "guards",
    );
    f.run(json!({"action":"undraft","number":1}));
    assert_eq!(f.view()["issue"]["state"], "open");
    assert_eq!(f.view()["issue"]["draft"], false);
    let explicit = f.draft_handoff();
    f.reject(&explicit, "requires a draft");
}

#[test]
fn draft_handoff_rejects_stale_version_claim_and_reservation_atomically() {
    for (sql, message) in [
        (
            "UPDATE issues SET version=version+1 WHERE number=1",
            "version",
        ),
        (
            "UPDATE issues SET assignee='codex:new-owner' WHERE number=1",
            "assignee",
        ),
        (
            "INSERT INTO fleet_allocations VALUES('named:Ready QA',1,'local')",
            "reservation",
        ),
        (
            "INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,claimed_at,job,machine,owner_pid,owner_start) VALUES('new-run','named:Ready QA',1,'codex:owner','running',0,0,1,'{}','local',1,'test')",
            "reservation",
        ),
    ] {
        let mut f = Fixture::new();
        f.draft();
        let request = f.draft_handoff();
        f.sql(sql);
        f.reject(&request, message);
    }
    let mut f = Fixture::new();
    f.draft();
    f.sql("INSERT INTO fleet_allocations VALUES('named:Ready QA',1,'local')");
    let request = f.draft_handoff();
    f.reject(&request, "unreserved");
}

#[test]
fn draft_handoff_preserves_pr_and_dependency_requirements() {
    for (change, message) in [
        ("DELETE FROM issue_pull_requests", "Attach the task's PR"),
        (
            "UPDATE project_settings SET prs_enabled=0",
            "pull requests enabled",
        ),
        (
            "UPDATE issues SET manual_blocked=1,state='blocked' WHERE number=1",
            "manual hold",
        ),
    ] {
        let mut f = Fixture::new();
        f.draft();
        f.sql(change);
        let request = f.draft_handoff();
        f.reject(&request, message);
    }
    let mut f = Fixture::new();
    f.draft();
    f.run(json!({"action":"create","title":"Unfinished repair","body":"","labels":[]}));
    f.run(json!({"action":"set_blockers","number":1,"blockers":[2],"force":false}));
    let request = f.draft_handoff();
    f.reject(&request, "unfinished dependencies");
}
