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
            .env("HEY_BOSS_CODEX", self.0.join("missing-codex"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args(["worker", "--json"]);
        command
    }
    fn add_worker(&self, id: &str) {
        // Observing workers must not require a Codex installation. Seed saved
        // worker metadata without registering or launching a real worker.
        let config = serde_json::to_string(&Settings {
            name: format!("QA {id}"),
            enabled: true,
            ..Settings::default()
        })
        .unwrap();
        rusqlite::Connection::open(self.0.join("issues.db")).unwrap().execute(
            "INSERT INTO issue_workers(id,kind,config,version,owner_pid,updated_at) VALUES(?1,'cli',?2,1,?3,1)",
            rusqlite::params![id, config, std::process::id()],
        ).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn json_status_and_watch_limit_finished_attempts_without_hiding_live_work() {
    let fixture = Fixture::new("history");
    fixture.add_worker("chosen");
    let db = rusqlite::Connection::open(fixture.0.join("issues.db")).unwrap();
    db.execute(
        "INSERT INTO projects(id,name,next_number) VALUES('named:QA','QA',1)",
        [],
    )
    .unwrap();
    for (n, state, finished) in [
        (1, "running", None),
        (2, "awaiting_model", None),
        (3, "awaiting_claim", None),
        (4, "paused", None),
        (5, "approval_waiting", None),
        (6, "completed", Some(0)),
        (7, "blocked", Some(7)),
        (8, "cancelled", Some(8)),
        (9, "paused", Some(9)),
        (10, "approval_waiting", Some(10)),
    ] {
        db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,finished_at) VALUES(?1,'named:QA',?2,'{\"issue\":{\"title\":\"QA attempt\"}}','agent',?3,?4,'start','qa',?2,0,'chosen',?5)", rusqlite::params![format!("run-{n}"), n, state, std::process::id(), finished]).unwrap();
    }
    for history in [0, 1, 3, 20] {
        for action in ["status", "watch"] {
            let mut command = fixture.command();
            command.args(["--id", "chosen", "--history", &history.to_string(), action]);
            if action == "watch" {
                command.args(["--count", "1"]);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            let snapshot = if action == "watch" {
                &value["snapshots"][0]
            } else {
                &value
            };
            let runs = snapshot["runs"].as_array().unwrap();
            assert_eq!(
                runs.iter().filter(|r| r["finished_at"].is_null()).count(),
                5
            );
            let finished: Vec<_> = runs
                .iter()
                .filter(|r| !r["finished_at"].is_null())
                .map(|r| r["number"].as_i64().unwrap())
                .collect();
            assert_eq!(
                finished,
                (6..=10).rev().take(history).collect::<Vec<_>>(),
                "{action}, history {history}"
            );
            assert_eq!(snapshot["active"], 5);
            if action == "status" && history == 20 {
                use hey_boss::worker_tui::{
                    Dashboard,
                    backend::{Client, Request},
                    ui,
                };
                use ratatui::{Terminal, backend::TestBackend};
                use std::{
                    os::unix::fs::PermissionsExt,
                    sync::{Arc, atomic::AtomicBool},
                };
                let wrapper = fixture.0.join("dashboard-cli");
                fs::write(&wrapper, format!(
                    "#!/bin/sh\nexport HEY_BOSS_ISSUE_DB=\"$(dirname \"$0\")/issues.db\"\nunset HEY_BOSS_ISSUE_HOST HEY_BOSS_ISSUE_PROJECT\nexec '{}' \"$@\"\n",
                    env!("CARGO_BIN_EXE_hey-boss").replace('\'', "'\\''")
                )).unwrap();
                fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
                let client = Client {
                    binary: wrapper,
                    host: None,
                    directory: Some(fixture.0.clone()),
                    timeout: Duration::from_secs(10),
                };
                let refreshed = client
                    .execute(
                        &Request::Refresh(Some("chosen".into())),
                        &Arc::new(AtomicBool::new(false)),
                    )
                    .unwrap();
                assert_eq!(
                    refreshed["runs"].as_array().unwrap().len(),
                    10,
                    "The dashboard must request history explicitly"
                );
                let mut app = Dashboard::default();
                app.apply(refreshed);
                for (width, height) in [(48, 12), (80, 24), (120, 36)] {
                    for history_tab in [false, true] {
                        app.history = history_tab;
                        app.normalize_run();
                        assert_eq!(app.runs().len(), 5);
                        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                        terminal.draw(|frame| ui::render(frame, &app)).unwrap();
                        let buffer = terminal.backend().buffer();
                        let text: String = (0..height)
                            .map(|y| {
                                (0..width)
                                    .map(|x| buffer[(x, y)].symbol())
                                    .collect::<String>()
                                    + "\n"
                            })
                            .collect();
                        assert!(text.contains("QA attempt"), "{text}");
                        assert!(text.contains("q quit"), "{text}");
                        assert_eq!(text.contains(" · history"), history_tab, "{text}");
                        println!("{width} × {height}, history {history_tab}:\n{text}");
                    }
                }
            }
        }
    }
    let stored: i64 = db
        .query_row("SELECT count(*) FROM worker_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, 10, "Filtering must not delete saved attempts");
}

#[test]
fn worker_status_displays_unique_names_while_preserving_storage_keys() {
    let fixture = Fixture::new("project-names");
    fixture.add_worker("chosen");
    let local = "local:machine:/workspace/hey-gh";
    let db = rusqlite::Connection::open(fixture.0.join("issues.db")).unwrap();
    db.execute(
        "INSERT INTO projects(id,name,next_number) VALUES(?1,'hey-gh',1)",
        [local],
    )
    .unwrap();
    let config = serde_json::to_string(&Settings {
        projects: vec![local.into()],
        directories: [(local.into(), "/workspace/hey-gh".into())].into(),
        ..Settings::default()
    })
    .unwrap();
    db.execute(
        "UPDATE issue_workers SET config=?1 WHERE id='chosen'",
        [config],
    )
    .unwrap();
    let output = fixture
        .command()
        .args(["--id", "chosen", "status"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let saved: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(saved["config"]["projects"][0], local);
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&fixture.0)
        .env("HEY_BOSS_ISSUE_DB", fixture.0.join("issues.db"))
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .env_remove("HEY_BOSS_ISSUE_PROJECT")
        .args(["worker", "--id", "chosen", "status"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("Projects: hey-gh"), "{human}");
    assert!(
        human.contains("Checkout: hey-gh · /workspace/hey-gh"),
        "{human}"
    );
    assert!(!human.contains(local), "{human}");
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
