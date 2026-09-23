use rusqlite::{Connection, params};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hey-boss-allocation-{}-{name}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let f = Self(root);
        f.json(&["create", "--title", "Resume safely"]);
        f
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &self.0)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args([
                "issue",
                "--project",
                "Allocation fixture",
                "--agent",
                "human:allocation-test",
                "--json",
            ])
            .args(args);
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn db(&self, role: &str) -> Connection {
        let db = Connection::open(self.0.join("issues.db")).unwrap();
        let machine = self.json(&["whoami"])["agent"]["machine"]
            .as_str()
            .unwrap()
            .to_owned();
        db.execute(
            "UPDATE fleet_meta SET role=?1,node=?2",
            params![role, machine],
        )
        .unwrap();
        db
    }
    fn denial(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(4));
        serde_json::from_slice::<Value>(&o.stdout).unwrap()["error"].clone()
    }
    fn rpc(&self, actor: Option<Value>, operation: Value) -> Output {
        self.rpc_id(actor, operation, None)
    }
    fn rpc_id(&self, actor: Option<Value>, operation: Value, request_id: Option<&str>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &self.0)
            .args(["issue", "rpc"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let request = serde_json::json!({"version":1,"project":{"id":"named:Allocation fixture","name":"Allocation fixture"},"project_override":null,"actor":actor,"operation":operation,"request_id":request_id});
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn boss(f: &Fixture) -> Value {
    let mut actor = f.json(&["whoami"])["agent"].clone();
    actor["id"] = serde_json::json!("human:boss");
    actor["kind"] = serde_json::json!("human");
    actor["source"] = serde_json::json!("web interface");
    actor
}

fn release(version: i64, machine: &str) -> Value {
    serde_json::json!({"action":"release_allocation","number":1,"if_version":version,"expected_machine":machine})
}

#[test]
fn boss_releases_an_offline_reservation_with_audited_revision() {
    let f = Fixture::new("release");
    let db = f.db("controller");
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'offline-machine')",
        [],
    )
    .unwrap();
    let before = f.json(&["view", "1"])["issue"].clone();
    let output = f.rpc(
        Some(boss(&f)),
        release(before["version"].as_i64().unwrap(), "offline-machine"),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["changed"], true);
    assert_eq!(result["allocation"]["reserved_machine"], Value::Null);
    assert_eq!(
        result["issue"]["version"],
        before["version"].as_i64().unwrap() + 1
    );
    assert_eq!(result["issue"]["assignee"], Value::Null);
    assert_eq!(result["issue"]["body"], before["body"]);
    let history = f.json(&["history", "1"]);
    let event = history["events"].as_array().unwrap().last().unwrap();
    assert_eq!(event["action"], "allocation_released");
    assert_eq!(event["actor"], "human:boss");
    assert_eq!(event["data"]["machine"], "offline-machine");
    f.json(&["claim", "1"]);
}

#[test]
fn allocation_release_rejects_changed_reservations_and_assigned_work() {
    let f = Fixture::new("release-guards");
    let db = f.db("controller");
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'offline-machine')",
        [],
    )
    .unwrap();
    let version = f.json(&["view", "1"])["issue"]["version"].as_i64().unwrap();
    for op in [
        release(version, "other-machine"),
        release(version + 1, "offline-machine"),
    ] {
        let output = f.rpc(Some(boss(&f)), op);
        assert_eq!(output.status.code(), Some(4));
    }
    f.json(&["claim", "1", "--force"]);
    let current = f.json(&["view", "1"]);
    let output = f.rpc(
        Some(boss(&f)),
        release(
            current["issue"]["version"].as_i64().unwrap(),
            "offline-machine",
        ),
    );
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(f.json(&["view", "1"])["issue"], current["issue"]);
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reserved_machine"],
        "offline-machine"
    );
}

#[test]
fn only_boss_on_the_supervisor_can_release_an_allocation() {
    let f = Fixture::new("release-authority");
    let db = f.db("controller");
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'offline-machine')",
        [],
    )
    .unwrap();
    let version = f.json(&["view", "1"])["issue"]["version"].as_i64().unwrap();
    let output = f.rpc(
        Some(f.json(&["whoami"])["agent"].clone()),
        release(version, "offline-machine"),
    );
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "forbidden");
    db.execute("UPDATE fleet_meta SET role='agent'", [])
        .unwrap();
    let output = f.rpc(Some(boss(&f)), release(version, "offline-machine"));
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reserved_machine"],
        "offline-machine"
    );
}

#[test]
fn release_retry_does_not_remove_a_new_reservation() {
    let f = Fixture::new("release-retry");
    let db = f.db("controller");
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'offline-machine')",
        [],
    )
    .unwrap();
    let version = f.json(&["view", "1"])["issue"]["version"].as_i64().unwrap();
    let actor = boss(&f);
    let op = release(version, "offline-machine");
    let first = f.rpc_id(Some(actor.clone()), op.clone(), Some("release-once"));
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'new-machine')",
        [],
    )
    .unwrap();
    let retry = f.rpc_id(Some(actor), op, Some("release-once"));
    assert!(retry.status.success());
    assert_eq!(first.stdout, retry.stdout);
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reserved_machine"],
        "new-machine"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM events WHERE action='allocation_released'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn release_does_not_open_pickup_during_an_unclaimed_worker_attempt() {
    let f = Fixture::new("release-active");
    let db = f.db("controller");
    db.execute_batch("INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'offline-machine');
        INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at)
        VALUES('active-run','named:Allocation fixture',1,'{}','codex:offline','running',1,'start','offline-machine',0,0);").unwrap();
    let version = f.json(&["view", "1"])["issue"]["version"].as_i64().unwrap();
    let output = f.rpc(Some(boss(&f)), release(version, "offline-machine"));
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reserved_machine"],
        "offline-machine"
    );
}

#[test]
fn local_reservation_follows_the_workers_startup_and_claim_deadlines() {
    let f = Fixture::new("worker-deadline");
    let db = f.db("agent");
    db.execute(
        "INSERT INTO fleet_allocations SELECT 'named:Allocation fixture',1,node FROM fleet_meta",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,reservation_expires) SELECT 'run','named:Allocation fixture',1,'{}','codex:pending','awaiting_model',1,'start',node,0,0,123456 FROM fleet_meta", []).unwrap();
    let deadline = || {
        db.query_row(
            "SELECT expires_at FROM fleet_allocation_deadlines",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(deadline(), 123456);
    db.execute(
        "UPDATE worker_runs SET state='awaiting_claim',reservation_expires=234567",
        [],
    )
    .unwrap();
    assert_eq!(deadline(), 234567);
}

#[test]
fn an_expired_companion_reservation_cannot_claim_offline() {
    let f = Fixture::new("expired");
    let db = f.db("agent");
    db.execute(
        "INSERT INTO fleet_allocations SELECT 'named:Allocation fixture',1,node FROM fleet_meta",
        [],
    )
    .unwrap();
    db.execute("UPDATE fleet_allocation_deadlines SET expires_at=0", [])
        .unwrap();
    assert_eq!(
        f.denial(&["claim", "1"])["code"],
        "fleet_allocation_expired"
    );
    assert_eq!(f.json(&["view", "1"])["issue"]["assignee"], Value::Null);
}

#[test]
fn an_expired_supervisor_reservation_allows_another_machine_to_claim() {
    let f = Fixture::new("expired-supervisor");
    let db = f.db("controller");
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'remote-machine')",
        [],
    )
    .unwrap();
    db.execute("UPDATE fleet_allocation_deadlines SET expires_at=0", [])
        .unwrap();
    f.json(&["claim", "1"]);
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reason"],
        "allocated_here"
    );
}

#[test]
fn missing_allocation_is_distinct_and_inspection_is_read_only() {
    let f = Fixture::new("missing");
    let db = f.db("agent");
    let before = f.json(&["view", "1"]);
    let events: i64 = db
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    let error = f.denial(&["claim", "1"]);
    assert_eq!(error["code"], "fleet_allocation_missing");
    assert_eq!(error["details"]["reason"], "allocation_missing");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("issue allocation 1")
    );
    let info = f.json(&["allocation", "1"])["allocation"].clone();
    assert_eq!(info["reason"], "allocation_missing");
    assert_eq!(info["reserved_machine"], Value::Null);
    assert_eq!(info["connection"]["state"], "unknown");
    assert_eq!(f.json(&["view", "1"])["issue"], before["issue"]);
    assert_eq!(
        db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        events
    );
    // The pre-existing explicit takeover remains an override, never a sync.
    f.json(&["claim", "1", "--force"]);
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reserved_machine"],
        Value::Null
    );
    assert_eq!(f.run(&["allocation", "999"]).status.code(), Some(3));
}

#[test]
fn reservation_reports_machine_and_explicit_takeover_preserves_allocation() {
    let f = Fixture::new("reserved");
    let db = f.db("controller");
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'remote-machine')",
        [],
    )
    .unwrap();
    let error = f.denial(&["claim", "1"]);
    assert_eq!(error["code"], "fleet_reserved");
    assert_eq!(error["details"]["reserved_machine"], "remote-machine");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("remote-machine")
    );
    f.json(&["claim", "1", "--force"]);
    assert_eq!(
        f.json(&["view", "1"])["allocation"]["reserved_machine"],
        "remote-machine"
    );
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reason"],
        "reserved_elsewhere"
    );
}

#[test]
fn cached_inventory_names_a_reserved_device_without_releasing_offline_work() {
    let f = Fixture::new("inventory");
    let db = f.db("controller");
    db.execute_batch("CREATE TABLE IF NOT EXISTS fleet_state(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'remote-machine');").unwrap();
    let machines = serde_json::json!({"devbox":{"host":"devbox","hostname":"Remote device","node":"remote-machine","state":"disconnected"}});
    db.execute(
        "INSERT INTO fleet_state VALUES('machines',?1)",
        [machines.to_string()],
    )
    .unwrap();
    let info = f.json(&["allocation", "1"])["allocation"].clone();
    assert_eq!(info["reserved_host"], "Remote device");
    assert_eq!(info["reserved_ssh_host"], "devbox");
    assert!(info["recovery"].as_str().unwrap().contains("ssh 'devbox'"));
    assert_eq!(f.denial(&["claim", "1"])["code"], "fleet_reserved");
    let unsafe_host = serde_json::json!({"devbox":{"host":"-oProxyCommand=bad","hostname":"Remote device","node":"remote-machine"}});
    db.execute(
        "UPDATE fleet_state SET value=?1 WHERE key='machines'",
        [unsafe_host.to_string()],
    )
    .unwrap();
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reserved_ssh_host"],
        Value::Null
    );
}

#[test]
fn authoritative_manual_claim_allocates_atomically_and_can_resume_after_release() {
    let f = Fixture::new("resume");
    let db = f.db("controller");
    let machine = f.json(&["whoami"])["agent"]["machine"].clone();
    f.json(&["claim", "1"]);
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reserved_machine"],
        machine
    );
    f.json(&["unassign", "1"]);
    // A pull carries the supervisor allocation to the companion; no worker changes.
    db.execute("UPDATE fleet_meta SET role='agent'", [])
        .unwrap();
    f.json(&["claim", "1"]);
    assert_eq!(
        f.json(&["allocation", "1"])["allocation"]["reason"],
        "allocated_here"
    );
}

#[test]
fn failed_authoritative_claim_never_leaves_a_reservation() {
    let f = Fixture::new("rollback");
    let db = f.db("controller");
    f.json(&["close", "1"]);
    f.denial(&["claim", "1"]);
    assert_eq!(
        db.query_row("SELECT count(*) FROM fleet_allocations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn supervisor_rpc_reserves_the_remote_caller_and_rejects_other_machines() {
    let f = Fixture::new("remote");
    let db = f.db("controller");
    let mut remote = f.json(&["whoami"])["agent"].clone();
    remote["id"] = serde_json::json!("human:remote");
    remote["machine"] = serde_json::json!("remote-machine");
    remote["host"] = serde_json::json!("Remote device");
    let claim = serde_json::json!({"action":"claim","number":1,"force":false});
    let output = f.rpc(Some(remote.clone()), claim.clone());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let inspection = f.rpc(
        None,
        serde_json::json!({"action":"allocation","number":1,"machine":"remote-machine"}),
    );
    let info: Value = serde_json::from_slice(&inspection.stdout).unwrap();
    assert_eq!(info["allocation"]["reason"], "allocated_here");
    assert_eq!(info["allocation"]["reserved_host"], "Remote device");
    assert_eq!(info["allocation"]["authoritative"], true);
    assert_eq!(f.denial(&["claim", "1"])["code"], "fleet_reserved");
    // Even a missing reservation does not allow stealing a live session's ownership.
    db.execute("DELETE FROM fleet_allocations", []).unwrap();
    f.denial(&["claim", "1"]);
    assert_eq!(
        db.query_row("SELECT count(*) FROM fleet_allocations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn forwarded_view_uses_the_caller_while_native_and_actorless_reads_use_the_store() {
    let f = Fixture::new("view-caller");
    let db = f.db("agent");
    let native = boss(&f);
    let store_machine = native["machine"].as_str().unwrap();
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,?1)",
        [store_machine],
    )
    .unwrap();
    let mut remote = native.clone();
    remote["id"] = serde_json::json!("codex:forwarded-session");
    remote["kind"] = serde_json::json!("codex");
    remote["session_id"] = serde_json::json!("forwarded-session");
    remote["machine"] = serde_json::json!("initiating-machine");
    let before = f.json(&["view", "1"])["issue"].clone();
    for role in ["controller", "agent"] {
        db.execute("UPDATE fleet_meta SET role=?1", [role]).unwrap();
        for (actor, machine, reason) in [
            (
                Some(remote.clone()),
                "initiating-machine",
                "reserved_elsewhere",
            ),
            (Some(native.clone()), store_machine, "allocated_here"),
            (None, store_machine, "allocated_here"),
        ] {
            let output = f.rpc(actor, serde_json::json!({"action":"view","number":1}));
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            let view: Value = serde_json::from_slice(&output.stdout).unwrap();
            let output = f.rpc(
                None,
                serde_json::json!({"action":"allocation","number":1,"machine":machine}),
            );
            let allocation: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(view["allocation"], allocation["allocation"]);
            assert_eq!(view["allocation"]["reason"], reason);
            assert_eq!(view["allocation"]["caller_machine"], machine);
            assert_eq!(view["allocation"]["store_machine"], store_machine);
            assert_eq!(view["issue"], before);
        }
        let denied = f.rpc(
            Some(remote.clone()),
            serde_json::json!({"action":"claim","number":1,"force":false}),
        );
        assert_eq!(denied.status.code(), Some(4));
        let error: Value = serde_json::from_slice(&denied.stdout).unwrap();
        assert_eq!(error["error"]["code"], "fleet_reserved");
        assert_eq!(
            error["error"]["details"]["caller_machine"],
            "initiating-machine"
        );
    }
    assert_eq!(f.json(&["view", "1"])["issue"], before);
    f.json(&["claim", "1"]);
}

#[test]
fn forwarded_view_keeps_missing_and_unknown_allocation_diagnostics_read_only() {
    let f = Fixture::new("view-missing");
    let db = f.db("agent");
    let mut remote = boss(&f);
    remote["machine"] = serde_json::json!("initiating-machine");
    let before = f.json(&["view", "1"])["issue"].clone();
    let events: i64 = db
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    for (role, reason) in [
        ("agent", "allocation_missing"),
        ("controller", "unallocated"),
        ("standalone", "unallocated"),
    ] {
        db.execute("UPDATE fleet_meta SET role=?1", [role]).unwrap();
        let output = f.rpc(
            Some(remote.clone()),
            serde_json::json!({"action":"view","number":1}),
        );
        assert!(output.status.success());
        let view: Value = serde_json::from_slice(&output.stdout).unwrap();
        let output = f.rpc(
            None,
            serde_json::json!({"action":"allocation","number":1,"machine":"initiating-machine"}),
        );
        let allocation: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(view["allocation"], allocation["allocation"]);
        assert_eq!(view["allocation"]["reason"], reason);
        assert_eq!(view["allocation"]["reserved_machine"], Value::Null);
        assert_eq!(
            view["allocation"]["connection"]["state"],
            match role {
                "agent" => "unknown",
                "controller" => "local",
                _ => "standalone",
            }
        );
        assert_eq!(view["issue"], before);
    }
    assert_eq!(
        db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        events
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM fleet_allocations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn remote_cli_view_forwards_machine_identity_without_requiring_an_agent_flag() {
    let f = Fixture::new("view-cli");
    let db = f.db("agent");
    let machine = f.json(&["whoami"])["agent"]["machine"].clone();
    db.execute("UPDATE fleet_meta SET node='backend-machine'", [])
        .unwrap();
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'backend-machine')",
        [],
    )
    .unwrap();
    let bin = f.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let shim = bin.join("ssh");
    fs::write(&shim, "#!/bin/sh\nexec \"$ISSUE_TEST_BIN\" issue rpc\n").unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
    for explicit_agent in [true, false] {
        let mut command = f.command(&["view", "1", "--host", "devbox"]);
        if !explicit_agent {
            // Rebuild the command without the fixture's explicit agent identity.
            command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
            command
                .current_dir(&f.0)
                .env("HEY_BOSS_ISSUE_DB", f.0.join("issues.db"))
                .env("HEY_BOSS_FLEET_STATE", &f.0)
                .env_remove("HEY_BOSS_ISSUE_HOST")
                .env_remove("HEY_BOSS_ISSUE_PROJECT")
                .env_remove("HEY_BOSS_AGENT_ID")
                .env_remove("CODEX_THREAD_ID")
                .args([
                    "issue",
                    "--project",
                    "Allocation fixture",
                    "--json",
                    "view",
                    "1",
                    "--host",
                    "devbox",
                ]);
        }
        let output = command
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("ISSUE_TEST_BIN", env!("CARGO_BIN_EXE_hey-boss"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let view: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(view["allocation"]["caller_machine"], machine);
        assert_eq!(view["allocation"]["store_machine"], "backend-machine");
        assert_eq!(view["allocation"]["reason"], "reserved_elsewhere");
        assert!(
            view["allocation"]["inspect_command"]
                .as_str()
                .unwrap()
                .contains("--host 'devbox'")
        );
    }
}

#[test]
fn private_home_inspection_skips_discovery_but_claims_still_require_identity() {
    let f = Fixture::new("view-no-discovery");
    let db = f.db("agent");
    let machine = f.json(&["whoami"])["agent"]["machine"].clone();
    db.execute("UPDATE fleet_meta SET node='backend-machine'", [])
        .unwrap();
    db.execute(
        "INSERT INTO fleet_allocations VALUES('named:Allocation fixture',1,'backend-machine')",
        [],
    )
    .unwrap();
    let before = f.json(&["view", "1"])["issue"].clone();
    let events: i64 = db
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    let home = f.0.join("home");
    let bin = f.0.join("bin");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&bin).unwrap();
    // Fail discovery at its entry point instead of consulting real processes or
    // histories. The marker detects even a quick scan followed by a fallback.
    let shim = bin.join("ps");
    fs::write(&shim, "#!/bin/sh\n: > \"$DISCOVERY_MARKER\"\nexit 17\n").unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
    let ssh = bin.join("ssh");
    fs::write(
        &ssh,
        "#!/bin/sh\ncat > \"$INSPECTION_REQUEST\"\nexec \"$ISSUE_TEST_BIN\" issue rpc < \"$INSPECTION_REQUEST\"\n",
    )
    .unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let marker = f.0.join("discovery-attempted");
    let request = f.0.join("inspection-request.json");
    let command = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .current_dir(&f.0)
            .env("HOME", &home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("HEY_BOSS_ISSUE_DB", f.0.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &f.0)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .env_remove("HEY_BOSS_AGENT_ID")
            .env_remove("CODEX_THREAD_ID")
            .env("DISCOVERY_MARKER", &marker)
            .env("INSPECTION_REQUEST", &request)
            .env("ISSUE_TEST_BIN", env!("CARGO_BIN_EXE_hey-boss"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .args(["issue", "--project", "Allocation fixture", "--json"])
            .args(args);
        command
    };
    for args in [vec!["view", "1"], vec!["view", "1", "--host", "devbox"]] {
        let output = command(&args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let view: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(view["allocation"]["caller_machine"], machine);
        assert_eq!(view["allocation"]["store_machine"], "backend-machine");
        assert_eq!(view["allocation"]["reason"], "reserved_elsewhere");
        assert_eq!(view["issue"], before);
        assert!(
            !marker.exists(),
            "Read-only inspection attempted session discovery"
        );
    }
    for (explicit, configured, thread, id, source) in [
        (None, None, None, "human:boss", "terminal inspection"),
        (
            None,
            None,
            Some("thread"),
            "codex:thread",
            "CODEX_THREAD_ID",
        ),
        (
            None,
            Some("codex:configured"),
            Some("thread"),
            "codex:configured",
            "HEY_BOSS_AGENT_ID",
        ),
        (
            Some("codex:explicit"),
            Some("codex:configured"),
            Some("thread"),
            "codex:explicit",
            "--agent",
        ),
        (
            Some("human:reader"),
            None,
            Some("thread"),
            "human:reader",
            "--agent",
        ),
    ] {
        let mut command = command(&["view", "1", "--host", "devbox"]);
        if let Some(explicit) = explicit {
            command.args(["--agent", explicit]);
        }
        if let Some(configured) = configured {
            command.env("HEY_BOSS_AGENT_ID", configured);
        }
        if let Some(thread) = thread {
            command.env("CODEX_THREAD_ID", thread);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let forwarded: Value = serde_json::from_slice(&fs::read(&request).unwrap()).unwrap();
        let actor = &forwarded["actor"];
        assert_eq!(actor["id"], id);
        assert_eq!(actor["source"], source);
        assert_eq!(actor["machine"], machine);
        assert_eq!(actor["cwd"], f.0.canonicalize().unwrap().to_str().unwrap());
        assert_eq!(actor["invocation"], Value::Null);
        assert_eq!(actor["creation_run"], Value::Null);
        if let Some(session) = id.strip_prefix("codex:") {
            assert_eq!(actor["kind"], "codex");
            assert_eq!(actor["session_id"], session);
        } else {
            assert_eq!(actor["session_id"], Value::Null);
        }
        assert!(!marker.exists());
    }
    let output = command(&["view", "1", "--agent", ""]).output().unwrap();
    assert!(
        !output.status.success(),
        "Invalid explicit identities must not fall back"
    );
    assert!(!marker.exists());
    let output = command(&["claim", "1"]).output().unwrap();
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "identity_unavailable");
    assert!(marker.exists(), "Mutations must retain verified discovery");
    assert_eq!(f.json(&["view", "1"])["issue"], before);
    assert_eq!(
        db.query_row("SELECT count(*) FROM events", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        events
    );
}
