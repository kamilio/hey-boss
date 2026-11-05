use hey_boss::event_log::{self, Event};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-event-cli-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        fs::create_dir(&root).unwrap();
        let db = event_log::open(&root.join("issues.db")).unwrap();
        for (id, kind, subject) in [
            ("first", "agent.failed", "run-1"),
            ("second", "machine.disk_low", "/"),
        ] {
            event_log::append(
                &db,
                &Event {
                    id: id.into(),
                    kind: kind.into(),
                    version: 1,
                    origin: "devbox".into(),
                    time: 100,
                    project: None,
                    subject: subject.into(),
                    data: json!({"code":"ENOSPC"}),
                },
            )
            .unwrap();
        }
        Self(root)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(args)
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", self.0.join("fleet"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .output()
            .unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}",
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
fn list_show_and_resume_are_readable_and_machine_consumable() {
    let f = Fixture::new();
    let text = f.run(&["events"]);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("agent.failed") && text.contains("devbox") && text.contains("run-1"));
    let page = f.json(&["events", "list", "--type", "agent.*", "--json"]);
    assert_eq!(page["events"].as_array().unwrap().len(), 1);
    assert_eq!(page["events"][0]["id"], "first");
    let next = f.json(&[
        "events",
        "list",
        "--after",
        page["cursor"].as_str().unwrap(),
        "--json",
    ]);
    assert_eq!(next["events"], json!([]));
    let detail = f.json(&["events", "show", "first", "--json"]);
    assert_eq!(detail["data"]["code"], "ENOSPC");
    let types = f.json(&["events", "types", "--json"]);
    assert!(
        types
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["type"] == "agent.started" && v["count"] == 0)
    );
    let types = f.json(&[
        "events",
        "types",
        "--type",
        "machine.*",
        "--machine",
        "another",
        "--json",
    ]);
    assert!(
        types.as_array().unwrap().iter().all(|v| v["type"]
            .as_str()
            .unwrap()
            .starts_with("machine.")
            && v["count"] == 0)
    );
    let types = f.json(&["events", "types", "--json"]);
    assert!(
        types
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["type"] == "agent.failed")
    );
}

#[test]
fn invalid_cursors_fail_explicitly_in_json_instead_of_replaying_history() {
    let f = Fixture::new();
    let output = f.run(&["events", "list", "--after", "wrong:2", "--json"]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["ok"], false);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("replica")
    );
}

#[test]
fn follow_delivers_late_events_after_its_checkpoint_and_stops_on_interrupt() {
    use std::{
        io::{BufRead, BufReader},
        process::Stdio,
        sync::mpsc,
        time::Duration,
    };
    let f = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["events", "follow", "--type", "agent.*", "--json"])
        .env("HEY_BOSS_ISSUE_DB", f.0.join("issues.db"))
        .env("HEY_BOSS_FLEET_STATE", f.0.join("fleet"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if send
                .send(serde_json::from_str::<Value>(&line).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let result = || {
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(10)).unwrap()["id"],
            "first"
        );
        assert!(receive.recv_timeout(Duration::from_secs(10)).unwrap()["checkpoint"].is_string());
        let db = event_log::open(&f.0.join("issues.db")).unwrap();
        event_log::append(
            &db,
            &Event {
                id: "late".into(),
                kind: "agent.failed".into(),
                version: 1,
                origin: "devbox".into(),
                time: 1,
                project: None,
                subject: "run-2".into(),
                data: json!({"code":"ENOSPC"}),
            },
        )
        .unwrap();
        let event = receive.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(event["id"], "late");
        assert!(event["cursor"].is_string());
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(result));
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let status = child.wait().unwrap();
    reader.join().unwrap();
    outcome.unwrap();
    assert!(status.success());
}
