use serde_json::{Value, json};
use std::{fs, process::Command};

#[test]
fn compact_reads_skip_history_preserve_health_and_paginate() {
    let root = std::env::temp_dir().join(format!("hb-compact-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let db_path = root.join("issues.db");
    drop(hey_boss::issues::Store::open(&db_path).unwrap());
    let mut owner = hey_boss::database::Owner::start(&db_path).unwrap().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&root)
            .env("HEY_BOSS_ISSUE_DB", &db_path)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
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
        "issue",
        "--project",
        "Compact",
        "--agent",
        "qa",
        "create",
        "--title",
        "Large history",
        "--json",
    ]);
    {
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) VALUES
          ('named:Compact',2,'Child','','open','qa',0,0,1,'[\"ready\"]',2),
          ('named:Compact',3,'Other','','closed','qa',0,0,1,'[]',3);
          INSERT INTO issue_subtasks VALUES('named:Compact',1,2,0,'qa');
          INSERT INTO fleet_allocations VALUES('named:Compact',1,'other-machine');
          UPDATE fleet_allocation_deadlines SET expires_at=0 WHERE issue_number=1;
          UPDATE issues SET blockers='[3]',assignee='qa' WHERE number=2;
          INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose,status,error) VALUES('named:Compact',1,'https://github.com/example/repo/pull/1','qa',0,'fix','unknown','offline');").unwrap();
        let huge = "history".repeat(200_000);
        db.execute("INSERT INTO issue_commits VALUES('named:Compact',1,?1,'example/repo','Commit','qa',0,?2)", rusqlite::params!["a".repeat(40), json!({"detail":huge}).to_string()]).unwrap();
        db.execute("UPDATE issues SET body=?1 WHERE number=1", [&huge])
            .unwrap();
    }
    let full = ok(&[
        "issue",
        "--project",
        "Compact",
        "list",
        "--state",
        "all",
        "--json",
    ]);
    assert!(full.to_string().len() > 1_000_000);
    let mut numbers = Vec::new();
    for offset in 0..3 {
        let page = ok(&[
            "issue",
            "--project",
            "Compact",
            "list",
            "--state",
            "all",
            "--compact",
            "--limit",
            "1",
            "--offset",
            &offset.to_string(),
            "--json",
        ]);
        assert_eq!(page["projection"], "compact");
        assert!(page.to_string().len() < 12_000);
        assert_eq!(page["issues"].as_array().unwrap().len(), 1);
        let issue = &page["issues"][0];
        assert!(issue.get("commits").is_none());
        assert!(issue.get("body").is_none());
        assert!(issue["allocation"].is_object());
        numbers.push(issue["number"].as_i64().unwrap());
        assert_eq!(
            page["next_offset"],
            if offset < 2 {
                json!(offset + 1)
            } else {
                Value::Null
            }
        );
        if issue["number"] == 1 {
            assert_eq!(issue["allocation"]["reason"], "allocation_expired");
            assert_eq!(issue["pull_requests"][0]["purpose"], "fix");
            assert_eq!(issue["pull_requests"][0]["error"], "offline");
        }
        if issue["number"] == 2 {
            assert_eq!(issue["parent_number"], 1);
            assert_eq!(issue["blocker_numbers"], json!([3]));
        }
    }
    assert_eq!(numbers, vec![1, 2, 3]);
    let filtered = ok(&[
        "issue",
        "--project",
        "Compact",
        "list",
        "--compact",
        "--label",
        "ready",
        "--assignee",
        "qa",
        "--json",
    ]);
    assert_eq!(filtered["issues"].as_array().unwrap().len(), 1);
    for number in [1, 2, 3] {
        ok(&[
            "mm",
            "--project",
            "Compact",
            "--agent",
            "qa",
            "issue",
            &number.to_string(),
            "--id",
            &format!("issue-{number}"),
            "--json",
        ]);
    }
    ok(&[
        "mm",
        "--project",
        "Compact",
        "--agent",
        "qa",
        "link",
        "issue-1",
        "issue-2",
        "--kind",
        "depends-on",
        "--json",
    ]);
    let mut ids = std::collections::BTreeSet::new();
    for offset in 0..3 {
        let page = ok(&[
            "mm",
            "--project",
            "Compact",
            "show",
            "--compact",
            "--limit",
            "1",
            "--offset",
            &offset.to_string(),
            "--json",
        ]);
        assert_eq!(page["projection"], "compact");
        assert_eq!(page["total"], 3);
        assert_eq!(page["nodes"].as_array().unwrap().len(), 1);
        assert!(page.to_string().len() < 12_000);
        let node = &page["nodes"][0];
        assert!(node.get("body").is_none());
        assert!(node["issue"]["allocation"].is_object());
        assert!(ids.insert(node["id"].as_str().unwrap().to_owned()));
        assert_eq!(
            page["next_offset"],
            if offset < 2 {
                json!(offset + 1)
            } else {
                Value::Null
            }
        );
    }
    for args in [
        vec!["issue", "list", "--compact"],
        vec!["mm", "show", "--compact"],
        vec!["mm", "show", "--limit", "1", "--json"],
        vec!["mm", "show", "--compact", "--bodies", "full", "--json"],
    ] {
        assert!(!run(&args).status.success());
    }
    // Missing resources remain explicit even when the saved map still refers to them.
    {
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch("UPDATE issues SET deleted_at=1 WHERE number=3;")
            .unwrap();
    }
    let missing = ok(&["mm", "--project", "Compact", "show", "--compact", "--json"]);
    let unavailable = missing["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["reference"] == "3")
        .unwrap();
    assert_eq!(unavailable["available"], false);
    assert_eq!(unavailable["resource_health"], "missing_or_deleted");
    ok(&[
        "issue",
        "--project",
        "Compact",
        "list",
        "--compact",
        "--all",
        "--json",
    ]);
    assert!(
        ok(&[
            "mm",
            "--project",
            "Compact",
            "show",
            "--compact",
            "--offset",
            "100",
            "--json"
        ])["nodes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    {
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute("UPDATE fleet_meta SET role='agent' WHERE id=1", [])
            .unwrap();
    }
    let replica = ok(&[
        "issue",
        "--project",
        "Compact",
        "list",
        "--compact",
        "--label",
        "ready",
        "--json",
    ]);
    assert_eq!(replica["health"]["authoritative"], false);
    assert_eq!(
        replica["issues"][0]["allocation"]["reason"],
        "allocation_missing"
    );
    owner.stop();
    fs::remove_dir_all(root).unwrap();
}
