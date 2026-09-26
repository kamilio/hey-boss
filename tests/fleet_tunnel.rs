//! Ordinary draft edits must reach the authority without reverse SSH or a claim.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Fleet {
    root: PathBuf,
    services: Vec<Child>,
}
impl Fleet {
    fn new() -> Self {
        let root = PathBuf::from(format!("/tmp/hb-tunnel-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        for side in ["main", "peer", "bin"] {
            fs::create_dir(root.join(side)).unwrap();
        }
        for side in ["main", "peer"] {
            fs::write(
                root.join(side).join("inventory.json"),
                if side == "main" {
                    r#"{"ssh_hosts":["fixture.invalid"]}"#
                } else {
                    r#"{"ssh_hosts":[]}"#
                },
            )
            .unwrap();
            fs::write(root.join(side).join("desired.json"), r#"{"machines":{}}"#).unwrap();
        }
        let mut fleet = Self {
            root,
            services: vec![],
        };
        // Own the database services too, so no fixture daemon survives the test.
        for side in ["main", "peer"] {
            let service = fleet
                .command(side, &["fleet", "companion"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            fleet.services.push(service);
            let socket = fleet.database_socket(side);
            let deadline = Instant::now() + Duration::from_secs(30);
            while !socket.exists() {
                assert!(Instant::now() < deadline, "Database owner did not start");
                thread::sleep(Duration::from_millis(50));
            }
        }
        fleet
    }
    fn database_socket(&self, side: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        let path = self
            .root
            .canonicalize()
            .unwrap()
            .join(side)
            .join("issues.db");
        let key = format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()));
        PathBuf::from(format!(
            "/tmp/hey-boss-db-{}/{}.sock",
            unsafe { libc::getuid() },
            &key[..24]
        ))
    }
    fn command(&self, side: &str, args: &[&str]) -> Command {
        let path = self.root.join(side);
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.args(args)
            .current_dir(&path)
            .env("HEY_BOSS_ISSUE_DB", path.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &path)
            .env("HEY_BOSS_FLEET_CONFIG", path.join("inventory.json"))
            .env("HEY_BOSS_FLEET_DESIRED", path.join("desired.json"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT");
        c
    }
    fn cli(&self, side: &str, args: &[&str], expected: i32) -> Value {
        let out = self.command(side, args).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn issue(&self, side: &str, args: &[&str], expected: i32) -> Value {
        let mut all = vec![
            "issue",
            "--project",
            "Tunnel QA",
            "--agent",
            "codex:tunnel-test",
            "--json",
        ];
        all.extend_from_slice(args);
        self.cli(side, &all, expected)
    }
    fn sql(&self, side: &str, sql: &str) -> Value {
        let path = self.root.join(side).join("issues.db");
        let mut c = self
            .command(
                side,
                &["fleet", "database", "--path", path.to_str().unwrap()],
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(c.stdin.take().unwrap(), "{}", json!({"sql":sql,"args":[]})).unwrap();
        let out = c.wait_with_output().unwrap();
        assert!(out.status.success());
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["ok"], true, "{value}");
        value["rows"].clone()
    }
    fn start(&mut self) {
        let ssh = self.root.join("bin/ssh");
        fs::write(&ssh, "#!/bin/sh\nexport HEY_BOSS_ISSUE_DB=\"$TUNNEL_PEER/issues.db\" HEY_BOSS_FLEET_STATE=\"$TUNNEL_PEER\" HEY_BOSS_FLEET_CONFIG=\"$TUNNEL_PEER/inventory.json\" HEY_BOSS_FLEET_DESIRED=\"$TUNNEL_PEER/desired.json\"\nexec \"$TUNNEL_BINARY\" fleet companion --stdio\n").unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let service = self
            .command("main", &["fleet", "supervisor"])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("TUNNEL_PEER", self.root.join("peer"))
            .env("TUNNEL_BINARY", env!("CARGO_BIN_EXE_hey-boss"))
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        self.services.push(service);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let out = self
                .command("peer", &["fleet", "capabilities"])
                .output()
                .unwrap();
            let ready = serde_json::from_slice::<Value>(&out.stdout).unwrap_or(Value::Null);
            if out.status.success()
                && ready["route"] == "supervisor_tunnel"
                && ready["capabilities"]["issue_draft"] == true
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Tunnel not connected: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
}
impl Drop for Fleet {
    fn drop(&mut self) {
        for service in self.services.iter_mut().rev() {
            unsafe {
                libc::kill(service.id() as i32, libc::SIGTERM);
            }
            let _ = service.wait();
        }
        for side in ["main", "peer"] {
            let socket = self.database_socket(side);
            for extension in ["sock", "lock", "startup", "log"] {
                let _ = fs::remove_file(socket.with_extension(extension));
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn connected_tunnel_guards_drafts_and_reopen_without_reverse_ssh() {
    let mut f = Fleet::new();
    f.start();
    // Two fixture processes share a physical UUID; real fleet devices do not.
    f.sql(
        "main",
        "UPDATE fleet_meta SET node='tunnel-main' WHERE id=1",
    );
    for title in ["Eligible", "Assigned", "Reserved", "Stale", "Unauthorized"] {
        f.issue("main", &["create", "--title", title], 0);
    }
    f.issue("main", &["claim", "2"], 0);
    f.sql("main", "INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,reservation_expires,job,owner_pid,owner_start,machine) VALUES('reserved','named:Tunnel QA',3,'codex:worker','starting',1,1,9223372036854775807,'{}',123,'test','other')");
    let caps = f.cli("peer", &["fleet", "capabilities"], 0);
    assert_eq!(caps["route"], "supervisor_tunnel");
    assert_eq!(caps["capabilities"]["issue_draft"], true);
    assert_eq!(caps["capabilities"]["issue_metadata"], true);
    assert!(caps["usage"].as_str().unwrap().contains("--supervisor"));
    let status = f.cli("peer", &["fleet", "status", "--json"], 0);
    assert_eq!(status["capabilities"]["issue_draft"], true);
    assert_eq!(status["view"], "summary");
    assert_eq!(status["authoritative"], true);
    let details = f.cli(
        "peer",
        &["fleet", "status", "--records", "signals", "--limit", "1"],
        0,
    );
    assert_eq!(details["view"], "signals");
    assert!(details["records"].is_array());
    let before = f.issue("peer", &["view", "1", "--supervisor"], 0);
    assert_eq!(before["issue"]["version"], 1);
    // The original reported command works, including stable automatic replay.
    let args = ["edit", "1", "--draft", "--if-version", "1"];
    let saved = f.issue("peer", &args, 0);
    assert_eq!(saved["store"]["host"], "supervisor");
    assert_eq!(saved["issue"]["draft"], true);
    assert_eq!(saved["issue"]["assignee"], Value::Null);
    assert_eq!(saved, f.issue("peer", &args, 0));
    assert_eq!(f.issue("main", &["view", "1"], 0)["issue"], saved["issue"]);
    for (number, version) in [("2", "2"), ("3", "1")] {
        let denied = f.issue(
            "peer",
            &[
                "edit",
                number,
                "--draft",
                "--if-version",
                version,
                "--request-id",
                number,
            ],
            4,
        );
        assert_eq!(denied["error"]["code"], "conflict");
        assert!(
            denied["error"]["message"]
                .as_str()
                .unwrap()
                .contains("unreserved")
        );
        assert_eq!(
            f.issue("main", &["view", number], 0)["issue"]["draft"],
            false
        );
    }
    assert_eq!(
        f.issue(
            "peer",
            &[
                "edit",
                "4",
                "--draft",
                "--if-version",
                "99",
                "--request-id",
                "stale"
            ],
            4
        )["error"]["code"],
        "conflict"
    );
    assert_eq!(
        f.issue("peer", &["edit", "4", "--draft"], 2)["error"]["code"],
        "invalid_input"
    );
    assert_eq!(
        f.issue(
            "peer",
            &[
                "edit",
                "5",
                "--draft",
                "--if-version",
                "1",
                "--label",
                "yolo",
                "--request-id",
                "forbidden"
            ],
            1
        )["error"]["code"],
        "forbidden"
    );
    assert_eq!(
        f.sql(
            "main",
            "SELECT DISTINCT actor FROM requests WHERE request_id LIKE 'draft-%'"
        ),
        json!([["codex:tunnel-test"]])
    );
    assert_eq!(
        f.sql(
            "peer",
            "SELECT count(*) FROM requests WHERE request_id LIKE 'draft-%'"
        ),
        json!([[0]])
    );
    assert_eq!(
        f.sql(
            "main",
            "SELECT count(*) FROM fleet_allocations WHERE issue_number=1"
        ),
        json!([[0]])
    );
    // Chief may expose unfinished work without acquiring or releasing ownership.
    for title in [
        "Incomplete delivery",
        "Dependency",
        "Dependent delivery",
        "Held",
        "Reserved delivery",
        "Unknown owner",
    ] {
        f.issue("main", &["create", "--title", title], 0);
    }
    for number in ["6", "8", "10", "11"] {
        f.issue("main", &["close", number], 0);
    }
    f.issue("main", &["blocked-by", "8", "7"], 0);
    f.issue("main", &["close", "7"], 0);
    f.issue("main", &["blocked-by", "9", "7"], 0);
    f.issue(
        "main",
        &["block", "9", "--comment", "Manual investigation hold"],
        0,
    );
    f.issue("main", &["reopen", "7"], 0);
    f.sql(
        "main",
        "INSERT INTO fleet_allocations VALUES('named:Tunnel QA',10,'offline-device')",
    );
    f.sql("main", "INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,job,owner_pid,owner_start,machine) VALUES('uncertain','named:Tunnel QA',11,'codex:unknown','unknown',1,1,'{}',123,'test','offline')");
    let reopen = |number: &str, key: &str, hold: bool, expected| {
        let current = f.issue("peer", &["view", number, "--supervisor"], 0);
        let version = current["issue"]["version"].to_string();
        let mut args = vec![
            "reopen",
            number,
            "--supervisor",
            "--if-version",
            &version,
            "--request-id",
            key,
        ];
        if hold {
            args.push("--clear-manual-hold");
        }
        f.issue("peer", &args, expected)
    };
    let saved = reopen("6", "chief-reopen", false, 0);
    assert_eq!(saved["issue"]["state"], "open");
    assert_eq!(saved["issue"]["assignee"], Value::Null);
    assert_eq!(saved["store"]["host"], "supervisor");
    let args = [
        "reopen",
        "6",
        "--supervisor",
        "--if-version",
        "2",
        "--request-id",
        "chief-reopen",
    ];
    assert_eq!(f.issue("peer", &args, 0), saved);
    let mut stale = args;
    stale[6] = "stale-reopen";
    assert_eq!(f.issue("peer", &stale, 4)["error"]["code"], "conflict");
    let mut changed = args;
    changed[4] = "3";
    assert_eq!(f.issue("peer", &changed, 4)["error"]["code"], "conflict");
    let dependent = reopen("8", "chief-dependent", false, 0);
    assert_eq!(dependent["issue"]["state"], "blocked");
    assert_eq!(dependent["issue"]["assignee"], Value::Null);
    let held = f.issue("main", &["view", "9"], 0);
    assert_eq!(held["issue"]["manual_blocked"], true);
    assert_eq!(
        reopen("9", "chief-held", false, 4)["error"]["code"],
        "conflict"
    );
    assert_eq!(f.issue("main", &["view", "9"], 0)["issue"], held["issue"]);
    let cleared = reopen("9", "chief-clear-hold", true, 0);
    assert_eq!(cleared["issue"]["state"], "blocked");
    assert_eq!(cleared["issue"]["manual_blocked"], false);
    for number in ["2", "3", "10", "11"] {
        let before = f.issue("main", &["view", number], 0);
        assert_eq!(
            reopen(number, &format!("refused-{number}"), false, 4)["error"]["code"],
            "conflict"
        );
        assert_eq!(
            f.issue("main", &["view", number], 0)["issue"],
            before["issue"]
        );
    }
    for args in [
        vec!["reopen", "6", "--if-version", "3"],
        vec!["reopen", "6", "--request-id", "no-version"],
        vec![
            "reopen",
            "6",
            "--if-version",
            "0",
            "--request-id",
            "zero-version",
        ],
    ] {
        let mut args = args;
        args.push("--supervisor");
        assert_eq!(f.issue("peer", &args, 2)["error"]["code"], "invalid_input");
    }
    assert_eq!(
        f.sql(
            "main",
            "SELECT DISTINCT actor FROM requests WHERE request_id LIKE 'chief-%'"
        ),
        json!([["codex:tunnel-test"]])
    );
    assert_eq!(
        f.sql(
            "peer",
            "SELECT count(*) FROM requests WHERE request_id LIKE 'chief-%'"
        ),
        json!([[0]])
    );
    assert_eq!(
        f.sql(
            "main",
            "SELECT count(*) FROM fleet_allocations WHERE issue_number IN (6,8,9)"
        ),
        json!([[0]])
    );
    assert_eq!(
        f.sql(
            "main",
            "SELECT node FROM fleet_allocations WHERE issue_number=10"
        ),
        json!([["offline-device"]])
    );
    assert_eq!(
        f.sql(
            "main",
            "SELECT finished_at FROM worker_runs WHERE id='uncertain'"
        ),
        json!([[null]])
    );
    assert_eq!(
        f.cli("peer", &["fleet", "capabilities"], 0)["capabilities"]["issue_reopen"],
        true
    );
    let supervisor = f.services.last_mut().unwrap();
    unsafe {
        libc::kill(supervisor.id() as i32, libc::SIGTERM);
    }
    assert!(supervisor.wait().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::os::unix::net::UnixStream::connect(f.root.join("peer/fleet-authority.sock")).is_ok()
    {
        assert!(
            Instant::now() < deadline,
            "Relay still accepts connections after shutdown"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let offline = f.issue(
        "peer",
        &[
            "edit",
            "4",
            "--draft",
            "--if-version",
            "1",
            "--request-id",
            "offline",
        ],
        1,
    );
    assert_eq!(offline["error"]["code"], "fleet_unavailable");
    assert_eq!(
        f.issue(
            "peer",
            &[
                "reopen",
                "6",
                "--supervisor",
                "--if-version",
                "3",
                "--request-id",
                "offline-reopen"
            ],
            1
        )["error"]["code"],
        "fleet_unavailable"
    );
    assert_eq!(f.issue("main", &["view", "4"], 0)["issue"]["draft"], false);
    assert_eq!(
        f.sql(
            "peer",
            "SELECT count(*) FROM requests WHERE request_id='offline'"
        ),
        json!([[0]])
    );
}
