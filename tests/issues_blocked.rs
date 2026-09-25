use serde_json::{Value, json};
use std::process::Command;

#[test]
fn blocked_lifecycle_preserves_history_and_requires_reopening() {
    let root = std::env::temp_dir().join(format!(
        "hey-boss-blocked-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let run = |agent: &str, args: &[&str], code: i32| -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&root)
            .env("HEY_BOSS_ISSUE_DB", root.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args([
                "issue",
                "--project",
                "Blocked QA",
                "--json",
                "--agent",
                agent,
            ])
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    run("owner", &["create", "--title", "Needs help"], 0);
    run("owner", &["claim", "1"], 0);
    run(
        "other",
        &["block", "1", "--comment", "External dependency"],
        4,
    );
    let blocked = run(
        "owner",
        &[
            "block",
            "1",
            "--comment",
            "External dependency",
            "--request-id",
            "block-once",
        ],
        0,
    );
    assert_eq!(blocked["issue"]["state"], "blocked");
    assert_eq!(blocked["issue"]["assignee"], Value::Null);
    assert_eq!(blocked["issue"]["closed_at"], Value::Null);
    assert_eq!(
        run(
            "owner",
            &[
                "block",
                "1",
                "--comment",
                "External dependency",
                "--request-id",
                "block-once"
            ],
            0
        ),
        blocked
    );
    assert_eq!(
        run("owner", &["list", "--state", "blocked"], 0)["issues"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(run("owner", &["list"], 0)["issues"], json!([]));
    assert_eq!(run("owner", &["projects"], 0)["projects"][0]["blocked"], 1);
    run("owner", &["claim", "1"], 4);
    run("owner", &["reopen", "1", "--if-version", "1"], 4);
    run("owner", &["delete", "1"], 0);
    assert_eq!(
        run("owner", &["restore", "1"], 0)["issue"]["state"],
        "blocked"
    );
    let reopened = run("owner", &["reopen", "1"], 0);
    assert_eq!(reopened["issue"]["state"], "open");
    let view = run("owner", &["view", "1"], 0);
    assert_eq!(view["comments"].as_array().unwrap().len(), 1);
    assert!(
        run("owner", &["history", "1"], 0)["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "blocked")
    );

    // Recreate the shipped v12 CHECK constraint in a closed synthetic store.
    // The real startup migration must preserve child references and triggers.
    run(
        "owner",
        &["subtask", "create", "1", "--title", "Dependency"],
        0,
    );
    {
        let db = rusqlite::Connection::open(root.join("issues.db")).unwrap();
        db.execute_batch("PRAGMA writable_schema=ON;
          UPDATE sqlite_master SET sql=replace(sql,'''open'',''blocked'',''ready'',''closed''','''open'',''closed''') WHERE name='issues';
          PRAGMA writable_schema=OFF; PRAGMA user_version=12;
          CREATE TABLE migration_audit(number INTEGER);
          CREATE TRIGGER migration_audit_insert AFTER INSERT ON issues BEGIN INSERT INTO migration_audit VALUES(NEW.number); END;").unwrap();
    }
    assert_eq!(
        run("owner", &["block", "2"], 0)["issue"]["state"],
        "blocked"
    );
    assert_eq!(
        run("owner", &["view", "1"], 0)["issue"]["subtasks"]["open_descendants"],
        1
    );
    run("owner", &["create", "--title", "After migration"], 0);
    {
        let db = rusqlite::Connection::open(root.join("issues.db")).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            15
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM migration_audit", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "Migration copies must not emit journal changes; custom triggers must survive"
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM issue_pickup_ready WHERE number=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "A blocked child prevents parent pickup"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
