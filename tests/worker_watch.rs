use hey_boss::issues::{Store, worker::Settings};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hey-boss-watch-{}-{name}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        Store::open(&root.join("issues.db")).unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args(["worker", "--json"]);
        command
    }
    fn add_worker(&self, id: &str) {
        Store::open(&self.0.join("issues.db"))
            .unwrap()
            .register_worker(
                Some(id),
                &Settings {
                    name: format!("QA {id}"),
                    enabled: true,
                    ..Settings::default()
                },
                "watch-qa",
            )
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn watch_streams_fresh_inventory_then_exits_at_count_without_controls() {
    let fixture = Fixture::new("fresh");
    fixture.add_worker("first");
    let mut child = fixture
        .command()
        .args(["watch", "--count", "2"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(first["snapshots"].as_array().unwrap().len(), 1);
    fixture.add_worker("second");
    let second: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(second["snapshots"].as_array().unwrap().len(), 2);
    assert!(second["observed_at"].as_u64() > first["observed_at"].as_u64());
    assert!(lines.next().is_none());
    assert!(child.wait().unwrap().success());
    let db = rusqlite::Connection::open(fixture.0.join("issues.db")).unwrap();
    let controls: i64 = db
        .query_row("SELECT count(*) FROM issue_workers WHERE stop_requested<>0 OR json_extract(config,'$.enabled')<>1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(controls, 0);
}

#[test]
fn watch_handles_empty_queue_selection_and_termination() {
    let fixture = Fixture::new("empty");
    let output = fixture
        .command()
        .args(["watch", "--count", "1"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let empty: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(empty["snapshots"].as_array().unwrap().len(), 0);
    fixture.add_worker("chosen");
    fixture.add_worker("other");
    let output = fixture
        .command()
        .args(["--id", "chosen", "watch", "--count", "1"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let selected: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(selected["snapshots"].as_array().unwrap().len(), 1);
    assert_eq!(selected["snapshots"][0]["worker_id"], "chosen");
    let output = fixture
        .command()
        .args(["--id", "missing", "watch", "--count", "1"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let mut child = fixture
        .command()
        .arg("watch")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(3) {
            child.kill().unwrap();
            panic!("watch ignored termination");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
