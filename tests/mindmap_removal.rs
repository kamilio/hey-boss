use std::process::Command;

#[test]
fn retired_commands_and_resource_flags_are_rejected() {
    for args in [
        vec!["mm", "--help"],
        vec!["mindmap", "--help"],
        vec!["project", "init", "--mindmaps", "true", "--help"],
        vec!["artifact", "links", "--node", "topic", "--help"],
        vec!["attachment", "list", "--node", "topic", "--help"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(&args)
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "retired interface accepted: {args:?}"
        );
    }
}

#[test]
fn retired_protocol_operations_are_rejected() {
    for operation in [
        serde_json::json!({"action":"mindmap","operation":{"command":"show"}}),
        serde_json::json!({"action":"artifact","operation":{"command":"links","node":"topic"}}),
        serde_json::json!({"action":"attachment","operation":{"command":"list","target":{"kind":"node","id":"topic"}}}),
    ] {
        assert!(
            serde_json::from_value::<hey_boss::issues::Operation>(operation.clone()).is_err(),
            "retired operation accepted: {operation}"
        );
    }
}

#[test]
fn fresh_stores_do_not_create_map_schema() {
    let root = std::env::temp_dir().join(format!("hb-retired-maps-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("issues.db");
    drop(hey_boss::issues::Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'mindmap%'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(db.query_row("SELECT count(*) FROM pragma_table_info('project_settings') WHERE name='mindmaps_enabled'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
