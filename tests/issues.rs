use rusqlite::Connection;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    cwd: PathBuf,
    db: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("out")
            .join(format!(
                "issues-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
        let cwd = root.join("project");
        fs::create_dir_all(&cwd).unwrap();
        let db = root.join("state/issues.db");
        Self { root, cwd, db }
    }
    fn cmd(&self, agent: &str, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.current_dir(&self.cwd)
            .env("HEY_BOSS_ISSUE_DB", &self.db)
            .env("GIT_CEILING_DIRECTORIES", &self.root)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_AGENT_ID")
            .env_remove("CODEX_THREAD_ID")
            .args(["issue", "--json", "--agent", agent])
            .args(args);
        c
    }
    fn run(&self, agent: &str, args: &[&str]) -> Value {
        success(self.cmd(agent, args).output().unwrap())
    }
    fn fail(&self, agent: &str, args: &[&str], code: i32) -> Value {
        let o = self.cmd(agent, args).output().unwrap();
        assert_eq!(
            o.status.code(),
            Some(code),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn stdin(&self, args: &[&str], bytes: &[u8]) -> Output {
        let mut child = self
            .cmd("session-a", args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        child.wait_with_output().unwrap()
    }
    fn create(&self) -> Value {
        self.run(
            "session-a",
            &[
                "create",
                "--title",
                "Reconnect",
                "--body",
                "## Problem\nDrops after sleep.",
            ],
        )
    }
    fn sql(&self) -> Connection {
        Connection::open(&self.db).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn success(o: Output) -> Value {
    assert!(
        o.status.success(),
        "{} {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn projects_register_on_first_use_sort_by_activity_and_stay_hidden() {
    let f = Fixture::new();
    let a = f.run("session-a", &["list", "--project", "Alpha"])["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let b = f.run("session-a", &["list", "--project", "Beta"])["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.sql()
        .execute(
            "UPDATE projects SET activity_at=CASE WHEN id=?1 THEN 1000 ELSE 2000 END",
            [&a],
        )
        .unwrap();
    let listed = f.run("session-a", &["projects", "--project", "Alpha"]);
    assert_eq!(listed["projects"][0]["id"], b);
    assert_eq!(listed["projects"][1]["activity_at"], 1000);
    f.run("session-a", &["list", "--project", "Alpha"]);
    assert_eq!(
        f.run("session-a", &["projects", "--project", "Alpha"])["projects"],
        listed["projects"],
        "Reading never promotes a project"
    );
    f.run(
        "session-a",
        &[
            "create",
            "--project",
            "Alpha",
            "--title",
            "Keep this",
            "--body",
            "## Durable Markdown",
        ],
    );
    f.run("session-a", &["claim", "1", "--project", "Alpha"]);
    assert_eq!(
        f.run("session-a", &["projects", "--project", "Alpha"])["projects"][0]["id"],
        a
    );
    let hidden = f.run(
        "session-a",
        &[
            "hide-project",
            "--project",
            "Alpha",
            "--request-id",
            "hide-alpha",
        ],
    );
    assert_eq!(hidden["changed"], true);
    assert_eq!(
        f.run(
            "session-a",
            &[
                "hide-project",
                "--project",
                "Alpha",
                "--request-id",
                "hide-alpha"
            ]
        )["hidden_at"],
        hidden["hidden_at"]
    );
    assert_eq!(
        f.run("session-a", &["projects", "--project", "Alpha"])["projects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.run(
        "session-a",
        &[
            "comment",
            "1",
            "--project",
            "Alpha",
            "--body",
            "Still working",
        ],
    );
    let all = f.run("session-a", &["projects", "--all", "--project", "Alpha"]);
    assert_eq!(
        all["projects"][0]["hidden_at"], hidden["hidden_at"],
        "New activity must not undo hiding"
    );
    let mut store = hey_boss::issues::Store::open(&f.db).unwrap();
    store
        .discover_projects(&[(
            hey_boss::issues::Project {
                id: a.clone(),
                name: "Alpha".into(),
            },
            i64::MAX,
        )])
        .unwrap();
    drop(store);
    assert_eq!(
        f.run("session-a", &["projects", "--project", "Alpha"])["projects"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "Discovery must not resurrect hidden projects"
    );
    let issue = f.run("session-a", &["view", "1", "--project", "Alpha"]);
    assert_eq!(issue["issue"]["body"], "## Durable Markdown");
    assert_eq!(issue["issue"]["assignee"], "session-a");
    assert_eq!(issue["comments"].as_array().unwrap().len(), 1);
    f.run("session-a", &["restore-project", "--project", "Alpha"]);
    let restored = f.run("session-a", &["projects", "--project", "Alpha"]);
    assert_eq!(restored["projects"].as_array().unwrap().len(), 2);
    assert!(restored["projects"][0]["hidden_at"].is_null());
}

#[test]
fn project_metadata_migrates_old_databases_without_losing_history_or_numbers() {
    let f = Fixture::new();
    let created = f.create();
    f.run(
        "session-a",
        &["comment", "1", "--body", "Preserve this comment"],
    );
    let project = created["project"]["id"].as_str().unwrap();
    let updated = f.run("session-a", &["view", "1"])["issue"]["updated_at"]
        .as_i64()
        .unwrap();
    f.sql().execute_batch("DROP TABLE mindmap_links; DROP TABLE mindmap_nodes; DROP TABLE mindmaps; DROP VIEW issue_pickup_ready; DROP TABLE issue_subtasks; DROP INDEX worker_sort_order; DROP INDEX issue_sort_order; ALTER TABLE issues DROP COLUMN sort_order; ALTER TABLE projects DROP COLUMN issue_order_version; DROP TABLE issue_pull_requests; DROP TABLE project_settings; DROP TABLE issue_workers; DROP TABLE worker_events; DROP TABLE worker_runs; DROP TABLE project_workers; DROP TABLE worker_pool; DROP INDEX project_activity; ALTER TABLE projects DROP COLUMN created_at; ALTER TABLE projects DROP COLUMN activity_at; ALTER TABLE projects DROP COLUMN hidden_at; DROP TABLE global_settings_requests; DROP TABLE global_settings; PRAGMA user_version=1;").unwrap();
    let migrated = f.run("session-a", &["projects"]);
    assert_eq!(migrated["projects"][0]["id"], project);
    assert_eq!(migrated["projects"][0]["activity_at"], updated);
    assert!(migrated["projects"][0]["hidden_at"].is_null());
    assert_eq!(
        f.run("session-a", &["view", "1"])["comments"][0]["body"],
        "Preserve this comment"
    );
    assert_eq!(f.create()["issue"]["number"], 2);
    assert_eq!(
        f.sql()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
}

#[test]
fn worker_schema_migrates_version_two_preserving_hidden_projects_and_history() {
    let f = Fixture::new();
    let created = f.create();
    let id = created["project"]["id"].as_str().unwrap();
    f.run(
        "session-a",
        &["comment", "1", "--body", "Preserve this Markdown"],
    );
    f.run("session-a", &["hide-project"]);
    f.sql().execute_batch("DROP TABLE mindmap_links; DROP TABLE mindmap_nodes; DROP TABLE mindmaps; DROP VIEW issue_pickup_ready; DROP TABLE issue_subtasks; DROP INDEX worker_sort_order; DROP INDEX issue_sort_order; ALTER TABLE issues DROP COLUMN sort_order; ALTER TABLE projects DROP COLUMN issue_order_version; DROP TABLE issue_pull_requests; DROP TABLE project_settings; DROP TABLE issue_workers; DROP TABLE worker_events; DROP TABLE worker_runs; DROP TABLE project_workers; DROP TABLE worker_pool; DROP TABLE global_settings_requests; DROP TABLE global_settings; PRAGMA user_version=2;").unwrap();
    let status = f.run("session-a", &["worker", "status"]);
    assert_eq!(status["config"]["prompt"], Value::Null);
    assert_eq!(status["config"]["enabled"], false);
    assert_eq!(
        f.run("session-a", &["view", "1"])["comments"][0]["body"],
        "Preserve this Markdown"
    );
    let projects = f.run("session-a", &["projects", "--all"]);
    assert_eq!(projects["projects"][0]["id"], id);
    assert!(projects["projects"][0]["hidden_at"].is_number());
    assert_eq!(f.create()["issue"]["number"], 2);
    assert_eq!(
        f.sql()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
}
fn git(cwd: &Path, args: &[&str]) {
    let o = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Issue test")
        .env("GIT_AUTHOR_EMAIL", "issues@example.invalid")
        .env("GIT_COMMITTER_NAME", "Issue test")
        .env("GIT_COMMITTER_EMAIL", "issues@example.invalid")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn lifecycle_markdown_and_audit_are_durable() {
    let f = Fixture::new();
    let md = "# Reconnect\n\n- [ ] Wake from sleep\n\n```sh\necho '$HOME `date`'\n```\nZażółć 🦀\n";
    let source = f.cwd.join("issue.md");
    fs::write(&source, md).unwrap();
    let created = f.run(
        "session-a",
        &[
            "create",
            "--title",
            "Reconnect",
            "--file",
            source.to_str().unwrap(),
            "--label",
            "bug",
        ],
    );
    assert_eq!(created["issue"]["number"], 1);
    fs::remove_file(source).unwrap();
    assert_eq!(f.run("session-a", &["view", "1"])["issue"]["body"], md);
    let edited = f.run(
        "session-a",
        &[
            "edit",
            "1",
            "--title",
            "Reconnect after wake",
            "--if-version",
            "1",
            "--label",
            "network",
        ],
    );
    assert_eq!(edited["issue"]["body"], md);
    assert_eq!(edited["issue"]["labels"], json!(["bug", "network"]));
    let note = "## Evidence\nConnection never retries.\n";
    let commented = success(f.stdin(&["comment", "1", "--body", "-"], note.as_bytes()));
    assert!(commented["comment_id"].is_i64());
    f.run("session-a", &["claim", "1"]);
    let closed = f.run(
        "session-a",
        &["close", "1", "--comment", "Fixed in abc123."],
    );
    assert_eq!(closed["issue"]["state"], "closed");
    assert!(closed["issue"]["assignee"].is_null());
    assert_eq!(closed["issue"]["closed_by"], "session-a");
    assert_eq!(f.run("session-a", &["list"])["issues"], json!([]));
    assert_eq!(
        f.run("session-a", &["list", "--state", "closed"])["issues"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let view = f.run("session-a", &["view", "1"]);
    assert_eq!(view["comments"][0]["body"], note);
    assert_eq!(view["comments"][1]["body"], "Fixed in abc123.");
    let history = f.run("session-a", &["history", "1"]);
    let events = history["events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| e["action"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "created",
            "edited",
            "commented",
            "claimed",
            "commented",
            "closed"
        ]
    );
    assert_eq!(events[0]["data"]["issue"]["body"], md);
    f.run("session-a", &["reopen", "1"]);
    f.run("session-a", &["delete", "1"]);
    assert_eq!(
        f.run("session-a", &["list", "--state", "all"])["issues"],
        json!([])
    );
    assert_eq!(
        f.run("session-a", &["list", "--state", "deleted"])["issues"][0]["number"],
        1
    );
    f.fail(
        "session-a",
        &["comment", "1", "--body", "cannot edit deleted"],
        3,
    );
    let restored = f.run("session-a", &["restore", "1"]);
    assert_eq!(restored["issue"]["state"], "open");
    assert!(restored["issue"]["assignee"].is_null());
    assert_eq!(f.create()["issue"]["number"], 2);
    assert_eq!(
        f.sql()
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        fs::metadata(&f.db).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn claims_are_atomic_and_repeated_claims_are_idempotent() {
    let f = Fixture::new();
    f.create();
    let mut children = Vec::new();
    for n in 0..12 {
        children.push(
            f.cmd(&format!("worker-{n}"), &["claim", "1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let results = children
        .into_iter()
        .map(|c| c.wait_with_output().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.status.success()).count(), 1);
    for r in &results {
        assert!(
            [Some(0), Some(4)].contains(&r.status.code()),
            "{} {}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
    }
    let value = f.run("observer", &["view", "1"]);
    let owner = value["issue"]["assignee"].as_str().unwrap();
    assert_eq!(f.run(owner, &["claim", "1"])["changed"], false);
    for op in ["claim", "unassign", "close", "delete"] {
        f.fail("other", &[op, "1"], 4);
    }
    f.run("other", &["claim", "1", "--force"]);
    assert_eq!(
        f.run("other", &["list", "--mine"])["issues"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.run(owner, &["list", "--mine"])["issues"], json!([]));
    f.run("other", &["unassign", "1"]);
    assert_eq!(f.run("other", &["unassign", "1"])["changed"], false);
    assert_eq!(
        f.run("other", &["list", "--unassigned"])["issues"][0]["number"],
        1
    );
}

#[test]
fn concurrent_creation_and_versioned_edits_do_not_lose_work() {
    let f = Fixture::new();
    let mut children = Vec::new();
    // Include concurrent schema initialization on a completely fresh database.
    for n in 0..10 {
        children.push(
            f.cmd("worker", &["create", "--title", &format!("task {n}")])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let mut numbers = children
        .into_iter()
        .map(|c| {
            success(c.wait_with_output().unwrap())["issue"]["number"]
                .as_i64()
                .unwrap()
        })
        .collect::<Vec<_>>();
    numbers.sort();
    assert_eq!(numbers, (1..=10).collect::<Vec<_>>());
    let mut children = Vec::new();
    for title in ["one", "two"] {
        children.push(
            f.cmd(
                "worker",
                &["edit", "1", "--title", title, "--if-version", "1"],
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        );
    }
    let statuses = children
        .into_iter()
        .map(|c| c.wait_with_output().unwrap().status.code().unwrap())
        .collect::<Vec<_>>();
    assert!(statuses == [0, 4] || statuses == [4, 0], "{statuses:?}");
}

#[test]
fn retries_are_deduplicated_even_after_later_changes() {
    let f = Fixture::new();
    let args = [
        "create",
        "--title",
        "retry",
        "--body",
        "same",
        "--request-id",
        "create-once",
    ];
    let first = f.run("session-a", &args);
    f.run("session-a", &["edit", "1", "--body", "new content"]);
    assert_eq!(f.run("session-a", &args), first);
    let args = [
        "comment",
        "1",
        "--body",
        "once",
        "--request-id",
        "comment-once",
    ];
    assert_eq!(f.run("session-a", &args), f.run("session-a", &args));
    assert_eq!(
        f.run("session-a", &["view", "1"])["comments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let args = [
        "close",
        "1",
        "--comment",
        "done",
        "--request-id",
        "close-once",
    ];
    assert_eq!(f.run("session-a", &args), f.run("session-a", &args));
    f.fail(
        "session-a",
        &[
            "create",
            "--title",
            "different",
            "--request-id",
            "create-once",
        ],
        4,
    );
    assert_eq!(f.create()["issue"]["number"], 2);
    // Failed mutations roll back their actor, comments, events and retry keys.
    f.fail("session-b", &["claim", "1", "--request-id", "later"], 4);
    f.run("session-a", &["reopen", "1"]);
    f.run("session-b", &["claim", "1", "--request-id", "later"]);
}

#[test]
fn project_identity_groups_worktrees_and_normalizes_origins() {
    let f = Fixture::new();
    git(&f.cwd, &["init", "-q"]);
    git(&f.cwd, &["commit", "--allow-empty", "-qm", "initial"]);
    git(
        &f.cwd,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:example/hey-boss.git",
        ],
    );
    let primary = f.create();
    assert_eq!(primary["project"]["id"], "github.com/example/hey-boss");
    let worktree = f.root.join("linked");
    git(
        &f.cwd,
        &[
            "worktree",
            "add",
            "-qb",
            "linked",
            worktree.to_str().unwrap(),
        ],
    );
    let linked = success(
        f.cmd("session-a", &["list"])
            .current_dir(&worktree)
            .output()
            .unwrap(),
    );
    assert_eq!(linked["project"], primary["project"]);
    assert_eq!(linked["issues"][0]["number"], 1);
    git(
        &f.cwd,
        &[
            "remote",
            "set-url",
            "origin",
            "https://token@github.com/example/hey-boss.git?secret=redacted",
        ],
    );
    assert_eq!(
        f.run("session-a", &["whoami"])["project"],
        primary["project"]
    );
    let second = f.root.join("second");
    fs::create_dir(&second).unwrap();
    git(&second, &["init", "-q"]);
    git(
        &second,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/another/hey-boss.git",
        ],
    );
    success(
        f.cmd("session-a", &["create", "--title", "other"])
            .current_dir(&second)
            .output()
            .unwrap(),
    );
    f.fail("session-a", &["list", "--project", "hey-boss"], 4);
    assert_eq!(
        f.run(
            "session-a",
            &["list", "--project", "github.com/another/hey-boss"]
        )["issues"][0]["title"],
        "other"
    );
    git(&f.cwd, &["remote", "remove", "origin"]);
    let primary = f.run("session-a", &["whoami"]);
    let linked = success(
        f.cmd("session-a", &["whoami"])
            .current_dir(&worktree)
            .output()
            .unwrap(),
    );
    assert_eq!(primary["project"], linked["project"]);
    assert!(
        primary["project"]["id"]
            .as_str()
            .unwrap()
            .starts_with("local:")
    );
}

#[test]
fn plain_directories_and_explicit_projects_are_isolated() {
    let f = Fixture::new();
    let one = f.create();
    let second = f.root.join("different/project");
    fs::create_dir_all(&second).unwrap();
    let two = success(
        f.cmd("session-a", &["create", "--title", "other"])
            .current_dir(&second)
            .output()
            .unwrap(),
    );
    assert_ne!(one["project"]["id"], two["project"]["id"]);
    assert_eq!(two["issue"]["number"], 1);
    f.fail("session-a", &["list", "--project", "project"], 4);
    let custom = f.run(
        "session-a",
        &["create", "--title", "custom", "--project", "shared"],
    );
    assert_eq!(custom["project"]["id"], "named:shared");
    assert_eq!(
        f.run("session-a", &["list", "--project", "shared"])["issues"][0]["title"],
        "custom"
    );
}

#[test]
fn filtering_pagination_and_field_validation() {
    let f = Fixture::new();
    for title in ["first bug", "second bug", "third"] {
        f.run("a", &["create", "--title", title, "--label", "bug"]);
    }
    let first = f.run(
        "a",
        &["list", "--search", "bug", "--label", "bug", "--limit", "1"],
    );
    assert_eq!(first["issues"][0]["number"], 1);
    assert_eq!(first["next_offset"], 1);
    let next = f.run(
        "a",
        &["list", "--search", "bug", "--limit", "1", "--offset", "1"],
    );
    assert_eq!(next["issues"][0]["number"], 2);
    assert!(next["next_offset"].is_null());
    f.fail("a", &["edit", "1"], 2);
    f.fail("a", &["create", "--title", " "], 2);
    f.fail("a", &["view", "0"], 2);
    f.fail("a", &["comment", "1", "--body", " "], 2);
    f.fail("a", &["view", "99"], 3);
    f.fail("a", &["list", "--request-id", "no"], 2);
    assert_eq!(
        f.stdin(&["comment", "1", "--body", "-"], &[0xff])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        f.stdin(&["edit", "1", "--body", "-"], &vec![b'x'; 1024 * 1024 + 1])
            .status
            .code(),
        Some(2)
    );
    f.run("a", &["edit", "1", "--body", ""]);
    assert_eq!(f.run("a", &["view", "1"])["issue"]["body"], "");
    f.run("a", &["edit", "1", "--remove-label", "bug"]);
    assert_eq!(
        f.run("a", &["list", "--label", "bug"])["issues"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let page = f.run("a", &["history", "1", "--limit", "1"]);
    assert_eq!(page["events"][0]["action"], "created");
    assert_eq!(page["next_offset"], 1);
}

#[test]
fn session_environment_is_stable_and_overrides_are_explicit() {
    let f = Fixture::new();
    let make = || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        cmd.current_dir(&f.cwd)
            .env("HEY_BOSS_ISSUE_DB", &f.db)
            .env("GIT_CEILING_DIRECTORIES", &f.root)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_AGENT_ID")
            .env("CODEX_THREAD_ID", "restored-thread");
        cmd
    };
    let first = success(
        make()
            .args(["issues", "whoami", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(first["agent"]["id"], "codex:restored-thread");
    success(
        make()
            .args(["issue", "create", "--title", "restored", "--json"])
            .output()
            .unwrap(),
    );
    success(
        make()
            .args(["issue", "claim", "1", "--json"])
            .output()
            .unwrap(),
    );
    let resumed = success(
        make()
            .args(["issue", "list", "--mine", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(resumed["issues"][0]["assignee"], "codex:restored-thread");
    let explicit = success(
        make()
            .env("HEY_BOSS_AGENT_ID", "environment")
            .args(["issue", "whoami", "--agent", "explicit", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(explicit["agent"]["id"], "explicit");
    assert!(explicit["agent"]["pid"].is_null());
}

#[test]
fn rpc_uses_callers_context_and_never_forwards_recursively() {
    let f = Fixture::new();
    let request = json!({"version":1,"project":{"id":"github.com/example/shared","name":"shared"},"project_override":null,
        "actor":{"id":"codex:remote-session","kind":"codex","session_id":"remote-session","machine":"remote-machine","host":"remote-host","pid":null,"process_start":null,"cwd":"/remote/repo","source":"CODEX_THREAD_ID"},
        "operation":{"action":"create","title":"Remote","body":"# Markdown\n$(not a shell command)","labels":[]},"request_id":"remote-once"});
    let result = success(f.stdin(&["rpc"], request.to_string().as_bytes()));
    assert_eq!(result["project"]["id"], "github.com/example/shared");
    assert_eq!(result["issue"]["created_by"], "codex:remote-session");
    assert_eq!(
        success(f.stdin(&["rpc"], request.to_string().as_bytes())),
        result
    );
    let mut bad = request.clone();
    bad["version"] = json!(99);
    assert_eq!(
        f.stdin(&["rpc"], bad.to_string().as_bytes()).status.code(),
        Some(2)
    );
    bad = request;
    bad["actor"] = Value::Null;
    assert_eq!(
        f.stdin(&["rpc"], bad.to_string().as_bytes()).status.code(),
        Some(2)
    );
    assert_eq!(
        f.stdin(&["rpc"], br#"{"version":1,"unexpected":true}"#)
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn remote_transport_keeps_markdown_out_of_shell_and_has_no_local_fallback() {
    let f = Fixture::new();
    let bin = f.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let shim = bin.join("ssh");
    fs::write(&shim, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ISSUE_SSH_ARGS\"\nexec \"$ISSUE_TEST_BIN\" issue rpc\n").unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
    let args = f.root.join("ssh-args");
    let remote_db = f.root.join("remote/issues.db");
    let mut command = f.cmd(
        "caller",
        &[
            "create",
            "--title",
            "Remote",
            "--body",
            "`touch /tmp/nope`\n$(echo secret)",
            "--host",
            "devbox",
            "--request-id",
            "one",
        ],
    );
    command
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("HEY_BOSS_ISSUE_DB", &remote_db)
        .env("HEY_BOSS_ISSUE_HOST", "must-not-recurse")
        .env("ISSUE_TEST_BIN", env!("CARGO_BIN_EXE_hey-boss"))
        .env("ISSUE_SSH_ARGS", &args);
    let result = success(command.output().unwrap());
    assert_eq!(result["issue"]["created_by"], "caller");
    assert_eq!(result["store"]["host"], "devbox");
    let arguments = fs::read_to_string(&args).unwrap();
    assert!(!arguments.contains("secret"));
    assert!(!arguments.contains("touch"));
    assert!(!f.db.exists());
    fs::write(&shim, "#!/bin/sh\nexit 255\n").unwrap();
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["error"]["code"],
        "transport_error"
    );
    assert!(!f.db.exists());
    f.fail("a", &["list", "--host=-oProxyCommand=bad"], 2);
}

#[test]
fn existing_unrelated_and_future_databases_are_not_modified() {
    let f = Fixture::new();
    fs::create_dir_all(f.db.parent().unwrap()).unwrap();
    let db = f.sql();
    db.execute_batch("CREATE TABLE unrelated(data TEXT); INSERT INTO unrelated VALUES ('keep');")
        .unwrap();
    f.fail("a", &["list"], 2);
    assert_eq!(
        db.query_row("SELECT data FROM unrelated", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    let g = Fixture::new();
    g.create();
    g.sql().pragma_update(None, "user_version", 99).unwrap();
    g.fail("a", &["create", "--title", "blocked"], 2);
    assert_eq!(
        g.sql()
            .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn large_markdown_is_complete_and_history_pages_make_progress() {
    let f = Fixture::new();
    // JSON escaping expands these valid UTF-8 bodies sixfold. A response must
    // still fit the remote transport without truncating an individual body.
    let body = vec![0_u8; 1024 * 1024];
    success(f.stdin(&["create", "--title", "Large", "--body", "-"], &body));
    for _ in 0..3 {
        success(f.stdin(&["comment", "1", "--body", "-"], &body));
    }
    let view = f.run("reader", &["view", "1"]);
    assert_eq!(view["issue"]["body"].as_str().unwrap().as_bytes(), body);
    assert_eq!(view["more_comments"], true);
    assert_eq!(view["comments"].as_array().unwrap().len(), 1);
    assert_eq!(
        view["comments"][0]["body"].as_str().unwrap().as_bytes(),
        body
    );
    let first = f.run("reader", &["history", "1"]);
    assert_eq!(first["next_offset"], 2);
    assert_eq!(first["events"].as_array().unwrap().len(), 2);
    let second = f.run("reader", &["history", "1", "--offset", "2"]);
    assert_eq!(second["events"].as_array().unwrap().len(), 2);
    assert!(second["next_offset"].is_null());
}

#[test]
fn project_instructions_and_pr_links_are_durable_and_visible_in_claim_and_cli() {
    let f = Fixture::new();
    f.create();
    assert_eq!(
        f.run("session-a", &["settings", "show"])["prs_enabled"],
        false
    );
    f.run("session-a", &["settings","set","--prs-enabled","--prompt","Implement {{issue_command}}. Attach every PR using hey-boss issue pr add {{number}} '<pr-url>'."]);
    let claim = f.run("session-a", &["claim", "1"]);
    assert!(
        claim["instructions"]
            .as_str()
            .unwrap()
            .contains("hey-boss issue pr add 1 '<pr-url>'")
    );
    let url = "https://github.com/example/repo/pull/123";
    assert_eq!(
        f.run("session-a", &["pr", "add", "1", url])["changed"],
        true
    );
    assert_eq!(
        f.run("session-a", &["pr", "add", "1", url])["changed"],
        false
    );
    f.run(
        "session-a",
        &["pr", "add", "1", "https://github.com/example/repo/pull/124"],
    );
    assert_eq!(
        f.run("session-a", &["view", "1"])["issue"]["pull_requests"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        f.run("session-a", &["list"])["issues"][0]["pull_requests"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for unsafe_url in [
        "javascript:alert(1)",
        "https://user:pass@example.com/pull/1",
        "https:///pull/1",
        "https://example.com/has space",
    ] {
        f.fail("session-a", &["pr", "add", "1", unsafe_url], 2);
    }
    assert_eq!(
        f.run("session-a", &["pr", "remove", "1", url])["changed"],
        true
    );
    assert_eq!(
        f.run("session-a", &["pr", "list", "1"])["pull_requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.run("session-a", &["settings", "set", "--no-prs"]);
    assert_eq!(
        f.run("session-a", &["settings", "show"])["prompt"],
        "Implement {{issue_command}}. Attach every PR using hey-boss issue pr add {{number}} '<pr-url>'."
    );
}

#[test]
fn schema_three_migration_preserves_legacy_independent_worker_settings() {
    let f = Fixture::new();
    let v = f.create();
    let p = v["project"]["id"].as_str().unwrap();
    let db = f.sql();
    db.execute("INSERT INTO project_workers VALUES(?1,?2,7,123)",rusqlite::params![p,serde_json::to_string(&json!({"cwd":f.cwd,"prompt":"Implement {{issue_command}}. {{commit_instruction}}","concurrency":3,"labels":["ready"],"use_goal":true,"enabled":false})).unwrap()]).unwrap();
    db.execute_batch("DROP TABLE mindmap_links; DROP TABLE mindmap_nodes; DROP TABLE mindmaps; DROP VIEW issue_pickup_ready; DROP TABLE issue_subtasks; DROP INDEX worker_sort_order; DROP INDEX issue_sort_order; ALTER TABLE issues DROP COLUMN sort_order; ALTER TABLE projects DROP COLUMN issue_order_version; DROP TABLE issue_pull_requests; DROP TABLE project_settings; DROP INDEX worker_runs_worker; ALTER TABLE worker_runs DROP COLUMN worker_id; ALTER TABLE worker_runs DROP COLUMN reservation_expires; ALTER TABLE worker_runs DROP COLUMN claimed_at; DROP TABLE issue_workers; DROP TABLE global_settings_requests; DROP TABLE global_settings; PRAGMA user_version=3;").unwrap();
    let s = f.run("session-a", &["worker", "status"]);
    assert_eq!(s["config"]["concurrency"], 3);
    assert_eq!(s["config"]["tags"][0], "ready");
    assert_eq!(s["config"]["use_goal"], true);
    assert_eq!(s["config"]["enabled"], false);
    assert_eq!(s["version"], 7);
    assert_eq!(
        f.run("session-a", &["view", "1"])["issue"]["title"],
        v["issue"]["title"]
    );
}

#[test]
fn worker_project_environment_routes_bare_commands_and_explicit_flag_wins() {
    let f = Fixture::new();
    f.run(
        "session-a",
        &[
            "create",
            "--project",
            "Worker route",
            "--title",
            "Worker issue",
        ],
    );
    f.run(
        "session-a",
        &[
            "create",
            "--project",
            "Other route",
            "--title",
            "Other issue",
        ],
    );
    let bare = success(
        f.cmd("session-a", &["view", "1"])
            .env("HEY_BOSS_ISSUE_PROJECT", "named:Worker route")
            .output()
            .unwrap(),
    );
    assert_eq!(bare["issue"]["title"], "Worker issue");
    let explicit = success(
        f.cmd("session-a", &["view", "1", "--project", "Other route"])
            .env("HEY_BOSS_ISSUE_PROJECT", "named:Worker route")
            .output()
            .unwrap(),
    );
    assert_eq!(explicit["issue"]["title"], "Other issue");
}

#[test]
fn project_overview_counts_open_claimed_closed_deleted_and_prints_columns() {
    let f = Fixture::new();
    for title in ["Claimed", "Available", "Closed", "Deleted"] {
        f.run("session-a", &["create", "--title", title]);
    }
    f.run("session-a", &["claim", "1"]);
    f.run("session-a", &["close", "3"]);
    f.run("session-a", &["delete", "4"]);
    let listed = f.run("session-a", &["projects"]);
    let p = &listed["projects"][0];
    assert_eq!(p["open"], 2);
    assert_eq!(p["unassigned"], 1);
    assert_eq!(p["closed"], 1);
    assert_eq!(p["deleted"], 1);
    let out = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&f.cwd)
        .env("HEY_BOSS_ISSUE_DB", &f.db)
        .env("GIT_CEILING_DIRECTORIES", &f.root)
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args(["issue", "projects"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    for column in [
        "NAME",
        "OPEN",
        "CLAIMED",
        "UNASSIGNED",
        "CLOSED",
        "DELETED",
        "PROJECT",
    ] {
        assert!(text.contains(column), "{text}");
    }
}

#[test]
fn issue_order_is_shared_durable_paginated_filtered_and_new_issues_append() {
    let f = Fixture::new();
    for title in ["One", "Two", "Three"] {
        f.run(
            "session-a",
            &["create", "--title", title, "--label", "ready"],
        );
    }
    let order = |args: &[&str]| {
        f.run("session-a", args)["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["number"].as_i64().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&["list"]), vec![1, 2, 3]);
    let version = f.run("session-a", &["list"])["order_version"]
        .as_i64()
        .unwrap()
        .to_string();
    let moved = f.run(
        "session-a",
        &["move", "3", "--before", "1", "--if-order-version", &version],
    );
    assert_eq!(moved["changed"], true);
    assert_eq!(order(&["list"]), vec![3, 1, 2]);
    assert_eq!(order(&["list", "--limit", "2"]), vec![3, 1]);
    assert_eq!(order(&["list", "--limit", "2", "--offset", "2"]), vec![2]);
    f.run("session-a", &["create", "--title", "Four"]);
    assert_eq!(order(&["list"]), vec![3, 1, 2, 4]);
    assert_eq!(order(&["list", "--label", "ready"]), vec![3, 1, 2]);
    f.run("session-a", &["move", "3", "--after", "2"]);
    assert_eq!(order(&["list"]), vec![1, 2, 3, 4]);
    f.run("session-a", &["move", "1"]);
    assert_eq!(order(&["list"]), vec![2, 3, 4, 1]);
    f.run("session-a", &["close", "3"]);
    f.run("session-a", &["delete", "2"]);
    assert_eq!(order(&["list"]), vec![4, 1]);
    f.run("session-a", &["restore", "2"]);
    f.run("session-a", &["reopen", "3"]);
    assert_eq!(order(&["list"]), vec![2, 3, 4, 1]);
    let unchanged = f.run("session-a", &["move", "1"]);
    assert_eq!(unchanged["changed"], false);
    assert_eq!(
        unchanged["order_version"],
        f.run("session-a", &["list"])["order_version"]
    );
    let pr_url = "https://github.com/example/repo/pull/31";
    f.run("session-a", &["pr", "add", "3", pr_url]);
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&f.cwd)
        .env("HEY_BOSS_ISSUE_DB", &f.db)
        .env("GIT_CEILING_DIRECTORIES", &f.root)
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args(["issue", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.find("#2").unwrap() < text.find("#3").unwrap()
            && text.find("#3").unwrap() < text.find("#4").unwrap()
            && text.find("#4").unwrap() < text.find("#1").unwrap(),
        "{text}"
    );
    assert!(
        text.find("#3").unwrap() < text.find(pr_url).unwrap()
            && text.find(pr_url).unwrap() < text.find("#4").unwrap(),
        "PR must be printed under its own issue: {text}"
    );
    assert!(
        f.run("session-a", &["history", "3"])["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "reordered")
    );
}

#[test]
fn concurrent_reorders_reject_stale_lists_without_losing_issue_content() {
    let f = Fixture::new();
    for title in ["One", "Two", "Three"] {
        f.run(
            "session-a",
            &["create", "--title", title, "--body", "# Preserve me"],
        );
    }
    let v = f.run("session-a", &["list"])["order_version"]
        .as_i64()
        .unwrap()
        .to_string();
    let mut first = f
        .cmd(
            "session-a",
            &["move", "3", "--before", "1", "--if-order-version", &v],
        )
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let second = f
        .cmd(
            "session-b",
            &["move", "2", "--before", "1", "--if-order-version", &v],
        )
        .output()
        .unwrap();
    let first_status = first.wait().unwrap();
    assert_eq!(
        usize::from(first_status.success()) + usize::from(second.status.success()),
        1
    );
    assert!(first_status.code() == Some(4) || second.status.code() == Some(4));
    let db = f.sql();
    let mut q = db
        .prepare("SELECT count(*),count(DISTINCT sort_order) FROM issues")
        .unwrap();
    let (count, distinct): (i64, i64) = q.query_row([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(count, distinct);
    for number in ["1", "2", "3"] {
        assert_eq!(
            f.run("session-a", &["view", number])["issue"]["body"],
            "# Preserve me"
        );
    }
    f.fail("session-a", &["move", "1", "--before", "1"], 2);
    f.fail("session-a", &["move", "1", "--before", "999"], 3);
    f.fail("session-a", &["move", "1", "--if-order-version", &v], 4);
}

#[test]
fn schema_four_migration_initializes_order_without_losing_prs_claims_or_history() {
    let f = Fixture::new();
    f.create();
    f.run(
        "session-a",
        &["pr", "add", "1", "https://github.com/example/repo/pull/1"],
    );
    f.run("session-a", &["claim", "1"]);
    f.run("session-a", &["comment", "1", "--body", "Preserve this"]);
    f.sql().execute_batch("DROP TABLE mindmap_links; DROP TABLE mindmap_nodes; DROP TABLE mindmaps; DROP VIEW issue_pickup_ready; DROP TABLE issue_subtasks; DROP INDEX worker_sort_order; DROP INDEX issue_sort_order; ALTER TABLE issues DROP COLUMN sort_order; ALTER TABLE projects DROP COLUMN issue_order_version; ALTER TABLE project_settings DROP COLUMN boss_name; DROP TABLE global_settings_requests; DROP TABLE global_settings; PRAGMA user_version=4;").unwrap();
    let migrated = f.run("session-a", &["view", "1"]);
    assert_eq!(migrated["issue"]["sort_order"], 1);
    assert_eq!(migrated["issue"]["assignee"], "session-a");
    assert_eq!(
        migrated["issue"]["pull_requests"].as_array().unwrap().len(),
        1
    );
    assert_eq!(migrated["comments"][0]["body"], "Preserve this");
    assert_eq!(f.create()["issue"]["sort_order"], 2);
    assert_eq!(
        f.sql()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
}

#[test]
fn repeated_issue_moves_match_user_order_and_preserve_unique_positions() {
    let f = Fixture::new();
    let mut expected: Vec<i64> = (1..=7).collect();
    for number in &expected {
        f.run(
            "session-a",
            &["create", "--title", &format!("Issue {number}")],
        );
    }
    for step in 0..90 {
        let number = step % 7 + 1;
        let mut anchor = (step * 5 + 3) % 7 + 1;
        if anchor == number {
            anchor = anchor % 7 + 1;
        }
        expected.retain(|n| *n != number);
        if step % 10 == 0 {
            f.run("session-a", &["move", &number.to_string()]);
            expected.push(number);
        } else {
            let before = step % 2 == 0;
            let place = expected.iter().position(|n| *n == anchor).unwrap() + usize::from(!before);
            f.run(
                "session-a",
                &[
                    "move",
                    &number.to_string(),
                    if before { "--before" } else { "--after" },
                    &anchor.to_string(),
                ],
            );
            expected.insert(place, number);
        }
        let list = f.run("session-a", &["list"]);
        let actual: Vec<_> = list["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["number"].as_i64().unwrap())
            .collect();
        assert_eq!(actual, expected, "Move {step}");
        let unique: i64 = f
            .sql()
            .query_row("SELECT count(DISTINCT sort_order) FROM issues", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(unique, 7);
    }
}

#[test]
fn boss_assignment_rename_filter_and_ownership_preserve_issue_data() {
    let f = Fixture::new();
    f.create();
    f.run("session-a", &["edit", "1", "--label", "ready"]);
    f.run("session-a", &["comment", "1", "--body", "Keep this"]);
    f.run(
        "session-a",
        &["pr", "add", "1", "https://github.com/example/repo/pull/42"],
    );
    assert_eq!(
        f.run("session-a", &["settings", "show"])["boss_name"],
        "Boss"
    );
    let assigned = f.run("session-a", &["assign-to-boss", "1"]);
    assert_eq!(assigned["issue"]["assignee"], "human:boss");
    assert_eq!(assigned["issue"]["assignee_name"], "Boss");
    assert!(assigned.get("instructions").is_none());
    let repeated = f.run("session-b", &["assign-to-boss", "1"]);
    assert_eq!(repeated["changed"], false);
    assert_eq!(repeated["issue"]["version"], assigned["issue"]["version"]);
    f.create();
    f.run("session-b", &["claim", "2"]);
    f.create();
    let boss_list = f.run(
        "session-a",
        &["list", "--assignee", "boss", "--label", "ready"],
    );
    assert_eq!(boss_list["issues"].as_array().unwrap().len(), 1);
    assert_eq!(boss_list["issues"][0]["number"], 1);
    assert_eq!(
        f.run("session-a", &["list", "--assignee", "session-b"])["issues"][0]["number"],
        2
    );
    assert_eq!(
        f.run("session-a", &["list", "--unassigned"])["issues"][0]["number"],
        3
    );
    f.fail("session-b", &["claim", "1"], 4);
    f.fail("session-a", &["assign-to-boss", "2"], 4);
    f.fail("session-a", &["unassign", "1"], 4);
    f.run(
        "session-a",
        &["settings", "set", "--boss-name", "Alex & Co"],
    );
    let renamed = f.run("session-a", &["view", "1"]);
    assert_eq!(renamed["issue"]["assignee"], "human:boss");
    assert_eq!(renamed["issue"]["assignee_name"], "Alex & Co");
    assert_eq!(renamed["issue"]["body"], "## Problem\nDrops after sleep.");
    assert_eq!(renamed["issue"]["labels"], json!(["ready"]));
    assert_eq!(renamed["comments"][0]["body"], "Keep this");
    assert_eq!(
        renamed["issue"]["pull_requests"].as_array().unwrap().len(),
        1
    );
    assert_eq!(renamed["assignee_agent"]["pid"], Value::Null);
    f.fail("session-a", &["settings", "set", "--boss-name", "   "], 2);
    let mut plain = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
    plain
        .current_dir(&f.cwd)
        .env("HEY_BOSS_ISSUE_DB", &f.db)
        .env("GIT_CEILING_DIRECTORIES", &f.root)
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args([
            "issue",
            "list",
            "--agent",
            "session-a",
            "--assignee",
            "boss",
        ]);
    assert!(
        String::from_utf8(plain.output().unwrap().stdout)
            .unwrap()
            .contains("Alex & Co")
    );
    f.run("human:boss", &["unassign", "1"]);
    f.run("session-a", &["assign-to-boss", "2", "--force"]);
    f.run("human:boss", &["close", "2"]);
    f.fail("session-a", &["assign-to-boss", "2"], 4);
    let events = f.run("session-a", &["history", "2"]);
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["actor"] == "session-a" && e["data"]["assignee"] == "human:boss")
    );
}

#[test]
fn schema_five_migration_preserves_settings_order_claims_and_history() {
    let f = Fixture::new();
    f.create();
    f.create();
    f.run("session-a", &["claim", "1"]);
    f.run("session-a", &["comment", "1", "--body", "Preserve"]);
    f.run("session-a", &["move", "2", "--before", "1"]);
    f.run(
        "session-a",
        &[
            "settings",
            "set",
            "--prs-enabled",
            "--prompt",
            "/goal Implement {{issue_command}}.",
        ],
    );
    f.sql()
        .execute_batch("DROP TABLE mindmap_links; DROP TABLE mindmap_nodes; DROP TABLE mindmaps; DROP VIEW issue_pickup_ready; DROP TABLE issue_subtasks; ALTER TABLE project_settings DROP COLUMN boss_name; DROP TABLE global_settings_requests; DROP TABLE global_settings; PRAGMA user_version=5;")
        .unwrap();
    let settings = f.run("session-a", &["settings", "show"]);
    assert_eq!(settings["boss_name"], "Boss");
    assert_eq!(settings["prs_enabled"], true);
    assert_eq!(settings["prompt"], "/goal Implement {{issue_command}}.");
    let issues = f.run("session-a", &["list"]);
    assert_eq!(issues["issues"][0]["number"], 2);
    assert_eq!(issues["issues"][1]["assignee"], "session-a");
    assert_eq!(
        f.run("session-a", &["view", "1"])["comments"][0]["body"],
        "Preserve"
    );
    assert_eq!(
        f.sql()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
}

#[test]
fn global_profile_changes_every_project_without_changing_assignments_or_creating_projects() {
    let f = Fixture::new();
    let global = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&f.cwd)
            .env("HEY_BOSS_ISSUE_DB", &f.db)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args(["settings", "--json"])
            .args(args)
            .output()
            .unwrap()
    };
    let settings = success(global(&["show"]));
    assert_eq!(settings["boss_name"], "Boss");
    assert_eq!(
        f.sql()
            .query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    for name in ["Alpha", "Beta"] {
        f.run(
            "session-a",
            &["create", "--project", name, "--title", "Preserve"],
        );
        f.run("session-a", &["assign-to-boss", "--project", name, "1"]);
    }
    let before = f.run("session-a", &["view", "--project", "Alpha", "1"]);
    let renamed = success(global(&[
        "set",
        "--boss-name",
        "Alex <Boss>",
        "--if-version",
        "1",
        "--request-id",
        "global-name",
    ]));
    assert_eq!(renamed["version"], 2);
    assert_eq!(
        success(global(&[
            "set",
            "--boss-name",
            "Alex <Boss>",
            "--if-version",
            "1",
            "--request-id",
            "global-name"
        ])),
        renamed
    );
    assert_eq!(
        global(&["set", "--boss-name", "Other", "--if-version", "1"])
            .status
            .code(),
        Some(4)
    );
    assert_eq!(
        global(&["set", "--boss-name", "   "]).status.code(),
        Some(2)
    );
    assert_eq!(
        global(&["set", "--boss-name", "Other", "--request-id", "global-name"])
            .status
            .code(),
        Some(4)
    );
    for name in ["Alpha", "Beta"] {
        let issue = f.run("session-a", &["view", "--project", name, "1"]);
        assert_eq!(issue["issue"]["assignee"], "human:boss");
        assert_eq!(issue["issue"]["assignee_name"], "Alex <Boss>");
        assert_eq!(issue["issue"]["version"], before["issue"]["version"]);
        assert_eq!(issue["issue"]["body"], before["issue"]["body"]);
        assert_eq!(
            f.run("session-a", &["settings", "show", "--project", name])["version"],
            0
        );
    }
    assert_eq!(
        f.sql()
            .query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn schema_six_migration_preserves_latest_custom_boss_name_globally() {
    let f = Fixture::new();
    for (name, boss, activity) in [("Alpha", "Older", 100), ("Beta", "Latest", 200)] {
        f.run(
            "session-a",
            &["create", "--project", name, "--title", "Preserved"],
        );
        f.run(
            "session-a",
            &["settings", "set", "--project", name, "--boss-name", boss],
        );
        f.sql()
            .execute(
                "UPDATE projects SET activity_at=?1 WHERE id=?2",
                rusqlite::params![activity, format!("named:{name}")],
            )
            .unwrap();
    }
    f.sql().execute_batch("DROP TABLE mindmap_links; DROP TABLE mindmap_nodes; DROP TABLE mindmaps; DROP VIEW issue_pickup_ready; DROP TABLE issue_subtasks; DROP TABLE global_settings_requests; DROP TABLE global_settings; PRAGMA user_version=6;").unwrap();
    for name in ["Alpha", "Beta"] {
        assert_eq!(
            f.run("session-a", &["view", "--project", name, "1"])["boss"]["name"],
            "Latest"
        );
    }
}

#[test]
fn subtasks_are_atomic_full_issues_with_order_progress_and_durable_unlink() {
    let f = Fixture::new();
    f.create();
    let child = f.run(
        "session-a",
        &[
            "subtask",
            "create",
            "1",
            "--title",
            "Child",
            "--body",
            "## Child Markdown",
            "--label",
            "ready",
            "--if-version",
            "1",
            "--request-id",
            "create-child",
        ],
    );
    assert_eq!(child["issue"]["number"], 2);
    assert_eq!(child["issue"]["parent"]["number"], 1);
    assert_eq!(child["parent_issue"]["subtasks"]["total"], 1);
    assert_eq!(
        f.run(
            "session-a",
            &[
                "subtask",
                "create",
                "1",
                "--title",
                "Child",
                "--body",
                "## Child Markdown",
                "--label",
                "ready",
                "--if-version",
                "1",
                "--request-id",
                "create-child"
            ]
        ),
        child
    );
    f.run(
        "session-a",
        &["subtask", "create", "1", "--title", "Second"],
    );
    f.run("session-a", &["move", "3", "--before", "2"]);
    let listed = f.run("session-a", &["subtask", "list", "1"]);
    assert_eq!(listed["issues"][0]["number"], 3);
    assert_eq!(listed["issues"][1]["body"], "## Child Markdown");
    f.run(
        "session-a",
        &["pr", "add", "2", "https://github.com/example/repo/pull/7"],
    );
    f.run("session-a", &["close", "2"]);
    assert_eq!(
        f.run("session-a", &["view", "1"])["issue"]["subtasks"]["closed"],
        1
    );
    let version = f.run("session-a", &["view", "1"])["issue"]["version"]
        .as_i64()
        .unwrap();
    f.run(
        "session-a",
        &[
            "subtask",
            "remove",
            "1",
            "2",
            "--if-version",
            &version.to_string(),
            "--request-id",
            "unlink-child",
        ],
    );
    let unlinked = f.run("session-a", &["view", "2"]);
    assert!(unlinked["issue"]["parent"].is_null());
    assert_eq!(unlinked["issue"]["state"], "closed");
    assert_eq!(unlinked["issue"]["body"], "## Child Markdown");
    assert_eq!(unlinked["issue"]["labels"], json!(["ready"]));
    assert_eq!(
        unlinked["issue"]["pull_requests"].as_array().unwrap().len(),
        1
    );
    assert!(
        f.run("session-a", &["history", "2"])["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "parent_removed")
    );
}

#[test]
fn subtask_lifecycle_does_not_cascade_and_deleted_links_remain_accessible() {
    let f = Fixture::new();
    f.create();
    f.run("session-a", &["subtask", "create", "1", "--title", "Child"]);
    f.run("session-a", &["close", "1"]);
    assert_eq!(f.run("session-a", &["view", "2"])["issue"]["state"], "open");
    f.run("session-a", &["delete", "1"]);
    assert!(f.run("session-a", &["view", "2"])["issue"]["deleted_at"].is_null());
    assert!(!f.run("session-a", &["view", "2"])["issue"]["parent"]["deleted_at"].is_null());
    f.run("session-a", &["restore", "1"]);
    f.run("session-a", &["delete", "2"]);
    let parent = f.run("session-a", &["view", "1"]);
    assert_eq!(parent["issue"]["subtasks"]["total"], 0);
    assert_eq!(parent["issue"]["subtasks"]["deleted"], 1);
    assert!(
        f.run("session-a", &["subtask", "list", "1"])["issues"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.run("session-a", &["subtask", "list", "1", "--all"])["issues"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.run("session-a", &["delete", "1"]);
    f.run("session-a", &["subtask", "remove", "1", "2"]);
    assert!(f.run("session-a", &["view", "2"])["issue"]["parent"].is_null());
}

#[test]
fn subtask_versions_parent_conflicts_and_cycles_roll_back_without_history() {
    let f = Fixture::new();
    for _ in 0..3 {
        f.create();
    }
    f.run(
        "session-a",
        &[
            "subtask",
            "add",
            "1",
            "2",
            "--if-version",
            "1",
            "--if-child-version",
            "1",
        ],
    );
    f.fail("session-a", &["subtask", "add", "3", "2"], 4);
    let before = f.run("session-a", &["history", "2"]);
    f.fail(
        "session-a",
        &["subtask", "remove", "1", "2", "--if-child-version", "1"],
        4,
    );
    f.fail("session-a", &["subtask", "add", "2", "1"], 2);
    assert_eq!(f.run("session-a", &["history", "2"]), before);
    f.run("session-a", &["delete", "1"]);
    let db = f.sql();
    assert!(
        db.execute(
            "INSERT INTO issue_subtasks VALUES(?1,2,1,0,'session-a')",
            [f.run("session-a", &["view", "2"])["project"]["id"]
                .as_str()
                .unwrap()]
        )
        .is_err()
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM issue_subtasks", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    f.fail("session-a", &["subtask", "add", "2", "2"], 2);
    f.fail("session-a", &["subtask", "add", "2", "0"], 2);
}

#[test]
fn subtask_depth_guards_insert_update_and_atomic_child_creation() {
    let f = Fixture::new();
    f.create();
    for parent in 1..=8 {
        f.run(
            "session-a",
            &[
                "subtask",
                "create",
                &parent.to_string(),
                "--title",
                "Nested",
            ],
        );
    }
    let before = f.run("session-a", &["view", "9"]);
    f.fail(
        "session-a",
        &["subtask", "create", "9", "--title", "Too deep"],
        2,
    );
    assert_eq!(f.run("session-a", &["view", "9"]), before);
    assert_eq!(
        f.sql()
            .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert_eq!(
        f.run("session-a", &["create", "--title", "Another root"])["issue"]["number"],
        10
    );
    f.run(
        "session-a",
        &["subtask", "create", "10", "--title", "Other branch"],
    );
    let db = f.sql();
    assert!(
        db.execute(
            "UPDATE issue_subtasks SET parent_number=9 WHERE child_number=11",
            []
        )
        .is_err()
    );
    assert!(
        db.execute(
            "UPDATE issue_subtasks SET parent_number=9 WHERE child_number=2",
            []
        )
        .is_err()
    );
    assert_eq!(
        f.run("session-a", &["view", "11"])["issue"]["parent"]["number"],
        10
    );
}

#[test]
fn subtask_capacity_counts_deleted_relationships_and_failed_create_is_atomic() {
    let f = Fixture::new();
    f.create();
    for _ in 0..100 {
        f.run("session-a", &["subtask", "create", "1", "--title", "Child"]);
    }
    f.fail(
        "session-a",
        &["subtask", "create", "1", "--title", "Over capacity"],
        2,
    );
    f.run("session-a", &["delete", "2"]);
    f.fail(
        "session-a",
        &["subtask", "create", "1", "--title", "Still full"],
        2,
    );
    assert_eq!(
        f.sql()
            .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        101
    );
    f.run("session-a", &["subtask", "remove", "1", "2"]);
    assert_eq!(
        f.run(
            "session-a",
            &["subtask", "create", "1", "--title", "Capacity restored"]
        )["issue"]["number"],
        102
    );
}

#[test]
fn worker_readiness_follows_nested_open_descendants_and_ignores_deleted_subtrees() {
    let f = Fixture::new();
    f.create();
    f.run(
        "session-a",
        &["subtask", "create", "1", "--title", "Intermediate"],
    );
    f.run("session-a", &["subtask", "create", "2", "--title", "Leaf"]);
    f.run("session-a", &["close", "2"]);
    let ready = || {
        let db = f.sql();
        db.prepare("SELECT number FROM issue_pickup_ready ORDER BY number")
            .unwrap()
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(ready(), vec![3]);
    assert_eq!(
        f.run("session-a", &["view", "1"])["issue"]["subtasks"]["open_descendants"],
        1
    );
    assert_eq!(f.run("session-a", &["worker", "status"])["eligible"], 1);
    f.run("session-a", &["claim", "1"]);
    f.run("session-a", &["unassign", "1"]);
    f.run("session-a", &["delete", "2"]);
    assert_eq!(ready(), vec![1, 3]);
    f.run("session-a", &["restore", "2"]);
    assert_eq!(ready(), vec![3]);
    f.run("session-a", &["close", "3"]);
    assert_eq!(ready(), vec![1]);
}

#[test]
fn concurrent_subtask_parenting_has_one_winner() {
    let f = Fixture::new();
    for _ in 0..3 {
        f.create();
    }
    let a = f
        .cmd("session-a", &["subtask", "add", "1", "3"])
        .spawn()
        .unwrap();
    let b = f
        .cmd("session-b", &["subtask", "add", "2", "3"])
        .spawn()
        .unwrap();
    let statuses = [
        a.wait_with_output().unwrap().status.code().unwrap(),
        b.wait_with_output().unwrap().status.code().unwrap(),
    ];
    assert!(statuses == [0, 4] || statuses == [4, 0], "{statuses:?}");
    assert_eq!(
        f.sql()
            .query_row(
                "SELECT count(*) FROM issue_subtasks WHERE child_number=3",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn schema_seven_subtask_migration_preserves_existing_issue_and_global_profile() {
    let f = Fixture::new();
    f.create();
    f.run("session-a", &["claim", "1"]);
    let before = f.run("session-a", &["view", "1"]);
    f.sql()
        .execute_batch(
            "DROP TABLE mindmap_links; DROP TABLE mindmap_nodes; DROP TABLE mindmaps; DROP VIEW issue_pickup_ready; DROP TABLE issue_subtasks; PRAGMA user_version=7;",
        )
        .unwrap();
    assert_eq!(f.run("session-a", &["view", "1"]), before);
    assert_eq!(
        f.sql()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
    f.run(
        "session-a",
        &["subtask", "create", "1", "--title", "Migrated child"],
    );
    assert_eq!(
        f.run("session-a", &["view", "1"])["issue"]["assignee"],
        "session-a"
    );
}

#[test]
fn schema_eight_graph_sync_migration_keeps_links_and_reinstates_safe_upsert() {
    let f = Fixture::new();
    f.create();
    f.run("session-a", &["subtask", "create", "1", "--title", "Child"]);
    let before = f.run("session-a", &["view", "1"]);
    f.sql().execute_batch("DROP TABLE mindmap_links; DROP TABLE mindmap_nodes; DROP TABLE mindmaps; DROP TABLE fleet_subtask_receipts; DROP TABLE fleet_deferred_subtasks; PRAGMA user_version=8;").unwrap();
    assert_eq!(f.run("session-a", &["view", "1"]), before);
    assert_eq!(
        f.sql()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
    f.sql().execute_batch("INSERT INTO issue_subtasks SELECT * FROM issue_subtasks WHERE child_number=2 ON CONFLICT(project_id,child_number) DO UPDATE SET parent_number=excluded.parent_number;").unwrap();
    assert_eq!(f.run("session-a", &["view", "1"]), before);
}
