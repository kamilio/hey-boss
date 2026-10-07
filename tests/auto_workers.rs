#[path = "support/projects.rs"]
mod projects;
#[path = "support/environment.rs"]
mod signing;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    desired: PathBuf,
    owner: hey_boss::database::Owner,
    owner_lock: PathBuf,
    signing: Option<signing::Fixture>,
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
        // Own the service in-process so CLI tests do not leave detached daemons.
        let root = root.canonicalize().unwrap();
        let database = root.join("issues.db");
        let owner = hey_boss::database::Owner::start(&database)
            .unwrap()
            .unwrap();
        let identity = format!(
            "{:x}",
            Sha256::digest(database.as_os_str().as_encoded_bytes())
        );
        let owner_lock = PathBuf::from(format!(
            "/tmp/hey-boss-db-{}/{}.lock",
            unsafe { libc::getuid() },
            &identity[..24]
        ));
        Self {
            desired: root.join("desired.json"),
            root,
            owner,
            owner_lock,
            signing: None,
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = self
            .signing
            .as_ref()
            .map(|signing| signing.command(env!("CARGO_BIN_EXE_hey-boss")))
            .unwrap_or_else(|| Command::new(env!("CARGO_BIN_EXE_hey-boss")));
        c.args(args)
            .current_dir(self.root.join("checkout"))
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", self.root.join("state"))
            .env("HEY_BOSS_FLEET_DESIRED", &self.desired)
            .env("HEY_BOSS_FLEET_BINARY", env!("CARGO_BIN_EXE_hey-boss"))
            .env(
                "HEY_BOSS_CODEX",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-drain.mjs"),
            )
            .env("HEY_BOSS_TEST_CLI", env!("CARGO_BIN_EXE_hey-boss"))
            .env("HEY_BOSS_INBOX_SOCKET", self.root.join("absent.sock"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .env_remove("HEY_BOSS_FLEET_MANAGED");
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        let mut command = self.command(args);
        if !args.contains(&"--json") {
            command.arg("--json");
        }
        let output = command.output().unwrap();
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
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let db = rusqlite::Connection::open(self.root.join("issues.db")).unwrap();
            let live: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM issue_workers WHERE owner_pid IS NOT NULL)",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if !live {
                break;
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "Fixture workers did not stop; retained {}",
                    self.root.display()
                );
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.owner.stop();
        fs::remove_dir_all(&self.root).unwrap();
        fs::remove_file(&self.owner_lock).unwrap();
    }
}

#[test]
fn repeated_launches_reuse_workers_and_graceful_removal_finishes_the_agent() {
    let f = Fixture::new("drain");
    let project = hey_boss::issues::identity::project(
        &f.root,
        &hey_boss::issues::identity::machine().unwrap(),
    )
    .unwrap();
    projects::seed(&f.root.join("issues.db"), &[&project.id]);
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
        let snapshot = f.cli(&["auto-workers", "--json", "run"]);
        assert_eq!(snapshot["workers"].as_array().unwrap().len(), 1);
        assert_eq!(snapshot["workers"][0]["pid"], pid);
    }
    f.cli(&["worker", "--json", "pause", id]);
    let paused = f.cli(&["auto-workers", "--json", "status"]);
    assert_eq!(paused["workers"][0]["config"]["enabled"], false);
    assert_eq!(paused["workers"][0]["intent"], "pause");
    let observed = f.cli(&["auto-workers", "watch", "--json", "--count", "1"]);
    assert_eq!(observed["workers"][0]["intent"], "pause");
    assert_eq!(observed["workers"][0]["pid"], pid);
    let text = f.command(&["auto-workers", "status"]).output().unwrap();
    assert!(text.status.success());
    assert!(serde_json::from_slice::<Value>(&text.stdout).is_err());
    assert!(String::from_utf8_lossy(&text.stdout).contains(id));
    assert!(
        !f.command(&["auto-workers"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        f.cli(&["auto-workers", "list", "--json"])["workers"][0]["intent"],
        "pause"
    );
    for _ in 0..2 {
        let resumed = f.cli(&["auto-workers", "--json", "run"]);
        assert_eq!(resumed["workers"][0]["config"]["enabled"], false);
        assert_eq!(resumed["workers"][0]["intent"], "pause");
        assert_eq!(resumed["workers"][0]["pid"], pid);
        assert_eq!(resumed["workers"][0]["active"], 1);
        assert_eq!(resumed["workers"][0]["config"]["concurrency"], 1);
        assert_eq!(unsafe { libc::kill(agent, 0) }, 0);
    }
    let saved = f.cli(&["auto-workers", "config", "--json"]);
    assert_eq!(saved["workers"][0]["intent"], "pause");
    assert_eq!(saved["workers"][0]["config"]["enabled"], false);
    assert!(saved["workers"][0]["local_revision"].is_i64());
    f.cli(&["auto-workers", "--json", "remove", id]);
    let draining = f.cli(&["auto-workers", "--json", "run"]);
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
    f.cli(&["auto-workers", "--json", "run"]);
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
        f.cli(&["auto-workers", "--json", "run"])["workers"]
            .as_array()
            .unwrap()
            .is_empty(),
        "Retired worker restarted"
    );
}

#[test]
fn launch_preserves_saved_pauses_and_stopped_workers() {
    let f = Fixture::new("resume-offline");
    fs::write(
        f.root.join("state/auto-workers.json"),
        json!({"worker_ids":["paused","stopped"]}).to_string(),
    )
    .unwrap();
    fs::write(
        f.root.join("desired.json"),
        json!({"machines":{"local":{"workers":[
            {"id":"paused","intent":"pause","config":{"enabled":false}},
            {"id":"stopped","intent":"stop","config":{"enabled":true}}
        ]}}})
        .to_string(),
    )
    .unwrap();
    let observed = f.cli(&["auto-workers", "--json", "status"]);
    assert!(observed["workers"].as_array().unwrap().is_empty());
    assert_eq!(
        f.cli(&["auto-workers", "config", "--json"])["workers"][0]["intent"],
        "pause"
    );
    let started = f.cli(&["auto-workers", "--json", "run"]);
    assert_eq!(started["workers"].as_array().unwrap().len(), 1);
    assert_eq!(started["workers"][0]["id"], "paused");
    assert!(started["workers"][0]["pid"].is_null());
    assert_eq!(started["workers"][0]["config"]["enabled"], false);
    assert_eq!(started["workers"][0]["intent"], "pause");
    assert_eq!(
        f.cli(&["auto-workers", "config", "--json"])["workers"][1]["intent"],
        "stop"
    );
}

#[test]
fn observation_never_probes_saved_checkouts_but_startup_validates_them() {
    use std::os::unix::fs::PermissionsExt;

    let f = Fixture::new("observe-without-checkout-probes");
    let checkout = f.root.join("saved-checkout");
    fs::create_dir(&checkout).unwrap();
    fs::write(
        f.root.join("state/auto-workers.json"),
        json!({"worker_ids":["saved"]}).to_string(),
    )
    .unwrap();
    fs::write(
        &f.desired,
        json!({"machines":{"local":{"workers":[
            {"id":"saved","intent":"pause","config":{
                "projects":["named:Saved"],
                "directories":{"named:Saved":checkout}
            }}
        ]}}})
        .to_string(),
    )
    .unwrap();
    let bin = f.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\nprintf x >> \"$HEY_BOSS_TEST_GIT_PROBES\"\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let probes = f.root.join("git-probes");
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let observe = || {
        for args in [
            vec!["auto-workers", "status", "--json"],
            vec!["auto-workers", "config", "--json"],
            vec!["auto-workers", "watch", "--json", "--count", "1"],
        ] {
            let output = f
                .command(&args)
                .env("PATH", &path)
                .env("HEY_BOSS_TEST_GIT_PROBES", &probes)
                .env("HEY_BOSS_CODEX", f.root.join("missing-codex"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["ok"], true);
            if args.contains(&"config") {
                assert_eq!(
                    value["workers"][0]["config"]["directories"]["named:Saved"],
                    checkout.to_str().unwrap()
                );
                assert_eq!(value["workers"][0]["intent"], "pause");
            }
        }
        assert!(!probes.exists(), "Observation ran Git checkout validation");
    };
    observe();
    fs::remove_dir(&checkout).unwrap();
    observe();
    let rejected = f
        .command(&["auto-workers", "run", "--json"])
        .output()
        .unwrap();
    assert!(
        !rejected.status.success(),
        "Startup accepted an unavailable checkout"
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
fn invalid_configuration_never_starts_an_earlier_valid_worker() {
    let f = Fixture::new("invalid");
    fs::write(
        f.root.join("state/auto-workers.json"),
        json!({"worker_ids":["valid","invalid"]}).to_string(),
    )
    .unwrap();
    fs::write(
        f.root.join("desired.json"),
        json!({"machines":{"local":{"workers":[
            {"id":"valid","config":{"enabled":true}},
            {"id":"invalid","config":{"concurrency":0}}
        ]}}})
        .to_string(),
    )
    .unwrap();
    let rejected = f
        .command(&["auto-workers", "run", "--json"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    let error: Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(error["ok"], false);
    assert!(error["error"].is_object());
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
    let mut f = Fixture::new("layouts");
    let signing = signing::Fixture::new();
    signing.success("setup");
    f.signing = Some(signing);
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
    let saved = f.cli(&["auto-workers", "config", "--json"]);
    assert_eq!(saved["workers"].as_array().unwrap().len(), 3);
    assert_eq!(
        f.cli(&["auto-workers", "--json", "run"])["workers"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn dashboard_owns_only_explicit_workers_and_cannot_control_other_sessions() {
    let f = Fixture::new("scope");
    let owned = f.cli(&[
        "auto-workers",
        "add",
        "--id",
        "owned",
        "--name",
        "Owned",
        "-C",
        f.root.join("checkout").to_str().unwrap(),
    ]);
    assert_eq!(owned["worker_id"], "owned");
    let path = f.root.join("state/fleet-main.json");
    let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    for id in ["other-local", "other-remote"] {
        let mut foreign = saved["workers"][0].clone();
        foreign["id"] = json!(id);
        foreign["config"]["enabled"] = json!(false);
        foreign["intent"] = json!("pause");
        saved["workers"]
            .as_array_mut()
            .unwrap()
            .push(foreign.clone());
        db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at,machine) VALUES(?1,'cli',?2,1,0,?3)", rusqlite::params![id,foreign["config"].to_string(),if id=="other-remote" {"remote"} else {"local"}]).unwrap();
    }
    fs::write(&path, saved.to_string()).unwrap();
    // Scope must apply before overview limits and decoding other workers' data.
    for n in 0..105 {
        db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at,machine) VALUES(?1,'cli',?2,1,9999999999999,'remote')", rusqlite::params![format!("unrelated-{n}"), json!({"concurrency":"not a number"}).to_string()]).unwrap();
    }
    for args in [
        vec!["auto-workers", "--json", "status"],
        vec!["auto-workers", "--json", "run"],
    ] {
        let snapshot = f.cli(&args);
        assert_eq!(
            snapshot["workers"].as_array().unwrap().len(),
            1,
            "Imported another worker: {snapshot}"
        );
        assert_eq!(snapshot["workers"][0]["id"], "owned");
    }
    assert!(
        !f.command(&["auto-workers", "remove", "other-local"])
            .output()
            .unwrap()
            .status
            .success()
    );
    for id in ["other-local", "other-remote"] {
        let enabled: bool = db
            .query_row(
                "SELECT json_extract(config,'$.enabled') FROM issue_workers WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !enabled,
            "Launching auto-workers changed an unrelated worker"
        );
    }
}

#[test]
fn yaml_owns_declared_workers_without_an_ownership_file_and_reloads_idempotently() {
    let mut f = Fixture::new("yaml-live");
    f.desired = f.root.join("fleet.yaml");
    let yaml = "# worker fleet\nmachines:\n  local:\n    workers:\n      - id: declared\n        intent: running\n        config: {}\n";
    fs::write(&f.desired, yaml).unwrap();
    let first = f.cli(&["auto-workers", "run"]);
    let pid = first["workers"][0]["pid"].clone();
    assert!(pid.is_u64(), "{first}");
    assert!(!f.root.join("state/auto-workers.json").exists());
    assert_eq!(f.cli(&["auto-workers", "run"])["workers"][0]["pid"], pid);
    assert_eq!(fs::read_to_string(&f.desired).unwrap(), yaml);
    fs::write(&f.desired, yaml.replace("intent: running", "intent: pause")).unwrap();
    for _ in 0..2 {
        let paused = f.cli(&["auto-workers", "run"]);
        assert_eq!(paused["workers"][0]["intent"], "pause");
        assert_eq!(paused["workers"][0]["config"]["enabled"], false);
        assert_eq!(paused["workers"][0]["pid"], pid);
    }
    fs::write(&f.desired, "machines: [").unwrap();
    assert_eq!(
        f.cli(&["auto-workers", "run"])["workers"][0]["intent"],
        "pause"
    );
    fs::write(&f.desired, "machines: {local: {workers: []}}\n").unwrap();
    f.cli(&["auto-workers", "run"]);
    f.wait(|s| s["workers"].as_array().unwrap().is_empty());
    assert!(
        f.cli(&["auto-workers", "run"])["workers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn migration_keeps_existing_ids_and_processes() {
    let mut f = Fixture::new("yaml-migration");
    f.cli(&[
        "auto-workers",
        "add",
        "--id",
        "existing",
        "--name",
        "Existing",
        "-C",
        f.root.join("checkout").to_str().unwrap(),
    ]);
    let before = f.cli(&["auto-workers", "status"]);
    f.desired = f.root.join("desired.yaml");
    // The local pending definition has not yet been synchronized to legacy fleet.json.
    let after = f.cli(&["auto-workers", "run"]);
    assert_eq!(after["workers"][0]["id"], before["workers"][0]["id"]);
    assert_eq!(after["workers"][0]["pid"], before["workers"][0]["pid"]);
    assert!(
        fs::read_to_string(&f.desired)
            .unwrap()
            .contains("id: existing")
    );
    assert_eq!(
        f.cli(&["auto-workers", "run"])["workers"][0]["pid"],
        before["workers"][0]["pid"]
    );
}

#[test]
fn yaml_add_writes_the_authoritative_file_and_no_ownership_list() {
    let mut f = Fixture::new("yaml-add");
    f.desired = f.root.join("fleet.yaml");
    fs::write(&f.desired, "machines: {local: {workers: []}}\n").unwrap();
    f.cli(&[
        "auto-workers",
        "add",
        "--id",
        "added",
        "--name",
        "Added",
        "-C",
        f.root.join("checkout").to_str().unwrap(),
    ]);
    assert!(
        fs::read_to_string(&f.desired)
            .unwrap()
            .contains("id: added")
    );
    assert!(!f.root.join("state/auto-workers.json").exists());
    f.cli(&["worker", "pause", "added"]);
    assert!(
        fs::read_to_string(&f.desired)
            .unwrap()
            .contains("intent: pause")
    );
    assert_eq!(
        f.cli(&["auto-workers", "run"])["workers"][0]["intent"],
        "pause"
    );
}

#[test]
fn removal_finishes_even_after_the_old_checkout_is_deleted() {
    let mut f = Fixture::new("yaml-deleted-checkout");
    f.desired = f.root.join("fleet.yaml");
    fs::write(&f.desired, "machines: {local: {workers: []}}\n").unwrap();
    let checkout = f.root.join("retired-checkout");
    fs::create_dir(&checkout).unwrap();
    f.cli(&[
        "auto-workers",
        "add",
        "--id",
        "retired",
        "--name",
        "Retired",
        "-C",
        checkout.to_str().unwrap(),
    ]);
    fs::remove_dir(&checkout).unwrap();
    fs::write(&f.desired, "machines: {local: {workers: []}}\n").unwrap();
    f.cli(&["auto-workers", "run"]);
    f.cli(&["auto-workers", "run"]);
    f.wait(|s| s["workers"].as_array().unwrap().is_empty());
}

#[test]
fn invalid_checkout_does_not_partially_apply_other_worker_changes() {
    let mut f = Fixture::new("yaml-preflight");
    f.desired = f.root.join("fleet.yaml");
    fs::write(
        &f.desired,
        "machines: {local: {workers: [{id: valid, intent: running, config: {concurrency: 1}}]}}\n",
    )
    .unwrap();
    f.cli(&["auto-workers", "run"]);
    fs::write(&f.desired, "machines:\n  local:\n    workers:\n      - id: valid\n        intent: running\n        config: {concurrency: 3}\n      - id: bad\n        intent: running\n        config: {projects: [named:Bad], directory: /missing/checkout/hb688}\n").unwrap();
    assert!(
        !f.command(&["auto-workers", "run", "--json"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let current = f.cli(&["worker", "--id", "valid", "status"]);
    assert_eq!(current["workers"][0]["config"]["concurrency"], 1);
}
