#[path = "support/projects.rs"]
mod projects;
use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};

fn request(operation: Value, human: bool) -> Request {
    serde_json::from_value(json!({
        "version": 1, "project": {"id": "named:Comments", "name": "Comments"},
        "actor": {"id": if human {"human:boss"} else {"codex:test"},
            "kind": if human {"human"} else {"codex"}, "machine": "test",
            "host": "test", "cwd": "/tmp", "source": "test"},
        "operation": operation
    }))
    .unwrap()
}

#[test]
fn agent_comment_limits_apply_before_mutations_and_offer_an_override() {
    let dir = std::env::temp_dir().join(format!("hey-boss-comment-limits-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = Store::open(&dir.join("issues.db")).unwrap();
    projects::seed(&dir.join("issues.db"), &["named:Comments"]);
    store
        .execute(&request(
            json!({"action":"create", "title":"Comments", "body":"", "labels":[]}),
            false,
        ))
        .unwrap();
    let document = store
        .execute(&request(
            json!({"action":"artifact", "operation": {
                "command":"create", "title":"Review", "body":"", "issue":1
            }}),
            false,
        ))
        .unwrap();
    let id = &document["artifact"]["id"];

    for mut operation in [
        json!({"action":"comment", "number":1}),
        json!({"action":"close", "number":1, "force":false}),
        json!({"action":"block", "number":1, "force":false}),
        json!({"action":"artifact", "operation":{"command":"comment", "id":id}}),
    ] {
        let field = if matches!(operation["action"].as_str(), Some("close" | "block")) {
            "comment"
        } else {
            "body"
        };
        for body in [
            "x".repeat(301),
            "one\ntwo\nthree".into(),
            "one\u{2028}two\u{2029}three".into(),
        ] {
            let target = if operation["action"] == "artifact" {
                &mut operation["operation"]
            } else {
                &mut operation
            };
            target[field] = json!(body);
            let error = store
                .execute(&request(operation.clone(), false))
                .unwrap_err();
            assert_eq!(error.code, "comment_too_long");
            assert!(
                error
                    .message
                    .contains("Write so user can understand it, who is gonna read this?")
            );
            assert!(error.message.contains("--allow-long-comment"));
            assert!(error.message.contains("Do not sound like a robot."));
        }
    }
    let unchanged = store
        .execute(&request(json!({"action":"view", "number":1}), false))
        .unwrap();
    assert_eq!(unchanged["issue"]["state"], "open");
    assert_eq!(unchanged["comment_count"], 0);

    for body in [
        "All tests pass.",
        "Fixed the crash.\nTests pass.",
        "Fixed the crash.\r\nTests pass.",
        &"🦀".repeat(300),
    ] {
        store
            .execute(&request(
                json!({"action":"comment", "number":1, "body":body}),
                false,
            ))
            .unwrap();
    }
    let long = "Detailed note.\n".repeat(50);
    for human in [false, true] {
        for operation in [
            json!({"action":"comment", "number":1, "body":long, "allow_long_comment":!human}),
            json!({"action":"artifact", "operation":{"command":"comment", "id":id, "body":long, "allow_long_comment":!human}}),
        ] {
            store.execute(&request(operation, human)).unwrap();
        }
    }
    for action in ["block", "close"] {
        store.execute(&request(json!({"action":action, "number":1, "force":false, "comment":long, "allow_long_comment":true}), false)).unwrap();
    }
    let closed = store
        .execute(&request(json!({"action":"view", "number":1}), false))
        .unwrap();
    assert_eq!(closed["issue"]["state"], "closed");
    assert_eq!(
        closed["comments"].as_array().unwrap().last().unwrap()["body"],
        long
    );
    // The readability override does not bypass the existing storage limit.
    assert!(store.execute(&request(json!({"action":"comment", "number":1, "body":"x".repeat(1024*1024+1), "allow_long_comment":true}), false)).is_err());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
