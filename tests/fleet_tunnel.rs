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
        static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = PathBuf::from(format!(
            "/tmp/hb-tunnel-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
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
fn companion_moves_persist_with_guarded_retries_and_no_offline_fallback() {
    let mut f = Fleet::new();
    f.start();
    f.sql(
        "main",
        "UPDATE fleet_meta SET node='tunnel-main' WHERE id=1",
    );
    for title in ["One", "Two", "Three", "Four"] {
        f.issue("main", &["create", "--at-bottom", "--title", title], 0);
    }
    let order = |f: &Fleet, side: &str| -> Vec<i64> {
        f.issue(side, &["list"], 0)["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["number"].as_i64().unwrap())
            .collect()
    };
    let wait_order = |f: &Fleet, expected: &[i64]| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let actual = order(f, "peer");
            if actual == expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Replica order {actual:?}, expected {expected:?}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    };
    wait_order(&f, &[1, 2, 3, 4]);
    let args = ["move", "3", "--before", "1", "--request-id", "move-once"];
    let saved = f.issue("peer", &args, 0);
    assert_eq!(saved["store"]["host"], "supervisor");
    assert_eq!(order(&f, "main"), vec![3, 1, 2, 4]);
    assert_eq!(
        f.issue("peer", &["view", "3", "--supervisor"], 0)["issue"],
        saved["issue"]
    );
    wait_order(&f, &[3, 1, 2, 4]);
    assert_eq!(
        f.cli("peer", &["fleet", "capabilities"], 0)["capabilities"]["issue_move"],
        true
    );

    let version = saved["order_version"].as_i64().unwrap().to_string();
    f.issue("main", &["move", "4", "--before", "3"], 0);
    // Replaying a successful request must not undo a different agent's move.
    assert_eq!(f.issue("peer", &args, 0), saved);
    assert_eq!(order(&f, "main"), vec![4, 3, 1, 2]);
    assert_eq!(
        f.issue(
            "peer",
            &[
                "move",
                "2",
                "--before",
                "1",
                "--if-order-version",
                &version,
                "--request-id",
                "stale-move"
            ],
            4
        )["error"]["code"],
        "conflict"
    );
    assert_eq!(
        f.issue(
            "peer",
            &["move", "3", "--after", "1", "--request-id", "move-once"],
            4
        )["error"]["code"],
        "conflict"
    );
    assert_eq!(order(&f, "main"), vec![4, 3, 1, 2]);
    f.issue("peer", &["move", "3", "--after", "2", "--supervisor"], 0);
    f.issue("peer", &["move", "4"], 0);
    assert_eq!(order(&f, "main"), vec![1, 2, 3, 4]);
    wait_order(&f, &[1, 2, 3, 4]);
    assert_eq!(
        f.sql(
            "main",
            "SELECT count(*) FROM events WHERE action='reordered' AND actor='codex:tunnel-test'"
        ),
        json!([[4]])
    );
    assert_eq!(
        f.sql(
            "peer",
            "SELECT count(*) FROM requests WHERE request_id='move-once'"
        ),
        json!([[0]])
    );
    assert_eq!(
        f.sql("main", "SELECT count(*) FROM fleet_allocations"),
        json!([[0]])
    );

    // A stale explicit guard races with another writer: exactly one can win.
    let version = f.issue("main", &["list"], 0)["order_version"]
        .as_i64()
        .unwrap()
        .to_string();
    let a = f
        .command(
            "peer",
            &[
                "issue",
                "--project",
                "Tunnel QA",
                "--agent",
                "codex:tunnel-test",
                "--json",
                "move",
                "3",
                "--before",
                "1",
                "--if-order-version",
                &version,
                "--request-id",
                "race-peer",
            ],
        )
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let b = f
        .command(
            "main",
            &[
                "issue",
                "--project",
                "Tunnel QA",
                "--agent",
                "codex:other",
                "--json",
                "move",
                "2",
                "--before",
                "1",
                "--if-order-version",
                &version,
            ],
        )
        .output()
        .unwrap();
    let a = a.wait_with_output().unwrap();
    assert_eq!(
        usize::from(a.status.success()) + usize::from(b.status.success()),
        1
    );
    assert!(a.status.code() == Some(4) || b.status.code() == Some(4));
    let final_order = order(&f, "main");
    wait_order(&f, &final_order);

    let mut supervisor = f.services.pop().unwrap();
    unsafe {
        libc::kill(supervisor.id() as i32, libc::SIGTERM);
    }
    assert!(supervisor.wait().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::os::unix::net::UnixStream::connect(f.root.join("peer/fleet-authority.sock")).is_ok()
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(50));
    }
    for extra in [vec![], vec!["--if-order-version", &version]] {
        let mut args = vec!["move", "4", "--before", "1", "--request-id", "offline-move"];
        args.extend(extra);
        let denied = f.issue("peer", &args, 1);
        assert_eq!(denied["error"]["code"], "fleet_unavailable");
        assert!(
            denied["error"]["message"]
                .as_str()
                .unwrap()
                .contains("No local fallback")
        );
    }
    assert_eq!(order(&f, "peer"), final_order);
    assert_eq!(
        f.sql(
            "peer",
            "SELECT count(*) FROM requests WHERE request_id='offline-move'"
        ),
        json!([[0]])
    );
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
        f.issue(
            "main",
            &["create", "--title", title, "--request-id", title],
            0,
        );
    }
    f.issue("main", &["claim", "2"], 0);
    f.sql("main", "INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,reservation_expires,job,owner_pid,owner_start,machine) VALUES('reserved','named:Tunnel QA',3,'codex:worker','starting',1,1,9223372036854775807,'{}',123,'test','other')");
    let caps = f.cli("peer", &["fleet", "capabilities"], 0);
    assert_eq!(caps["route"], "supervisor_tunnel");
    assert_eq!(caps["capabilities"]["issue_draft"], true);
    assert_eq!(caps["capabilities"]["issue_metadata"], true);
    assert_eq!(caps["capabilities"]["issue_request_status"], true);
    let receipt = f.issue("peer", &["request", "Eligible", "--supervisor"], 0);
    assert_eq!(receipt["store"]["host"], "supervisor");
    assert_eq!(receipt["request"]["state"], "recorded");
    assert_eq!(receipt["request"]["response"]["issue"]["number"], 1);
    let missing = f.issue("peer", &["request", "unknown", "--supervisor"], 0);
    assert_eq!(missing["request"]["state"], "not_recorded");
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
    let args = ["edit", "1", "--draft", "--request-id", "draft-once"];
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
        f.issue("peer", &["edit", "4", "--draft"], 0)["issue"]["draft"],
        true
    );
    f.issue("main", &["undraft", "4"], 0);
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
    let args = [
        "reopen",
        "6",
        "--if-version",
        "0",
        "--request-id",
        "zero-version",
        "--supervisor",
    ];
    assert_eq!(f.issue("peer", &args, 2)["error"]["code"], "invalid_input");
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
    // Dependency edits use the same authority and never acquire ownership.
    assert_eq!(caps["capabilities"]["issue_dependencies"], true);
    assert!(caps["usage"].as_str().unwrap().contains("blocked-by"));
    let help = f
        .command("peer", &["issue", "blocked-by", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("issue_dependencies") && help.contains("--request-id"));
    let version =
        f.issue("peer", &["view", "4", "--supervisor"], 0)["issue"]["version"].to_string();
    let add = [
        "blocked-by",
        "4",
        "7",
        "--supervisor",
        "--if-version",
        &version,
        "--request-id",
        "dependency-add",
    ];
    let added = f.issue("peer", &add, 0);
    assert_eq!(added["store"]["host"], "supervisor");
    assert_eq!(added["issue"]["blocker_numbers"], json!([7]));
    assert_eq!(added["issue"]["state"], "blocked");
    assert_eq!(added["issue"]["assignee"], Value::Null);
    assert_eq!(f.issue("peer", &add, 0), added);
    let upstream = f.issue("peer", &["view", "7", "--supervisor"], 0);
    let upstream_version = upstream["issue"]["version"].to_string();
    assert_eq!(
        f.issue(
            "peer",
            &[
                "blocked-by",
                "7",
                "4",
                "--supervisor",
                "--if-version",
                &upstream_version,
                "--request-id",
                "dependency-cycle"
            ],
            2
        )["error"]["code"],
        "invalid_input"
    );
    assert_eq!(
        f.issue("main", &["view", "7"], 0)["issue"],
        upstream["issue"]
    );
    let stale = [
        "blocked-by",
        "4",
        "--supervisor",
        "--if-version",
        &version,
        "--request-id",
        "dependency-stale",
    ];
    assert_eq!(f.issue("peer", &stale, 4)["error"]["code"], "conflict");
    let changed = [
        "blocked-by",
        "4",
        "--supervisor",
        "--if-version",
        &version,
        "--request-id",
        "dependency-add",
    ];
    assert_eq!(f.issue("peer", &changed, 4)["error"]["code"], "conflict");
    let current = added["issue"]["version"].to_string();
    let remove = [
        "blocked-by",
        "4",
        "--supervisor",
        "--if-version",
        &current,
        "--request-id",
        "dependency-remove",
    ];
    let removed = f.issue("peer", &remove, 0);
    assert_eq!(removed["issue"]["blocker_numbers"], json!([]));
    assert_eq!(removed["issue"]["state"], "open");
    assert_eq!(f.issue("peer", &remove, 0), removed);
    for number in ["2", "3", "10", "11"] {
        let before = f.issue("main", &["view", number], 0);
        let version = before["issue"]["version"].to_string();
        let key = format!("dependency-protected-{number}");
        assert_eq!(
            f.issue(
                "peer",
                &[
                    "blocked-by",
                    number,
                    "7",
                    "--supervisor",
                    "--if-version",
                    &version,
                    "--request-id",
                    &key
                ],
                4
            )["error"]["code"],
            "conflict"
        );
        assert_eq!(
            f.issue("main", &["view", number], 0)["issue"],
            before["issue"]
        );
    }
    for args in [
        vec![
            "blocked-by",
            "4",
            "--if-version",
            "0",
            "--request-id",
            "dependency-zero",
        ],
        vec![
            "blocked-by",
            "4",
            "--if-version",
            "1",
            "--request-id",
            "dependency-force",
            "--force",
        ],
    ] {
        let mut args = args;
        args.push("--supervisor");
        assert_eq!(f.issue("peer", &args, 2)["error"]["code"], "invalid_input");
    }
    assert_eq!(
        f.sql(
            "main",
            "SELECT DISTINCT actor FROM requests WHERE request_id LIKE 'dependency-%'"
        ),
        json!([["codex:tunnel-test"]])
    );
    assert_eq!(
        f.sql(
            "peer",
            "SELECT count(*) FROM requests WHERE request_id LIKE 'dependency-%'"
        ),
        json!([[0]])
    );
    assert_eq!(
        f.sql(
            "main",
            "SELECT count(*) FROM fleet_allocations WHERE issue_number=4"
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
            "SELECT finished_at FROM worker_runs WHERE id IN ('reserved','uncertain')"
        ),
        json!([[null], [null]])
    );
    // A successful retry must not rerun guards or overwrite a newer live owner.
    f.issue("main", &["claim", "4"], 0);
    let claimed = f.issue("main", &["view", "4"], 0);
    assert_eq!(f.issue("peer", &remove, 0), removed);
    assert_eq!(
        f.issue("main", &["view", "4"], 0)["issue"],
        claimed["issue"]
    );
    check_pr_attachments(&f);
    check_lifecycle_handoffs(&f);
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
    // The replica may not have received #4 before disconnect. Compare its
    // actual snapshot instead of requiring replication for a no-fallback check.
    let replica_snapshot =
        "SELECT number,state,assignee,blockers,version FROM issues ORDER BY number";
    let before = f.sql("peer", replica_snapshot);
    for action in ["ready", "close"] {
        let result = f.issue(
            "peer",
            &[
                action,
                "4",
                "--supervisor",
                "--request-id",
                "lifecycle-offline",
            ],
            1,
        );
        assert_eq!(result["error"]["code"], "fleet_unavailable");
        assert_eq!(f.sql("peer", replica_snapshot), before);
    }
    assert_eq!(
        f.issue(
            "peer",
            &[
                "blocked-by",
                "4",
                "7",
                "--supervisor",
                "--if-version",
                "1",
                "--request-id",
                "dependency-offline"
            ],
            1
        )["error"]["code"],
        "fleet_unavailable"
    );
    assert_eq!(f.sql("peer", replica_snapshot), before);
    let links_before = f.sql("peer", "SELECT * FROM issue_pull_requests");
    assert_eq!(
        f.issue(
            "peer",
            &[
                "pr",
                "add",
                "4",
                "https://github.com/example/repo/pull/123",
                "--supervisor",
                "--request-id",
                "pr-offline"
            ],
            1
        )["error"]["code"],
        "fleet_unavailable"
    );
    assert_eq!(
        f.issue("peer", &["pr", "list", "4", "--supervisor"], 1)["error"]["code"],
        "fleet_unavailable"
    );
    assert_eq!(
        f.sql("peer", "SELECT * FROM issue_pull_requests"),
        links_before
    );
    assert_eq!(
        f.sql(
            "peer",
            "SELECT count(*) FROM requests WHERE request_id='pr-offline'"
        ),
        json!([[0]])
    );
    assert_eq!(
        f.sql(
            "peer",
            "SELECT count(*) FROM requests WHERE request_id='dependency-offline'"
        ),
        json!([[0]])
    );
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

fn check_lifecycle_handoffs(f: &Fleet) {
    assert_eq!(
        f.cli("peer", &["fleet", "capabilities"], 0)["capabilities"]["issue_close"],
        true
    );
    for allocated in [false, true] {
        let created = f.issue(
            "main",
            &["create", "--title", "Completed lifecycle handoff"],
            0,
        );
        let number = created["issue"]["number"].to_string();
        f.issue(
            "main",
            &[
                "pr",
                "add",
                &number,
                "https://github.com/example/repo/pull/123",
            ],
            0,
        );
        if allocated {
            // Match the authenticated companion's physical UUID, not the
            // synthetic supervisor origin used by this in-process fleet.
            let node = f.sql("peer", "SELECT node FROM fleet_meta WHERE id=1")[0][0]
                .as_str()
                .unwrap()
                .to_owned();
            f.sql(
                "main",
                &format!(
                    "INSERT INTO fleet_allocations VALUES('named:Tunnel QA',{number},'{node}')"
                ),
            );
        }
        let key = format!("handoff-{number}");
        let mut ready_args = vec!["ready", &number, "--request-id", &key];
        if !allocated {
            ready_args.push("--supervisor");
        }
        let ready = f.issue("peer", &ready_args, 0);
        assert_eq!(ready["store"]["host"], "supervisor");
        assert_eq!(ready["issue"]["state"], "ready");
        assert_eq!(ready["issue"]["assignee"], "human:boss");
        assert_eq!(
            ready["issue"],
            f.issue("main", &["view", &number], 0)["issue"]
        );
        assert_eq!(ready, f.issue("peer", &ready_args, 0));
        let key = format!("close-{number}");
        let close_args = [
            "close",
            &number,
            "--supervisor",
            "--comment",
            "Verified complete",
            "--request-id",
            &key,
        ];
        // Concurrent callers use the same receipt despite taking snapshots on
        // either side of the first commit (an uncertain acknowledgement retry).
        let results = thread::scope(|scope| {
            let first = scope.spawn(|| f.issue("peer", &close_args, 0));
            let second = scope.spawn(|| f.issue("peer", &close_args, 0));
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_eq!(results.0, results.1);
        let closed = results.0;
        assert_eq!(closed["issue"]["state"], "closed");
        assert_eq!(closed["issue"]["closed_by"], "codex:tunnel-test");
        assert_eq!(
            closed["issue"],
            f.issue("main", &["view", &number], 0)["issue"]
        );
        assert_eq!(closed, f.issue("peer", &close_args, 0));
        assert_eq!(
            f.issue("peer", &["request", &key, "--supervisor"], 0)["request"]["response"]["issue"],
            closed["issue"]
        );
        assert_eq!(
            f.issue(
                "peer",
                &[
                    "close",
                    &number,
                    "--supervisor",
                    "--comment",
                    "Different",
                    "--request-id",
                    &key
                ],
                4
            )["error"]["code"],
            "conflict"
        );
        assert_eq!(f.sql("main", &format!("SELECT count(*) FROM comments WHERE project_id='named:Tunnel QA' AND issue_number={number} AND body='Verified complete'")), json!([[1]]));
    }
    // A true active reservation cannot be taken over, including with --force.
    assert_eq!(
        f.issue(
            "peer",
            &[
                "close",
                "3",
                "--supervisor",
                "--request-id",
                "close-reserved"
            ],
            4
        )["error"]["code"],
        "conflict"
    );
    assert_eq!(
        f.issue("peer", &["close", "3", "--supervisor", "--force"], 2)["error"]["code"],
        "invalid_input"
    );
    assert_eq!(
        f.sql(
            "main",
            "SELECT finished_at FROM worker_runs WHERE id='reserved'"
        ),
        json!([[null]])
    );
}

fn check_pr_attachments(f: &Fleet) {
    assert_eq!(
        f.cli("peer", &["fleet", "capabilities"], 0)["capabilities"]["issue_pr_attachments"],
        true
    );
    let help = f
        .command("peer", &["issue", "pr", "add", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("--supervisor")
    );
    let ownership = "SELECT number,state,assignee FROM issues ORDER BY number";
    let allocations = "SELECT * FROM fleet_allocations ORDER BY issue_number";
    let attempts = "SELECT id,state,finished_at FROM worker_runs ORDER BY id";
    let before = (
        f.sql("main", ownership),
        f.sql("main", allocations),
        f.sql("main", attempts),
    );
    // Assigned, reserved and unfinished attempts all accept additive metadata.
    // No stale replica version or owner is used to acquire/release their work.
    for number in ["2", "3", "11"] {
        let key = format!("pr-attach-{number}");
        let url = "https://github.com/example/repo/pull/123";
        let args = [
            "pr",
            "add",
            number,
            url,
            "--purpose",
            "prerequisite",
            "--supervisor",
            "--request-id",
            &key,
        ];
        let version = f.issue("peer", &["view", number, "--supervisor"], 0)["issue"]["version"]
            .as_i64()
            .unwrap();
        let saved = f.issue("peer", &args, 0);
        assert_eq!(saved["changed"], true);
        assert_eq!(saved["store"]["host"], "supervisor");
        assert_eq!(saved["pull_requests"][0]["purpose"], "prerequisite");
        assert_eq!(f.issue("peer", &args, 0), saved);
        let read = f.issue("peer", &["pr", "list", number, "--supervisor"], 0);
        assert_eq!(read["pull_requests"], saved["pull_requests"]);
        assert_eq!(read["store"]["host"], "supervisor");
        let receipt = f.issue("peer", &["request", &key, "--supervisor"], 0);
        assert_eq!(receipt["request"]["state"], "recorded");
        assert_eq!(receipt["request"]["replica"], false);
        assert_eq!(
            receipt["request"]["response"]["pull_requests"],
            saved["pull_requests"]
        );
        // Reusing a key for different content conflicts; a fresh key cannot
        // reclassify an existing URL or increment the issue version.
        let mut changed = args;
        changed[5] = "fix";
        assert_eq!(f.issue("peer", &changed, 4)["error"]["code"], "conflict");
        // Request IDs are scoped to actor/project, not issue number.
        let duplicate_key = format!("pr-duplicate-{number}");
        changed[8] = &duplicate_key;
        let duplicate = f.issue("peer", &changed, 0);
        assert_eq!(duplicate["changed"], false);
        assert_eq!(duplicate["pull_requests"], saved["pull_requests"]);
        let after = f.issue("peer", &["view", number, "--supervisor"], 0);
        assert_eq!(after["issue"]["version"], version + 1);
        // A newer purpose and version must survive replay of an older receipt.
        f.issue(
            "main",
            &[
                "pr",
                "classify",
                number,
                url,
                "--purpose",
                "supporting-evidence",
            ],
            0,
        );
        assert_eq!(f.issue("peer", &args, 0), saved);
        assert_eq!(
            f.issue("peer", &["pr", "list", number, "--supervisor"], 0)["pull_requests"][0]["purpose"],
            "supporting-evidence"
        );
    }
    assert_eq!(
        (
            f.sql("main", ownership),
            f.sql("main", allocations),
            f.sql("main", attempts)
        ),
        before
    );
    assert_eq!(
        f.sql(
            "peer",
            "SELECT count(*) FROM requests WHERE request_id LIKE 'pr-%'"
        ),
        json!([[0]])
    );
    assert_eq!(
        f.sql(
            "main",
            "SELECT count(*) FROM events WHERE action='pr_attached'"
        ),
        json!([[3]])
    );
    assert_eq!(
        f.sql(
            "main",
            "SELECT DISTINCT actor FROM requests WHERE request_id LIKE 'pr-%'"
        ),
        json!([["codex:tunnel-test"]])
    );
    // Stable automatic retry keys also work when --request-id is omitted.
    let args = [
        "pr",
        "add",
        "2",
        "https://github.com/example/repo/pull/124",
        "--supervisor",
    ];
    let saved = f.issue("peer", &args, 0);
    assert_eq!(f.issue("peer", &args, 0), saved);
    for args in [
        vec![
            "pr",
            "remove",
            "2",
            "https://github.com/example/repo/pull/124",
            "--supervisor",
        ],
        vec![
            "pr",
            "classify",
            "2",
            "https://github.com/example/repo/pull/124",
            "--purpose",
            "fix",
            "--supervisor",
        ],
        vec![
            "pr",
            "add",
            "2",
            "https://github.com/example/repo/commit/0123456789012345678901234567890123456789",
            "--supervisor",
        ],
    ] {
        assert_eq!(f.issue("peer", &args, 2)["error"]["code"], "invalid_input");
    }
}
