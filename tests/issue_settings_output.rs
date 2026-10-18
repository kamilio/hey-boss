#[path = "support/projects.rs"]
mod projects;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf, hey_boss::database::Owner);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hey-boss-settings-output-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        projects::seed(&path.join("issues.db"), &["named:Settings QA"]);
        let owner = hey_boss::database::Owner::start(&path.join("issues.db"))
            .unwrap()
            .unwrap();
        Self(path, owner)
    }

    fn text(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args(["issue", "--project", "Settings QA", "--agent", "human:qa"])
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.1.stop();
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn claims_show_iteration_count_before_instructions_without_counting_claims() {
    let f = Fixture::new();
    f.text(&["create", "--title", "Iteration history"]);
    let db = rusqlite::Connection::open(f.0.join("issues.db")).unwrap();
    for count in 0..=2 {
        if count > 0 {
            db.execute(
                "INSERT INTO issue_agent_launches(run_id,project_id,issue_number,launched_at) VALUES(?1,'named:Settings QA',1,1)",
                [format!("run-{count}")],
            )
            .unwrap();
        }
        for _ in 0..2 {
            let output = f.text(&["claim", "1"]);
            let label = format!("Iterations started: {count} (agent launches)\n");
            let count_position = output.find(&label).expect(&output);
            assert!(count_position < output.find("Instructions:").unwrap());
            let json: Value = serde_json::from_str(&f.text(&["claim", "1", "--json"])).unwrap();
            assert_eq!(json["issue"]["agent_launch_count"], count);
        }
    }
    assert!(!f.text(&["view", "1"]).contains("Iterations started:"));
}

#[test]
fn remote_running_attempt_is_visible_in_text_without_inventing_a_local_claim() {
    let f = Fixture::new();
    f.text(&["create", "--title", "Remote work"]);
    let db = rusqlite::Connection::open(f.0.join("issues.db")).unwrap();
    db.execute_batch("CREATE TABLE distributed_attempts(id TEXT PRIMARY KEY,machine TEXT,project_id TEXT,issue_number INTEGER,state TEXT,started_at INTEGER,finished_at INTEGER,summary TEXT,payload TEXT);
        INSERT INTO agents VALUES('codex:remote','{\"machine\":\"remote\",\"host\":\"devbox\"}',0);
        INSERT INTO distributed_attempts VALUES('working','remote','named:Settings QA',1,'running',2,NULL,'','{\"actor_id\":\"codex:remote\"}');").unwrap();
    for args in [vec!["list"], vec!["view", "1"]] {
        let output = f.text(&args);
        assert!(
            output.contains("working on devbox (codex:remote)"),
            "{output}"
        );
        assert!(!output.contains(" · unassigned · "), "{output}");
    }
    let json: Value = serde_json::from_str(&f.text(&["view", "1", "--json"])).unwrap();
    assert!(json["issue"]["assignee"].is_null());
    assert_eq!(json["issue"]["remote_attempts"][0]["id"], "working");
    db.execute("UPDATE issues SET state='closed'", []).unwrap();
    assert!(!f.text(&["view", "1"]).contains("working on devbox"));
    db.execute(
        "UPDATE issues SET state='open',assignment_target='machine:remote'",
        [],
    )
    .unwrap();
    assert!(
        f.text(&["list"])
            .contains("working on devbox (codex:remote)")
    );
    db.execute(
        "UPDATE distributed_attempts SET finished_at=3,state='completed'",
        [],
    )
    .unwrap();
    assert!(f.text(&["list"]).contains(" · assigned to remote · "));
    db.execute("UPDATE issues SET assignment_target=NULL", [])
        .unwrap();
    assert!(f.text(&["list"]).contains(" · unassigned · "));
}

#[test]
fn ordinary_issue_output_omits_absent_and_disabled_settings() {
    let f = Fixture::new();
    for args in [
        vec!["create", "--title", "Readable issue"],
        vec!["claim", "1"],
        vec!["unassign", "1"],
        vec!["view", "1"],
        vec!["list"],
    ] {
        let output = f.text(&args);
        assert!(output.starts_with("Settings QA (named:Settings QA)\n"));
        assert!(output.contains("#1 [open] Readable issue"));
        for label in [
            "Worktrees",
            "PRs enabled:",
            "Chief enabled:",
            "Chief prompt:",
            "Shared prompt:",
        ] {
            assert!(!output.contains(label), "{args:?}: {output}");
        }
    }
    f.text(&["settings", "set", "--prs-enabled", "--worktree"]);
    let output = f.text(&["unassign", "1"]);
    assert!(output.starts_with("Settings QA (named:Settings QA)\nPRs enabled: true\n#1"));
    assert!(!output.contains("Worktrees"));
    assert!(!output.contains("Chief"));
    let json: Value = serde_json::from_str(&f.text(&["view", "1", "--json"])).unwrap();
    assert_eq!(json["prs_enabled"], true);
    assert!(json.get("worktree_enabled").is_none());
}

#[test]
fn settings_output_distinguishes_worktree_permission_and_preserves_prompts_and_json() {
    let f = Fixture::new();
    let output = f.text(&[
        "settings",
        "set",
        "--worktree",
        "--prs-enabled",
        "--prompt",
        "Shared instructions.",
    ]);
    assert!(
        output.contains("Worktrees allowed: true\nPRs enabled: true\n"),
        "{output}"
    );
    assert!(output.contains("Shared prompt: Shared instructions.\n"));
    let output = f.text(&["settings", "set", "--no-worktree", "--no-prs"]);
    for label in ["Worktrees", "PRs enabled:", "Chief enabled:"] {
        assert!(!output.contains(label), "{output}");
    }
    let json: Value = serde_json::from_str(&f.text(&["settings", "show", "--json"])).unwrap();
    for key in ["worktree_enabled", "prs_enabled"] {
        assert_eq!(json[key], false, "{key}");
    }
    assert!(json.get("chief_prompt").is_none());
    assert!(json.get("chief_enabled").is_none());
    assert_eq!(json["prompt"], "Shared instructions.");
}
