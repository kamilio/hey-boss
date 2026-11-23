#[path = "support/projects.rs"]
mod projects;
use serde_json::{Value, json};
use std::process::{Command, Output};

#[test]
fn saved_filters_follow_url_project_persist_replace_and_delete() {
    let temp = tempfile::Builder::new()
        .prefix(".sf-")
        .tempdir_in(".")
        .unwrap();
    let root = temp.path().canonicalize().unwrap();
    let db = root.join("issues.db");
    projects::seed(&db, &["named:Alpha", "named:Beta"]);
    // An existing database gains the new table without changing its projects.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE project_issue_filters")
        .unwrap();
    let run = |args: &[&str]| -> Output {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(["issue", "--json"])
            .args(args)
            .current_dir(&root)
            .env("HEY_BOSS_ISSUE_DB", &db)
            .env("HEY_BOSS_FLEET_STATE", &root)
            .env("HEY_BOSS_ISSUE_PROJECT", "named:Beta")
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .output()
            .unwrap()
    };
    let value = |args: &[&str]| -> Value {
        let output = run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let url = "http://hey-boss.test/#project=named%3AAlpha&state=all&states=ready,open,blocked,ready&label=push-blocked&owner=all&page=8&inbox_state=archive";
    let saved = value(&["filter", "save", "Push blocked", url]);
    assert_eq!(saved["project"]["id"], "named:Alpha");
    let filters = &saved["saved_filters"][0]["filters"];
    assert_eq!(filters["states"], "open,blocked,ready");
    assert_eq!(filters["label"], "push-blocked");
    for ignored in ["project", "host", "page", "inbox_state", "view"] {
        assert!(filters.get(ignored).is_none());
    }
    assert_eq!(value(&["filter", "list"])["saved_filters"], json!([]));
    assert_eq!(
        value(&["--project", "named:Alpha", "filter", "list"])["saved_filters"],
        saved["saved_filters"]
    );
    assert_eq!(
        value(&["--project", "named:Alpha", "list", "--state", "all"])["saved_filters"],
        saved["saved_filters"]
    );

    let invalid = [
        "http://hey-boss.test/#project=named%3AAlpha&issue=1",
        "http://hey-boss.test/#project=named%3AAlpha&view=inbox",
        "javascript:alert(1)",
    ];
    for url in invalid {
        assert!(
            !run(&["filter", "save", "Push blocked", url])
                .status
                .success()
        );
    }
    assert!(
        !run(&[
            "--project",
            "named:Beta",
            "filter",
            "save",
            "Push blocked",
            url
        ])
        .status
        .success()
    );
    assert!(!run(&["filter", "save", "", url]).status.success());
    assert_eq!(
        value(&["--project", "named:Alpha", "filter", "list"])["saved_filters"],
        saved["saved_filters"]
    );

    let replaced = value(&[
        "filter",
        "save",
        "Push blocked",
        "http://hey-boss.test/#project=named%3AAlpha&state=blocked&blocked=hold&search=needs+review&min_launches=5",
    ]);
    assert_eq!(replaced["saved_filters"].as_array().unwrap().len(), 1);
    assert_eq!(
        replaced["saved_filters"][0]["filters"]["search"],
        "needs review"
    );
    assert_eq!(replaced["saved_filters"][0]["filters"]["blocked"], "hold");
    assert_eq!(replaced["saved_filters"][0]["filters"]["states"], "");
    assert_eq!(replaced["saved_filters"][0]["filters"]["label"], "");
    value(&[
        "filter",
        "save",
        "Push blocked",
        "http://hey-boss.test/#project=named%3ABeta&state=ready",
    ]);
    assert_eq!(
        value(&[
            "--project",
            "named:Alpha",
            "filter",
            "delete",
            "Push blocked"
        ])["saved_filters"],
        json!([])
    );
    assert_eq!(
        value(&["--project", "named:Alpha", "filter", "list"])["saved_filters"],
        json!([])
    );
    assert_eq!(
        value(&["filter", "list"])["saved_filters"][0]["name"],
        "Push blocked"
    );
}
