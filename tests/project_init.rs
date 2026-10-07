use hey_boss::issues::{Project, Store};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hb-project-init-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn db(&self) -> PathBuf {
        self.0.join("issues.db")
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(args)
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.db())
            .env("HEY_BOSS_FLEET_STATE", self.0.join("fleet"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .output()
            .unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let result = self.run(args);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn init_saves_workflow_and_preserves_existing_prompts() {
    let f = Fixture::new();
    let args = [
        "project",
        "init",
        "--project",
        "Atlas",
        "--prs",
        "true",
        "--worktree",
        "false",
        "--yes",
        "--json",
    ];
    let first = f.json(&args);
    assert_eq!(first["project"]["id"], "named:Atlas");
    assert_eq!(first["prs_enabled"], true);
    assert_eq!(first["worktree_enabled"], false);
    f.json(&[
        "issue",
        "--project",
        "Atlas",
        "--agent",
        "human:qa",
        "--json",
        "settings",
        "set",
        "--prompt",
        "Existing custom instructions.",
    ]);
    let again = f.json(&args);
    assert_eq!(again["prompt"], "Existing custom instructions.");
    let projects = f.json(&["issue", "--project", "Atlas", "--json", "projects"]);
    assert_eq!(projects["projects"].as_array().unwrap().len(), 1);
}

#[test]
fn unattended_init_requires_explicit_choices_and_confirmation() {
    let f = Fixture::new();
    for args in [
        vec!["project", "init"],
        vec!["project", "init", "--yes"],
        vec!["project", "init", "--prs", "true", "--worktree", "true"],
    ] {
        let result = f.run(&args);
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("--yes --prs <true|false> --worktree <true|false>")
        );
    }
    assert!(!f.db().exists(), "Refuse before accessing the registry");
}

#[test]
fn builder_omits_empty_discoveries_but_keeps_saved_and_initialized_projects() {
    let f = Fixture::new();
    let mut store = Store::open(&f.db()).unwrap();
    store
        .discover_projects(&[
            (
                Project {
                    id: "local:remote:/workspace/lab/run-27".into(),
                    name: "run-27".into(),
                },
                10,
            ),
            (
                Project {
                    id: "github.com/example/unused".into(),
                    name: "unused".into(),
                },
                10,
            ),
        ])
        .unwrap();
    drop(store);
    let db = Connection::open(f.db()).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    // Existing empty registry entries remain hidden; saved legacy work stays visible.
    db.execute_batch(
        "INSERT INTO projects(id,name,next_number) VALUES
    ('local:remote:/workspace/lab/run-27','run-27',1),
    ('github.com/example/unused','unused',1),('named:Saved','Saved',1)",
    )
    .unwrap();
    f.json(&[
        "issue",
        "--project",
        "Saved",
        "--agent",
        "human:qa",
        "--json",
        "create",
        "--title",
        "Keep this",
    ]);
    f.json(&[
        "project",
        "init",
        "--project",
        "Initialized",
        "--prs",
        "false",
        "--worktree",
        "true",
        "--yes",
        "--json",
    ]);
    for all in [false, true] {
        let mut args = vec!["issue", "--project", "Observer", "--json", "projects"];
        if all {
            args.push("--all");
        }
        let projects = f.json(&args);
        let names: Vec<_> = projects["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.contains(&"Saved") && names.contains(&"Initialized"));
    }
    assert_eq!(
        Connection::open(f.db())
            .unwrap()
            .query_row(
                "SELECT count(*) FROM projects WHERE name IN ('run-27','unused')",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn builder_keeps_projects_explicitly_assigned_to_saved_workers() {
    let f = Fixture::new();
    drop(Store::open(&f.db()).unwrap());
    Connection::open(f.db()).unwrap().execute_batch(
        "INSERT INTO projects(id,name,next_number) VALUES('named:Worker project','Worker project',1);
         INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES('saved','managed','{\"projects\":[\"named:Worker project\"],\"enabled\":false}',1,0);"
    ).unwrap();
    let projects = f.json(&["issue", "--project", "Observer", "--json", "projects"]);
    assert_eq!(projects["projects"].as_array().unwrap().len(), 1);
    assert_eq!(projects["projects"][0]["name"], "Worker project");
}
