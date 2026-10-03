use serde_json::{Value, json};
use std::{fs, process::Command};

#[test]
fn compact_detail_preserves_requirements_and_guards_without_hydrating_history() {
    let root = std::env::temp_dir().join(format!("hb-compact-detail-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("issues.db");
    drop(hey_boss::issues::Store::open(&path).unwrap());
    let mut owner = hey_boss::database::Owner::start(&path).unwrap().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&root)
            .env("HEY_BOSS_ISSUE_DB", &path)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args([
                "issue",
                "--project",
                "Detail",
                "--agent",
                "reader",
                "--json",
            ])
            .args(args)
            .output()
            .unwrap()
    };
    let ok = |args: &[&str]| -> Value {
        let out = run(args);
        assert!(
            out.status.success(),
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    ok(&[
        "create",
        "--title",
        "Requirements",
        "--body",
        "Full **requirements** 🦀",
    ]);
    ok(&["create", "--title", "Dependency"]);
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("UPDATE issues SET labels='[\"review\"]', blockers='[2]' WHERE number=1;
            INSERT INTO fleet_allocations VALUES('named:Detail',1,'another-machine');
            INSERT INTO issue_subtasks VALUES('named:Detail',1,2,0,'reader');
            INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose,status,error)
              VALUES('named:Detail',1,'https://github.com/example/repo/pull/1','reader',0,'prerequisite','unknown','offline');
            INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES
              (1,'named:Detail',1,'reader','First',1),(2,'named:Detail',1,'reader','Second',2),(3,'named:Detail',1,'reader','Latest',3);
            INSERT INTO events(project_id,issue_number,actor,action,created_at,data)
              VALUES('named:Detail',1,'reader','comment_resolved',4,'{\"comment_id\":3}');").unwrap();
        let huge = "provenance".repeat(200_000);
        db.execute("INSERT INTO issue_commits VALUES('named:Detail',1,?1,'example/repo','Commit','reader',0,?2)", rusqlite::params!["a".repeat(40), json!({"detail":huge}).to_string()]).unwrap();
        db.execute("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Detail',1,'reader','pr_attached',1,?1)", [json!({"url":"https://github.com/example/repo/pull/1","origin":{"detail":huge}}).to_string()]).unwrap();
    }
    let full = ok(&["view", "1"]);
    assert!(full.to_string().len() > 3_000_000);
    assert_eq!(full["comments"][2]["body"], "Latest");
    for route in [
        vec!["view", "1", "--compact"],
        vec!["view", "1", "--compact", "--supervisor"],
    ] {
        let mut args = route;
        args.extend(["--comments-limit", "2"]);
        let page = ok(&args);
        assert!(page.to_string().len() < 20_000);
        assert_eq!(page["projection"], "compact_detail");
        for key in [
            "body",
            "title",
            "state",
            "version",
            "labels",
            "blockers",
            "assignee",
            "closed_at",
            "deleted_at",
            "assignment_target",
        ] {
            assert_eq!(page["issue"][key], full["issue"][key], "{key}");
        }
        for key in ["allocation", "ready_guard", "subtasks"] {
            assert_eq!(page[key], full[key], "{key}");
        }
        assert!(page["issue"].get("commits").is_none());
        assert!(page["issue"]["pull_requests"][0].get("origin").is_none());
        assert_eq!(page["issue"]["pull_requests"][0]["purpose"], "prerequisite");
        assert_eq!(
            page["comments"],
            json!([full["comments"][1], full["comments"][2]])
        );
        assert_eq!(page["completeness"]["body"], true);
        assert_eq!(page["completeness"]["comments"]["complete"], false);
        assert_eq!(page["completeness"]["comments"]["next_offset"], 2);
        assert!(
            page["completeness"]["omitted"]
                .as_array()
                .unwrap()
                .contains(&json!("issue.commits"))
        );
    }
    let tail = ok(&[
        "view",
        "1",
        "--compact",
        "--supervisor",
        "--comments-offset",
        "2",
    ]);
    assert_eq!(tail["comments"][0]["body"], "First");
    assert_eq!(tail["comments"].as_array().unwrap().len(), 1);
    assert!(tail["completeness"]["comments"]["next_offset"].is_null());
    assert_eq!(tail["completeness"]["comments"]["complete"], false);
    let small = ok(&["view", "2", "--compact"]);
    assert_eq!(small["completeness"]["comments"]["complete"], true);
    for args in [
        vec!["view", "1", "--comments-limit", "1"],
        vec!["view", "1", "--compact", "--comments-limit", "0"],
        vec!["view", "1", "--compact", "--comments-limit", "101"],
    ] {
        assert!(!run(&args).status.success());
    }
    // Even an oversized comment and requirements must remain complete.
    let large_comment = "🦀".repeat(32_000);
    let large_body = "界".repeat(300_000);
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("UPDATE comments SET body=?1 WHERE id=3", [&large_comment])
            .unwrap();
        db.execute("UPDATE issues SET body=?1 WHERE number=1", [&large_body])
            .unwrap();
    }
    let large = ok(&["view", "1", "--compact"]);
    assert_eq!(large["issue"]["body"], large_body);
    assert_eq!(large["comments"].as_array().unwrap().len(), 1);
    assert_eq!(large["comments"][0]["body"], large_comment);
    assert_eq!(large["completeness"]["comments"]["next_offset"], 1);
    assert_eq!(
        large["completeness"]["comments"]["byte_target_exceeded"],
        true
    );
    let empty = ok(&["view", "1", "--compact", "--comments-offset", "99"]);
    assert!(empty["comments"].as_array().unwrap().is_empty());
    assert_eq!(empty["completeness"]["comments"]["complete"], false);
    owner.stop();
    fs::remove_dir_all(root).unwrap();
}
