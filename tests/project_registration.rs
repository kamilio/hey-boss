use hey_boss::issues::{Project, Store};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-registration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn db(&self) -> PathBuf {
        self.0.join("issues.db")
    }
    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(cwd)
            .args(args)
            .env("HEY_BOSS_ISSUE_DB", self.db())
            .env("HEY_BOSS_FLEET_STATE", self.0.join("fleet"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .output()
            .unwrap()
    }
    fn json(&self, cwd: &Path, args: &[&str]) -> Value {
        let output = self.run(cwd, args);
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn init(&self, cwd: &Path) -> Value {
        self.json(
            cwd,
            &[
                "project",
                "init",
                "--yes",
                "--prs",
                "false",
                "--worktree",
                "false",
                "--json",
            ],
        )
    }
    fn count(&self) -> i64 {
        Connection::open(self.db())
            .unwrap()
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ordinary_commands_reject_unknown_projects_without_registering_them() {
    let f = Fixture::new();
    for args in [
        vec!["issue", "--agent", "human:qa", "list"],
        vec![
            "issue",
            "--agent",
            "human:qa",
            "create",
            "--title",
            "Must not be saved",
        ],
        vec![
            "issue",
            "--agent",
            "human:qa",
            "--project",
            "Typo",
            "settings",
            "set",
            "--no-prs",
        ],
        vec![
            "artifact",
            "--agent",
            "human:qa",
            "create",
            "--title",
            "Must not be saved",
            "--body",
            "Test",
        ],
        vec!["mm", "--agent", "human:qa", "add", "Must not be saved"],
        vec!["notif", "alert", "Test", "--title", "Must not be sent"],
    ] {
        let output = f.run(&f.0, &args);
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("hey-boss project init"),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(f.count(), 0, "{args:?} created a project");
    }
    assert!(
        f.json(&f.0, &["issue", "projects", "--json"])["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.count(), 0);
}

#[test]
fn commands_reuse_nearest_registered_parent_and_do_not_match_sibling_names() {
    let f = Fixture::new();
    let parent = f.0.join("parent");
    let child = parent.join("nested/deep");
    fs::create_dir_all(&child).unwrap();
    let initialized = f.init(&parent);
    let listed = f.json(&child, &["issue", "list", "--json"]);
    assert_eq!(listed["project"], initialized["project"]);
    let created = f.json(
        &child,
        &[
            "issue",
            "--agent",
            "human:qa",
            "create",
            "--title",
            "In parent",
            "--json",
        ],
    );
    assert_eq!(created["project"], initialized["project"]);
    let alias = f.0.join("symlink");
    std::os::unix::fs::symlink(&child, &alias).unwrap();
    assert_eq!(
        f.json(&alias, &["issue", "list", "--json"])["project"],
        initialized["project"]
    );
    let sibling = f.0.join("unrelated/parent");
    fs::create_dir_all(&sibling).unwrap();
    let rejected = f.run(&sibling, &["issue", "list"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("hey-boss project init"));
    assert_eq!(f.count(), 1);
}

#[test]
fn discovery_and_notifications_never_create_projects() {
    let f = Fixture::new();
    let mut store = Store::open(&f.db()).unwrap();
    let project = Project {
        id: "github.com/example/uninitialized".into(),
        name: "uninitialized".into(),
    };
    store.discover_projects(&[(project.clone(), 100)]).unwrap();
    assert_eq!(f.count(), 0);
    let error = store.notification_project(&project, None).unwrap_err();
    assert!(error.message.contains("hey-boss project init"));
    assert_eq!(f.count(), 0);
}

#[test]
fn git_subdirectories_and_worktrees_share_an_initialized_project() {
    let f = Fixture::new();
    let repo = f.0.join("repo");
    fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .current_dir(&repo)
                .args(["-c", "user.name=QA", "-c", "user.email=qa@example.invalid"])
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        )
    };
    git(&["init", "-q"]);
    git(&["commit", "--allow-empty", "-qm", "initial"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/registered.git",
    ]);
    let initialized = f.init(&repo);
    let child = repo.join("src/deep");
    fs::create_dir_all(&child).unwrap();
    let worktree = f.0.join("linked");
    git(&[
        "worktree",
        "add",
        "-qb",
        "topic",
        worktree.to_str().unwrap(),
    ]);
    for path in [&child, &worktree] {
        assert_eq!(
            f.json(path, &["issue", "list", "--json"])["project"],
            initialized["project"]
        );
    }
    assert_eq!(f.count(), 1);
}

#[test]
fn explicit_selection_overrides_directory_and_unknown_selectors_do_not_register() {
    let f = Fixture::new();
    let parent = f.0.join("registered");
    fs::create_dir(&parent).unwrap();
    let initialized = f.init(&parent);
    let project = initialized["project"]["id"].as_str().unwrap();
    for selector in [project, "registered"] {
        assert_eq!(
            f.json(&f.0, &["issue", "--project", selector, "list", "--json"])["project"],
            initialized["project"]
        );
    }
    let rejected = f.run(&parent, &["issue", "--project", "misspelled", "list"]);
    assert!(!rejected.status.success());
    let message = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        message.contains("--project <name-or-id>") && message.contains("hey-boss project init")
    );
    assert_eq!(f.count(), 1);
}

#[test]
fn parent_resolution_chooses_the_nearest_directory_on_the_same_machine() {
    let f = Fixture::new();
    let parent = f.0.join("outer");
    let inner = parent.join("inner");
    let nested = inner.join("deep");
    fs::create_dir_all(&nested).unwrap();
    // Set up the inner project first so each directory is independently registered.
    let inner_project = f.init(&inner);
    f.init(&parent);
    assert_eq!(
        f.json(&nested, &["issue", "list", "--json"])["project"],
        inner_project["project"]
    );
    let unrelated = f.0.join("outer-other");
    fs::create_dir(&unrelated).unwrap();
    assert!(!f.run(&unrelated, &["issue", "list"]).status.success());
    assert_eq!(f.count(), 2);
}

#[test]
fn global_worker_retries_and_receipts_do_not_require_or_create_a_project() {
    use hey_boss::issues::Request;
    use serde_json::json;
    let f = Fixture::new();
    let mut store = Store::open(&f.db()).unwrap();
    let mut request: Request = serde_json::from_value(json!({
        "version":1,"project":{"id":"named:Uninitialized","name":"Uninitialized"},
        "actor":{"id":"human:qa","kind":"human","machine":"test","host":"test","cwd":f.0,"source":"test"},
        "request_id":"worker-once",
        "operation":{"action":"configure_worker","worker_id":null,"config":{"enabled":false},"if_version":null}
    })).unwrap();
    let saved = store.execute(&request).unwrap();
    assert_eq!(store.execute(&request).unwrap(), saved);
    request.operation =
        serde_json::from_value(json!({"action":"request_status","id":"worker-once"})).unwrap();
    request.request_id = None;
    let receipt = store.execute(&request).unwrap();
    assert_eq!(receipt["request"]["response"], saved);
    assert_eq!(f.count(), 0);
}

#[test]
fn init_preview_and_failed_save_leave_no_registry_rows() {
    use hey_boss::issues::Request;
    use serde_json::json;
    let f = Fixture::new();
    let mut store = Store::open(&f.db()).unwrap();
    let mut request: Request = serde_json::from_value(json!({
        "version":1,"project":{"id":"named:Preview","name":"Preview"},
        "actor":{"id":"human:qa","kind":"human","machine":"test","host":"test","cwd":f.0,"source":"test"},
        "operation":{"action":"project_init","settings":null}
    })).unwrap();
    assert_eq!(store.execute(&request).unwrap()["version"], 0);
    assert_eq!(f.count(), 0);
    request.operation = serde_json::from_value(json!({"action":"project_init","settings":{"prs_enabled":false,"worktree_enabled":false,"if_version":1}})).unwrap();
    assert_eq!(store.execute(&request).unwrap_err().code, "conflict");
    assert_eq!(f.count(), 0);
}

#[test]
fn legacy_root_home_and_other_machine_rows_do_not_capture_unrelated_directories() {
    let f = Fixture::new();
    let machine = hey_boss::issues::identity::machine().unwrap();
    let child = Project {
        id: format!("local:{machine}:/Users/hb-fixture/unknown/nested"),
        name: "nested".into(),
    };
    let mut store = Store::open(&f.db()).unwrap();
    let db = Connection::open(f.db()).unwrap();
    for (id, name) in [
        (format!("local:{machine}:/"), "root"),
        (format!("local:{machine}:/Users/hb-fixture"), "home"),
        (
            "local:other-machine:/Users/hb-fixture/unknown".into(),
            "unknown",
        ),
    ] {
        db.execute(
            "INSERT INTO projects(id,name,next_number) VALUES(?1,?2,1)",
            [&id, name],
        )
        .unwrap();
    }
    assert_eq!(
        store.notification_project(&child, None).unwrap_err().code,
        "project_not_initialized"
    );
    assert_eq!(f.count(), 3);
}
