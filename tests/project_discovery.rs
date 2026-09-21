use hey_boss::issues::{Project, Store};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static SERIAL: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hey-boss-project-discovery-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn db(&self) -> PathBuf {
        self.0.join("issues.db")
    }
    fn run(&self, project: &str, args: &[&str]) -> Value {
        let (command, args) = if args.first().is_some_and(|c| *c == "artifact" || *c == "mm") {
            (args[0], &args[1..])
        } else {
            ("issue", args)
        };
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.db())
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args([
                command,
                "--agent",
                "human:qa",
                "--project",
                project,
                "--json",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
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
fn automatic_discovery_does_not_register_temporary_agent_folders() {
    let f = Fixture::new();
    let mut store = Store::open(&f.db()).unwrap();
    let projects = [
        "local:remote:/tmp/hey-boss-test/claude",
        "local:remote:/private/tmp/hey-boss-test/codex",
        "local:remote:/private/var/folders/ab/cdef/T/hey-boss-test",
        "local:remote:/var/folders/ab/cdef/T/hey-boss-test",
    ]
    .map(|id| {
        (
            Project {
                id: id.into(),
                name: "hey-boss-test".into(),
            },
            100,
        )
    });
    store.discover_projects(&projects).unwrap();
    let db = Connection::open(f.db()).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .discover_projects(&[
            (
                Project {
                    id: "github.com/example/hey-boss".into(),
                    name: "hey-boss".into(),
                },
                100,
            ),
            (
                Project {
                    id: "local:remote:/home/dev/Workspace/hey-boss".into(),
                    name: "hey-boss".into(),
                },
                100,
            ),
        ])
        .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    store
        .discover_projects(&[(
            Project {
                id: "local:remote:/home/dev/Workspace/another-project".into(),
                name: "another-project".into(),
            },
            100,
        )])
        .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn legacy_empty_temporary_projects_are_omitted_but_saved_work_remains_accessible() {
    let f = Fixture::new();
    let empty = "local:remote:/tmp/hey-boss-empty";
    let saved = "local:remote:/tmp/hey-boss-saved";
    let artifact = "local:remote:/tmp/hey-boss-artifact";
    let map = "local:remote:/tmp/hey-boss-map";
    let settings = "local:remote:/tmp/hey-boss-settings";
    let deleted = "local:remote:/tmp/hey-boss-deleted";
    f.run(empty, &["list"]);
    f.run(saved, &["create", "--title", "Keep my issue"]);
    f.run(
        artifact,
        &[
            "artifact",
            "create",
            "--title",
            "Keep my document",
            "--body",
            "Saved work",
        ],
    );
    f.run(map, &["mm", "add", "Keep my map"]);
    f.run(
        settings,
        &["settings", "set", "--prompt", "Keep my instructions"],
    );
    f.run(deleted, &["create", "--title", "Restorable issue"]);
    f.run(deleted, &["delete", "1"]);
    for args in [vec!["projects"], vec!["projects", "--all"]] {
        let value = f.run("Atlas", &args);
        let projects = value["projects"].as_array().unwrap();
        assert!(!projects.iter().any(|p| p["id"] == empty));
        assert!(projects.iter().any(|p| p["id"] == saved));
        assert!(projects.iter().any(|p| p["id"] == artifact));
        for durable in [map, settings, deleted] {
            assert!(projects.iter().any(|p| p["id"] == durable));
        }
    }
    assert_eq!(
        f.run(saved, &["view", "1"])["issue"]["title"],
        "Keep my issue"
    );
    // Filtering is reversible and never deletes registry rows or content.
    assert_eq!(
        Connection::open(f.db())
            .unwrap()
            .query_row("SELECT count(*) FROM projects WHERE id=?1", [empty], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    f.run(empty, &["create", "--title", "Now durable"]);
    assert!(
        f.run("Atlas", &["projects"])["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == empty)
    );
}

#[test]
fn worktrees_share_the_repository_queue_even_when_their_names_differ() {
    for origin in [Some("git@github.com:example/hey-boss.git"), None] {
        let f = Fixture::new();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(&f.0)
                .args(["-c", "user.name=QA", "-c", "user.email=qa@example.invalid"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["commit", "--allow-empty", "-qm", "initial"]);
        if let Some(origin) = origin {
            git(&["remote", "add", "origin", origin]);
        }
        // No explicit project override: exercise real repository identification.
        let run = |cwd: &std::path::Path, args: &[&str]| {
            let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
                .current_dir(cwd)
                .env("HEY_BOSS_ISSUE_DB", f.db())
                .env_remove("HEY_BOSS_ISSUE_HOST")
                .env_remove("HEY_BOSS_ISSUE_PROJECT")
                .args(["issue", "--agent", "human:qa", "--json"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice::<Value>(&output.stdout).unwrap()
        };
        let primary = run(&f.0, &["create", "--title", "One repository queue"]);
        let linked = f.0.join("feature-checkout");
        git(&[
            "worktree",
            "add",
            "-qb",
            "feature",
            linked.to_str().unwrap(),
        ]);
        let list = run(&linked, &["list"]);
        assert_eq!(list["project"], primary["project"]);
        assert_eq!(list["issues"][0]["title"], "One repository queue");
        assert_eq!(
            run(&linked, &["projects"])["projects"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}
