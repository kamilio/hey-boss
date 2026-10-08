use hey_boss::issues::{Project, Store};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture {
    root: PathBuf,
    owner: Option<hey_boss::database::Owner>,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // SystemTime can repeat across concurrent tests, sharing rows and letting
        // one fixture delete another's database. Never reuse a fixture directory.
        let root = std::env::temp_dir().join(format!(
            "hb-registration-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).unwrap();
        let mut fixture = Self { root, owner: None };
        fs::create_dir(fixture.root.join("home")).unwrap();
        fs::create_dir(fixture.root.join("tmp")).unwrap();
        // Own and join the service here; CLI calls must not bootstrap detached
        // companions that can outlive their private database.
        fixture.owner = Some(
            hey_boss::database::Owner::start(&fixture.db())
                .unwrap()
                .expect("fresh fixture must own its database"),
        );
        fixture
    }
    fn db(&self) -> PathBuf {
        self.root.join("issues.db")
    }
    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        // Neither CLI nor Git may inherit the caller's project, fleet, agent,
        // home configuration, or injected Git settings.
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("HEY_BOSS_ISSUE_DB", self.db())
            .env("HEY_BOSS_FLEET_STATE", self.root.join("fleet"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(cwd);
        command
    }
    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_hey-boss"), cwd)
            .args(args)
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
        Connection::open_with_flags(self.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.owner.take());
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn fixture_owns_an_empty_database_and_stops_it_before_removing_state() {
    for unwind in [false, true] {
        let f = Fixture::new();
        let root = f.root.clone();
        let path = f.db();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _fixture = f;
            let connection = hey_boss::database::Connection::connect(&path)
                .expect("fixture must own its database before launching CLI commands");
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM projects", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            if unwind {
                panic!("exercise fixture cleanup after a failed assertion");
            }
        }));
        assert_eq!(result.is_err(), unwind);
        assert!(!root.exists(), "fixture directory survived teardown");
        assert!(hey_boss::database::Connection::connect(&root.join("issues.db")).is_err());
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
        let output = f.run(&f.root, &args);
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("hey-boss project init"),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(f.count(), 0, "{args:?} created a project");
    }
    assert!(
        f.json(&f.root, &["issue", "projects", "--json"])["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.count(), 0);
}

#[test]
fn commands_reuse_nearest_registered_parent_and_do_not_match_sibling_names() {
    let f = Fixture::new();
    let parent = f.root.join("parent");
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
    let alias = f.root.join("symlink");
    std::os::unix::fs::symlink(&child, &alias).unwrap();
    assert_eq!(
        f.json(&alias, &["issue", "list", "--json"])["project"],
        initialized["project"]
    );
    let sibling = f.root.join("unrelated/parent");
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
    let repo = f.root.join("repo");
    fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(
            f.command("git", &repo)
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
    let worktree = f.root.join("linked");
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
    let parent = f.root.join("registered");
    fs::create_dir(&parent).unwrap();
    let initialized = f.init(&parent);
    let project = initialized["project"]["id"].as_str().unwrap();
    for selector in [project, "registered"] {
        assert_eq!(
            f.json(&f.root, &["issue", "--project", selector, "list", "--json"])["project"],
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
    let parent = f.root.join("outer");
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
    let unrelated = f.root.join("outer-other");
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
        "actor":{"id":"human:qa","kind":"human","machine":"test","host":"test","cwd":f.root,"source":"test"},
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
        "actor":{"id":"human:qa","kind":"human","machine":"test","host":"test","cwd":f.root,"source":"test"},
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
