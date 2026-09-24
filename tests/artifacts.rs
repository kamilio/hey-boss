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
fn permanent_delete_is_scoped_revision_checked_and_retryable_with_file_cleanup() {
    let dir = std::env::temp_dir().join(format!("hey-boss-artifact-delete-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("issues.db");
    let mut store = Store::open(&path).unwrap();
    store
        .execute(&request(
            json!({"action":"create","title":"Keep issue","body":"","labels":[]}),
        ))
        .unwrap();
    store.execute(&request(json!({"action":"mindmap","operation":{"command":"add","kind":"text","title":"Keep node","body":"","alias":"topic"}}))).unwrap();
    let mut creation = request(
        json!({"action":"artifact","operation":{"command":"create","title":"Junk","body":"Discard","issue":1}}),
    );
    creation.request_id = Some("create-junk".into());
    let created = store.execute(&creation).unwrap();
    let id = created["artifact"]["id"].as_str().unwrap();
    run(&mut store, json!({"command":"link","id":id,"node":"topic"}));
    let comment = run(
        &mut store,
        json!({"command":"comment","id":id,"body":"Thread"}),
    );
    run(
        &mut store,
        json!({"command":"comment","id":id,"body":"Reply","parent":comment["comments"][0]["id"]}),
    );
    let mut files = Vec::new();
    for name in ["one.txt", "two.txt"] {
        let value = store.execute(&request(json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"artifact","id":id},"name":name,"data":"anVuaw=="}}))).unwrap();
        files.push(value["attachment"]["id"].as_str().unwrap().to_owned());
    }
    let deletion = |version| {
        request(
            json!({"action":"artifact","operation":{"command":"delete","id":id,"if_version":version}}),
        )
    };
    assert_eq!(
        store.execute(&deletion(0)).unwrap_err().code,
        "invalid_input"
    );
    let mut other = deletion(1);
    other.project_override = Some("Other".into());
    assert_eq!(store.execute(&other).unwrap_err().code, "not_found");
    run(
        &mut store,
        json!({"command":"archive","id":id,"archived":true,"if_version":1}),
    );
    assert_eq!(store.execute(&deletion(1)).unwrap_err().code, "conflict");
    assert_eq!(
        run(&mut store, json!({"command":"view","id":id}))["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut deletion = deletion(2);
    deletion.request_id = Some("delete-junk".into());
    let result = store.execute(&deletion).unwrap();
    assert_eq!(result["deleted"], id);
    for file in &files {
        let file_path = dir.join("issues.attachments").join(file);
        assert!(!file_path.exists());
        // A replay also completes an interrupted post-commit file unlink.
        std::fs::write(file_path, "junk").unwrap();
        assert_eq!(
            store
                .execute(&request(
                    json!({"action":"attachment","operation":{"command":"download","id":file}})
                ))
                .unwrap_err()
                .code,
            "not_found"
        );
    }
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.execute(&deletion).unwrap(), result);
    assert_eq!(store.execute(&creation).unwrap_err().code, "not_found");
    for file in &files {
        assert!(!dir.join("issues.attachments").join(file).exists());
    }
    assert_eq!(
        store
            .execute(&request(
                json!({"action":"artifact","operation":{"command":"view","id":id}})
            ))
            .unwrap_err()
            .code,
        "not_found"
    );
    for archived in [false, true] {
        assert!(
            run(&mut store, json!({"command":"list","archived":archived}))["artifacts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        run(&mut store, json!({"command":"links","issue":1}))["artifacts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        run(&mut store, json!({"command":"links","node":"topic"}))["artifacts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    for table in [
        "artifacts",
        "artifact_comments",
        "artifact_links",
        "file_attachments",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(
        db.query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM mindmap_nodes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(db);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn guarded_edits_bound_contention_and_preserve_concurrent_versions() {
    let path = std::env::temp_dir().join(format!("hey-boss-contention-{}.db", std::process::id()));
    let mut store = Store::open(&path).unwrap();
    let created = run(
        &mut store,
        json!({"command":"create","title":"Contention","body":"Original"}),
    );
    let id = created["artifact"]["id"].as_str().unwrap();
    store
        .execute(&request(
            json!({"action":"create","title":"Preview","body":"","labels":[]}),
        ))
        .unwrap();
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let error = store.execute(&request(json!({"action":"artifact","operation":{"command":"edit","id":id,"body":"Pending","if_version":1}}))).unwrap_err();
    assert_eq!(error.code, "database_busy");
    assert!(
        // Allow shared CI scheduling jitter around the bounded retry window.
        started.elapsed() < std::time::Duration::from_secs(9),
        "Contention must have a short bounded wait"
    );
    assert!(
        error.message.contains("retry"),
        "Busy errors must explain recovery"
    );
    // WAL readers must remain responsive during a writer.
    assert_eq!(
        run(&mut store, json!({"command":"view","id":id}))["artifact"]["body"],
        "Original"
    );
    writer.execute_batch("ROLLBACK").unwrap();
    drop(writer);

    // Release after the first busy timeout, with a newer committed version.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let lock_path = path.clone();
    let writer = std::thread::spawn(move || {
        let db = rusqlite::Connection::open(lock_path).unwrap();
        db.execute_batch(
            "BEGIN IMMEDIATE; UPDATE artifacts SET body='Concurrent',version=version+1",
        )
        .unwrap();
        ready_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2300));
        db.execute_batch("COMMIT").unwrap();
    });
    ready_rx.recv().unwrap();
    let error = store.execute(&request(json!({"action":"artifact","operation":{"command":"edit","id":id,"body":"Stale","if_version":1}}))).unwrap_err();
    assert_eq!(error.code, "conflict");
    writer.join().unwrap();
    let view = run(&mut store, json!({"command":"view","id":id}));
    assert_eq!(view["artifact"]["body"], "Concurrent");
    assert_eq!(view["artifact"]["version"], 2);
    run(
        &mut store,
        json!({"command":"edit","id":id,"body":"Fresh","if_version":2}),
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn store_open_retries_transient_read_locks() {
    let path = std::env::temp_dir().join(format!(
        "hey-boss-read-contention-{}.db",
        std::process::id()
    ));
    drop(Store::open(&path).unwrap());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let lock_path = path.clone();
    let writer = std::thread::spawn(move || {
        let db = rusqlite::Connection::open(lock_path).unwrap();
        db.execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE")
            .unwrap();
        ready_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2300));
        db.execute_batch("COMMIT").unwrap();
    });
    ready_rx.recv().unwrap();
    let started = std::time::Instant::now();
    let mut store = Store::open(&path).unwrap();
    store
        .execute(&request(
            json!({"action":"projects","include_hidden":false}),
        ))
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    writer.join().unwrap();
    drop(store);
    std::fs::remove_file(path).unwrap();
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
    let first_node = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["title"] == "first")
        .unwrap()["id"]
        .clone();
    assert_eq!(
        run(&mut store, json!({"command":"view","id":id}))["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut other = request(json!({"action":"artifact","operation":{"command":"view","id":id}}));
    other.project.id = "named:Other".into();
    other.project.name = "Other".into();
    assert_eq!(store.execute(&other).unwrap_err().code, "not_found");
    store.execute(&request(json!({"action":"mindmap","operation":{"command":"remove","node":"first","recursive":false}}))).unwrap();
    assert_eq!(
        run(&mut store, json!({"command":"view","id":id}))["artifact"]["title"],
        "Shared"
    );
    let viewed = run(&mut store, json!({"command":"view","id":id}));
    let removed_link = viewed["backlinks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|link| link["target"] == first_node)
        .unwrap();
    assert!(removed_link["title"].is_null());
}

#[test]
fn selection_anchors_use_the_readers_markdown_dialect() {
    let path = std::env::temp_dir().join(format!(
        "hey-boss-artifact-punctuation-{}.db",
        std::process::id()
    ));
    let mut store = Store::open(&path).unwrap();
    let text =
        "It's a \"quoted\" plan with literal $dollars$ and ~~changes~~.\n\n<mark>literal</mark>";
    let created = run(
        &mut store,
        json!({"command":"create","title":"Plan","body":text}),
    );
    let id = created["artifact"]["id"].as_str().unwrap();
    let commented = run(
        &mut store,
        json!({"command":"comment","id":id,"body":"Review this passage","quote":"It's a \"quoted\" plan with literal $dollars$ and changes."}),
    );
    assert_eq!(commented["comments"][0]["outdated"], false);
    assert!(
        created["artifact"]["body_html"]
            .as_str()
            .unwrap()
            .contains("&lt;mark&gt;")
    );
    let commented = run(
        &mut store,
        json!({"command":"comment","id":id,"body":"Discuss the literal example","quote":"<mark>literal</mark>"}),
    );
    assert_eq!(commented["comments"][1]["outdated"], false);
}

#[test]
fn selection_context_matches_rendered_table_cells_and_footnotes() {
    let path = std::env::temp_dir().join(format!(
        "hey-boss-artifact-rendered-context-{}.db",
        std::process::id()
    ));
    let mut store = Store::open(&path).unwrap();
    let created = run(
        &mut store,
        json!({"command":"create","title":"Context","body":"| Left | Right |\n| --- | --- |\n| Row | Keep **context** |\n\nA final **selected passage** with a footnote.[^note]\n\n[^note]: Keep explanation."}),
    );
    let id = created["artifact"]["id"].as_str().unwrap();
    let commented = run(
        &mut store,
        json!({"command":"comment","id":id,"body":"Review","quote":"selected passage","prefix":"RowKeep context\n\nA final ","suffix":" with a footnote.1\n1\nKeep explanation."}),
    );
    assert_eq!(commented["comments"][0]["outdated"], false);
}
