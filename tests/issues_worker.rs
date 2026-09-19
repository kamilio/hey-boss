use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Fixture {
    root: PathBuf,
    db: PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = (if mode == "offline-updates" {
            PathBuf::from("/tmp")
        } else {
            std::env::temp_dir()
        })
        .join(format!("hey-boss-workers-{}-{}", std::process::id(), mode));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("mode.txt"), mode).unwrap();
        let db = root.join("issues.db");
        Self { root, db }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", &self.db)
            .env(
                "HEY_BOSS_CODEX",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-worker.py"),
            )
            .env("HEY_BOSS_TEST_CLI", self.notification_cli())
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args([
                "issue",
                "--project",
                "Worker fixture",
                "--agent",
                "human:worker-test",
                "--json",
            ])
            .args(args);
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        let o = self.command(args).output().unwrap();
        assert!(
            o.status.success(),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn setup(&self, extra: &[&str]) {
        self.cli(&[
            "create",
            "--title",
            "Fixture issue",
            "--body",
            "## Requirements\nCheck {{title}} stays literal.",
        ]);
        fs::write(
            self.root.join("worker-args.json"),
            serde_json::to_vec(extra).unwrap(),
        )
        .unwrap();
    }
    fn notification_cli(&self) -> PathBuf {
        let copied = self.root.join("bin/hey-boss");
        if copied.exists() {
            copied
        } else {
            env!("CARGO_BIN_EXE_hey-boss").into()
        }
    }
    fn worker(&self) -> Worker {
        self.worker_binary(std::path::Path::new(env!("CARGO_BIN_EXE_hey-boss")))
    }
    fn worker_binary(&self, binary: &std::path::Path) -> Worker {
        let extra: Vec<String> =
            serde_json::from_slice(&fs::read(self.root.join("worker-args.json")).unwrap()).unwrap();
        Worker(
            Command::new(binary)
                .current_dir(&self.root)
                .env("HEY_BOSS_ISSUE_DB", &self.db)
                .env("HEY_BOSS_TEST_CLI", self.notification_cli())
                .env(
                    "HEY_BOSS_CODEX",
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/codex-worker.py"),
                )
                .env_remove("HEY_BOSS_ISSUE_HOST")
                .args([
                    "worker",
                    "--project",
                    "Worker fixture",
                    "--directory",
                    self.root.to_str().unwrap(),
                ])
                .args(extra)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }
    fn control(&self, command: &str, run: &str) {
        let s = self.cli(&["worker", "status"]);
        let worker = s["worker_id"].as_str().unwrap();
        let db = rusqlite::Connection::open(&self.db).unwrap();
        if command == "stop" {
            db.execute(
                "UPDATE worker_runs SET stop_requested=1 WHERE id=?1 AND worker_id=?2",
                [run, worker],
            )
            .unwrap();
        } else {
            db.execute(
                "UPDATE worker_runs SET retry_allowed=1 WHERE id=?1 AND worker_id=?2",
                [run, worker],
            )
            .unwrap();
        }
    }
    fn wait(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let status = self.cli(&["worker", "status"]);
            if predicate(&status) {
                return status;
            }
            assert!(Instant::now() < deadline, "Timed out: {status}");
            thread::sleep(Duration::from_millis(50));
        }
    }
    fn transcript(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("protocol.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Worker(Child);
impl Worker {
    fn stop(&mut self) {
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "Scheduler failed to stop");
            thread::sleep(Duration::from_millis(25));
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn codex_protocol_goal_completion_and_prompt_variables() {
    let f = Fixture::new("completed");
    f.setup(&[
        "--prompt",
        "/goal Assign and implement `{{issue_command}}`. {{commit_instruction}}",
    ]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "completed");
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "closed");
    let transcript = f.transcript();
    let goals: Vec<_> = transcript
        .iter()
        .filter(|v| v["method"] == "thread/goal/set")
        .collect();
    assert_eq!(goals.len(), 2);
    assert_eq!(goals[0]["params"]["status"], "active");
    assert_eq!(goals[1]["params"]["status"], "complete");
    assert!(goals[1]["params"].get("objective").is_none());
    let turn = transcript
        .iter()
        .find(|v| v["method"] == "turn/start")
        .unwrap();
    let text = turn["params"]["input"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("Assign and implement `hey-boss issue view 1`"));
    assert_eq!(
        text,
        "Assign and implement `hey-boss issue view 1`. Commit your changes."
    );
    w.stop();
}
#[test]
fn custom_prompt_slash_goal_preserves_all_lines() {
    let f = Fixture::new("blocked");
    f.setup(&[
        "--prompt",
        "/goal Fix {{title}}\nRetrieve {{issue_command}}. {{body}}",
    ]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "blocked");
    assert_eq!(s["eligible"], 0);
    let issue = f.cli(&["view", "1"]);
    assert_eq!(issue["issue"]["state"], "open");
    assert!(issue["issue"]["assignee"].is_null());
    let t = f.transcript();
    let goal = t.iter().find(|v| v["method"] == "thread/goal/set").unwrap();
    assert_eq!(
        goal["params"]["objective"],
        "Fix Fixture issue\nRetrieve hey-boss issue view 1. ## Requirements\nCheck {{title}} stays literal."
    );
    let turn = t.iter().find(|v| v["method"] == "turn/start").unwrap();
    assert!(
        turn["params"]["input"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Fix Fixture issue\nRetrieve hey-boss issue view")
    );
    w.stop();
}
#[test]
fn codex_disconnection_releases_capacity_and_retains_failure() {
    let f = Fixture::new("disconnect");
    f.setup(&[]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert!(
        !f.transcript()
            .iter()
            .any(|v| v["method"] == "thread/goal/set"),
        "Plain prompts must not create a native goal"
    );
    assert_eq!(s["runs"][0]["state"], "failed");
    assert_eq!(s["active"], 0);
    assert_eq!(s["eligible"], 0);
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    w.stop();
}
#[test]
fn unfinished_unassigned_issues_are_reserved_again_after_retry_delay() {
    let f = Fixture::new("automatic-retry");
    fs::write(f.root.join("mode.txt"), "disconnect").unwrap();
    f.setup(&[]);
    let mut first = f.worker();
    f.wait(|s| s["runs"][0]["finished_at"].is_number());
    first.stop();
    for state in [
        "failed",
        "cancelled",
        "blocked",
        "interrupted",
        "claim_timeout",
    ] {
        let db = rusqlite::Connection::open(&f.db).unwrap();
        db.execute(
            "UPDATE worker_runs SET state=?1,finished_at=0,retry_allowed=0",
            [state],
        )
        .unwrap();
        assert_eq!(
            f.cli(&["worker", "status"])["eligible"],
            1,
            "{state} must not exclude an open unassigned issue permanently"
        );
        fs::write(f.root.join("mode.txt"), "delay").unwrap();
        let mut retry = f.worker();
        f.wait(|s| s["active"] == 1 && s["runs"][0]["state"] == "running");
        assert!(!f.cli(&["view", "1"])["issue"]["assignee"].is_null());
        retry.stop();
    }
}
#[test]
fn approval_hold_is_not_retried_automatically_even_after_delay() {
    let f = Fixture::new("approval-hold");
    fs::write(f.root.join("mode.txt"), "approval").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let first = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    let id = first["runs"][0]["id"].as_str().unwrap().to_owned();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE worker_runs SET finished_at=0", [])
        .unwrap();
    assert_eq!(f.cli(&["worker", "status"])["eligible"], 0);
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.control("retry", &id);
    f.wait(|s| s["active"] == 1 && s["runs"][0]["state"] == "running");
    worker.stop();
}
#[test]
fn private_issue_database_does_not_modify_default_fleet_configuration() {
    let f = Fixture::new("private-fleet-scope");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let home = f.root.join("home");
    let config = home.join(".local/share/hey-boss/fleet-main.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = br#"{"role":"controller","revision":"sentinel","workers":[]}"#;
    fs::write(&config, original).unwrap();
    let mut worker = Worker(
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&f.root)
            .env("HOME", &home)
            .env("HEY_BOSS_ISSUE_DB", &f.db)
            .env(
                "HEY_BOSS_CODEX",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-worker.py"),
            )
            .env("HEY_BOSS_TEST_CLI", env!("CARGO_BIN_EXE_hey-boss"))
            .env_remove("HEY_BOSS_FLEET_STATE")
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args([
                "worker",
                "--project",
                "Worker fixture",
                "--directory",
                f.root.to_str().unwrap(),
                "--json",
            ])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    worker.stop();
    assert_eq!(fs::read(config).unwrap(), original);
}

#[test]
fn legacy_upgrade_state_migrates_without_stopping_active_sessions() {
    let f = Fixture::new("legacy-upgrade");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let before = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let id = before["worker_id"].as_str().unwrap();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE issue_workers SET config=json_set(config,'$.enabled',json('false'),'$.upgrading',json('true')) WHERE id=?1", [id]).unwrap();
    let after = f.cli(&["worker", "status"]);
    assert_eq!(after["active"], 1);
    assert_eq!(after["config"]["enabled"], true);
    assert_eq!(after["upgrading"], true);
    assert_eq!(
        after["runs"][0]["session_id"],
        before["runs"][0]["session_id"]
    );
    assert!(
        db.query_row(
            "SELECT json_type(config,'$.upgrading') IS NULL FROM issue_workers WHERE id=?1",
            [id],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    worker.stop();
}

#[test]
fn offline_replica_launches_only_work_allocated_to_its_machine() {
    let f = Fixture::new("offline-replica-pickup");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let actor = f.cli(&["whoami"])["agent"].clone();
    let node = actor["machine"].as_str().unwrap();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE fleet_meta SET role='agent',node=?1", [node])
        .unwrap();
    assert_eq!(f.cli(&["worker", "status"])["eligible"], 0);
    assert_eq!(
        f.command(&["claim", "1"]).output().unwrap().status.code(),
        Some(4)
    );
    let project = f.cli(&["projects"])["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    db.execute(
        "INSERT INTO fleet_allocations VALUES(?1,1,'another-machine')",
        [&project],
    )
    .unwrap();
    assert_eq!(f.cli(&["worker", "status"])["eligible"], 0);
    db.execute("UPDATE fleet_allocations SET node=?1", [node])
        .unwrap();
    assert_eq!(f.cli(&["worker", "status"])["eligible"], 1);
    let mut worker = f.worker();
    f.wait(|s| s["active"] == 1 && s["runs"][0]["state"] == "running");
    worker.stop();
}
#[test]
fn replacing_cli_drains_active_sessions_then_restores_same_worker() {
    let f = Fixture::new("upgrade-handoff");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let directory = f.root.join("bin");
    fs::create_dir_all(&directory).unwrap();
    let binary = directory.join("hey-boss");
    fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &binary).unwrap();
    let mut worker = f.worker_binary(&binary);
    let initial = f.wait(|s| s["active"] == 1 && s["runs"][0]["state"] == "running");
    let id = initial["worker_id"].as_str().unwrap().to_owned();
    let run = initial["runs"][0]["id"].as_str().unwrap().to_owned();
    let replacement = directory.join("hey-boss.new");
    fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &replacement).unwrap();
    fs::rename(replacement, binary).unwrap();
    let still_running = f.wait(|status| status["upgrading"] == true);
    assert_eq!(still_running["active"], 1);
    assert_eq!(still_running["runs"][0]["id"], run);
    assert_eq!(still_running["version"], initial["version"]);
    assert_eq!(still_running["upgrading"], true);
    assert_eq!(still_running["workers"][0]["upgrading"], true);
    f.control("stop", &run);
    let restored = f.wait(|s| {
        s["version"].as_i64().unwrap() > initial["version"].as_i64().unwrap()
            && s["workers"][0]["pid"].is_number()
    });
    assert_eq!(restored["worker_id"], id);
    assert_eq!(restored["upgrading"], false);
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.control("retry", &run);
    f.wait(|s| s["runs"][0]["state"] == "completed");
    worker.stop();
}
#[test]
fn terminal_history_is_separate_bounded_and_can_be_hidden() {
    let f = Fixture::new("display-history");
    fs::write(f.root.join("mode.txt"), "disconnect").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    f.wait(|s| s["runs"][0]["finished_at"].is_number());
    worker.stop();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    for n in 1..=5 {
        db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,finished_at) SELECT ?1,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,finished_at FROM worker_runs LIMIT 1", [format!("historical-{n}")]).unwrap();
    }
    for (limit, expected) in [(2, 2), (0, 0)] {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&f.root)
            .env("HEY_BOSS_ISSUE_DB", &f.db)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args(["worker", "--history", &limit.to_string(), "status"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("Active sessions (0)"));
        assert_eq!(
            text.lines()
                .filter(|line| line.contains(" · failed · "))
                .count(),
            expected
        );
        assert_eq!(
            text.contains("Recent attempts (history; these do not use slots)"),
            limit > 0
        );
    }
    assert_eq!(
        f.cli(&["worker", "status"])["runs"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
}
#[test]
fn approval_blocks_without_auto_approving() {
    let f = Fixture::new("approval");
    f.setup(&[]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "blocked");
    assert!(
        s["runs"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("approval")
    );
    w.stop();
}
#[test]
fn stop_and_shutdown_reap_codex_before_releasing_claim() {
    let f = Fixture::new("delay");
    f.setup(&[
        "--prompt",
        "/goal Assign and implement `{{issue_command}}`. {{commit_instruction}}",
    ]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["state"] == "running" && s["runs"][0]["goal"].is_object());
    let id = s["runs"][0]["id"].as_str().unwrap();
    let pid = s["runs"][0]["pid"].as_u64().unwrap() as i32;
    f.control("stop", id);
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "cancelled");
    assert_eq!(s["runs"][0]["goal"]["status"], "paused");
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    f.control("retry", id);
    f.wait(|s| s["active"] == 1 && s["runs"][0]["goal"].is_object());
    w.stop();
    let s = f.cli(&["worker", "status"]);
    assert_eq!(s["active"], 0);
    assert_eq!(s["runs"][0]["state"], "cancelled");
    assert_eq!(s["runs"][0]["goal"]["status"], "paused");
}
#[test]
fn orphan_recovery_stops_process_and_preserves_session() {
    let f = Fixture::new("delay-orphan");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["state"] == "running");
    let pid = s["runs"][0]["pid"].as_u64().unwrap() as i32;
    let session = s["runs"][0]["session_id"].clone();
    let _ = w.0.kill();
    let _ = w.0.wait();
    let mut replacement = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "interrupted");
    assert_eq!(s["runs"][0]["session_id"], session);
    assert_eq!(s["active"], 0);
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    replacement.stop();
}

#[test]
fn unclaimed_completion_never_closes_issue() {
    let f = Fixture::new("unclaimed");
    f.setup(&[]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "blocked");
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "open");
    w.stop();
}
#[test]
fn missed_manual_claim_deadline_stops_codex_and_frees_slot() {
    let f = Fixture::new("delay-unclaimed");
    f.setup(&["--claim-timeout", "5"]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["state"] == "awaiting_claim");
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    let pid = s["runs"][0]["pid"].as_u64().unwrap() as i32;
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["active"], 0);
    assert_eq!(s["free"], 1);
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "open");
    w.stop();
}

#[test]
fn independent_workers_have_separate_capacity_tags_and_atomic_reservations() {
    let f = Fixture::new("parallel");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&["--concurrency", "2", "--tag", "ready"]);
    f.cli(&["edit", "1", "--label", "ready"]);
    f.cli(&["create", "--title", "Ready two", "--label", "ready"]);
    f.cli(&["create", "--title", "Backlog one", "--label", "backlog"]);
    f.cli(&["create", "--title", "Backlog two", "--label", "backlog"]);
    let mut first = f.worker();
    f.wait(|s| {
        s["active"] == 2
            && s["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["claimed_at"].is_number())
    });
    fs::write(
        f.root.join("worker-args.json"),
        serde_json::to_vec(&["--concurrency", "2", "--tag", "backlog"]).unwrap(),
    )
    .unwrap();
    let mut second = f.worker();
    let s = f.wait(|s| {
        s["workers"].as_array().unwrap().len() == 2
            && s["workers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["active"] == 2)
    });
    assert_eq!(s["config"]["tags"][0], "backlog");
    assert_eq!(s["free"], 0);
    let db = rusqlite::Connection::open(&f.db).unwrap();
    let distinct: i64 = db
        .query_row(
            "SELECT count(DISTINCT issue_number) FROM worker_runs WHERE finished_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        distinct, 4,
        "No shared project/global cap and no duplicate reservation"
    );
    f.cli(&["create", "--title", "Unrestricted fifth"]);
    fs::write(f.root.join("worker-args.json"), "[]").unwrap();
    let mut third = f.worker();
    f.wait(|s| s["workers"].as_array().unwrap().len() == 3 && s["active"] == 1);
    let s = f.cli(&["worker", "status"]);
    assert!(s["config"]["tags"].as_array().unwrap().is_empty());
    assert_eq!(s["runs"][0]["number"], 5);
    let text = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .env("HEY_BOSS_ISSUE_DB", &f.db)
        .args(["worker", "status"])
        .output()
        .unwrap();
    let text = String::from_utf8(text.stdout).unwrap();
    for expected in [
        "Slots:",
        "free",
        "busy",
        "Pipeline:",
        "manual claim",
        "Codex",
        "m0",
    ] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    third.stop();
    second.stop();
    first.stop();
}

#[test]
fn reservation_blocks_other_agents_until_expiry_and_preserves_takeover() {
    let f = Fixture::new("lock");
    fs::write(f.root.join("mode.txt"), "delay-unclaimed").unwrap();
    f.setup(&["--claim-timeout", "5"]);
    let mut w = f.worker();
    f.wait(|s| s["runs"][0]["state"] == "awaiting_claim");
    let result = f.command(&["claim", "1"]).output().unwrap();
    assert_eq!(result.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&result.stdout).contains("reserved"));
    let boss = f.command(&["assign-to-boss", "1"]).output().unwrap();
    assert_eq!(boss.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&boss.stdout).contains("reserved"));
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute(
        "UPDATE worker_runs SET reservation_expires=0 WHERE finished_at IS NULL",
        [],
    )
    .unwrap();
    assert_eq!(
        f.cli(&["claim", "1"])["issue"]["assignee"],
        "human:worker-test"
    );
    f.wait(|s| s["active"] == 0);
    assert_eq!(
        f.cli(&["view", "1"])["issue"]["assignee"],
        "human:worker-test"
    );
    w.stop();
}

#[test]
fn disconnected_mac_does_not_block_issue_completion_pickup_or_worker_stop_and_updates_replay() {
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    let f = Fixture::new("offline-updates");
    let state = f.root.join("companion-state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("bridge-protocol"), "1").unwrap();
    let executable = f.root.join("bin/hey-boss");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &executable).unwrap();
    fs::write(executable.with_extension("state"), state.to_str().unwrap()).unwrap();
    let broker_start = || {
        let broker = Worker(
            Command::new(env!("CARGO_BIN_EXE_hey-boss"))
                .args(["companion", "serve", "--state"])
                .arg(&state)
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(state.join("daemon.sock")).is_err() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        broker
    };
    let broker = broker_start();
    f.setup(&["--concurrency", "1"]);
    f.cli(&["create", "--title", "Second offline issue"]);
    f.cli(&["create", "--title", "Third offline issue"]);
    let mut worker = f.worker();
    let status = f.wait(|s| {
        s["runs"].as_array().is_some_and(|runs| {
            runs.len() == 3 && runs.iter().all(|r| r["finished_at"].is_number())
        })
    });
    for run in status["runs"].as_array().unwrap() {
        assert_eq!(run["state"], "completed");
    }
    for number in ["1", "2", "3"] {
        assert_eq!(f.cli(&["view", number])["issue"]["state"], "closed");
    }
    let queued = || {
        fs::read_dir(state.join("queue"))
            .unwrap()
            .map(|p| p.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect::<Vec<_>>()
    };
    let paths = queued();
    assert_eq!(paths.len(), 3, "Every offline update was saved");
    for path in &paths {
        let entry: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(entry["upstream"].is_null());
        assert_eq!(entry["request"]["command"], "update");
    }
    let began = Instant::now();
    worker.stop();
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "Stopping must not wait for queue delivery"
    );
    assert_eq!(queued().len(), 3);
    drop(broker);
    fs::remove_file(state.join("daemon.sock")).unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let restarted = broker_start();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut delivered = Vec::new();
    while delivered.len() < 3 {
        match bridge.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                if request["command"] == "update" {
                    assert_eq!(request["project"], "Offline worker QA");
                    delivered.push(request["question"].as_str().unwrap().to_owned());
                }
                stream
                    .write_all(br#"{"task_id":"replayed-offline-update","status":"pending"}"#)
                    .unwrap();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("Bridge failed: {e}"),
        }
        assert!(
            Instant::now() < deadline,
            "Queue did not replay all updates: {delivered:?}"
        );
    }
    for number in [1, 2, 3] {
        assert!(
            delivered
                .iter()
                .any(|text| text.contains(&format!("issue view {number}`"))),
            "Lost update for issue {number}"
        );
    }
    drop(restarted);
}

#[test]
fn worker_refreshes_order_before_each_reservation_and_preserves_tag_filters() {
    let f = Fixture::new("order-live");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&["--tag", "ready"]);
    f.cli(&["edit", "1", "--label", "ready"]);
    f.cli(&["create", "--title", "Second ready", "--label", "ready"]);
    f.cli(&["create", "--title", "Third ready", "--label", "ready"]);
    f.cli(&["create", "--title", "Unready backlog"]);
    let mut worker = f.worker();
    let running = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let run = running["runs"][0]["id"].as_str().unwrap();
    f.cli(&["move", "3", "--before", "2"]);
    f.cli(&["move", "4", "--before", "3"]);
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.control("stop", run);
    let result = f.wait(|s| {
        s["runs"]
            .as_array()
            .is_some_and(|r| r.len() == 3 && r.iter().all(|r| r["finished_at"].is_number()))
    });
    let turns: Vec<_> = f
        .transcript()
        .into_iter()
        .filter(|v| v["method"] == "turn/start")
        .map(|v| v["params"]["input"][0]["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(turns.len(), 3, "{result}");
    for (text, number) in turns.iter().zip([1, 3, 2]) {
        assert!(text.contains(&format!("issue view {number}`")), "{turns:?}");
    }
    assert_eq!(f.cli(&["view", "4"])["issue"]["state"], "open");
    assert_eq!(f.cli(&["view", "3"])["issue"]["state"], "closed");
    assert_eq!(f.cli(&["view", "2"])["issue"]["state"], "closed");
    worker.stop();
}

#[test]
fn worker_starts_with_saved_order_instead_of_creation_order() {
    let f = Fixture::new("order-start");
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.setup(&[]);
    f.cli(&["create", "--title", "Two"]);
    f.cli(&["create", "--title", "Three"]);
    f.cli(&["move", "3", "--before", "1"]);
    let mut worker = f.worker();
    f.wait(|s| {
        s["runs"]
            .as_array()
            .is_some_and(|r| r.len() == 3 && r.iter().all(|r| r["state"] == "completed"))
    });
    let turns: Vec<_> = f
        .transcript()
        .into_iter()
        .filter(|v| v["method"] == "turn/start")
        .map(|v| v["params"]["input"][0]["text"].as_str().unwrap().to_owned())
        .collect();
    for (text, number) in turns.iter().zip([3, 1, 2]) {
        assert!(text.contains(&format!("issue view {number}`")), "{turns:?}");
    }
    worker.stop();
}

#[test]
fn worker_skips_boss_assignment_until_human_releases_it() {
    let f = Fixture::new("boss-skip");
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.setup(&[]);
    f.cli(&["assign-to-boss", "1"]);
    f.cli(&[
        "create",
        "--title",
        "Agent work",
        "--body",
        "Pick this instead",
    ]);
    let mut w = f.worker();
    let s = f.wait(|s| {
        s["runs"].as_array().is_some_and(|runs| {
            runs.iter()
                .any(|r| r["number"] == 2 && r["finished_at"].is_number())
        })
    });
    assert!(
        s["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["number"] != 1)
    );
    assert_eq!(f.cli(&["view", "1"])["issue"]["assignee"], "human:boss");
    f.cli(&["unassign", "1", "--force"]);
    f.wait(|s| {
        s["runs"].as_array().is_some_and(|runs| {
            runs.iter()
                .any(|r| r["number"] == 1 && r["finished_at"].is_number())
        })
    });
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "closed");
    w.stop();
}

#[test]
fn parallel_worker_refreshes_subtask_readiness_before_reserving_parents() {
    let f = Fixture::new("subtasks-completed");
    f.setup(&["--concurrency", "2", "--prompt", "/goal"]);
    f.cli(&["subtask", "create", "1", "--title", "Intermediate"]);
    f.cli(&["subtask", "create", "2", "--title", "Nested leaf"]);
    f.cli(&["create", "--title", "Independent"]);
    f.cli(&["subtask", "create", "1", "--title", "Sibling leaf"]);
    let mut worker = f.worker();
    let status = f.wait(|s| {
        s["runs"].as_array().is_some_and(|runs| {
            runs.len() == 5 && runs.iter().all(|r| r["finished_at"].is_number())
        })
    });
    let runs = status["runs"].as_array().unwrap();
    assert!(runs.iter().all(|r| r["state"] == "completed"), "{status}");
    for (parent, child) in [(1, 2), (2, 3), (1, 5)] {
        let p = runs.iter().find(|r| r["number"] == parent).unwrap();
        let c = runs.iter().find(|r| r["number"] == child).unwrap();
        assert!(
            p["started_at"].as_i64().unwrap() >= c["finished_at"].as_i64().unwrap(),
            "Parent {parent} was reserved before child {child} completed: {status}"
        );
    }
    for number in 1..=5 {
        assert_eq!(
            f.cli(&["view", &number.to_string()])["issue"]["state"],
            "closed"
        );
    }
    let transcript = f.transcript();
    let starts: Vec<_> = transcript
        .iter()
        .filter(|v| v["method"] == "turn/start")
        .collect();
    assert_eq!(starts.len(), 5);
    let first: Vec<_> = starts
        .iter()
        .take(2)
        .map(|v| v["params"]["input"][0]["text"].as_str().unwrap())
        .collect();
    assert!(
        first.iter().any(|s| s.contains("issue view 3"))
            && first.iter().any(|s| s.contains("issue view 4")),
        "Fresh queue must reserve the first two ready issues: {first:?}"
    );
    assert_eq!(
        transcript
            .iter()
            .filter(|v| v["method"] == "thread/goal/set" && v["params"]["status"] == "complete")
            .count(),
        5
    );
    worker.stop();
}

#[test]
fn model_queue_wait_does_not_consume_manual_claim_deadline() {
    let f = Fixture::new("delay-model-start");
    f.setup(&["--claim-timeout", "5"]);
    let mut worker = f.worker();
    f.wait(|s| s["runs"][0]["state"] == "awaiting_model");
    thread::sleep(Duration::from_secs(5));
    let status = f.cli(&["worker", "status"]);
    assert!(status["runs"][0]["finished_at"].is_null());
    f.wait(|s| s["runs"][0]["state"] == "awaiting_claim");
    let status = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(status["runs"][0]["state"], "claim_timeout");
    worker.stop();
}

#[test]
fn writer_contention_does_not_exit_worker_or_kill_claimed_session() {
    let f = Fixture::new("writer-contention");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let running = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute_batch("BEGIN IMMEDIATE; UPDATE issues SET title=title WHERE number=1")
        .unwrap();
    thread::sleep(Duration::from_secs(12));
    assert!(
        worker.0.try_wait().unwrap().is_none(),
        "Worker exited during transient writer contention"
    );
    let current = f.cli(&["worker", "status"]);
    assert_eq!(current["runs"][0]["pid"], running["runs"][0]["pid"]);
    assert!(current["runs"][0]["finished_at"].is_null());
    db.execute_batch("ROLLBACK").unwrap();
    worker.stop();
}
