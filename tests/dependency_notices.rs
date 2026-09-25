use hey_boss::issues::{Request, Store};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

struct Fixture {
    root: std::path::PathBuf,
    store: Store,
    db: Connection,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-dependency-notices-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        let db = Connection::open(root.join("issues.db")).unwrap();
        let mut f = Self { root, store, db };
        f.run(json!({"action":"create","title":"Connector integration","body":"","labels":[]}));
        for title in ["Contract", "Independent settings", "OAuth"] {
            f.run(
                json!({"action":"create_subtask","number":1,"title":title,"body":"","labels":[]}),
            );
        }
        f
    }
    fn run(&mut self, operation: Value) -> Value {
        let request: Request = serde_json::from_value(json!({"version":1,
            "project":{"id":"named:Notices","name":"Notices"},
            "actor":{"id":"codex:notice-test","kind":"codex","session_id":"notice-test","machine":"test","host":"test","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},
            "operation":operation})).unwrap();
        self.store.execute(&request).unwrap()
    }
    fn explicit(&mut self) {
        self.run(json!({"action":"configure_project","subtask_scheduling":"explicit","prs_enabled":true}));
    }
    // Execute the pre-explicit-scheduling binary's exact write sequence. In
    // particular it does not check whether the comment INSERT was ignored.
    fn legacy_notice(&self, number: i64, dependencies: &[i64]) {
        let body = format!(
            "Dependency rework: upstream tasks {dependencies:?} need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies."
        );
        self.db.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Notices',?1,'codex:notice-test',?2,123)", params![number,body]).unwrap();
        let id = self.db.last_insert_rowid();
        self.db.execute("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Notices',?1,'codex:notice-test','commented',123,?2)", params![number,json!({"comment_id":id,"body":body}).to_string()]).unwrap();
        self.db.execute("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Notices',?1,'codex:notice-test','dependency_rework',123,?2)", params![number,json!({"dependencies":dependencies.iter().map(|n|json!([n,1])).collect::<Vec<_>>()} ).to_string()]).unwrap();
    }
    fn notices(&self, table: &str, number: i64) -> i64 {
        let predicate = if table == "comments" {
            "body LIKE 'Dependency rework:%'"
        } else {
            "action IN ('dependency_rework','commented') AND created_at=123"
        };
        self.db.query_row(&format!("SELECT count(*) FROM {table} WHERE project_id='named:Notices' AND issue_number=?1 AND {predicate}"), [number], |r|r.get(0)).unwrap()
    }
    fn reserve(&self, number: i64) {
        self.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at) VALUES(?1,'named:Notices',?2,'{}','codex:notice-test','running',1,'start','test',0,0)",params![format!("run-{number}"),number]).unwrap();
    }
    fn queue(&self, request: &str, number: i64, dependencies: &[i64]) {
        let body = format!(
            "Dependency rework: upstream tasks {dependencies:?} need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies."
        );
        self.db.execute("INSERT INTO agent_steering(request_id,run_id,scope,text,created_at) VALUES(?1,?2,'dependency',?3,123)", params![request,format!("run-{number}"),body]).unwrap();
    }
    fn queue_state(&self, request: &str) -> String {
        self.db
            .query_row(
                "SELECT state FROM agent_steering WHERE request_id=?1",
                [request],
                |r| r.get(0),
            )
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn legacy_local_clients_cannot_publish_former_sibling_notices() {
    let mut f = Fixture::new();
    f.explicit();
    f.run(json!({"action":"claim","number":3,"force":false}));
    f.run(json!({"action":"comment","number":3,"body":"Keep this real comment"}));
    let before = f.run(json!({"action":"view","number":3}))["issue"].clone();
    f.legacy_notice(3, &[2]);
    assert_eq!(f.notices("comments", 3), 0);
    assert_eq!(
        f.notices("events", 3),
        0,
        "Ignored INSERT must not reuse an unrelated last_insert_rowid"
    );
    let after = f.run(json!({"action":"view","number":3}))["issue"].clone();
    assert_eq!(
        before, after,
        "Notice admission must not mutate claims, versions or grouping"
    );
}

#[test]
fn declared_and_parent_completion_notices_remain_valid() {
    let mut f = Fixture::new();
    f.explicit();
    f.run(json!({"action":"set_blockers","number":4,"blockers":[2],"force":false}));
    f.legacy_notice(4, &[2]);
    f.legacy_notice(1, &[2, 3, 4]);
    assert_eq!(f.notices("comments", 4), 1);
    assert_eq!(f.notices("events", 4), 2);
    assert_eq!(f.notices("comments", 1), 1);
    f.legacy_notice(4, &[2, 3]);
    assert_eq!(
        f.notices("comments", 4),
        1,
        "Mixed obsolete stacks must not be published"
    );
}

#[test]
fn sequential_notices_and_ordinary_comments_are_unchanged() {
    let mut f = Fixture::new();
    f.legacy_notice(3, &[2]);
    assert_eq!(f.notices("comments", 3), 1);
    f.explicit();
    for body in [
        "Dependency rework: this is a human discussion",
        "Dependency rework: upstream tasks [not JSON] need work.",
    ] {
        f.run(json!({"action":"comment","number":3,"body":body}));
    }
    assert_eq!(
        f.notices("comments", 3),
        3,
        "History and authored discussion remain intact"
    );
}

#[test]
fn mode_change_rejects_only_obsolete_queued_steering_and_preserves_live_reservations() {
    let mut f = Fixture::new();
    f.run(json!({"action":"set_blockers","number":4,"blockers":[2],"force":false}));
    f.reserve(3);
    f.reserve(4);
    f.queue("obsolete", 3, &[2]);
    f.queue("declared", 4, &[2]);
    f.queue("delivered", 3, &[2]);
    f.db.execute(
        "UPDATE agent_steering SET state='delivered' WHERE request_id='delivered'",
        [],
    )
    .unwrap();
    f.db.execute("INSERT INTO agent_steering(request_id,run_id,scope,text,created_at) VALUES('human','run-3','session','Keep the current implementation',123)",[]).unwrap();
    f.explicit();
    assert_eq!(f.queue_state("obsolete"), "rejected");
    assert_eq!(f.queue_state("declared"), "queued");
    assert_eq!(f.queue_state("delivered"), "delivered");
    assert_eq!(f.queue_state("human"), "queued");
    f.queue("legacy-after-switch", 3, &[2]);
    assert_eq!(f.queue_state("legacy-after-switch"), "rejected");
    assert_eq!(
        f.db.query_row(
            "SELECT count(*) FROM worker_runs WHERE finished_at IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    f.store = Store::open(&f.root.join("issues.db")).unwrap();
    assert_eq!(f.queue_state("declared"), "queued");
}

#[test]
fn mixed_legacy_notice_does_not_suppress_current_declared_rework() {
    let mut f = Fixture::new();
    f.explicit();
    f.run(json!({"action":"set_blockers","number":4,"blockers":[2],"force":false}));
    f.run(json!({"action":"add_pull_request","number":2,"url":"https://github.com/example/repo/pull/2"}));
    f.run(json!({"action":"ready","number":2,"force":false}));
    f.run(json!({"action":"claim","number":4,"force":false}));
    f.legacy_notice(4, &[2, 3]);
    assert_eq!(f.notices("comments", 4), 0);
    f.run(json!({"action":"reopen","number":2}));
    let view = f.run(json!({"action":"view","number":4}));
    assert_eq!(view["issue"]["assignee"], "codex:notice-test");
    assert_eq!(view["issue"]["blocked_by"][0]["number"], 2);
    let comments = view["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0]["body"]
            .as_str()
            .unwrap()
            .starts_with("Dependency rework: upstream tasks [2]")
    );
}

#[test]
fn stale_delivery_snapshot_is_rejected_after_declared_link_is_removed() {
    let mut f = Fixture::new();
    f.explicit();
    f.run(json!({"action":"set_blockers","number":4,"blockers":[2],"force":false}));
    f.reserve(4);
    f.queue("removed-link", 4, &[2]);
    assert_eq!(f.queue_state("removed-link"), "queued");
    f.db.execute(
        "UPDATE issues SET blockers='[]' WHERE project_id='named:Notices' AND number=4",
        [],
    )
    .unwrap();
    let changed=f.db.execute("UPDATE agent_steering SET state='sending' WHERE request_id='removed-link' AND state='queued'",[]).unwrap();
    assert_eq!(changed, 0);
    assert_eq!(f.queue_state("removed-link"), "rejected");
}

#[test]
fn notice_admission_does_not_scan_unrelated_issues() {
    let mut f = Fixture::new();
    f.explicit();
    f.db.execute_batch("WITH RECURSIVE numbers(n) AS (SELECT 100 UNION ALL SELECT n+1 FROM numbers WHERE n<2100)
        INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels)
        SELECT 'named:Notices',n,'Unrelated work','','open','codex:notice-test',0,0,1,'[]' FROM numbers;").unwrap();
    let body = "Dependency rework: upstream tasks [2] need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies.";
    let mut insert=f.db.prepare("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Notices',3,'codex:notice-test',?1,123)").unwrap();
    assert_eq!(insert.execute([body]).unwrap(), 0);
    let steps = insert.get_status(rusqlite::StatementStatus::VmStep);
    assert!(
        steps < 1000,
        "Notice guard scanned unrelated work: {steps} VM steps"
    );
}
