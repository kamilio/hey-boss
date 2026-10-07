#[path = "support/projects.rs"]
mod projects;
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
            .join(format!("dep-chains-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        projects::seed(&root.join("issues.db"), &["named:DepChains"]);
        Self { root, store }
    }

    fn request(op: Value) -> Request {
        serde_json::from_value(json!({
            "version": 1,
            "project": {"id": "named:DepChains", "name": "DepChains"},
            "actor": {
                "id": "codex:test",
                "kind": "codex",
                "session_id": "test",
                "machine": "test",
                "host": "test",
                "pid": null,
                "process_start": null,
                "cwd": "/tmp",
                "source": "test"
            },
            "operation": op
        }))
        .unwrap()
    }

    fn run(&mut self, op: Value) -> Value {
        self.store.execute(&Self::request(op)).unwrap()
    }

    fn view(&mut self, n: i64) -> Value {
        self.run(json!({"action": "view", "number": n}))["issue"].clone()
    }

    fn pickup_ready(&self) -> Vec<i64> {
        let db = rusqlite::Connection::open(self.root.join("issues.db")).unwrap();
        db.prepare("SELECT number FROM issue_pickup_ready ORDER BY number")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn atomic_create_with_blockers_starts_blocked_immediately() {
    let mut f = Fixture::new("atomic-create");
    let first = f.run(json!({
        "action": "create",
        "title": "Foundation storage layer",
        "body": "Implement schema",
        "labels": []
    }))["issue"]["number"]
        .as_i64()
        .unwrap();
    assert_eq!(first, 1);

    let second_res = f.run(json!({
        "action": "create",
        "title": "API endpoints on top of storage",
        "body": "Use storage schema",
        "labels": [],
        "blockers": [1]
    }));
    let second = &second_res["issue"];
    assert_eq!(second["number"], 2);
    assert_eq!(second["state"], "blocked");
    assert_eq!(second["blocker_numbers"], json!([1]));
    assert_eq!(second["blocked_by"][0]["number"], 1);
    assert_eq!(second["blocked_by"][0]["source"], "linked");

    // Only #1 may be pickup-ready; #2 was created already blocked.
    assert_eq!(f.pickup_ready(), vec![1]);

    // Viewing #1 shows that it is blocking #2 and will unblock #2 on release.
    let first_view = f.view(1);
    assert_eq!(first_view["blocking"][0]["number"], 2);
    assert_eq!(first_view["blocking"][0]["state"], "blocked");
    assert_eq!(first_view["blocking"][0]["actively_blocked"], true);
    assert_eq!(first_view["blocking"][0]["unblocks_on_release"], true);
}

#[test]
fn atomic_then_chain_creates_entire_sequence_and_unblocks_step_by_step_on_ready_and_close() {
    let mut f = Fixture::new("then-chain");
    let created = f.run(json!({
        "action": "create",
        "title": "Step 1: Database schema",
        "body": "Base layer",
        "labels": ["stack"],
        "then_titles": [
            "Step 2: Service layer",
            "Step 3: Web UI"
        ]
    }));

    let chain = created["created_chain"].as_array().unwrap();
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0]["number"], 1);
    assert_eq!(chain[0]["state"], "open");
    assert_eq!(chain[1]["number"], 2);
    assert_eq!(chain[1]["state"], "blocked");
    assert_eq!(chain[1]["blocker_numbers"], json!([1]));
    assert_eq!(chain[2]["number"], 3);
    assert_eq!(chain[2]["state"], "blocked");
    assert_eq!(chain[2]["blocker_numbers"], json!([2]));

    assert_eq!(f.pickup_ready(), vec![1]);

    // Attach PR #101 to Step 1 and mark Step 1 Ready (without pre-configuring prs_enabled).
    f.run(json!({
        "action": "add_pull_request",
        "number": 1,
        "url": "https://github.com/example/repo/pull/101"
    }));
    f.run(json!({
        "action": "ready",
        "number": 1,
        "force": false
    }));

    // Step 1 being Ready immediately unblocks Step 2, while Step 3 remains blocked by Step 2.
    assert_eq!(f.view(1)["state"], "ready");
    assert_eq!(f.view(2)["state"], "open");
    assert_eq!(f.view(3)["state"], "blocked");
    assert_eq!(f.pickup_ready(), vec![2]);

    // Fetching Step 2 provides prerequisite context on demand.
    let fetched2 = f.view(2);
    let prerequisite = &fetched2["dependency_context"][0];
    assert_eq!(prerequisite["number"], 1);
    assert_eq!(prerequisite["state"], "ready");
    assert_eq!(
        prerequisite["pull_requests"][0]["url"],
        "https://github.com/example/repo/pull/101"
    );

    // Claiming retains structured context and generic stacked-PR guidance.
    let claim2 = f.run(json!({"action": "claim", "number": 2, "force": false}));
    assert_eq!(
        claim2["issue"]["dependency_context"],
        fetched2["dependency_context"]
    );
    assert_eq!(claim2["issue"]["blocker_links"][0]["number"], 1);
    assert_eq!(claim2["issue"]["blocker_links"][0]["state"], "ready");
    assert_eq!(claim2["issue"]["blocker_links"][0]["satisfied"], true);
    assert_eq!(
        claim2["issue"]["blocker_links"][0]["pull_requests"][0]["url"],
        "https://github.com/example/repo/pull/101"
    );
    assert_eq!(claim2["issue"]["blocking"][0]["number"], 3);
    assert_eq!(claim2["issue"]["blocking"][0]["unblocks_on_release"], true);
    let instructions = claim2["instructions"].as_str().unwrap();
    assert!(
        !instructions.contains("https://github.com/example/repo/pull/101"),
        "Prerequisite PR context should be fetched, not injected into instructions: {instructions}"
    );
    assert!(
        instructions.contains("dependency branch as your branch start and PR base"),
        "Expected stacked PR branch/base guidance in instructions: {instructions}"
    );

    // Closing Step 2 unblocks Step 3 automatically.
    f.run(json!({"action": "close", "number": 2, "force": false}));
    assert_eq!(f.view(2)["state"], "closed");
    assert_eq!(f.view(3)["state"], "open");
    assert_eq!(f.pickup_ready(), vec![3]);
}

#[test]
fn subtasks_do_not_invent_implicit_sibling_blockers_by_default() {
    let mut f = Fixture::new("subtasks-explicit-default");
    let parent = f.run(json!({
        "action": "create",
        "title": "Parent epic",
        "body": "",
        "labels": []
    }))["issue"]["number"]
        .as_i64()
        .unwrap();
    let sub1 = f.run(json!({
        "action": "create_subtask",
        "number": parent,
        "title": "Subtask A",
        "body": "",
        "labels": []
    }))["issue"]["number"]
        .as_i64()
        .unwrap();
    let sub2 = f.run(json!({
        "action": "create_subtask",
        "number": parent,
        "title": "Subtask B",
        "body": "",
        "labels": []
    }))["issue"]["number"]
        .as_i64()
        .unwrap();

    // Both subtasks are open and pickup-ready; only explicit blocked-by links block siblings.
    assert_eq!(f.view(sub1)["state"], "open");
    assert_eq!(f.view(sub2)["state"], "open");
    assert_eq!(f.pickup_ready(), vec![sub1, sub2]);

    // Creating a subtask with explicit blockers=[sub1] starts it blocked immediately.
    let sub3 = f.run(json!({
        "action": "create_subtask",
        "number": parent,
        "title": "Subtask C depends on A",
        "body": "",
        "labels": [],
        "blockers": [sub1]
    }))["issue"]
        .clone();
    assert_eq!(sub3["state"], "blocked");
    assert_eq!(sub3["blocker_numbers"], json!([sub1]));
    assert_eq!(f.pickup_ready(), vec![sub1, sub2]);
}
