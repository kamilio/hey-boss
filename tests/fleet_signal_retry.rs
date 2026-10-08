//! Real CLI + supervisor with a proxy that loses an accepted mutation's reply.
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    supervisor: Option<Child>,
    companion_owner: Option<hey_boss::database::Owner>,
}
impl Fixture {
    fn new() -> Self {
        static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = PathBuf::from(format!(
            "/tmp/hb-signal-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("client")).unwrap();
        fs::write(root.join("inventory.json"), r#"{"ssh_hosts":[]}"#).unwrap();
        // No declared worker can launch; only durable signals are exercised.
        fs::write(root.join("desired.json"), r#"{"machines":{}}"#).unwrap();
        let mut fixture = Self {
            root,
            supervisor: None,
            companion_owner: None,
        };
        fixture.supervisor = Some(
            fixture
                .command(&["fleet", "supervisor"])
                .env("HEY_BOSS_FLEET_STATE", &fixture.root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while !fixture.root.join("fleet.sock").exists() {
            assert!(Instant::now() < deadline, "supervisor did not start");
            thread::sleep(Duration::from_millis(20));
        }
        fixture
    }
    fn companion(&mut self) {
        let database = self.root.join("client/issues.db");
        self.companion_owner = Some(hey_boss::database::Owner::host(&database).unwrap());
        // Context creates metadata before reporting the absent relay.
        let _ = self.command(&["fleet", "status"]).output().unwrap();
        rusqlite::Connection::open(database)
            .unwrap()
            .execute("UPDATE fleet_meta SET role='agent' WHERE id=1", [])
            .unwrap();
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .args(args)
            .current_dir(&self.root)
            .env(
                "HEY_BOSS_ISSUE_DB",
                self.root.join(if self.companion_owner.is_some() {
                    "client/issues.db"
                } else {
                    "issues.db"
                }),
            )
            .env("HEY_BOSS_FLEET_STATE", self.root.join("client"))
            .env("HEY_BOSS_FLEET_CONFIG", self.root.join("inventory.json"))
            .env("HEY_BOSS_FLEET_DESIRED", self.root.join("desired.json"))
            .env_remove("HEY_BOSS_ISSUE_HOST");
        command
    }
    fn run(&self, args: &[&str], drop_reply: bool) -> (Output, Value) {
        let companion = self.companion_owner.is_some();
        let socket = self.root.join(if companion {
            "client/fleet-authority.sock"
        } else {
            "client/fleet.sock"
        });
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let upstream = self.root.join("fleet.sock");
        let proxy = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut client = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Value::Null;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            BufReader::new(&mut client)
                .read_until(b'\n', &mut request)
                .unwrap();
            if companion {
                let envelope: Value = serde_json::from_slice(&request).unwrap();
                assert_eq!(envelope["request"]["kind"], "worker_signal");
                let mut signal = envelope["request"].clone();
                signal["kind"] = json!("signal");
                request = serde_json::to_vec(&signal).unwrap();
                request.push(b'\n');
            }
            let mut server = UnixStream::connect(upstream).unwrap();
            server
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            server.write_all(&request).unwrap();
            server.shutdown(std::net::Shutdown::Write).unwrap();
            let mut reply = Vec::new();
            server.read_to_end(&mut reply).unwrap();
            let receipt: Value = serde_json::from_slice(&reply).unwrap();
            if companion && receipt["ok"] == false {
                reply = serde_json::to_vec(
                    &json!({"ok":false,"error":{"code":"fleet_error","message":receipt["error"]}}),
                )
                .unwrap();
            }
            if !drop_reply {
                client.write_all(&reply).unwrap();
            }
            receipt
        });
        let output = self.command(args).output().unwrap();
        let receipt = proxy.join().unwrap();
        fs::remove_file(socket).unwrap();
        (output, receipt)
    }
    fn signals(&self) -> Vec<Value> {
        let output = self
            .command(&["fleet", "status", "--records", "signals"])
            .env("HEY_BOSS_FLEET_STATE", &self.root)
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        value["records"].as_array().unwrap().clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = &mut self.supervisor {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.companion_owner.take();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn both_cli_entry_points_preserve_receipts_after_a_lost_response() {
    for (worker, companion) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut fixture = Fixture::new();
        if companion {
            fixture.companion();
        }
        let args = if worker {
            vec![
                "worker",
                "--json",
                "restart",
                "fixture-worker",
                "--request-id",
                "restart-once",
            ]
        } else {
            vec![
                "fleet",
                "signal",
                "local",
                "fixture-worker",
                "restart",
                "--request-id",
                "restart-once",
            ]
        };
        let (lost, accepted) = fixture.run(&args, true);
        assert!(!lost.status.success());
        let error = format!(
            "{}{}",
            String::from_utf8_lossy(&lost.stdout),
            String::from_utf8_lossy(&lost.stderr)
        );
        assert!(error.contains("Outcome unknown"), "{error}");
        assert!(error.contains("--request-id restart-once"), "{error}");
        assert_eq!(accepted["id"], "restart-once");
        assert_eq!(fixture.signals().len(), 1);
        let (retry, receipt) = fixture.run(&args, false);
        assert!(
            retry.status.success(),
            "{}",
            String::from_utf8_lossy(&retry.stderr)
        );
        assert_eq!(receipt, accepted);
        assert_eq!(
            serde_json::from_slice::<Value>(&retry.stdout).unwrap(),
            accepted
        );
        assert_eq!(fixture.signals().len(), 1);
        let changed_args = if worker {
            vec![
                "worker",
                "--json",
                "restart",
                "different-worker",
                "--request-id",
                "restart-once",
            ]
        } else {
            vec![
                "fleet",
                "signal",
                "local",
                "fixture-worker",
                "stop",
                "--request-id",
                "restart-once",
            ]
        };
        let (changed, rejected) = fixture.run(&changed_args, false);
        assert!(!changed.status.success());
        assert_eq!(rejected["ok"], false);
        assert!(
            rejected["error"]
                .as_str()
                .unwrap()
                .contains("different payload")
        );
        assert_eq!(fixture.signals().len(), 1);
        let (new, receipt) = fixture.run(
            &[
                "worker",
                "--json",
                "restart",
                "fixture-worker",
                "--request-id",
                "restart-again",
            ],
            false,
        );
        assert!(new.status.success());
        assert_eq!(receipt["id"], "restart-again");
        assert_eq!(fixture.signals().len(), 2);
    }
}

#[test]
fn generated_ids_are_visible_on_failure_and_new_for_each_invocation() {
    let fixture = Fixture::new();
    let mut ids = vec![];
    for args in [
        vec!["fleet", "signal", "local", "fixture-worker", "restart"],
        vec!["worker", "--json", "restart", "fixture-worker"],
    ] {
        let (lost, receipt) = fixture.run(&args, true);
        assert!(!lost.status.success());
        let id = receipt["id"].as_str().unwrap();
        assert!(String::from_utf8_lossy(&lost.stderr).contains(&format!("Request ID: {id}")));
        let mut retry_args = args.clone();
        retry_args.extend(["--request-id", id]);
        let (retry, recovered) = fixture.run(&retry_args, false);
        assert!(retry.status.success());
        assert_eq!(recovered, receipt);
        ids.push(id.to_owned());
    }
    assert_ne!(ids[0], ids[1]);
    assert_eq!(fixture.signals().len(), 2);
    let failed = fixture
        .command(&[
            "worker",
            "--json",
            "restart",
            "fixture-worker",
            "--request-id",
            "not-sent",
        ])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    let error: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(
        error["error"]["details"],
        json!({"request_id":"not-sent", "outcome":"not_sent"})
    );
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Not sent")
    );
    assert_eq!(fixture.signals().len(), 2);
}

#[test]
fn malformed_companion_acknowledgment_preserves_the_unknown_outcome() {
    let mut fixture = Fixture::new();
    fixture.companion();
    let listener = UnixListener::bind(fixture.root.join("client/fleet-authority.sock")).unwrap();
    let relay = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(&mut stream).read_line(&mut line).unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["request"]["id"], "malformed-reply");
        stream
            .write_all(b"{\"ok\":false,\"error\":null}\n")
            .unwrap();
    });
    let output = fixture
        .command(&[
            "worker",
            "--json",
            "restart",
            "fixture-worker",
            "--request-id",
            "malformed-reply",
        ])
        .output()
        .unwrap();
    relay.join().unwrap();
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["error"]["details"],
        json!({"request_id":"malformed-reply","outcome":"unknown"})
    );
}
