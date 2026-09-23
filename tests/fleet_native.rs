//! Fleet migration and shutdown tests run only against private fixture databases.
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        // Darwin's default temp directory leaves too little room for Unix sockets.
        let root = PathBuf::from("/tmp").join(format!(
            "hb-native-runtime-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("inventory.json"), "{\"ssh_hosts\":[]}").unwrap();
        fs::write(root.join("desired.json"), "{\"machines\":{}}").unwrap();
        Self { root }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.args(args)
            .current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &self.root)
            .env("HEY_BOSS_FLEET_CONFIG", self.root.join("inventory.json"))
            .env("HEY_BOSS_FLEET_DESIRED", self.root.join("desired.json"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_FLEET_SUPERVISED")
            .env(
                "HEY_BOSS_CODEX",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-worker.py"),
            )
            .env("HEY_BOSS_TEST_CLI", env!("CARGO_BIN_EXE_hey-boss"));
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        let output = self.command(args).output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn issue(&self) {
        self.cli(&[
            "issue",
            "--project",
            "Worker fixture",
            "--agent",
            "human:fixture",
            "--json",
            "create",
            "--title",
            "Keep running",
            "--body",
            "Fixture requirements",
        ]);
    }
    fn worker_status(&self) -> Value {
        self.cli(&[
            "worker",
            "--project",
            "Worker fixture",
            "--json",
            "--history",
            "20",
            "status",
        ])
    }
    fn wait_for(&self, condition: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = self.worker_status();
            if condition(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not reach expected state: {state}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
    fn service(&self, kind: &str) -> Service {
        Service(
            self.command(&["fleet", kind])
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Service(Child);
impl Service {
    fn terminate(&mut self) {
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.0.try_wait().unwrap().is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "fixture service failed to stop");
            thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn status_runs_without_python() {
    let f = Fixture::new();
    f.issue();
    let socket = f.root.join("fleet.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut line = String::new();
                    BufReader::new(stream.try_clone().unwrap())
                        .read_line(&mut line)
                        .unwrap();
                    let v: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(v["kind"], "status");
                    stream.write_all(b"{\"ok\":true,\"machines\":[]}").unwrap();
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        }
    });
    let output = f
        .command(&["fleet", "status"])
        .env("PATH", f.root.join("no-python"))
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "fleet still requires Python: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["ok"],
        true
    );
}

#[test]
fn takeover_stops_only_selected_agent_and_keeps_issue_out_of_pickup() {
    let f = Fixture::new();
    f.issue();
    f.issue();
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    let mut worker = Service(
        f.command(&[
            "worker",
            "--project",
            "Worker fixture",
            "--directory",
            f.root.to_str().unwrap(),
            "--concurrency",
            "2",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap(),
    );
    let running = f.wait_for(|s| {
        s["runs"]
            .as_array()
            .is_some_and(|runs| runs.iter().filter(|r| r["state"] == "running").count() == 2)
    });
    let run = running["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == 1)
        .unwrap();
    let mut supervisor = f.service("supervisor");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f.root.join("fleet.sock").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(50));
    }
    let request = serde_json::json!({"kind":"takeover","host":"local","run":run["id"]});
    let call = || {
        let mut stream = UnixStream::connect(f.root.join("fleet.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        writeln!(stream, "{request}").unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        serde_json::from_slice::<Value>(&bytes).unwrap()
    };
    let first = call();
    assert_eq!(first["ok"], true);
    assert_eq!(first["stopped"], false);
    assert!(first["resume_command"].is_null());
    let stopped = f.wait_for(|s| {
        s["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["number"] == 1 && r["finished_at"].is_number())
    });
    assert_eq!(stopped["active"], 1);
    assert_eq!(stopped["eligible"], 0);
    assert!(
        stopped["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["number"] == 2
                && r["state"] == "running"
                && !r["stop_requested"].as_bool().unwrap())
    );
    let issue = f.cli(&[
        "issue",
        "--project",
        "Worker fixture",
        "--json",
        "view",
        "1",
    ]);
    assert_eq!(issue["issue"]["assignee"], "human:boss");
    assert_eq!(issue["issue"]["state"], "open");
    let resumed = call();
    assert_eq!(resumed["stopped"], true);
    let command = resumed["resume_command"].as_str().unwrap();
    assert!(command.contains(f.root.to_str().unwrap()));
    assert!(command.contains(run["session_id"].as_str().unwrap()));
    assert!(command.contains("codex resume"));
    assert!(command.contains("--approve-for-me"));
    assert_eq!(
        unsafe { libc::kill(run["pid"].as_u64().unwrap() as i32, 0) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    worker.terminate();
    supervisor.terminate();
}

#[test]
fn supervisor_shutdown_preserves_running_worker_and_agent() {
    let f = Fixture::new();
    f.issue();
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    let mut worker = Service(
        f.command(&[
            "worker",
            "--project",
            "Worker fixture",
            "--directory",
            f.root.to_str().unwrap(),
            "--json",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap(),
    );
    let running = f.wait_for(|s| {
        s["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| r["state"] == "running")
    });
    let run = running["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["state"] == "running")
        .unwrap()
        .clone();
    let mut supervisor = f.service("supervisor");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f.root.join("fleet.sock").exists() {
        assert!(Instant::now() < deadline, "supervisor did not start");
        thread::sleep(Duration::from_millis(50));
    }
    let status = f.cli(&["fleet", "status"]);
    assert_eq!(status["machines"][0]["workers"][0]["pid"], worker.0.id());
    supervisor.terminate();
    assert!(
        worker.0.try_wait().unwrap().is_none(),
        "supervisor stopped worker"
    );
    let after = f.worker_status();
    assert_eq!(after["runs"][0]["id"], run["id"]);
    assert_eq!(after["runs"][0]["state"], "running");
    assert_eq!(after["runs"][0]["stop_requested"], false);
    // Explicitly stop only our synthetic worker after checking the migration invariant.
    f.cli(&[
        "worker",
        "--json",
        "stop",
        running["worker_id"].as_str().unwrap(),
    ]);
    worker.0.wait().unwrap();
}

#[test]
fn companion_takeover_acknowledges_stop_and_journals_boss_assignment() {
    let f = Fixture::new();
    f.issue();
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    let mut worker = Service(
        f.command(&[
            "worker",
            "--project",
            "Worker fixture",
            "--directory",
            f.root.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap(),
    );
    let status = f.wait_for(|s| {
        s["runs"]
            .as_array()
            .is_some_and(|runs| runs.iter().any(|r| r["state"] == "running"))
    });
    let run = &status["runs"][0];
    let mut companion = Service(
        f.command(&["fleet", "companion", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let mut output = BufReader::new(companion.0.stdout.take().unwrap());
    let mut input = companion.0.stdin.take().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["kind"],
        "hello"
    );
    let request =
        serde_json::json!({"version":1,"kind":"takeover","id":"takeover-fixture","run":run["id"]});
    let steering = serde_json::json!({"version":1,"kind":"steer","id":"steer-fixture","request_id":"companion-steering","run":run["id"],"scope":"issue","text":"Check keyboard navigation"});
    for _ in 0..2 {
        writeln!(input, "{steering}").unwrap();
        input.flush().unwrap();
        line.clear();
        output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["kind"], "steer");
        assert_eq!(response["id"], "steer-fixture");
        assert_eq!(response["result"]["ok"], true);
    }
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM agent_steering", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let body: String = db
        .query_row("SELECT body FROM issues WHERE number=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(body.matches("Check keyboard navigation").count(), 1);
    for stopped in [false, true] {
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        line.clear();
        output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["kind"], "takeover");
        assert_eq!(response["id"], "takeover-fixture");
        assert_eq!(response["result"]["ok"], true);
        assert_eq!(response["result"]["stopped"], stopped);
        if !stopped {
            f.wait_for(|s| s["runs"][0]["finished_at"].is_number());
        } else {
            assert!(
                response["result"]["resume_command"]
                    .as_str()
                    .unwrap()
                    .contains(run["session_id"].as_str().unwrap())
            );
        }
    }
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM fleet_outbox WHERE table_name='issues' AND json_extract(after_json,'$.assignee')='human:boss')",[],|r|r.get::<_,bool>(0)).unwrap());
    drop(input);
    companion.0.wait().unwrap();
    worker.terminate();
}

#[test]
fn streamed_snapshot_survives_disconnect_and_preserves_edits_made_during_transfer() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use flate2::{Compression, write::GzEncoder};
    use sha2::{Digest, Sha256};
    let source = Fixture::new();
    source.issue();
    let database = |request: Value| {
        let mut child = source
            .command(&[
                "fleet",
                "database",
                "--path",
                source.root.join("issues.db").to_str().unwrap(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["ok"], true, "{response}");
        response
    };
    database(serde_json::json!({"replica":"capture","role":"controller","node":"source"}));
    let db = rusqlite::Connection::open(source.root.join("issues.db")).unwrap();
    db.execute_batch("BEGIN;
        WITH RECURSIVE n(number) AS (VALUES(2) UNION ALL SELECT number+1 FROM n WHERE number<101)
        INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels)
        SELECT 'named:Worker fixture',number,'Imported issue',hex(zeroblob(100000)),'open','human:fixture',1,1,1,'[]' FROM n;
        UPDATE issues SET title='Canonical title' WHERE number=1;
        UPDATE projects SET next_number=102;
        COMMIT;").unwrap();
    drop(db);
    let snapshot = database(serde_json::json!({"replica":"snapshot","node":"target"}));
    let payload: Value = serde_json::from_str(snapshot["rows"][0][0].as_str().unwrap()).unwrap();
    let cursor = payload["cursor"].clone();
    let frame = serde_json::json!({"version":1,"kind":"pull","payload":payload,"receipts":[]});
    let encoded = serde_json::to_vec(&frame).unwrap();
    assert!(encoded.len() > hey_boss::issues::WIRE_LIMIT);
    let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
    gzip.write_all(&encoded).unwrap();
    let compressed = gzip.finish().unwrap();
    let hash = format!("{:x}", Sha256::digest(&compressed));
    let parts: Vec<_> = compressed
        .chunks(16384)
        .map(|chunk| STANDARD.encode(chunk))
        .collect();
    assert!(parts.len() > 2);
    let target = Fixture::new();
    target.issue();
    let db = rusqlite::Connection::open(target.root.join("issues.db")).unwrap();
    db.execute("UPDATE fleet_meta SET role='agent'", [])
        .unwrap();
    drop(db);
    let spawn = || {
        let mut child = Service(
            target
                .command(&["fleet", "companion", "--stdio"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let hello: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(hello["capabilities"]["pull_gzip_chunks"], true);
        assert_eq!(
            hello["build"],
            concat!(
                "hey-boss ",
                env!("CARGO_PKG_VERSION"),
                " (build ",
                env!("HEY_BOSS_BUILD_ID"),
                ")"
            )
        );
        let input = child.0.stdin.take().unwrap();
        (child, input, output)
    };
    let send = |input: &mut std::process::ChildStdin, frame: Value| {
        writeln!(input, "{frame}").unwrap();
        input.flush().unwrap();
    };
    let begin = serde_json::json!({"version":1,"kind":"pull_begin","transfer":"test","encoding":"gzip-base64"});
    let part = |index: usize| serde_json::json!({"version":1,"kind":"pull_chunk","transfer":"test","index":index,"data":parts[index]});
    let (mut interrupted, mut input, _) = spawn();
    send(&mut input, begin.clone());
    send(&mut input, part(0));
    drop(input);
    assert!(interrupted.0.wait().unwrap().success());
    let db = rusqlite::Connection::open(target.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT title FROM issues WHERE number=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Keep running"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM fleet_state WHERE key='cursor'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    drop(db);
    let (mut resumed, mut input, mut output) = spawn();
    send(&mut input, begin);
    send(&mut input, part(0));
    let db = rusqlite::Connection::open(target.root.join("issues.db")).unwrap();
    db.execute(
        "UPDATE issues SET title='Edit during transfer',version=version+1 WHERE number=1",
        [],
    )
    .unwrap();
    drop(db);
    for index in 1..parts.len() {
        send(&mut input, part(index));
    }
    send(
        &mut input,
        serde_json::json!({"version":1,"kind":"pull_end","transfer":"test","parts":parts.len(),"bytes":compressed.len(),"sha256":hash}),
    );
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let ack: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(ack["kind"], "ack");
    assert_eq!(ack["cursor"], cursor);
    let db = rusqlite::Connection::open(target.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        101
    );
    assert_eq!(
        db.query_row("SELECT title FROM issues WHERE number=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Edit during transfer"
    );
    assert_eq!(
        db.query_row("SELECT length(body) FROM issues WHERE number=2", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        200000
    );
    assert!(
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM fleet_outbox WHERE table_name='issues')",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    drop(db);
    drop(input);
    assert!(resumed.0.wait().unwrap().success());
}

#[test]
fn companion_protocol_runs_without_python_and_eof_leaves_execution_independent() {
    let f = Fixture::new();
    f.issue();
    let mut companion = f
        .command(&["fleet", "companion", "--stdio"])
        .env("PATH", f.root.join("no-python"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(companion.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let hello: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(hello["version"], 1);
    assert_eq!(hello["kind"], "hello");
    assert!(hello["workers"].as_array().unwrap().is_empty());
    let mut input = companion.stdin.take().unwrap();
    input
        .write_all(b"{\"version\":1,\"kind\":\"ping\"}\n")
        .unwrap();
    input.flush().unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let heartbeat: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(heartbeat["kind"], "heartbeat");
    assert_eq!(heartbeat["version"], 1);
    drop(input);
    assert!(companion.wait().unwrap().success());
}

#[test]
fn companion_shutdown_preserves_existing_worker_and_claimed_agent() {
    let f = Fixture::new();
    f.issue();
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    let mut worker = Service(
        f.command(&[
            "worker",
            "--project",
            "Worker fixture",
            "--directory",
            f.root.to_str().unwrap(),
            "--json",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap(),
    );
    let running = f.wait_for(|s| {
        s["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| r["state"] == "running")
    });
    fs::write(f.root.join("fleet-agent.json"),serde_json::to_vec(&serde_json::json!({"role":"agent","controller":"fixture","revision":"fixture","workers":[{"id":running["worker_id"],"config":running["config"],"intent":"running"}]})).unwrap()).unwrap();
    let mut companion = f.service("companion");
    thread::sleep(Duration::from_millis(500));
    assert!(companion.0.try_wait().unwrap().is_none());
    companion.terminate();
    assert!(worker.0.try_wait().unwrap().is_none());
    let after = f.worker_status();
    assert_eq!(after["runs"][0]["id"], running["runs"][0]["id"]);
    assert_eq!(after["runs"][0]["state"], "running");
    assert_eq!(after["runs"][0]["stop_requested"], false);
    f.cli(&[
        "worker",
        "--json",
        "stop",
        running["worker_id"].as_str().unwrap(),
    ]);
    worker.0.wait().unwrap();
}

#[test]
fn replaying_legacy_worker_configuration_does_not_rewrite_defaults() {
    let f = Fixture::new();
    f.issue();
    let mut companion = f
        .command(&["fleet", "companion", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(companion.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let mut input = companion.stdin.take().unwrap();
    let frame = serde_json::json!({"version":1,"kind":"configure","controller":"fixture","revision":"legacy","workers":[{"id":"legacy-worker","intent":"pause","config":{"concurrency":1,"directory":f.root,"enabled":false,"projects":["named:Worker fixture"],"tags":[]}}]});
    let mut versions = vec![];
    for _ in 0..2 {
        writeln!(input, "{frame}").unwrap();
        input.flush().unwrap();
        line.clear();
        output.read_line(&mut line).unwrap();
        let ack: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(ack["revision"], "legacy", "{ack}");
        versions.push(f.worker_status()["workers"][0]["version"].clone());
    }
    drop(input);
    assert!(companion.wait().unwrap().success());
    assert_eq!(
        versions[0], versions[1],
        "Repeated configuration must be a no-op"
    );
}

#[test]
fn large_status_response_is_complete_over_the_local_socket() {
    let f = Fixture::new();
    f.issue();
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS fleet_state(key TEXT PRIMARY KEY,value TEXT NOT NULL)",
    )
    .unwrap();
    let saved = serde_json::json!({"offline-fixture":{"host":"offline-fixture","hostname":"h".repeat(65536),"workers":[]}});
    db.execute(
        "INSERT INTO fleet_state VALUES('machines',?1)",
        [saved.to_string()],
    )
    .unwrap();
    drop(db);
    let mut supervisor = f.service("supervisor");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f.root.join("fleet.sock").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(50));
    }
    let status = f.cli(&["fleet", "status"]);
    let machine = status["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["host"] == "offline-fixture")
        .unwrap();
    assert_eq!(machine["hostname"].as_str().unwrap().len(), 65536);
    supervisor.terminate();
}
