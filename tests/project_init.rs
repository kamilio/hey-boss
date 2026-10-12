use hey_boss::issues::{Project, Store};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hb-project-init-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
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
fn init_controls_drafts_per_project_and_preserves_them_on_rerun() {
    let f = Fixture::new();
    let init = [
        "project",
        "init",
        "--project",
        "Legacy",
        "--yes",
        "--prs",
        "false",
        "--worktree",
        "false",
        "--json",
    ];
    assert_eq!(f.json(&init)["drafts_enabled"], true);
    let existing = f.json(&[
        "issue",
        "--project",
        "Legacy",
        "--agent",
        "human:qa",
        "--json",
        "create",
        "--title",
        "Existing draft",
        "--draft",
    ]);
    let mut disabled = init.to_vec();
    disabled.extend(["--drafts", "false"]);
    assert_eq!(f.json(&disabled)["drafts_enabled"], false);
    assert_eq!(
        f.json(&init)["drafts_enabled"],
        false,
        "Reruns preserve the saved choice"
    );
    let rejected = f.run(&[
        "issue",
        "--project",
        "Legacy",
        "--agent",
        "human:qa",
        "create",
        "--title",
        "New draft",
        "--draft",
    ]);
    assert_eq!(rejected.status.code(), Some(2));
    let message = "Project \"Legacy\" is not using drafts. Use regular tasks.";
    assert!(String::from_utf8_lossy(&rejected.stderr).contains(message));
    let rejected_json = f.run(&[
        "issue",
        "--project",
        "Legacy",
        "--agent",
        "human:qa",
        "--json",
        "create",
        "--title",
        "New draft",
        "--draft",
    ]);
    assert_eq!(rejected_json.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&rejected_json.stdout).unwrap();
    assert_eq!(error["error"]["message"], message);
    let number = existing["issue"]["number"].to_string();
    assert_eq!(
        f.json(&["issue", "--project", "Legacy", "--json", "view", &number])["issue"]["draft"],
        true
    );
    let regular = f.json(&[
        "issue",
        "--project",
        "Legacy",
        "--agent",
        "human:qa",
        "--json",
        "create",
        "--title",
        "Ordinary issue",
    ]);
    assert_eq!(
        regular["issue"]["number"],
        existing["issue"]["number"].as_i64().unwrap() + 1
    );
    assert_eq!(regular["issue"]["draft"], false);
    let other = [
        "project",
        "init",
        "--project",
        "Modern",
        "--yes",
        "--prs",
        "true",
        "--worktree",
        "true",
        "--json",
    ];
    assert_eq!(f.json(&other)["drafts_enabled"], true);
    f.json(&[
        "issue",
        "--project",
        "Modern",
        "--agent",
        "human:qa",
        "--json",
        "create",
        "--title",
        "Other project draft",
        "--draft",
    ]);
    let mut enabled = init.to_vec();
    enabled.extend(["--drafts", "true"]);
    assert_eq!(f.json(&enabled)["drafts_enabled"], true);
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

#[test]
fn project_resources_can_be_disabled_independently_without_losing_content() {
    let f = Fixture::new();
    let init = [
        "project",
        "init",
        "--project",
        "Resources",
        "--yes",
        "--prs",
        "false",
        "--worktree",
        "false",
        "--json",
    ];
    let settings = f.json(&init);
    assert_eq!(settings["artifacts_enabled"], true);
    assert_eq!(settings["mindmaps_enabled"], true);
    let artifact = f.json(&[
        "artifact",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "--json",
        "create",
        "--title",
        "Existing document",
        "--body",
        "Keep this",
    ]);
    let node = f.json(&[
        "mm",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "--json",
        "add",
        "Existing topic",
    ]);
    f.json(&[
        "issue",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "--json",
        "settings",
        "set",
        "--no-artifacts",
    ]);
    let rejected = f.run(&[
        "artifact",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "create",
        "--title",
        "Rejected",
        "--body",
        "Never saved",
    ]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("Project \"Resources\" is not using artifacts. Use regular tasks.")
    );
    f.json(&[
        "mm",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "--json",
        "add",
        "Still allowed",
    ]);
    f.json(&[
        "issue",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "--json",
        "settings",
        "set",
        "--no-mindmaps",
    ]);
    let rejected = f.run(&[
        "mm",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "add",
        "Rejected",
    ]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("Project \"Resources\" is not using mindmaps. Use regular tasks.")
    );
    assert_eq!(
        f.json(&[
            "artifact",
            "--project",
            "Resources",
            "--json",
            "view",
            artifact["artifact"]["id"].as_str().unwrap()
        ])["artifact"]["body"],
        "Keep this"
    );
    assert_eq!(
        f.json(&[
            "mm",
            "--project",
            "Resources",
            "--json",
            "view",
            node["node"]["id"].as_str().unwrap()
        ])["node"]["title"],
        "Existing topic"
    );
    fs::write(f.0.join("note.txt"), "attachment").unwrap();
    fs::write(f.0.join("document.md"), "[Note](note.txt)").unwrap();
    let imported = f.run(&[
        "artifact",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "create",
        "--title",
        "Rejected import",
        "--file",
        "document.md",
    ]);
    assert_eq!(imported.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&imported.stderr)
            .contains("is not using artifacts. Use regular tasks.")
    );
    let linked = f.run(&[
        "mm",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "link",
        node["node"]["id"].as_str().unwrap(),
        "pr:https://github.com/example/repo/pull/1",
    ]);
    assert_eq!(linked.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&linked.stderr)
            .contains("is not using mindmaps. Use regular tasks.")
    );
    let db = Connection::open(f.db()).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM artifacts WHERE project_id='named:Resources'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM file_attachments", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM mindmap_nodes WHERE project_id='named:Resources'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    drop(db);
    assert!(!f.0.join("issues.attachments").exists());
    let saved = f.json(&init);
    assert_eq!(saved["artifacts_enabled"], false);
    assert_eq!(saved["mindmaps_enabled"], false);
    let listed = f.json(&["issue", "--project", "Resources", "--json", "projects"]);
    let resource = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "named:Resources")
        .unwrap();
    assert_eq!(resource["artifacts_enabled"], false);
    assert_eq!(resource["mindmaps_enabled"], false);
    let text = f.run(&["issue", "--project", "Resources", "settings", "show"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("Artifacts enabled: false"));
    assert!(text.contains("Mindmaps enabled: false"));
    let mut enabled = init.to_vec();
    enabled.extend(["--artifacts", "true", "--mindmaps", "false"]);
    assert_eq!(f.json(&enabled)["artifacts_enabled"], true);
    f.json(&[
        "artifact",
        "--project",
        "Resources",
        "--agent",
        "human:qa",
        "--json",
        "create",
        "--title",
        "Allowed again",
        "--body",
        "Saved",
    ]);
    let other = f.json(&[
        "project",
        "init",
        "--project",
        "Other",
        "--yes",
        "--prs",
        "false",
        "--worktree",
        "false",
        "--json",
    ]);
    assert_eq!(other["artifacts_enabled"], true);
    assert_eq!(other["mindmaps_enabled"], true);
    let listed = f.json(&["issue", "--project", "Other", "--json", "projects"]);
    let projects = listed["projects"].as_array().unwrap();
    let resource = projects
        .iter()
        .find(|p| p["id"] == "named:Resources")
        .unwrap();
    assert_eq!(resource["artifacts_enabled"], true);
    assert_eq!(resource["mindmaps_enabled"], false);
    let other = projects.iter().find(|p| p["id"] == "named:Other").unwrap();
    assert_eq!(other["artifacts_enabled"], true);
    assert_eq!(other["mindmaps_enabled"], true);
}
