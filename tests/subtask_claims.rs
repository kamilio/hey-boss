use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    store: Store,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "hey-boss-subtask-claims-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        Self { root, store }
    }

    fn request(actor: &str, operation: Value) -> Request {
        serde_json::from_value(json!({"version":1,
            "project":{"id":"named:Subtask claims","name":"Subtask claims"},
            "actor":{"id":actor,"kind":"codex","session_id":actor,"machine":"test",
                "host":"test-host","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},
            "operation":operation}))
        .unwrap()
    }

    fn run(&mut self, operation: Value) -> Value {
        self.store
            .execute(&Self::request("codex:owner", operation))
            .unwrap()
    }

    fn create(&mut self) {
        self.run(json!({"action":"create","title":"Work","body":"Keep this body","labels":[]}));
    }

    fn snapshot(&self) -> String {
        let db = rusqlite::Connection::open(self.root.join("issues.db")).unwrap();
        let mut result = String::new();
        for table in [
            "issues",
            "issue_subtasks",
            "events",
            "projects",
            "requests",
            "fleet_outbox",
            "fleet_allocations",
            "worker_runs",
        ] {
            let mut stmt = db
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |row| {
                    (0..columns)
                        .map(|i| row.get::<_, rusqlite::types::Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            result.push_str(&format!("{table}: {rows:?}\n"));
        }
        result
    }

    fn rejects_without_changes(&mut self, actor: &str, operation: Value, parent: i64) {
        let before = self.snapshot();
        let error = self
            .store
            .execute(&Self::request(actor, operation))
            .unwrap_err();
        assert_eq!(error.code, "subtask_claim_conflict");
        assert!(error.message.contains(&format!("#{parent}")), "{error}");
        assert!(error.message.contains("mindmap"), "{error}");
        assert_eq!(
            self.snapshot(),
            before,
            "Rejected organization must roll back issues, links, versions, events, and numbering"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn subtask_organization_cannot_release_any_existing_parent_claim() {
    let mut f = Fixture::new("direct");
    f.create();
    f.create();
    f.run(json!({"action":"claim","number":1,"force":false}));
    f.run(json!({"action":"claim","number":2,"force":false}));
    for actor in ["codex:owner", "codex:organizer", "human:boss"] {
        f.rejects_without_changes(
            actor,
            json!({"action":"add_subtask","number":1,"child":2}),
            1,
        );
        f.rejects_without_changes(actor, json!({"action":"create_subtask","number":1,"title":"Follow-up","body":"Keep my draft","labels":[]}), 1);
    }
    assert_eq!(
        f.run(json!({"action":"view","number":1}))["issue"]["assignee"],
        "codex:owner"
    );
    assert_eq!(
        f.run(json!({"action":"view","number":2}))["issue"]["assignee"],
        "codex:owner"
    );
    f.create();
    assert_eq!(
        f.run(json!({"action":"view","number":3}))["issue"]["title"],
        "Work"
    );
}

#[test]
fn closed_children_and_unlink_preserve_claims_but_open_grandchildren_are_guarded() {
    let mut f = Fixture::new("ancestor");
    for _ in 0..3 {
        f.create();
    }
    f.run(json!({"action":"close","number":2,"force":false}));
    f.run(json!({"action":"claim","number":1,"force":false}));
    f.run(json!({"action":"add_subtask","number":1,"child":2}));
    let parent = f.run(json!({"action":"view","number":1}));
    assert_eq!(parent["issue"]["state"], "open");
    assert_eq!(parent["issue"]["assignee"], "codex:owner");
    let repeat = f.run(json!({"action":"add_subtask","number":1,"child":2}));
    assert_eq!(repeat["changed"], false);
    f.rejects_without_changes(
        "codex:organizer",
        json!({"action":"add_subtask","number":2,"child":3}),
        1,
    );
    f.rejects_without_changes("codex:owner", json!({"action":"create_subtask","number":2,"title":"Nested follow-up","body":"","labels":[]}), 1);
    f.run(json!({"action":"remove_subtask","number":1,"child":2}));
    assert_eq!(
        f.run(json!({"action":"view","number":1}))["issue"]["assignee"],
        "codex:owner"
    );
    f.run(json!({"action":"add_subtask","number":2,"child":3}));
    f.rejects_without_changes(
        "codex:organizer",
        json!({"action":"add_subtask","number":1,"child":2}),
        1,
    );
}

#[test]
fn unassigned_parent_still_waits_for_subtasks_and_recovers_after_unlink() {
    let mut f = Fixture::new("unassigned");
    f.create();
    f.run(json!({"action":"create_subtask","number":1,"title":"Child","body":"","labels":[]}));
    assert_eq!(
        f.run(json!({"action":"view","number":1}))["issue"]["state"],
        "blocked"
    );
    f.run(json!({"action":"remove_subtask","number":1,"child":2}));
    assert_eq!(
        f.run(json!({"action":"view","number":1}))["issue"]["state"],
        "open"
    );
    assert_eq!(
        f.run(json!({"action":"view","number":2}))["issue"]["parent"],
        Value::Null
    );
}

#[test]
fn subtask_help_explains_scheduling_and_ownership_safe_grouping() {
    for args in [
        vec!["issue", "subtask", "--help"],
        vec!["issue", "subtask", "add", "--help"],
        vec!["issue", "subtask", "create", "--help"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("Blocked"), "{help}");
        assert!(help.contains("claim"), "{help}");
        assert!(help.contains("mindmap"), "{help}");
    }
}

#[test]
fn mindmap_nesting_is_an_ownership_preserving_alternative() {
    let mut f = Fixture::new("mindmap");
    f.create();
    f.create();
    for number in [1, 2] {
        f.run(json!({"action":"claim","number":number,"force":false}));
    }
    let before = f.snapshot();
    f.run(json!({"action":"mindmap","operation":{"command":"add","title":"Parent","body":"","kind":"issue","reference":"1","alias":"parent-work"}}));
    f.run(json!({"action":"mindmap","operation":{"command":"add","title":"Follow-up","body":"","kind":"issue","reference":"2","under":"parent-work"}}));
    // Mindmaps advance project activity but never issue rows, links, or events.
    let after = f.snapshot();
    assert_eq!(
        before.split("projects:").next(),
        after.split("projects:").next()
    );
    for number in [1, 2] {
        let issue = f.run(json!({"action":"view","number":number}));
        assert_eq!(issue["issue"]["state"], "open");
        assert_eq!(issue["issue"]["assignee"], "codex:owner");
        assert_eq!(issue["issue"]["parent"], Value::Null);
    }
}
