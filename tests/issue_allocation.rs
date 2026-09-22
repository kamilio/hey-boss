use rusqlite::{Connection, params};
use serde_json::Value;
use std::{
    fs,
    io::Write,
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
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
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
            .args(args)
            .output()
            .unwrap()
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &self.0)
            .args(["issue", "rpc"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let request = serde_json::json!({"version":1,"project":{"id":"named:Allocation fixture","name":"Allocation fixture"},"project_override":null,"actor":actor,"operation":operation,"request_id":null});
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
