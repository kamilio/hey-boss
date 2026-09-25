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
fn siblings_wait_in_queue_order_and_claims_explain_the_sequence() {
    let mut f = Fixture::new("siblings");
    f.create(None);
    for _ in 0..3 {
        f.create(Some(1));
    }
    assert_eq!(f.ready(), vec![2]);
    assert_eq!(f.view(3)["state"], "blocked");
    assert_eq!(f.view(3)["blocked_by"][0]["number"], 2);
    assert!(
        f.store
            .execute(&Fixture::request(
                json!({"action":"claim","number":3,"force":false})
            ))
            .is_err()
    );
    f.close(2);
    assert_eq!(f.ready(), vec![3]);
    let claimed = f.run(json!({"action":"claim","number":3,"force":false}));
    assert!(
        claimed["instructions"]
            .as_str()
            .unwrap()
            .contains("Previous: #2 Step [closed]")
    );
    let context = &claimed["issue"]["subtask_context"];
    assert_eq!(context["position"], 2);
    assert_eq!(context["total"], 3);
    assert_eq!(context["parent"]["number"], 1);
    assert_eq!(context["previous"]["number"], 2);
    assert_eq!(context["previous"]["state"], "closed");
    assert_eq!(context["next"]["number"], 4);
    f.close(3);
    assert_eq!(f.ready(), vec![4]);
    f.close(4);
    assert_eq!(f.ready(), vec![1]);
    f.run(json!({"action":"reopen","number":4}));
    f.run(json!({"action":"reopen","number":2}));
    assert_eq!(f.ready(), vec![2]);
}

#[test]
fn nested_branches_cannot_overtake_even_a_closed_predecessor() {
    let mut f = Fixture::new("nested");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    f.create(Some(2));
    f.create(Some(3));
    assert_eq!(f.ready(), vec![4]);
    assert_eq!(f.view(5)["state"], "blocked");
    f.close(2);
    assert_eq!(f.ready(), vec![4]);
    f.close(4);
    assert_eq!(f.ready(), vec![5]);
    f.close(5);
    assert_eq!(f.ready(), vec![3]);
}

#[test]
fn reorder_delete_restore_and_unlink_recompute_dependencies() {
    let mut f = Fixture::new("edits");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    f.run(json!({"action":"move","number":3,"before":2}));
    assert_eq!(f.ready(), vec![3]);
    assert_eq!(f.view(2)["state"], "blocked");
    f.run(json!({"action":"delete","number":3,"force":false}));
    assert_eq!(f.ready(), vec![2]);
    f.run(json!({"action":"restore","number":3}));
    assert_eq!(f.ready(), vec![3]);
    f.run(json!({"action":"remove_subtask","number":1,"child":3}));
    assert_eq!(f.ready(), vec![2, 3]);
}

#[test]
fn sequence_edges_reject_cycles_and_reorders_cannot_release_claims() {
    let mut f = Fixture::new("guards");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    assert!(
        f.store
            .execute(&Fixture::request(
                json!({"action":"set_blockers","number":2,"blockers":[3],"force":false})
            ))
            .is_err()
    );
    f.run(json!({"action":"claim","number":2,"force":false}));
    assert!(
        f.store
            .execute(&Fixture::request(
                json!({"action":"move","number":3,"before":2})
            ))
            .is_err()
    );
    assert_eq!(f.view(2)["assignee"], "codex:test");
    assert_eq!(f.view(2)["subtask_context"]["position"], 1);
}

#[test]
fn worker_preview_includes_parent_previous_and_next_with_read_commands() {
    let mut f = Fixture::new("prompt");
    f.create(None);
    for _ in 0..3 {
        f.create(Some(1));
    }
    f.close(2);
    let preview = f.run(json!({"action":"worker_preview","number":3,"config":{"cwd":"/tmp"}}));
    let prompt = preview["prompt"].as_str().unwrap();
    assert!(prompt.contains("Subtask 2 of 3"), "{prompt}");
    assert!(prompt.contains("Parent: #1 Feature"), "{prompt}");
    assert!(prompt.contains("Previous: #2 Step [closed]"), "{prompt}");
    assert!(prompt.contains("Next: #4 Step [blocked]"), "{prompt}");
    assert!(
        prompt.contains("hey-boss issue view 2 --project"),
        "{prompt}"
    );
}

#[test]
fn startup_migrates_existing_sequences_and_sql_guards_unreconciled_replicas() {
    let mut f = Fixture::new("migration");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    {
        let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
        db.execute_batch("DROP VIEW issue_pickup_ready; CREATE VIEW issue_pickup_ready AS SELECT project_id,number FROM issues WHERE state='open'; UPDATE issues SET state='open' WHERE number=3;").unwrap();
    }
    f.store = Store::open(&f.root.join("issues.db")).unwrap();
    assert_eq!(f.view(3)["state"], "blocked");
    assert_eq!(f.ready(), vec![2]);
    {
        let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
        db.execute_batch("UPDATE issues SET state='open' WHERE number=3;")
            .unwrap();
    }
    assert_eq!(f.ready(), vec![2]);
}

#[test]
fn readiness_uses_project_indexes_and_independent_trees_can_run_together() {
    let mut f = Fixture::new("indexed");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    f.create(None);
    f.create(Some(4));
    f.create(Some(4));
    assert_eq!(f.ready(), vec![2, 5]);
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    db.execute_batch("WITH RECURSIVE n(x) AS (SELECT 100 UNION ALL SELECT x+1 FROM n WHERE x<2100) INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) SELECT 'named:Sequence',x,'Unrelated','','open','codex:test',0,0,1,'[]',x FROM n;").unwrap();
    let mut stmt = db
        .prepare(
            "SELECT number FROM issue_pickup_ready WHERE project_id='named:Sequence' AND number=3",
        )
        .unwrap();
    assert!(!stmt.exists([]).unwrap());
    let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
    assert!(
        steps < 2000,
        "Readiness scanned unrelated issues: {steps} VM steps"
    );
}

#[test]
fn migration_preserves_active_claims_and_retries_failed_graph_validation() {
    let mut f = Fixture::new("upgrade-claims");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    {
        let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
        db.execute_batch("DROP VIEW issue_pickup_ready; CREATE VIEW issue_pickup_ready AS SELECT project_id,number FROM issues WHERE state='open'; UPDATE issues SET state='open',assignee='codex:test' WHERE number=3; UPDATE issues SET blockers='[3]' WHERE number=2;").unwrap();
    }
    assert!(Store::open(&f.root.join("issues.db")).is_err());
    {
        let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
        let sql: String = db
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='issue_pickup_ready'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !sql.contains("sequence_ancestors"),
            "Failed migration must remain retryable"
        );
        db.execute_batch("UPDATE issues SET blockers='[]' WHERE number=2;")
            .unwrap();
    }
    f.store = Store::open(&f.root.join("issues.db")).unwrap();
    assert_eq!(f.view(3)["assignee"], "codex:test");
    f.run(json!({"action":"unassign","number":3,"force":false}));
    assert_eq!(f.view(3)["state"], "blocked");
    f.close(2);
    f.close(3);
    assert_eq!(f.ready(), vec![1]);
}

fn enable_prs(f: &mut Fixture) {
    f.run(json!({"action":"configure_project","prs_enabled":true}));
}
fn attach_pr(f: &mut Fixture, number: i64) {
    f.run(json!({"action":"add_pull_request","number":number,"url":format!("https://github.com/o/r/pull/{number}"),"purpose":"fix"}));
}
#[test]
fn workers_mark_ready_and_pr_projects_unblock_without_waiting_for_merge() {
    let mut f = Fixture::new("ready");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    enable_prs(&mut f);
    attach_pr(&mut f, 2);
    f.run(json!({"action":"claim","number":2,"force":false}));
    f.run(json!({"action":"ready","number":2,"force":false}));
    assert_eq!(f.view(2)["state"], "ready");
    assert_eq!(f.view(2)["assignee"], "human:boss");
    assert_eq!(f.ready(), vec![3]);
    assert_eq!(f.run(json!({"action":"list","state":"ready","all":true,"limit":100,"offset":0,"mine":false,"unassigned":false,"labels":[]}))["issues"][0]["number"],2);
    let claimed = f.run(json!({"action":"claim","number":3,"force":false}));
    assert!(claimed["instructions"].as_str().unwrap().contains("stack"));
    assert_eq!(
        claimed["issue"]["dependency_context"][0]["pull_requests"][0]["url"],
        "https://github.com/o/r/pull/2"
    );
}
#[test]
fn reopening_ready_dependencies_blocks_future_pickup_but_preserves_running_claims() {
    let mut f = Fixture::new("ready-rework");
    f.create(None);
    f.create(None);
    enable_prs(&mut f);
    f.run(json!({"action":"set_blockers","number":2,"blockers":[1],"force":false}));
    attach_pr(&mut f, 1);
    f.run(json!({"action":"ready","number":1,"force":false}));
    assert_eq!(f.ready(), vec![2]);
    f.run(json!({"action":"reopen","number":1}));
    assert_eq!(f.view(2)["state"], "blocked");
    f.run(json!({"action":"ready","number":1,"force":false}));
    f.run(json!({"action":"claim","number":2,"force":false}));
    f.run(json!({"action":"reopen","number":1}));
    assert_eq!(f.view(2)["assignee"], "codex:test");
    assert_eq!(f.view(2)["state"], "open");
    assert_eq!(f.view(2)["blocked_by"][0]["number"], 1);
    let detail = f.run(json!({"action":"view","number":2}));
    assert!(
        detail["comments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["body"].as_str().unwrap().contains("rework"))
    );
}
#[test]
fn ready_requires_pr_project_and_pr_attachment_and_is_not_a_worker_pickup_state() {
    let mut f = Fixture::new("ready-guards");
    f.create(None);
    assert!(
        f.store
            .execute(&Fixture::request(
                json!({"action":"ready","number":1,"force":false})
            ))
            .is_err()
    );
    enable_prs(&mut f);
    assert!(
        f.store
            .execute(&Fixture::request(
                json!({"action":"ready","number":1,"force":false})
            ))
            .is_err()
    );
    attach_pr(&mut f, 1);
    f.run(json!({"action":"ready","number":1,"force":false}));
    assert!(f.ready().is_empty());
    f.run(json!({"action":"close","number":1,"force":true}));
    assert_eq!(f.view(1)["state"], "closed");
}

#[test]
fn ready_chain_rework_cascades_and_settings_control_completion() {
    let mut f = Fixture::new("ready-chain");
    for _ in 0..4 {
        f.create(None);
    }
    enable_prs(&mut f);
    for n in 2..=4 {
        f.run(json!({"action":"set_blockers","number":n,"blockers":[n-1],"force":false}));
    }
    for n in 1..=3 {
        attach_pr(&mut f, n);
        f.run(json!({"action":"ready","number":n,"force":false}));
    }
    assert_eq!(f.ready(), vec![4]);
    f.run(json!({"action":"reopen","number":1}));
    for n in 2..=4 {
        assert_eq!(f.view(n)["state"], "blocked");
    }
    f.run(json!({"action":"ready","number":1,"force":false}));
    assert_eq!(f.ready(), vec![2]);
    f.run(json!({"action":"configure_project","prs_enabled":false}));
    assert!(f.ready().is_empty());
    f.run(json!({"action":"close","number":1,"force":true}));
    assert_eq!(f.ready(), vec![2]);
}

#[test]
fn upstream_rework_notice_survives_reconciliation_without_releasing_claim() {
    let mut f = Fixture::new("ready-notices");
    f.create(None);
    f.create(None);
    enable_prs(&mut f);
    f.run(json!({"action":"set_blockers","number":2,"blockers":[1],"force":false}));
    attach_pr(&mut f, 1);
    f.run(json!({"action":"ready","number":1,"force":false}));
    f.run(json!({"action":"claim","number":2,"force":false}));
    f.run(json!({"action":"reopen","number":1}));
    f.create(None);
    f.create(None);
    assert_eq!(f.view(2)["assignee"], "codex:test");
    let detail = f.run(json!({"action":"view","number":2}));
    assert_eq!(
        detail["comments"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["body"].as_str().unwrap().contains("Dependency rework"))
            .count(),
        1
    );
    f.run(json!({"action":"unassign","number":2,"force":false}));
    assert_eq!(f.view(2)["state"], "blocked");
}

#[test]
fn ready_upgrade_preserves_v14_relationships_comments_and_triggers() {
    let mut f = Fixture::new("ready-migration");
    f.create(None);
    f.create(Some(1));
    f.create(Some(1));
    enable_prs(&mut f);
    attach_pr(&mut f, 2);
    f.run(json!({"action":"comment","number":2,"body":"Preserve handoff"}));
    {
        let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
        // Recreate the previous release's constraint without changing its data.
        db.execute_batch("PRAGMA writable_schema=ON;
            UPDATE sqlite_master SET sql=replace(replace(sql,'''open'',''blocked'',''ready'',''closed''','''open'',''blocked'',''closed'''),'state IN (''open'',''ready'') OR assignee IS NULL','state=''open'' OR assignee IS NULL') WHERE name='issues';
            PRAGMA writable_schema=OFF; PRAGMA user_version=14;
            CREATE TABLE ready_migration_audit(number INTEGER);
            CREATE TRIGGER ready_migration_audit_insert AFTER INSERT ON issues BEGIN INSERT INTO ready_migration_audit VALUES(NEW.number); END;").unwrap();
    }
    f.store = Store::open(&f.root.join("issues.db")).unwrap();
    f.run(json!({"action":"ready","number":2,"force":false}));
    assert_eq!(f.ready(), vec![3]);
    assert_eq!(f.view(2)["parent"]["number"], 1);
    assert_eq!(
        f.run(json!({"action":"view","number":2}))["comments"][0]["body"],
        "Preserve handoff"
    );
    f.create(None);
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM ready_migration_audit", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
}
