use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "hey-boss-cli-revisions-{}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn run(&self, group: &str, args: &[&str], actor: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .env_remove("CODEX_THREAD_ID")
            .args([
                group,
                "--project",
                "named:Revisions",
                "--agent",
                actor,
                "--json",
            ])
            .args(args)
            .output()
            .unwrap()
    }
    fn value(&self, group: &str, args: &[&str]) -> Value {
        let output = self.run(group, args, "session-a");
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn assignment_handoff_needs_no_revision_and_preserves_ownership() {
    let f = Fixture::new("assignment");
    f.value("issue", &["create", "--title", "PR work"]);
    f.value("issue", &["claim", "1"]);
    f.value(
        "issue",
        &["pr", "add", "1", "https://github.com/example/repo/pull/1"],
    );
    let rejected = f.run("issue", &["assign", "1", "github"], "session-b");
    assert_eq!(rejected.status.code(), Some(4));
    let handed = f.value("issue", &["assign", "1", "github"]);
    assert_eq!(handed["issue"]["assignment"]["kind"], "github");
}

#[test]
fn artifact_lifecycle_needs_no_revision() {
    let f = Fixture::new("artifacts");
    let created = f.value(
        "artifact",
        &["create", "--title", "Draft", "--body", "Before"],
    );
    let id = created["artifact"]["id"].as_str().unwrap();
    let edited = f.value("artifact", &["edit", id, "--body", "After"]);
    assert_eq!(edited["artifact"]["body"], "After");
    f.value("artifact", &["archive", id]);
    f.value("artifact", &["restore", id]);
    assert_eq!(
        f.value("artifact", &["view", id])["artifact"]["archived"],
        false
    );
    f.value("artifact", &["delete", id]);
    assert_eq!(
        f.run("artifact", &["view", id], "session-a").status.code(),
        Some(3)
    );
}

#[test]
fn supervisor_metadata_and_batch_fill_their_own_guards() {
    let f = Fixture::new("metadata");
    f.value("issue", &["create", "--title", "Before"]);
    let edited = f.value("issue", &["edit", "1", "--title", "After", "--supervisor"]);
    assert_eq!(edited["issue"]["title"], "After");
    let batch = f.0.join("edits.json");
    fs::write(&batch, r#"[{"number":1,"add_labels":["reviewed"]}]"#).unwrap();
    f.value("issue", &["batch", "--file", batch.to_str().unwrap()]);
    assert_eq!(
        f.value("issue", &["view", "1"])["issue"]["labels"],
        serde_json::json!(["reviewed"])
    );
}

#[test]
fn command_help_does_not_expose_revision_flags() {
    for route in [
        vec!["issue"],
        vec!["issue", "edit"],
        vec!["issue", "assign"],
        vec!["issue", "ready"],
        vec!["issue", "transfer"],
        vec!["issue", "blocked-by"],
        vec!["issue", "reopen"],
        vec!["issue", "attempt", "hold"],
        vec!["issue", "attempt", "reconcile"],
        vec!["issue", "subtask", "create"],
        vec!["issue", "subtask", "add"],
        vec!["artifact", "edit"],
        vec!["artifact", "archive"],
        vec!["artifact", "restore"],
        vec!["artifact", "delete"],
        vec!["mm"],
        vec!["mm", "batch"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(&route)
            .arg("--help")
            .output()
            .unwrap();
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(!help.contains("--if-version"), "{route:?}: {help}");
    }
}

#[test]
fn ready_captures_snapshot_and_preserves_draft_scope() {
    let f = Fixture::new("ready");
    f.value(
        "issue",
        &["settings", "set", "--prs-enabled", "--drafts-enabled"],
    );
    f.value("issue", &["create", "--title", "Draft PR", "--draft"]);
    f.value(
        "issue",
        &["pr", "add", "1", "https://github.com/example/repo/pull/1"],
    );
    let ready = f.value("issue", &["ready", "1", "--keep-draft"]);
    assert_eq!(ready["issue"]["state"], "ready");
    assert_eq!(ready["issue"]["draft"], true);
    assert_eq!(ready["issue"]["assignee"], "human:boss");
}

#[test]
fn explicit_retry_returns_original_result_after_intervening_edits_or_deletion() {
    let f = Fixture::new("retry");
    f.value("issue", &["create", "--title", "Before"]);
    let args = [
        "edit",
        "1",
        "--title",
        "After",
        "--supervisor",
        "--request-id",
        "edit-once",
    ];
    let first = f.value("issue", &args);
    f.value("issue", &["edit", "1", "--title", "Later", "--supervisor"]);
    assert_eq!(f.value("issue", &args), first);
    assert_eq!(f.value("issue", &["view", "1"])["issue"]["title"], "Later");
    let changed = f.run(
        "issue",
        &[
            "edit",
            "1",
            "--title",
            "Different",
            "--supervisor",
            "--request-id",
            "edit-once",
        ],
        "session-a",
    );
    assert_eq!(changed.status.code(), Some(4));
    let artifact = f.value("artifact", &["create", "--title", "Note", "--body", "text"]);
    let id = artifact["artifact"]["id"].as_str().unwrap();
    let args = ["delete", id, "--request-id", "delete-once"];
    let first = f.value("artifact", &args);
    assert_eq!(f.value("artifact", &args), first);
}

#[test]
fn imported_artifact_retry_matches_content_hash_receipt() {
    let f = Fixture::new("import-retry");
    let created = f.value(
        "artifact",
        &["create", "--title", "Files", "--body", "Before"],
    );
    let id = created["artifact"]["id"].as_str().unwrap();
    fs::write(f.0.join("note.txt"), "Attached text").unwrap();
    fs::write(f.0.join("body.md"), "[Note](note.txt)").unwrap();
    let args = [
        "edit",
        id,
        "--file",
        "body.md",
        "--request-id",
        "import-once",
    ];
    let first = f.value("artifact", &args);
    assert_eq!(f.value("artifact", &args), first);
}
