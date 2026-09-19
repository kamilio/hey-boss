use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};

fn request(operation: Value) -> Request {
    serde_json::from_value(json!({"version":1,"project":{"id":"named:Artifacts","name":"Artifacts"},
        "actor":{"id":"human:boss","kind":"human","session_id":null,"machine":"test","host":"test","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},
        "operation":operation,"request_id":null})).unwrap()
}
fn run(store: &mut Store, operation: Value) -> Value {
    store
        .execute(&request(json!({"action":"artifact","operation":operation})))
        .unwrap()
}

#[test]
fn artifacts_keep_comments_links_and_conflicted_edits() {
    let path = std::env::temp_dir().join(format!("hey-boss-artifacts-{}.db", std::process::id()));
    let mut store = Store::open(&path).unwrap();
    store
        .execute(&request(
            json!({"action":"create","title":"Issue","body":"","labels":[]}),
        ))
        .unwrap();
    let doc = run(
        &mut store,
        json!({"command":"create","title":"Plan","body":"# Plan\nSelected text","issue":1}),
    );
    let id = doc["artifact"]["id"].as_str().unwrap();
    let comment = run(
        &mut store,
        json!({"command":"comment","id":id,"body":"Discuss","quote":"Selected text"}),
    );
    let comment_id = comment["comments"][0]["id"].as_i64().unwrap();
    run(
        &mut store,
        json!({"command":"edit","id":id,"title":"Renamed","body":"Changed","if_version":1}),
    );
    let conflict = store.execute(&request(json!({"action":"artifact","operation":{"command":"edit","id":id,"body":"Lost","if_version":1}}))).unwrap_err();
    assert_eq!(conflict.code, "conflict");
    let view = run(&mut store, json!({"command":"view","id":id}));
    assert_eq!(view["artifact"]["body"], "Changed");
    assert_eq!(view["comments"][0]["outdated"], true);
    assert_eq!(view["backlinks"][0]["title"], "Issue");
    run(
        &mut store,
        json!({"command":"resolve","id":id,"comment_id":comment_id,"resolved":true}),
    );
    run(
        &mut store,
        json!({"command":"archive","id":id,"archived":true,"if_version":2}),
    );
    let issue = store
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    assert_eq!(issue["artifacts"][0]["title"], "Renamed");
    assert_eq!(issue["artifacts"][0]["archived"], true);
    drop(store);
    let mut store = Store::open(&path).unwrap();
    let view = run(&mut store, json!({"command":"view","id":id}));
    assert_eq!(view["comments"][0]["resolved"], true);
    run(&mut store, json!({"command":"unlink","id":id,"issue":1}));
    assert!(
        run(&mut store, json!({"command":"view","id":id}))["backlinks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        run(&mut store, json!({"command":"list","archived":false}))["artifacts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    run(
        &mut store,
        json!({"command":"archive","id":id,"archived":false,"if_version":3}),
    );
    assert_eq!(
        run(&mut store, json!({"command":"list","query":"renamed"}))["artifacts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn artifacts_are_shared_by_nodes_and_scoped_to_their_project() {
    let path =
        std::env::temp_dir().join(format!("hey-boss-artifact-links-{}.db", std::process::id()));
    let mut store = Store::open(&path).unwrap();
    for alias in ["first", "second"] {
        store.execute(&request(json!({"action":"mindmap","operation":{"command":"add","kind":"text","title":alias,"body":"","alias":alias}}))).unwrap();
    }
    let created = run(
        &mut store,
        json!({"command":"create","title":"Shared","body":"A **selected** phrase","node":"first"}),
    );
    let id = created["artifact"]["id"].as_str().unwrap();
    run(
        &mut store,
        json!({"command":"link","id":id,"node":"second"}),
    );
    let commented = run(
        &mut store,
        json!({"command":"comment","id":id,"body":"Thread","quote":"selected","prefix":"A ","suffix":" phrase"}),
    );
    assert_eq!(commented["comments"][0]["outdated"], false);
    let parent = commented["comments"][0]["id"].as_i64().unwrap();
    run(
        &mut store,
        json!({"command":"comment","id":id,"body":"Reply","parent":parent}),
    );
    let graph = store
        .execute(&request(
            json!({"action":"mindmap","operation":{"command":"show"}}),
        ))
        .unwrap();
    assert_eq!(graph["nodes"][0]["artifacts"][0]["id"], id);
    assert_eq!(graph["nodes"][1]["artifacts"][0]["id"], id);
    assert_eq!(
        run(&mut store, json!({"command":"view","id":id}))["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut other = request(json!({"action":"artifact","operation":{"command":"view","id":id}}));
    other.project.id = "named:Other".into();
    assert_eq!(store.execute(&other).unwrap_err().code, "not_found");
    store.execute(&request(json!({"action":"mindmap","operation":{"command":"remove","node":"first","recursive":false}}))).unwrap();
    assert_eq!(
        run(&mut store, json!({"command":"view","id":id}))["artifact"]["title"],
        "Shared"
    );
    assert!(run(&mut store, json!({"command":"view","id":id}))["backlinks"][0]["title"].is_null());
}
