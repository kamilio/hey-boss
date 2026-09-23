use hey_boss::issues::{Actor, Operation, Project, Request, Store};
use serde_json::{Value, json};

fn operation(value: Value) -> Operation {
    serde_json::from_value(value).unwrap()
}

#[test]
fn transfer_preserves_issue_history_and_retries_without_duplicating() {
    let root = std::env::temp_dir().join(format!("hey-boss-transfer-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut store = Store::open(&root.join("issues.db")).unwrap();
    let actor = Actor {
        id: "human:boss".into(),
        kind: "human".into(),
        session_id: None,
        machine: "test".into(),
        host: "test".into(),
        pid: None,
        process_start: None,
        cwd: root.clone(),
        source: "test".into(),
        invocation: None,
        creation_run: None,
        model: None,
    };
    let mut call = |project: &str, value: Value, key: Option<&str>| {
        store.execute(&Request {
            version: 1,
            project: Project {
                id: "named:Source".into(),
                name: "Source".into(),
            },
            project_override: Some(project.into()),
            actor: Some(actor.clone()),
            operation: operation(value),
            request_id: key.map(str::to_owned),
        })
    };
    let create = json!({"action":"create","title":"Keep everything","body":"**Markdown**","labels":["ready"],"draft":true});
    call("Source", create.clone(), None).unwrap();
    call("Destination", create, None).unwrap();
    call("Source", json!({"action":"undraft","number":1}), None).unwrap();
    call(
        "Source",
        json!({"action":"claim","number":1,"force":false}),
        None,
    )
    .unwrap();
    call(
        "Source",
        json!({"action":"status","number":1,"level":"green","comment":"The fix passes tests."}),
        None,
    )
    .unwrap();
    call(
        "Source",
        json!({"action":"unassign","number":1,"force":false}),
        None,
    )
    .unwrap();
    call(
        "Source",
        json!({"action":"edit","number":1,"draft":true,"add_labels":[],"remove_labels":[]}),
        None,
    )
    .unwrap();
    let comment = call(
        "Source",
        json!({"action":"comment","number":1,"body":"Preserved comment"}),
        None,
    )
    .unwrap();
    call("Source", json!({"action":"resolve_comment","number":1,"comment_id":comment["comment_id"],"resolved":true}),None).unwrap();
    call("Source", json!({"action":"add_pull_request","number":1,"url":"https://github.com/example/repo/pull/1","purpose":"supporting-evidence"}), None).unwrap();
    call("Source",json!({"action":"mindmap","operation":{"command":"add","title":"Keep everything","body":"","kind":"issue","reference":"1","reference_project":"named:Source","alias":"moving"}}),None).unwrap();
    let source = call("Source", json!({"action":"view","number":1}), None).unwrap();
    let transfer = json!({"action":"transfer","number":1,"destination":"named:Destination","if_version":source["issue"]["version"]});
    let moved = call("Source", transfer.clone(), Some("move-once")).unwrap();
    assert_eq!(moved["project"]["id"], "named:Destination");
    assert_eq!(moved["issue"]["number"], 2);
    assert_eq!(moved["issue"]["body"], "**Markdown**");
    assert_eq!(moved["issue"]["labels"], json!(["ready"]));
    assert_eq!(moved["issue"]["draft"], true);
    assert_eq!(moved["issue"]["pull_requests"].as_array().unwrap().len(), 1);
    assert_eq!(
        moved["issue"]["pull_requests"][0]["purpose"],
        "supporting-evidence"
    );
    assert_eq!(call("Source", transfer, Some("move-once")).unwrap(), moved);
    let destination = call("Destination", json!({"action":"view","number":2}), None).unwrap();
    assert_eq!(destination["comments"][0]["body"], "Preserved comment");
    assert_eq!(destination["comments"][0]["resolved"], true);
    assert_eq!(
        destination["issue"]["status"]["comment"],
        "The fix passes tests."
    );
    let status_history = call(
        "Destination",
        json!({"action":"status_history","number":2,"limit":20,"offset":0}),
        None,
    )
    .unwrap();
    assert_eq!(status_history["updates"].as_array().unwrap().len(), 1);
    assert_eq!(status_history["updates"][0]["level"], "green");
    let map = call(
        "Source",
        json!({"action":"mindmap","operation":{"command":"view","node":"moving"}}),
        None,
    )
    .unwrap();
    assert_eq!(map["node"]["reference_project"], "named:Destination");
    assert_eq!(map["node"]["reference"], "2");
    let old = call("Source", json!({"action":"view","number":1}), None).unwrap();
    assert_eq!(old["moved_to"]["project"]["id"], "named:Destination");
    let projects = call("Source", json!({"action":"projects"}), None).unwrap();
    assert_eq!(
        projects["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "named:Source")
            .unwrap()["deleted"],
        0
    );
    assert!(call("Source",json!({"action":"list","state":"deleted","mine":false,"unassigned":false,"labels":[],"limit":100,"offset":0}),None).unwrap()["issues"].as_array().unwrap().is_empty());
    assert!(call("Source", json!({"action":"restore","number":1}), None).is_err());
    let history = call(
        "Destination",
        json!({"action":"history","number":2,"limit":100,"offset":0}),
        None,
    )
    .unwrap();
    assert!(
        history["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "transferred")
    );
    call(
        "Source",
        json!({"action":"create","title":"Guarded move","body":"","labels":[]}),
        None,
    )
    .unwrap();
    for (destination, version, code) in [
        ("Destination", 99, "conflict"),
        ("Missing", 1, "not_found"),
        ("Source", 1, "invalid_input"),
    ] {
        let error = call(
            "Source",
            json!({"action":"transfer","number":2,"destination":destination,"if_version":version}),
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, code);
    }
    call("Destination", json!({"action":"hide_project"}), None).unwrap();
    assert_eq!(
        call(
            "Source",
            json!({"action":"transfer","number":2,"destination":"Destination","if_version":1}),
            None
        )
        .unwrap_err()
        .code,
        "not_found"
    );
    call("Destination", json!({"action":"restore_project"}), None).unwrap();
    call(
        "Source",
        json!({"action":"create_subtask","number":2,"title":"Child","body":"","labels":[]}),
        None,
    )
    .unwrap();
    let parent = call("Source", json!({"action":"view","number":2}), None).unwrap();
    assert_eq!(call("Source",json!({"action":"transfer","number":2,"destination":"Destination","if_version":parent["issue"]["version"]}),None).unwrap_err().code,"conflict");
    call(
        "Source",
        json!({"action":"remove_subtask","number":2,"child":3}),
        None,
    )
    .unwrap();
    call(
        "Source",
        json!({"action":"assign_boss","number":2,"force":false}),
        None,
    )
    .unwrap();
    let assigned = call("Source", json!({"action":"view","number":2}), None).unwrap();
    let moved=call("Source",json!({"action":"transfer","number":2,"destination":"Destination","if_version":assigned["issue"]["version"]}),None).unwrap();
    assert_eq!(moved["issue"]["number"], 3);
    assert_eq!(moved["issue"]["assignee"], "human:boss");
    call(
        "Destination",
        json!({"action":"close","number":3,"force":false}),
        None,
    )
    .unwrap();
    let closed = call("Destination", json!({"action":"view","number":3}), None).unwrap();
    let back=call("Destination",json!({"action":"transfer","number":3,"destination":"Source","if_version":closed["issue"]["version"]}),None).unwrap();
    assert_eq!(back["issue"]["state"], "closed");
    assert_eq!(back["issue"]["closed_at"], closed["issue"]["closed_at"]);
    let db = rusqlite::Connection::open(root.join("issues.db")).unwrap();
    let child = call("Source", json!({"action":"view","number":3}), None).unwrap();
    let transfer_child = json!({"action":"transfer","number":3,"destination":"Destination","if_version":child["issue"]["version"]});
    db.execute("INSERT INTO agents(id,metadata,last_seen) SELECT 'codex:active',json_set(metadata,'$.id','codex:active','$.kind','codex'),last_seen FROM agents WHERE id='human:boss'",[]).unwrap();
    db.execute(
        "UPDATE issues SET assignee='codex:active' WHERE project_id='named:Source' AND number=3",
        [],
    )
    .unwrap();
    assert_eq!(
        call("Source", transfer_child.clone(), None)
            .unwrap_err()
            .code,
        "conflict"
    );
    db.execute(
        "UPDATE issues SET assignee=NULL WHERE project_id='named:Source' AND number=3",
        [],
    )
    .unwrap();
    db.execute("UPDATE fleet_meta SET role='agent'", [])
        .unwrap();
    assert_eq!(
        call("Source", transfer_child.clone(), None)
            .unwrap_err()
            .code,
        "conflict"
    );
    db.execute("UPDATE fleet_meta SET role='standalone'", [])
        .unwrap();
    call(
        "Source",
        json!({"action":"create","title":"Draft move","body":"","labels":[],"draft":true}),
        None,
    )
    .unwrap();
    call(
        "Destination",
        json!({"action":"configure_project","drafts_enabled":false}),
        None,
    )
    .unwrap();
    assert_eq!(
        call(
            "Source",
            json!({"action":"transfer","number":5,"destination":"Destination","if_version":1}),
            None
        )
        .unwrap_err()
        .code,
        "invalid_input"
    );
    let foreign_keys: i64 = db
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(foreign_keys, 0);
    let moved_child = call("Source", transfer_child, None).unwrap();
    assert_eq!(moved_child["drafts_enabled"], false);
    drop(db);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
