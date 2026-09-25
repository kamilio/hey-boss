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
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-worker.mjs"),
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
fn authority_replies_arrive_while_a_replica_pull_waits_for_the_writer() {
    use serde_json::json;
    use std::sync::mpsc;

    let fixture = Fixture::new();
    fixture.issue();
    let mut child = Service(
        fixture
            .command(&["fleet", "companion", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let output = child.0.stdout.take().unwrap();
    let (frames, received) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if frames
                .send(serde_json::from_str::<Value>(&line.unwrap()).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let next = || received.recv_timeout(Duration::from_secs(15)).unwrap();
    assert_eq!(next()["kind"], "hello");
    writeln!(input, "{}", json!({"version":1,"kind":"configure","capabilities":{"authority_rpc":true},"controller":"fixture","revision":"empty","workers":[]})).unwrap();
    assert_eq!(next()["kind"], "ack");

    let db = hey_boss::database::Connection::connect(&fixture.root.join("issues.db")).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    writeln!(input, "{}", json!({"version":1,"kind":"pull","payload":{"changes":[],"cursor":42,"allocations":[],"ranges":[]},"receipts":[]})).unwrap();
    let progress = next();
    assert_eq!(progress["progress"], "pull");
    assert!(progress.get("cursor").is_none());
    // Heartbeats must not fill the bounded input queue behind a slow pull.
    for _ in 0..100 {
        writeln!(input, "{}", json!({"version":1,"kind":"ping"})).unwrap();
    }
    let mut client = UnixStream::connect(fixture.root.join("fleet-authority.sock")).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    writeln!(
        client,
        "{}",
        json!({"database":fixture.root.join("issues.db"),"request":{"kind":"status"}})
    )
    .unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();
    let request = next();
    assert_eq!(request["kind"], "authority_request");
    writeln!(input, "{}", json!({"version":1,"kind":"authority_reply","id":request["id"],"result":{"ok":true,"supervisor":"fixture"}})).unwrap();
    let mut response = String::new();
    let result = BufReader::new(client).read_line(&mut response);
    // Release the fixture writer even when the regression assertion fails.
    db.execute_batch("ROLLBACK").unwrap();
    result.expect("authority reply was delayed behind the replica writer");
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap()["supervisor"],
        "fixture"
    );
    loop {
        let frame = next();
        if frame.get("cursor").is_some() && frame["kind"] == "ack" {
            assert_eq!(frame["cursor"], 42);
            break;
        }
    }
    drop(input);
    assert!(child.0.wait().unwrap().success());
    reader.join().unwrap();
    assert!(!fixture.root.join("fleet-authority.sock").exists());
}

#[test]
fn authoritative_mindmaps_and_status_round_trip_over_the_existing_fleet_stream() {
    use std::os::unix::fs::PermissionsExt;
    let main = Fixture::new();
    let peer = Fixture::new();
    main.cli(&[
        "mm",
        "--project",
        "Authority",
        "--agent",
        "human:fixture",
        "--json",
        "add",
        "Authoritative root",
        "--id",
        "root",
    ]);
    fs::write(
        main.root.join("inventory.json"),
        r#"{"ssh_hosts":["fixture.test"]}"#,
    )
    .unwrap();
    let bin = main.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    fs::write(&ssh, "#!/bin/sh\nexport HEY_BOSS_ISSUE_DB=\"$AUTHORITY_PEER/issues.db\" HEY_BOSS_FLEET_STATE=\"$AUTHORITY_PEER\" HEY_BOSS_FLEET_CONFIG=\"$AUTHORITY_PEER/inventory.json\" HEY_BOSS_FLEET_DESIRED=\"$AUTHORITY_PEER/desired.json\"\n\"$HEY_BOSS_TEST_CLI\" fleet companion --stdio\nresult=$?; exit \"$result\"\n").unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let mut supervisor = Service(
        main.command(&["fleet", "supervisor"])
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("AUTHORITY_PEER", &peer.root)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let result = peer.command(&["fleet", "status"]).output().unwrap();
        if result.status.success() {
            let status: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert!(status["supervisor"].is_string());
            assert!(
                status["machines"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["host"] == "fixture.test")
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "relay never became ready: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        thread::sleep(Duration::from_millis(100));
    }
    let mm = |args: &[&str]| {
        let mut all = vec![
            "mm",
            "--project",
            "Authority",
            "--agent",
            "human:fixture",
            "--json",
        ];
        all.extend_from_slice(args);
        peer.cli(&all)
    };
    let initial = mm(&["show"]);
    assert_eq!(initial["nodes"][0]["title"], "Authoritative root");
    // Workers explicitly inherit the installed issue database. That is not a
    // private-store mismatch and must work with the ordinary fleet state path.
    let home = peer.root.join("home");
    fs::create_dir_all(home.join(".local/share")).unwrap();
    std::os::unix::fs::symlink(&peer.root, home.join(".local/share/hey-boss")).unwrap();
    let inherited = peer
        .command(&["mm", "--project", "Authority", "--json", "show"])
        .env("HOME", &home)
        .env_remove("HEY_BOSS_FLEET_STATE")
        .output()
        .unwrap();
    assert!(
        inherited.status.success(),
        "{}{}",
        String::from_utf8_lossy(&inherited.stdout),
        String::from_utf8_lossy(&inherited.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&inherited.stdout).unwrap(),
        initial
    );
    let added = mm(&[
        "add",
        "Companion child",
        "--id",
        "child",
        "--under",
        "root",
        "--request-id",
        "authority-child",
    ]);
    let replay = mm(&[
        "add",
        "Companion child",
        "--id",
        "child",
        "--under",
        "root",
        "--request-id",
        "authority-child",
    ]);
    assert_eq!(added, replay, "the authority owns idempotency receipts");
    let main_db = rusqlite::Connection::open(main.root.join("issues.db")).unwrap();
    let receipt_actor: String = main_db
        .query_row(
            "SELECT actor FROM requests WHERE request_id='authority-child'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        receipt_actor, "human:fixture",
        "the relay preserves the caller"
    );
    drop(main_db);
    let graph = mm(&["show"]);
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(
        graph["version"], 2,
        "map revision must not become the fleet protocol version"
    );
    assert_eq!(
        main.cli(&["mm", "--project", "Authority", "--json", "show"]),
        graph
    );
    let stale = peer
        .command(&[
            "mm",
            "--project",
            "Authority",
            "--agent",
            "human:fixture",
            "--json",
            "--if-version",
            "0",
            "edit",
            "child",
            "--title",
            "Stale",
        ])
        .output()
        .unwrap();
    assert_eq!(
        stale.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&stale.stdout)
    );
    let error: Value = serde_json::from_slice(&stale.stdout).unwrap();
    assert_eq!(error["error"]["code"], "conflict");
    // The viewer's topic inspector uses the same authority for linked documents
    // and file bytes; a map that loads with broken resource panels is incomplete.
    let artifact = peer.cli(&[
        "artifact",
        "--project",
        "Authority",
        "--agent",
        "human:fixture",
        "--json",
        "create",
        "--title",
        "Design notes",
        "--body",
        "From the companion",
        "--node",
        "child",
    ]);
    let artifact_id = artifact["artifact"]["id"].as_str().unwrap();
    assert_eq!(
        peer.cli(&[
            "artifact",
            "--project",
            "Authority",
            "--json",
            "view",
            artifact_id
        ])["artifact"]["body"],
        "From the companion"
    );
    let file = peer.root.join("design.txt");
    fs::write(&file, "Authoritative attachment\n").unwrap();
    let uploaded = peer.cli(&[
        "attachment",
        "--project",
        "Authority",
        "--agent",
        "human:fixture",
        "--json",
        "upload",
        file.to_str().unwrap(),
        "--node",
        "child",
    ]);
    let attachment_id = uploaded["attachment"]["id"].as_str().unwrap();
    let listed = peer.cli(&[
        "attachment",
        "--project",
        "Authority",
        "--json",
        "list",
        "--node",
        "child",
    ]);
    assert_eq!(listed["attachments"][0]["id"], attachment_id);
    let downloaded = peer.root.join("downloaded.txt");
    peer.cli(&[
        "attachment",
        "--project",
        "Authority",
        "--json",
        "download",
        attachment_id,
        "--output",
        downloaded.to_str().unwrap(),
    ]);
    assert_eq!(fs::read(&downloaded).unwrap(), fs::read(&file).unwrap());
    let peer_db = rusqlite::Connection::open(peer.root.join("issues.db")).unwrap();
    assert_eq!(
        peer_db
            .query_row("SELECT count(*) FROM artifacts", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(!peer.root.join("issues.attachments").exists());
    assert_eq!(
        peer_db
            .query_row("SELECT count(*) FROM mindmap_nodes", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        peer_db
            .query_row(
                "SELECT count(*) FROM requests WHERE request_id='authority-child'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    // Chief metadata uses the same authenticated stream, without a work claim.
    let main_db = rusqlite::Connection::open(main.root.join("issues.db")).unwrap();
    // Both fixture processes share a physical machine UUID. Give captured
    // authority history a distinct origin, as it has on a real second device.
    main_db
        .execute(
            "UPDATE fleet_meta SET node='authority-fixture-main' WHERE id=1",
            [],
        )
        .unwrap();
    let issue = |fixture: &Fixture, args: &[&str], code| {
        let mut all = vec![
            "issue",
            "--project",
            "Authority",
            "--agent",
            "codex:chief",
            "--json",
        ];
        all.extend_from_slice(args);
        let output = fixture.command(&all).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    issue(
        &main,
        &[
            "create",
            "--title",
            "Closed cleanup",
            "--label",
            "rework needed",
        ],
        0,
    );
    issue(&main, &["close", "1"], 0);
    issue(&main, &["create", "--title", "Live worker"], 0);
    main.cli(&[
        "issue",
        "--project",
        "Authority",
        "--agent",
        "codex:worker",
        "--json",
        "claim",
        "2",
    ]);
    main_db
        .execute(
            "INSERT INTO fleet_allocations VALUES('named:Authority',2,'another-machine') ON CONFLICT(project_id,issue_number) DO UPDATE SET node=excluded.node",
            [],
        )
        .unwrap();
    for number in ["1", "2"] {
        let before = issue(&peer, &["--supervisor", "view", number], 0);
        assert_eq!(before["store"]["host"], "supervisor");
        let version = before["issue"]["version"].to_string();
        let key = format!("metadata-{number}");
        let args = [
            "--supervisor",
            "edit",
            number,
            "--if-version",
            &version,
            "--label",
            "reviewed",
            "--remove-label",
            "rework needed",
            "--request-id",
            &key,
        ];
        let saved = issue(&peer, &args, 0);
        assert_eq!(saved, issue(&peer, &args, 0));
        assert_eq!(saved["issue"]["assignee"], before["issue"]["assignee"]);
        assert_eq!(saved["issue"]["state"], before["issue"]["state"]);
        assert_eq!(saved["issue"]["labels"], serde_json::json!(["reviewed"]));
        assert_eq!(issue(&main, &["view", number], 0)["issue"], saved["issue"]);
        let stale = issue(
            &peer,
            &[
                "--supervisor",
                "edit",
                number,
                "--if-version",
                &version,
                "--label",
                "stale",
                "--request-id",
                "stale",
            ],
            4,
        );
        assert_eq!(stale["error"]["code"], "conflict");
    }
    assert_eq!(
        main_db
            .query_row(
                "SELECT actor FROM requests WHERE request_id='metadata-2'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "codex:chief"
    );
    assert_eq!(
        main_db
            .query_row(
                "SELECT node FROM fleet_allocations WHERE issue_number=2",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "another-machine"
    );
    assert_eq!(
        peer_db
            .query_row(
                "SELECT count(*) FROM requests WHERE request_id LIKE 'metadata-%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    for args in [
        vec!["--supervisor", "claim", "2", "--force"],
        vec!["--supervisor", "reopen", "1", "--if-version", "3"],
        vec![
            "--supervisor",
            "edit",
            "2",
            "--label",
            "unguarded",
            "--request-id",
            "unsupported",
        ],
        vec![
            "--supervisor",
            "edit",
            "2",
            "--draft",
            "--request-id",
            "unsupported",
        ],
    ] {
        assert_eq!(issue(&peer, &args, 2)["error"]["code"], "invalid_input");
    }
    supervisor.terminate();
    assert_eq!(supervisor.0.try_wait().unwrap().unwrap().code(), Some(0));
    let deadline = Instant::now() + Duration::from_secs(10);
    while peer.root.join("fleet-authority.sock").exists() {
        assert!(
            Instant::now() < deadline,
            "relay socket survived transport shutdown"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let disconnected = issue(
        &peer,
        &[
            "--supervisor",
            "edit",
            "2",
            "--if-version",
            "3",
            "--label",
            "offline",
            "--request-id",
            "offline-metadata",
        ],
        1,
    );
    assert_eq!(disconnected["error"]["code"], "fleet_unavailable");
    assert!(
        disconnected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("same --request-id")
    );
    assert_eq!(
        issue(&main, &["view", "2"], 0)["issue"]["labels"],
        serde_json::json!(["reviewed"])
    );
    let offline = peer
        .command(&["mm", "--project", "Authority", "--json", "show"])
        .output()
        .unwrap();
    assert_eq!(offline.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&offline.stdout).unwrap();
    assert_eq!(error["error"]["code"], "fleet_unavailable");
}

#[test]
fn absent_fleet_status_explains_how_to_restore_the_service() {
    let f = Fixture::new();
    let output = f.command(&["fleet", "status"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("hey-boss fleet setup"), "{error}");
    f.issue();
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    db.execute("UPDATE fleet_meta SET role='agent' WHERE id=1", [])
        .unwrap();
    let output = f.command(&["fleet", "status"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("existing supervisor connection"), "{error}");
    assert!(
        !error.contains("Run hey-boss fleet setup"),
        "a companion must not promote itself: {error}"
    );
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
            "run",
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
            "run",
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
            "run",
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
    // This fixture changes a running standalone worker into a companion without
    // a supervisor pull. Issue-scoped steering must not acknowledge an edit
    // until the synthetic supervisor has supplied its allocation.
    writeln!(input, "{steering}").unwrap();
    input.flush().unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let denied: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(denied["result"]["ok"], false);
    assert!(
        denied["result"]["error"]
            .as_str()
            .unwrap()
            .contains("Changes were not saved")
    );
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM agent_steering", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    db.execute("INSERT INTO fleet_allocations SELECT project_id,number,node FROM issues CROSS JOIN fleet_meta WHERE number=1 AND fleet_meta.id=1", []).unwrap();
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
    let ack: Value = loop {
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let ack: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(ack["kind"], "ack");
        if ack["progress"] != "pull" {
            break ack;
        }
        assert!(
            ack.get("cursor").is_none(),
            "Progress must not acknowledge durable application"
        );
    };
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
            "run",
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
