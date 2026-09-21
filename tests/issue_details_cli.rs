use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("hey-boss-details-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .env_remove("CODEX_THREAD_ID")
            .args(args)
            .output()
            .unwrap()
    }
    fn text(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn details_show_latest_status_comment_counts_and_artifact_actions() {
    let f = Fixture::new();
    f.text(&[
        "issue",
        "--project",
        "Details",
        "--agent",
        "session-a",
        "create",
        "--title",
        "Readable details",
        "--label",
        "cli",
    ]);
    f.text(&[
        "issue",
        "--project",
        "Details",
        "--agent",
        "session-a",
        "claim",
        "1",
    ]);
    for comment in ["Old status", "Latest status"] {
        f.text(&[
            "issue",
            "--project",
            "Details",
            "--agent",
            "session-a",
            "status",
            "1",
            "green",
            "--comment",
            comment,
        ]);
    }
    f.text(&[
        "issue",
        "--project",
        "Details",
        "--agent",
        "session-a",
        "pr",
        "add",
        "1",
        "https://github.com/example/repo/pull/7",
    ]);
    let doc: Value = serde_json::from_str(&f.text(&[
        "artifact",
        "--project",
        "Details",
        "--agent",
        "session-a",
        "--json",
        "create",
        "--title",
        "Design notes",
        "--body",
        "# Private linked content\nExact Markdown.\n",
        "--issue",
        "1",
    ]))
    .unwrap();
    let id = doc["artifact"]["id"].as_str().unwrap();
    let details = f.text(&["issue", "--project", "Details", "view", "1"]);
    assert!(details.contains("Latest status"));
    assert!(!details.contains("Old status"));
    assert!(details.contains("Comments: 0 shown · 0 total"));
    assert!(details.contains("Labels: cli"));
    assert!(details.contains("https://github.com/example/repo/pull/7"));
    assert!(details.contains("Design notes"));
    assert!(!details.contains("Private linked content"));
    assert!(details.contains("hey-boss artifact view ID"));
    assert!(details.contains("hey-boss artifact export ID --output PATH.md"));
    assert!(details.contains("--project 'named:Details'"));
    assert!(details.find("Linked artifacts:").unwrap() < details.find("Comments:").unwrap());
    let path = f.0.join("notes.md");
    let args = [
        "artifact",
        "--project",
        "Details",
        "export",
        id,
        "--output",
        path.to_str().unwrap(),
    ];
    assert!(f.text(&args).is_empty());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# Private linked content\nExact Markdown.\n"
    );
    assert!(!f.run(&args).status.success());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# Private linked content\nExact Markdown.\n"
    );
    assert_eq!(
        f.text(&["artifact", "--project", "Details", "export", id]),
        fs::read_to_string(&path).unwrap()
    );
}
