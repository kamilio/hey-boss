use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    store: Store,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("out")
            .join(format!("sequence-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        Self { root, store }
    }
    fn request(op: Value) -> Request {
        serde_json::from_value(json!({"version":1,"project":{"id":"named:Sequence","name":"Sequence"},"actor":{"id":"codex:test","kind":"codex","session_id":"test","machine":"test","host":"test","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},"operation":op})).unwrap()
    }
    fn run(&mut self, op: Value) -> Value {
        self.store.execute(&Self::request(op)).unwrap()
    }
    fn create(&mut self, parent: Option<i64>) -> i64 {
        let op = match parent {
            Some(n) => {
                json!({"action":"create_subtask","number":n,"title":"Step","body":"Requirements","labels":[]})
            }
            None => {
                json!({"action":"create","title":"Feature","body":"Parent requirements","labels":[]})
            }
        };
        self.run(op)["issue"]["number"].as_i64().unwrap()
    }
    fn view(&mut self, n: i64) -> Value {
        self.run(json!({"action":"view","number":n}))["issue"].clone()
    }
    fn ready(&self) -> Vec<i64> {
        let db = rusqlite::Connection::open(self.root.join("issues.db")).unwrap();
        db.prepare("SELECT number FROM issue_pickup_ready ORDER BY number")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
    fn close(&mut self, n: i64) {
        self.run(json!({"action":"close","number":n,"force":false}));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn explicit_dependencies_keep_nested_groups_without_sibling_blockers() {
    let mut f = Fixture::new("explicit-nested");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    f.create(Some(2));
    f.create(Some(3));
    assert_eq!(f.ready(), vec![4]);
    f.run(json!({"action":"configure_project","subtask_scheduling":"explicit"}));
    assert_eq!(f.ready(), vec![4, 5]);
    let fifth = f.view(5);
    assert_eq!(fifth["parent"]["number"], 3);
    assert_eq!(fifth["subtask_context"]["scheduling"], "explicit");
    assert_eq!(fifth["blocked_by"], json!([]));
    assert_eq!(fifth["dependency_context"], json!([]));
    assert_eq!(f.view(1)["state"], "blocked");
    f.run(json!({"action":"set_blockers","number":5,"blockers":[4],"force":false}));
    assert_eq!(f.ready(), vec![4]);
    assert_eq!(f.view(5)["blocked_by"][0]["source"], "linked");
    f.close(4);
    assert_eq!(f.ready(), vec![2, 5]);
    let claim = f.run(json!({"action":"claim","number":5,"force":false}));
    let instructions = claim["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("explicit dependencies"),
        "{instructions}"
    );
    assert!(
        !instructions.contains("Later subtasks wait"),
        "{instructions}"
    );
}

#[test]
fn explicit_ready_handoff_and_rework_preserve_running_claims() {
    let mut f = Fixture::new("explicit-ready");
    f.run(json!({"action":"configure_project","subtask_scheduling":"explicit","prs_enabled":true}));
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    f.create(Some(1));
    f.run(json!({"action":"set_blockers","number":4,"blockers":[2],"force":false}));
    f.run(json!({"action":"add_pull_request","number":2,"url":"https://github.com/example/repo/pull/2"}));
    f.run(json!({"action":"ready","number":2,"force":false}));
    assert_eq!(f.ready(), vec![3, 4]);
    let claim = f.run(json!({"action":"claim","number":4,"force":false}));
    assert_eq!(claim["issue"]["dependency_context"][0]["state"], "ready");
    f.run(json!({"action":"reopen","number":2}));
    assert_eq!(f.view(4)["assignee"], "codex:test");
    assert_eq!(f.view(4)["blocked_by"][0]["number"], 2);
}

#[test]
fn explicit_mode_survives_reopen_and_keeps_hierarchy_claim_guards() {
    let mut f = Fixture::new("explicit-restart");
    f.run(json!({"action":"configure_project","subtask_scheduling":"explicit"}));
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    f.store = Store::open(&f.root.join("issues.db")).unwrap();
    assert_eq!(f.ready(), vec![2, 3]);
    f.run(json!({"action":"claim","number":3,"force":false}));
    let before = f.view(3);
    let rejected = f.store.execute(&Fixture::request(json!({"action":"create_subtask","number":3,"title":"Unsafe child","body":"","labels":[]}))).unwrap_err();
    assert_eq!(rejected.code, "subtask_claim_conflict");
    assert_eq!(f.view(3), before);
}

#[test]
fn linked_and_descendant_reopen_errors_include_structured_sources() {
    let mut f = Fixture::new("explicit-errors");
    f.run(json!({"action":"configure_project","subtask_scheduling":"explicit"}));
    f.create(None);
    f.create(Some(1));
    f.create(None);
    f.run(json!({"action":"set_blockers","number":3,"blockers":[2],"force":false}));
    for (number, source) in [(1, "subtask"), (3, "linked")] {
        let error = f
            .store
            .execute(&Fixture::request(
                json!({"action":"reopen","number":number}),
            ))
            .unwrap_err();
        assert!(error.message.contains(&format!("#2 ({source})")), "{error}");
        assert_eq!(error.details.unwrap()["blocked_by"][0]["source"], source);
    }
}

#[test]
fn invalid_scheduling_is_rejected_and_closed_hold_cannot_bypass_dependencies() {
    let mut f = Fixture::new("explicit-invalid");
    assert!(
        f.store
            .execute(&Fixture::request(
                json!({"action":"configure_project","subtask_scheduling":"parallel-ish"})
            ))
            .is_err()
    );
    f.create(None);
    f.create(Some(1));
    f.close(1);
    let before = f.view(1);
    assert!(
        f.store
            .execute(&Fixture::request(
                json!({"action":"reopen","number":1,"clear_manual_hold":true})
            ))
            .is_err()
    );
    assert_eq!(f.view(1), before);
}

#[test]
fn explicit_pickup_ignores_siblings_but_checks_links_with_indexed_queries() {
    let mut f = Fixture::new("explicit-pickup");
    f.run(json!({"action":"configure_project","subtask_scheduling":"explicit"}));
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    db.execute_batch("WITH RECURSIVE n(x) AS (SELECT 100 UNION ALL SELECT x+1 FROM n WHERE x<2100) INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) SELECT 'named:Sequence',x,'Unrelated','','open','codex:test',0,0,1,'[]',x FROM n;").unwrap();
    let mut stmt = db
        .prepare(
            "SELECT number FROM issue_pickup_ready WHERE project_id='named:Sequence' AND number=3",
        )
        .unwrap();
    assert!(stmt.exists([]).unwrap());
    assert!(stmt.get_status(rusqlite::StatementStatus::VmStep) < 1000);
    // Replicated rows can arrive before state reconciliation. SQL must still gate pickup.
    db.execute("UPDATE issues SET blockers='[2]' WHERE number=3", [])
        .unwrap();
    assert!(!stmt.exists([]).unwrap());
    assert_eq!(f.view(3)["blocked_by"][0]["source"], "linked");
}

#[test]
fn scheduling_changes_preserve_claims_and_reject_cycles_atomically() {
    let mut f = Fixture::new("explicit-guards");
    f.run(json!({"action":"configure_project","subtask_scheduling":"explicit"}));
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    f.run(json!({"action":"claim","number":3,"force":false}));
    let before = f.view(3);
    assert!(
        f.store
            .execute(&Fixture::request(
                json!({"action":"configure_project","subtask_scheduling":"sequential"})
            ))
            .is_err()
    );
    assert_eq!(f.view(3), before);
    assert_eq!(
        f.run(json!({"action":"project_settings"}))["subtask_scheduling"],
        "explicit"
    );
    f.run(json!({"action":"unassign","number":3,"force":false}));
    f.run(json!({"action":"set_blockers","number":2,"blockers":[3],"force":false}));
    assert!(
        f.store
            .execute(&Fixture::request(
                json!({"action":"configure_project","subtask_scheduling":"sequential"})
            ))
            .is_err()
    );
    assert_eq!(f.ready(), vec![3]);
}

#[test]
fn clear_manual_hold_keeps_dependencies_and_checks_version_first() {
    let mut f = Fixture::new("explicit-hold");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    f.run(json!({"action":"block","number":3,"comment":null,"force":false}));
    let before = f.view(3);
    let stale = f
        .store
        .execute(&Fixture::request(
            json!({"action":"reopen","number":3,"if_version":0,"clear_manual_hold":true}),
        ))
        .unwrap_err();
    assert!(stale.message.contains("version"), "{stale}");
    assert_eq!(f.view(3), before);
    let blocked = f
        .store
        .execute(&Fixture::request(json!({"action":"reopen","number":3})))
        .unwrap_err();
    assert!(
        blocked.message.contains("#2 (previous_subtask)"),
        "{blocked}"
    );
    assert!(blocked.message.contains("--clear-manual-hold"), "{blocked}");
    let cleared = f.run(json!({"action":"reopen","number":3,"if_version":before["version"],"clear_manual_hold":true}));
    assert_eq!(cleared["issue"]["state"], "blocked");
    assert_eq!(cleared["issue"]["manual_blocked"], false);
    assert_eq!(cleared["issue"]["blocked_by"], before["blocked_by"]);
    assert_eq!(f.ready(), vec![2]);
    let repeated = f.run(json!({"action":"reopen","number":3,"clear_manual_hold":true}));
    assert_eq!(repeated["changed"], false);
    f.close(2);
    assert_eq!(f.view(3)["state"], "open");
    assert_eq!(f.ready(), vec![3]);
}
