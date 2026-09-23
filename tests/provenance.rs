use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};

fn request(operation: Value) -> Request {
    serde_json::from_value(json!({"version":1,"project":{"id":"named:Origins","name":"Origins"},
        "actor":{"id":"codex:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","kind":"codex","session_id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","machine":"test","host":"test-host","pid":null,"process_start":null,"cwd":"/tmp","source":"CODEX_THREAD_ID","model":"gpt-6-astra","invocation":{"offset":123,"call_id":"call-create"}},
        "operation":operation})).unwrap()
}

#[test]
fn creation_origin_survives_retries_edits_reopen_and_project_moves() {
    let root = std::env::temp_dir().join(format!("hey-boss-origin-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("issues.db");
    let mut store = Store::open(&path).unwrap();
    let mut create = request(
        json!({"action":"create","title":"Found during another task","body":"Context","labels":[]}),
    );
    create.request_id = Some("same-create".into());
    let created = store.execute(&create).unwrap();
    let origin = created["issue"]["origin"].clone();
    assert_eq!(
        origin["session_id"],
        create
            .actor
            .as_ref()
            .unwrap()
            .session_id
            .as_ref()
            .unwrap()
            .as_str()
    );
    assert_eq!(origin["model"], "gpt-6-astra");
    assert_eq!(origin["invocation"]["offset"], 123);
    assert_eq!(origin["actor_id"], create.actor.as_ref().unwrap().id);
    create.actor.as_mut().unwrap().invocation = None;
    assert_eq!(store.execute(&create).unwrap()["issue"]["origin"], origin);
    for operation in [
        json!({"action":"edit","number":1,"title":"Updated","add_labels":[],"remove_labels":[]}),
        json!({"action":"close","number":1,"force":false}),
        json!({"action":"reopen","number":1}),
    ] {
        store.execute(&request(operation)).unwrap();
    }
    assert_eq!(
        store
            .execute(&request(json!({"action":"view","number":1})))
            .unwrap()["issue"]["origin"],
        origin
    );
    let mut destination =
        request(json!({"action":"create","title":"Destination seed","body":"","labels":[]}));
    destination.project.id = "named:Destination".into();
    destination.project.name = "Destination".into();
    store.execute(&destination).unwrap();
    let moved = store
        .execute(&request(
            json!({"action":"transfer","number":1,"destination":"Destination","if_version":4}),
        ))
        .unwrap();
    assert_eq!(moved["issue"]["origin"], origin);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn artifact_origin_is_persistent_and_human_creation_has_no_invented_session() {
    let root =
        std::env::temp_dir().join(format!("hey-boss-artifact-origin-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("issues.db");
    let mut store = Store::open(&path).unwrap();
    let created = store.execute(&request(json!({"action":"artifact","operation":{"command":"create","title":"Investigation","body":"Findings"}}))).unwrap();
    let origin = created["artifact"]["origin"].clone();
    assert_eq!(origin["invocation"]["call_id"], "call-create");
    let id = created["artifact"]["id"].as_str().unwrap();
    let edited = store.execute(&request(json!({"action":"artifact","operation":{"command":"edit","id":id,"body":"More findings","if_version":1}}))).unwrap();
    assert_eq!(edited["artifact"]["origin"], origin);
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .execute(&request(
                json!({"action":"artifact","operation":{"command":"view","id":id}})
            ))
            .unwrap()["artifact"]["origin"],
        origin
    );
    let mut human = request(json!({"action":"create","title":"From Boss","body":"","labels":[]}));
    let actor = human.actor.as_mut().unwrap();
    actor.id = "human:boss".into();
    actor.kind = "human".into();
    actor.session_id = None;
    actor.invocation = None;
    actor.model = None;
    let origin = store.execute(&human).unwrap()["issue"]["origin"].clone();
    assert_eq!(origin["actor_id"], "human:boss");
    assert!(origin["session_id"].is_null());
    assert!(origin["run"].is_null());
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn creation_links_the_actual_source_run_across_projects() {
    let root = std::env::temp_dir().join(format!("hey-boss-source-run-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("issues.db");
    let mut store = Store::open(&path).unwrap();
    store
        .execute(&request(
            json!({"action":"create","title":"Original task","body":"","labels":[]}),
        ))
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,session_id,state,owner_pid,owner_start,machine,started_at,updated_at) VALUES('source-run','named:Origins',1,'{\"issue\":{\"title\":\"Original task\"}}','worker:source','aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee','running',1,'test','test',1,1);").unwrap();
    let mut other = request(json!({"action":"create","title":"Follow-up","body":"","labels":[]}));
    other.project.id = "named:Follow-ups".into();
    other.project.name = "Follow-ups".into();
    let origin = store.execute(&other).unwrap()["issue"]["origin"].clone();
    assert_eq!(origin["run"]["id"], "source-run");
    assert_eq!(origin["run"]["project_id"], "named:Origins");
    db.execute("UPDATE worker_runs SET finished_at=2", [])
        .unwrap();
    assert!(
        store.execute(&other).unwrap()["issue"]["origin"]["run"].is_null(),
        "Do not attribute new work to a finished attempt"
    );
    drop(db);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
