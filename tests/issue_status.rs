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
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn status_is_owner_only_and_separate_from_comments_and_revision() {
    let f = Fixture::new();
    f.create();
    assert!(f.run("owner", &["view", "1"], 0)["issue"]["status"].is_null());
    f.run(
        "owner",
        &["status", "1", "green", "--comment", "Starting the work."],
        4,
    );
    let claim = f.run("owner", &["claim", "1"], 0);
    f.run("other", &["status", "1", "red", "--comment", "Trouble."], 4);
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
