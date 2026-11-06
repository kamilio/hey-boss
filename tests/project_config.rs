use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    store: Store,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::current_dir().unwrap().join(format!(
            ".config-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let store = Store::open(&root.join("issues.db")).unwrap();
        let mut f = Self { root, store };
        f.call(json!({"action":"project_init","settings":{"prs_enabled":false,"worktree_enabled":false,"if_version":0}}));
        f
    }
    fn request(&self, operation: Value) -> Request {
        serde_json::from_value(json!({"version":1,"project":{"id":"named:Config test","name":"Config test"},"actor":{"id":"human:test","kind":"human","machine":"test","host":"test","cwd":self.root,"source":"test"},"operation":operation})).unwrap()
    }
    fn call(&mut self, operation: Value) -> Value {
        self.store.execute(&self.request(operation)).unwrap()
    }
    fn config(&mut self, operation: Value) -> Value {
        self.call(json!({"action":"project_config","operation":operation}))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn settings_history_undo_and_restore_preserve_unrelated_edits() {
    let mut f = Fixture::new();
    f.call(json!({"action":"configure_project","prs_enabled":true}));
    let history = f.config(json!({"command":"history"}));
    let id = history["revisions"][0]["id"].clone();
    assert_eq!(history["revisions"][0]["actor"]["id"], "human:test");
    f.call(json!({"action":"configure_project","worktree_enabled":true}));
    let undone = f.config(json!({"command":"undo","revision":id}));
    assert_eq!(undone["prs_enabled"], false);
    assert_eq!(undone["worktree_enabled"], true);
    let history = f.config(json!({"command":"history"}));
    assert_eq!(history["revisions"][0]["action"], "undo");
    assert_eq!(history["revisions"][0]["target"], id);
    let restored = f.config(json!({"command":"restore","revision":id}));
    assert_eq!(restored["prs_enabled"], true);
    assert_eq!(restored["worktree_enabled"], false);
}

#[test]
fn unchanged_saves_do_not_create_history_and_failed_saves_are_atomic() {
    let mut f = Fixture::new();
    f.call(json!({"action":"configure_project","prs_enabled":true}));
    let before = f.config(json!({"command":"history"}));
    f.call(json!({"action":"configure_project","prs_enabled":true}));
    let request = f.request(json!({"action":"configure_project","paused":true,"pause_reason":""}));
    assert!(f.store.execute(&request).is_err());
    assert_eq!(f.config(json!({"command":"history"})), before);
}

#[test]
fn undo_preview_guard_prevents_reverting_unseen_changes() {
    let mut f = Fixture::new();
    f.call(json!({"action":"configure_project","prs_enabled":true}));
    let history = f.config(json!({"command":"history"}));
    let id = &history["revisions"][0]["id"];
    f.call(json!({"action":"configure_project","worktree_enabled":true}));
    let request = f.request(json!({"action":"project_config","operation":{"command":"undo","revision":id,"if_digest":history["digest"]}}));
    assert_eq!(f.store.execute(&request).unwrap_err().code, "conflict");
}

impl Fixture {
    fn sync(&self) -> hey_boss::issues::Result<Value> {
        let request = self.request(json!({"action":"project_settings"}));
        hey_boss::issues::config_files::sync(
            &self.root.join("issues.db"),
            &self.root,
            &request.project,
        )
    }
}

#[test]
fn file_sync_migrates_identity_preserves_inheritance_and_round_trips() {
    let mut f = Fixture::new();
    fs::write(
        f.root.join(".hey-boss"),
        r#"{"id":"named:Config test","name":"Config test"}"#,
    )
    .unwrap();
    f.sync().unwrap();
    assert!(f.root.join(".hey-boss/project.json").is_file());
    assert_eq!(
        hey_boss::issues::identity::project(&f.root, "test")
            .unwrap()
            .id,
        "named:Config test"
    );
    assert!(!f.root.join(".hey-boss.identity-backup").exists());
    let settings = f.call(json!({"action":"project_settings"}));
    assert_eq!(settings["prompt_overrides"], json!({}));
    let history = f.config(json!({"command":"history"}));
    f.sync().unwrap();
    assert_eq!(history, f.config(json!({"command":"history"})));
    let yaml_path = f.root.join(".hey-boss/config.yaml");
    let yaml = fs::read_to_string(&yaml_path).unwrap();
    fs::write(
        &yaml_path,
        yaml.replace("prs_enabled: false", "prs_enabled: true"),
    )
    .unwrap();
    f.sync().unwrap();
    assert_eq!(
        f.call(json!({"action":"project_settings"}))["prs_enabled"],
        true
    );
    f.call(json!({"action":"configure_project","worktree_enabled":true}));
    f.sync().unwrap();
    assert!(
        fs::read_to_string(yaml_path)
            .unwrap()
            .contains("worktree_enabled: true")
    );
}

#[test]
fn prompt_rename_preserves_local_edits_and_archives_retired_files() {
    let mut f = Fixture::new();
    f.sync().unwrap();
    let dir = f.root.join(".hey-boss/prompts");
    fs::remove_file(dir.join("delivery-main.md")).unwrap();
    fs::write(dir.join("main.md"), "Unsynced delivery").unwrap();
    let archived = [
        "layout.md",
        "resume.md",
        "handoff.md",
        "goal_continue.md",
        "prs.md",
    ];
    for file in archived {
        fs::write(dir.join(file), "Old custom text").unwrap();
    }
    f.sync().unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("delivery-main.md")).unwrap(),
        "Unsynced delivery"
    );
    assert!(!dir.join("main.md").exists());
    assert_eq!(
        f.call(json!({"action":"project_settings"}))["prompt_overrides"],
        json!({"main":"Unsynced delivery"})
    );
    for file in archived {
        assert!(!dir.join(file).exists());
        assert_eq!(
            fs::read_to_string(dir.parent().unwrap().join("retired-prompts").join(file)).unwrap(),
            "Old custom text"
        );
    }
    assert_eq!(f.sync().unwrap()["changed"], false);
}

#[test]
fn invalid_file_preserves_database_and_other_unsynced_files() {
    let mut f = Fixture::new();
    f.sync().unwrap();
    let history = f.config(json!({"command":"history"}));
    fs::write(f.root.join(".hey-boss/config.yaml"), "broken: [").unwrap();
    let prompt = f.root.join(".hey-boss/prompts/workspace-checkout.md");
    fs::write(
        &prompt,
        include_str!("../src/issues/prompts/workspace-checkout.md"),
    )
    .unwrap();
    assert!(f.sync().is_err());
    assert_eq!(history, f.config(json!({"command":"history"})));
    assert_eq!(
        fs::read_to_string(prompt).unwrap(),
        include_str!("../src/issues/prompts/workspace-checkout.md")
    );
}

#[test]
fn local_and_remote_edits_merge_and_deleted_override_returns_to_default() {
    let mut f = Fixture::new();
    f.sync().unwrap();
    let prompt = f.root.join(".hey-boss/prompts/workspace-checkout.md");
    let custom = include_str!("../src/issues/prompts/delivery-main.md");
    fs::write(&prompt, custom).unwrap();
    f.call(json!({"action":"configure_project","worktree_enabled":true}));
    f.sync().unwrap();
    let settings = f.call(json!({"action":"project_settings"}));
    assert_eq!(settings["worktree_enabled"], true);
    assert_eq!(settings["prompt_overrides"]["checkout"], custom);
    fs::remove_file(&prompt).unwrap();
    f.sync().unwrap();
    assert_eq!(
        f.call(json!({"action":"project_settings"}))["prompt_overrides"],
        json!({})
    );
    assert_eq!(
        fs::read_to_string(prompt).unwrap(),
        include_str!("../src/issues/prompts/workspace-checkout.md").trim_end()
    );
}

#[test]
fn symlinked_prompt_cannot_read_or_overwrite_an_external_file() {
    let f = Fixture::new();
    f.sync().unwrap();
    let outside = f.root.join("outside.md");
    fs::write(&outside, "unchanged").unwrap();
    let prompt = f.root.join(".hey-boss/prompts/workspace-checkout.md");
    fs::remove_file(&prompt).unwrap();
    std::os::unix::fs::symlink(&outside, &prompt).unwrap();
    assert!(f.sync().is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "unchanged");
}

#[test]
fn interrupted_materialization_recovers_without_importing_its_own_partial_write() {
    let mut f = Fixture::new();
    f.sync().unwrap();
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    let raw: String = db
        .query_row("SELECT base FROM project_config_checkouts", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut base: Value = serde_json::from_str(&raw).unwrap();
    let mut pending = base["files"].clone();
    pending["config.yaml"] = json!(
        pending["config.yaml"]
            .as_str()
            .unwrap()
            .replace("prs_enabled: false", "prs_enabled: true")
    );
    f.call(json!({"action":"configure_project","prs_enabled":true}));
    let history = f.config(json!({"command":"history"}));
    base["pending"] = pending;
    db.execute(
        "UPDATE project_config_checkouts SET base=?1",
        [base.to_string()],
    )
    .unwrap();
    f.sync().unwrap();
    assert!(
        fs::read_to_string(f.root.join(".hey-boss/config.yaml"))
            .unwrap()
            .contains("prs_enabled: true")
    );
    assert_eq!(history, f.config(json!({"command":"history"})));
}

#[test]
fn interrupted_identity_migration_keeps_the_custom_project_resolvable() {
    let f = Fixture::new();
    fs::create_dir(f.root.join(".hey-boss")).unwrap();
    fs::write(
        f.root.join(".hey-boss.identity-backup"),
        r#"{"id":"named:Config test","name":"Config test"}"#,
    )
    .unwrap();
    assert_eq!(
        hey_boss::issues::identity::project(&f.root, "test")
            .unwrap()
            .id,
        "named:Config test"
    );
    f.sync().unwrap();
    assert!(f.root.join(".hey-boss/project.json").is_file());
    assert!(!f.root.join(".hey-boss.identity-backup").exists());
}

#[test]
fn crash_after_database_commit_does_not_reapply_an_old_file_edit() {
    let mut f = Fixture::new();
    f.sync().unwrap();
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    let old_base: String = db
        .query_row("SELECT base FROM project_config_checkouts", [], |r| {
            r.get(0)
        })
        .unwrap();
    let yaml = f.root.join(".hey-boss/config.yaml");
    let edit = fs::read_to_string(&yaml)
        .unwrap()
        .replace("prs_enabled: false", "prs_enabled: true");
    fs::write(&yaml, &edit).unwrap();
    f.sync().unwrap();
    f.call(json!({"action":"configure_project","prs_enabled":false}));
    let history = f.config(json!({"command":"history"}));
    // Simulate dying after the issue transaction committed, before saving the
    // new working-copy baseline. Another editor has since changed the setting.
    db.execute("UPDATE project_config_checkouts SET base=?1", [old_base])
        .unwrap();
    fs::write(&yaml, edit).unwrap();
    f.sync().unwrap();
    assert_eq!(f.config(json!({"command":"history"})), history);
    assert!(
        fs::read_to_string(yaml)
            .unwrap()
            .contains("prs_enabled: false")
    );
}
