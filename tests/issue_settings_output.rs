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
    f.text(&["settings", "set", "--prs-enabled", "--worktree", "--chief"]);
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
        "--chief",
        "--chief-prompt",
        "Organize.\nKeep Markdown.",
        "--prompt",
        "Shared instructions.",
    ]);
    assert!(
        output.contains("Worktrees allowed: true\nPRs enabled: true\nChief enabled: true\n"),
        "{output}"
    );
    assert!(output.contains(
        "Chief prompt: Organize.\nKeep Markdown.\nShared prompt: Shared instructions.\n"
    ));
    let output = f.text(&["settings", "set", "--no-worktree", "--no-prs", "--no-chief"]);
    for label in ["Worktrees", "PRs enabled:", "Chief enabled:"] {
        assert!(!output.contains(label), "{output}");
    }
    let json: Value = serde_json::from_str(&f.text(&["settings", "show", "--json"])).unwrap();
    for key in ["worktree_enabled", "prs_enabled", "chief_enabled"] {
        assert_eq!(json[key], false, "{key}");
    }
    assert_eq!(json["chief_prompt"], "Organize.\nKeep Markdown.");
    assert_eq!(json["prompt"], "Shared instructions.");
}
