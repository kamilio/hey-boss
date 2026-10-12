//! Upgrade compatibility for the scheduler replaced by Jobs.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-retired-chief-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        drop(hey_boss::issues::Store::open(&root.join("issues.db")).unwrap());
        Self(root)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST");
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn retired_chief_commands_are_not_advertised_or_accepted() {
    let fixture = Fixture::new();
    for args in [
        vec!["worker", "--help"],
        vec!["issue", "settings", "set", "--help"],
    ] {
        let result = fixture.command().args(args).output().unwrap();
        assert!(result.status.success());
        assert!(
            !String::from_utf8_lossy(&result.stdout)
                .to_lowercase()
                .contains("chief")
        );
    }
    for args in [
        vec!["worker", "run-chief"],
        vec!["worker", "run", "--chief"],
        vec!["issue", "settings", "set", "--chief"],
        vec!["issue", "settings", "set", "--chief-prompt", "legacy"],
    ] {
        assert!(
            !fixture
                .command()
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}

#[test]
fn upgrade_keeps_history_but_fences_old_schedulers_on_every_role() {
    let fixture = Fixture::new();
    let path = fixture.0.join("issues.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("DROP TRIGGER chief_retired_insert; DROP TRIGGER chief_retired_update;
        INSERT INTO projects(id,name,next_number) VALUES('named:Legacy','Legacy',1);
        INSERT INTO project_settings(project_id,prompt,version,chief_enabled,chief_prompt) VALUES('named:Legacy','Existing instructions',1,1,'Saved instructions');
        INSERT INTO project_chiefs(project_id,machine,cwd,session_id,state,summary) VALUES('named:Legacy','local','','saved-session','idle','Saved result');").unwrap();
    drop(hey_boss::issues::Store::open(&path).unwrap());
    for role in ["standalone", "controller", "agent"] {
        db.execute("UPDATE fleet_meta SET role=?1", [role]).unwrap();
        let error = db
            .execute("UPDATE project_chiefs SET state='running'", [])
            .unwrap_err();
        assert!(error.to_string().contains("retired"), "{error}");
        let error = db.execute("INSERT INTO project_chiefs(project_id,machine,cwd,state) VALUES('named:Legacy','other','','running')", []).unwrap_err();
        assert!(error.to_string().contains("retired"), "{error}");
    }
    db.execute("UPDATE fleet_meta SET role='standalone'", [])
        .unwrap();
    let saved: (String, String) = db
        .query_row("SELECT session_id,summary FROM project_chiefs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(saved, ("saved-session".into(), "Saved result".into()));
    let output = fixture
        .command()
        .args(["issue", "--project", "Legacy", "settings", "show", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let settings: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(settings.get("chief_enabled").is_none());
    assert!(settings.get("chief_prompt").is_none());
    assert_eq!(settings["prompt"], "Existing instructions");
    let result = fixture
        .command()
        .args([
            "issue",
            "--project",
            "Legacy",
            "settings",
            "set",
            "--no-worktree",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !db.query_row("SELECT chief_enabled FROM project_settings", [], |r| r
            .get::<_, bool>(0))
            .unwrap()
    );
    assert_eq!(
        db.query_row("SELECT chief_prompt FROM project_settings", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Saved instructions"
    );
}
