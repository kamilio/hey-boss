use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs, path::PathBuf, process::Command};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hey-boss-status-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn run(&self, actor: &str, args: &[&str], exit: i32) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args([
                "issue",
                "--project",
                "Status QA",
                "--agent",
                actor,
                "--json",
            ])
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn create(&self) {
        self.run("owner", &["create", "--title", "Ship status"], 0);
    }
    fn history(&self, offset: u32, before: Option<i64>) -> Value {
        use std::io::Write;
        use std::process::Stdio;
        let mut operation =
            serde_json::json!({"action":"status_history","number":1,"limit":2,"offset":offset});
        if let Some(before) = before {
            operation["before"] = before.into();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args(["issue", "rpc"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                serde_json::json!({
                    "version":1,"project":{"id":"named:Status QA","name":"Status QA"},
                    "project_override":null,"actor":null,"operation":operation,"request_id":null
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success());
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["ok"], true, "{value}");
        value
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn history_pages_keep_their_snapshot_when_new_updates_arrive() {
    let f = Fixture::new();
    f.create();
    f.run("owner", &["claim", "1"], 0);
    for step in 0..5 {
        f.run(
            "owner",
            &[
                "status",
                "1",
                "green",
                "--comment",
                &format!("Step {step}."),
            ],
            0,
        );
    }
    let first = f.history(0, None);
    let snapshot = first["snapshot_at"]
        .as_i64()
        .expect("History must return its reading boundary");
    for step in 0..3 {
        f.run(
            "owner",
            &[
                "status",
                "1",
                "orange",
                "--comment",
                &format!("New step {step}."),
            ],
            0,
        );
    }
    let second = f.history(2, Some(snapshot));
    let third = f.history(4, Some(snapshot));
    let comments: Vec<_> = [&first, &second, &third]
        .into_iter()
        .flat_map(|page| page["updates"].as_array().unwrap())
        .map(|update| update["comment"].as_str().unwrap())
        .collect();
    assert_eq!(
        comments,
        ["Step 4.", "Step 3.", "Step 2.", "Step 1.", "Step 0."]
    );
    assert!(third["next_offset"].is_null());
    assert_eq!(second["snapshot_at"], snapshot);
    assert_eq!(f.history(0, None)["updates"][0]["comment"], "New step 2.");
}

#[test]
fn status_warns_without_requiring_ownership_and_preserves_comments_and_revision() {
    let f = Fixture::new();
    f.create();
    assert!(f.run("owner", &["view", "1"], 0)["issue"]["status"].is_null());
    let unassigned = f.run(
        "owner",
        &["status", "1", "green", "--comment", "Starting the work."],
        0,
    );
    assert!(
        unassigned["ownership_warning"]
            .as_str()
            .unwrap()
            .contains("unassigned")
    );
    let claim = f.run("owner", &["claim", "1"], 0);
    let other = f.run("other", &["status", "1", "red", "--comment", "Trouble."], 0);
    assert!(
        other["ownership_warning"]
            .as_str()
            .unwrap()
            .contains("owner")
    );
    assert_eq!(other["assignee_agent"]["id"], "owner");
    assert!(other["assignee_presence"].is_string());
    assert_eq!(other["issue"]["assignee"], "owner");
    assert_eq!(other["issue"]["status"]["author"], "other");
    let args = [
        "status",
        "1",
        "green",
        "--comment",
        "  The fix is ready for testing.  ",
        "--request-id",
        "status-one",
    ];
    let first = f.run("owner", &args, 0);
    assert!(first["ownership_warning"].is_null());
    assert_eq!(first["issue"]["status"]["level"], "green");
    assert_eq!(
        first["issue"]["status"]["comment"],
        "The fix is ready for testing."
    );
    assert_eq!(first["issue"]["version"], claim["issue"]["version"]);
    assert_eq!(first["issue"]["updated_at"], claim["issue"]["updated_at"]);
    assert_eq!(f.run("owner", &args, 0), first);
    f.run(
        "owner",
        &[
            "status",
            "1",
            "red",
            "--comment",
            "Different content.",
            "--request-id",
            "status-one",
        ],
        4,
    );
    f.run(
        "owner",
        &[
            "status",
            "1",
            "orange",
            "--comment",
            "One test still fails.",
        ],
        0,
    );
    f.run(
        "owner",
        &["status", "1", "red", "--comment", "The device is offline."],
        0,
    );
    let view = f.run("owner", &["view", "1"], 0);
    assert_eq!(view["issue"]["status"]["level"], "red");
    assert!(view["comments"].as_array().unwrap().is_empty());
    assert_eq!(
        f.run("owner", &["list"], 0)["issues"][0]["status"],
        view["issue"]["status"]
    );
    let history = f.run("owner", &["status-history", "1", "--limit", "2"], 0);
    assert_eq!(history["updates"].as_array().unwrap().len(), 2);
    assert_eq!(history["updates"][0]["level"], "red");
    assert_eq!(history["updates"][1]["level"], "orange");
    assert_eq!(history["next_offset"], 2);
    assert_eq!(
        f.run("owner", &["status-history", "1", "--offset", "2"], 0)["updates"][0]["level"],
        "green"
    );
    f.run("owner", &["close", "1"], 0);
    f.run(
        "owner",
        &["status", "1", "green", "--comment", "Late update."],
        4,
    );
    assert_eq!(
        f.run("owner", &["view", "1"], 0)["issue"]["status"],
        view["issue"]["status"]
    );
}

#[test]
fn forced_takeover_can_publish_status_without_changing_fleet_allocation() {
    let f = Fixture::new();
    f.create();
    f.run("owner", &["claim", "1"], 0);
    let db = rusqlite::Connection::open(f.0.join("issues.db")).unwrap();
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Status QA',1,'another-machine')",
        [],
    )
    .unwrap();
    f.run("other", &["claim", "1"], 4);
    let claim = f.run("other", &["claim", "1", "--force"], 0);
    let update = f.run(
        "other",
        &["status", "1", "green", "--comment", "Takeover is working."],
        0,
    );
    assert!(update["ownership_warning"].is_null());
    assert_eq!(update["issue"]["assignee"], "other");
    assert_eq!(update["issue"]["version"], claim["issue"]["version"]);
    assert_eq!(
        db.query_row("SELECT node FROM fleet_allocations", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "another-machine"
    );
    f.run("owner", &["claim", "1"], 4);
}

#[test]
fn comments_warn_about_other_owners_and_status_still_rejects_drafts() {
    let f = Fixture::new();
    f.create();
    f.run("owner", &["claim", "1"], 0);
    let comment = f.run("other", &["comment", "1", "--body", "Useful finding."], 0);
    assert!(
        comment["ownership_warning"]
            .as_str()
            .unwrap()
            .contains("owner")
    );
    assert_eq!(comment["issue"]["assignee"], "owner");
    f.run("other", &["create", "--title", "Draft", "--draft"], 0);
    f.run(
        "other",
        &["status", "2", "green", "--comment", "Draft update."],
        4,
    );
}

#[test]
fn status_validates_short_plain_text_and_keeps_history_through_restore() {
    let f = Fixture::new();
    f.create();
    f.run("owner", &["claim", "1"], 0);
    for text in [
        "",
        "   ",
        "two\nlines",
        "two\u{2028}lines",
        "two\u{2029}paragraphs",
        "bad\u{0000}text",
    ] {
        if text.contains('\0') {
            continue;
        } // argv cannot contain NUL.
        f.run("owner", &["status", "1", "green", "--comment", text], 2);
    }
    f.run(
        "owner",
        &["status", "1", "green", "--comment", &"a".repeat(501)],
        2,
    );
    f.run(
        "owner",
        &["status", "1", "green", "--comment", "Checking the layout."],
        0,
    );
    f.run("owner", &["delete", "1"], 0);
    f.run("owner", &["status", "1", "red", "--comment", "Deleted."], 3);
    f.run("owner", &["restore", "1"], 0);
    assert_eq!(
        f.run("owner", &["status-history", "1"], 0)["updates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
