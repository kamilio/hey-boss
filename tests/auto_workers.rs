use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("hb-auto-{}-{name}", std::process::id()));
        fs::create_dir_all(root.join("state")).unwrap();
        fs::create_dir_all(root.join("checkout")).unwrap();
        fs::write(
            root.join("desired.json"),
            json!({"machines":{"local":{"workers":[]}}}).to_string(),
        )
        .unwrap();
        Self { root }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.args(args)
            .current_dir(self.root.join("checkout"))
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", self.root.join("state"))
            .env("HEY_BOSS_FLEET_DESIRED", self.root.join("desired.json"))
            .env("HEY_BOSS_FLEET_BINARY", env!("CARGO_BIN_EXE_hey-boss"))
            .env(
                "HEY_BOSS_CODEX",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-drain.mjs"),
            )
            .env("HEY_BOSS_TEST_CLI", env!("CARGO_BIN_EXE_hey-boss"))
            .env("HEY_BOSS_INBOX_SOCKET", self.root.join("absent.sock"))
            .env_remove("HEY_BOSS_ISSUE_HOST");
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        let output = self.command(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn wait(&self, mut predicate: impl FnMut(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let snapshot = self.cli(&["auto-workers", "--json", "status"]);
            if predicate(&snapshot) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "Timed out: {snapshot}");
            thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(db) = rusqlite::Connection::open(self.root.join("issues.db")) {
            let _ = db.execute("UPDATE issue_workers SET stop_requested=1", []);
            let _ = db.execute(
                "UPDATE worker_runs SET stop_requested=1 WHERE finished_at IS NULL",
                [],
            );
        }
        // Keep fixtures until detached workers have had time to shut down.
    }
}

#[test]
fn repeated_launches_reuse_workers_and_graceful_removal_finishes_the_agent() {
    let f = Fixture::new("drain");
    f.cli(&[
        "issue",
        "--json",
        "--agent",
        "human:auto-workers-test",
        "create",
        "--title",
        "Drain fixture",
        "--body",
        "Finish after release",
    ]);
    let added = f.cli(&[
        "auto-workers",
        "--json",
        "add",
        "--id",
        "fixture-worker",
        "--name",
        "Checkout",
        "--directory",
        f.root.join("checkout").to_str().unwrap(),
    ]);
    let id = added["worker_id"].as_str().unwrap();
    let before = f
        .wait(|s| s["workers"][0]["active"] == 1 && f.root.join("checkout/agent-started").exists());
    let pid = before["workers"][0]["pid"].clone();
    let agent = fs::read_to_string(f.root.join("checkout/agent-started"))
        .unwrap()
        .parse::<i32>()
        .unwrap();
    for _ in 0..2 {
        let replay = f.cli(&[
            "auto-workers",
            "--json",
            "add",
            "--id",
            "fixture-worker",
            "--name",
            "Checkout",
            "--directory",
            f.root.join("checkout").to_str().unwrap(),
        ]);
        assert_eq!(replay["worker_id"], id);
        let snapshot = f.cli(&["auto-workers", "--json"]);
        assert_eq!(snapshot["workers"].as_array().unwrap().len(), 1);
        assert_eq!(snapshot["workers"][0]["pid"], pid);
    }
    f.cli(&["auto-workers", "--json", "remove", id]);
    let draining = f.cli(&["auto-workers", "--json"]);
    assert_eq!(draining["workers"][0]["intent"], "drain");
    assert_eq!(draining["workers"][0]["config"]["enabled"], false);
    assert_eq!(draining["workers"][0]["active"], 1);
    assert_eq!(
        unsafe { libc::kill(agent, 0) },
        0,
        "Removal killed an active agent"
    );
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT stop_requested FROM worker_runs WHERE finished_at IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    fs::write(f.root.join("checkout/release-agent"), "").unwrap();
    f.wait(|s| s["workers"][0]["active"] == 0);
    f.cli(&["auto-workers", "--json"]);
    f.wait(|s| s["workers"].as_array().unwrap().is_empty());
    let state: String = db
        .query_row(
            "SELECT state FROM worker_runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "completed");
    assert!(
        f.cli(&["auto-workers", "--json"])["workers"]
            .as_array()
            .unwrap()
            .is_empty(),
        "Retired worker restarted"
    );
}

#[test]
fn invalid_configuration_never_starts_an_earlier_valid_worker() {
    let f = Fixture::new("invalid");
    fs::write(
        f.root.join("desired.json"),
        json!({"machines":{"local":{"workers":[
            {"id":"valid","config":{"enabled":true}},
            {"id":"invalid","config":{"concurrency":0}}
        ]}}})
        .to_string(),
    )
    .unwrap();
    assert!(
        !f.command(&["auto-workers", "--json"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM issue_workers", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn saves_separate_checkouts_and_a_shared_multi_project_pool() {
    let f = Fixture::new("layouts");
    let mut paths = Vec::new();
    for (directory, repo) in [
        ("one", "shared"),
        ("two", "shared"),
        ("left", "left"),
        ("right", "right"),
    ] {
        let path = f.root.join(directory);
        fs::create_dir_all(&path).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args([
                    "remote",
                    "add",
                    "origin",
                    &format!("https://github.com/fixture/{repo}.git")
                ])
                .current_dir(&path)
                .status()
                .unwrap()
                .success()
        );
        paths.push(path.canonicalize().unwrap());
    }
    for (i, path) in paths[..2].iter().enumerate() {
        f.cli(&[
            "auto-workers",
            "--json",
            "add",
            "--id",
            &format!("checkout-{i}"),
            "--name",
            &format!("Checkout {i}"),
            "-C",
            path.to_str().unwrap(),
        ]);
    }
    f.cli(&[
        "auto-workers",
        "--json",
        "add",
        "--id",
        "shared-pool",
        "--name",
        "Pool",
        "--concurrency",
        "2",
        "-C",
        paths[2].to_str().unwrap(),
        "-C",
        paths[3].to_str().unwrap(),
    ]);
    let status = f.cli(&["auto-workers", "--json", "status"]);
    let workers = status["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 3);
    for (i, path) in paths[..2].iter().enumerate() {
        let worker = workers
            .iter()
            .find(|w| w["id"] == format!("checkout-{i}"))
            .unwrap();
        assert_eq!(
            worker["config"]["projects"],
            json!(["github.com/fixture/shared"])
        );
        assert_eq!(worker["config"]["directory"], path.to_str().unwrap());
        assert_eq!(worker["config"]["concurrency"], 1);
    }
    let pool = workers.iter().find(|w| w["id"] == "shared-pool").unwrap();
    assert_eq!(pool["config"]["concurrency"], 2);
    assert_eq!(
        pool["config"]["directories"],
        json!({"github.com/fixture/left":paths[2],"github.com/fixture/right":paths[3]})
    );
    let saved = f.cli(&["auto-workers", "config"]);
    assert_eq!(saved["workers"].as_array().unwrap().len(), 3);
    assert_eq!(
        f.cli(&["auto-workers", "--json"])["workers"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
