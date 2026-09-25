use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn list_all_preserves_filters_and_queue_order_without_truncating() {
    let root = std::env::temp_dir().join(format!("hey-boss-list-all-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&root)
            .env("HEY_BOSS_ISSUE_DB", root.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .env_remove("HEY_BOSS_AGENT_ID")
            .env_remove("CODEX_THREAD_ID")
            .args(["issue", "--project", "List QA", "--agent", "qa", "--json"])
            .args(args)
            .output()
            .unwrap()
    };
    let ok = |args: &[&str]| -> Value {
        let output = run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    for _ in 0..105 {
        ok(&[
            "create",
            "--at-bottom",
            "--title",
            "Matching issue",
            "--label",
            "ready",
        ]);
    }
    ok(&["create", "--title", "Other issue"]);
    ok(&["claim", "1"]);
    ok(&["move", "105", "--before", "2"]);
    let page = ok(&[
        "list",
        "--label",
        "ready",
        "--unassigned",
        "--search",
        "Matching",
    ]);
    assert_eq!(page["issues"].as_array().unwrap().len(), 50);
    assert_eq!(page["next_offset"], 50);
    let list = ok(&[
        "list",
        "--all",
        "--state",
        "open",
        "--label",
        "ready",
        "--unassigned",
        "--search",
        "Matching",
    ]);
    assert_eq!(list["issues"].as_array().unwrap().len(), 104);
    assert_eq!(list["issues"][0]["number"], 105);
    assert_eq!(list["issues"][1]["number"], 2);
    assert!(list["next_offset"].is_null());
    let mine = ok(&["list", "--all", "--assignee", "qa"]);
    assert_eq!(mine["issues"].as_array().unwrap().len(), 1);
    assert_eq!(mine["issues"][0]["number"], 1);
    ok(&["claim", "2"]);
    ok(&["close", "2"]);
    let closed = ok(&["list", "--all", "--state", "closed"]);
    assert_eq!(closed["issues"].as_array().unwrap().len(), 1);
    assert_eq!(closed["issues"][0]["number"], 2);
    assert_eq!(
        ok(&["list", "--all", "--state", "all"])["issues"]
            .as_array()
            .unwrap()
            .len(),
        106
    );
    for flag in ["--limit", "--offset"] {
        assert!(!run(&["list", "--all", flag, "1"]).status.success());
    }
    fs::remove_dir_all(root).unwrap();
}
