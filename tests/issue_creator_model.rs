use serde_json::{Value, json};
use std::{fs, path::PathBuf, process::Command};

const SESSION: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "hey-boss-creator-model-{name}-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("codex/sessions")).unwrap();
        Self(root)
    }
    fn transcript(&self, events: &[Value]) {
        fs::write(
            self.0
                .join(format!("codex/sessions/rollout-{SESSION}.jsonl")),
            events.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
    }
    fn run(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &self.0)
            .env("CODEX_HOME", self.0.join("codex"))
            .env("CODEX_THREAD_ID", SESSION)
            .env_remove("HEY_BOSS_AGENT_ID")
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args(["issue", "--project", "Model QA"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "status {:?}: {} {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn create(&self) -> Value {
        serde_json::from_str::<Value>(&self.run(&[
            "--json",
            "create",
            "--title",
            "Automatically attributed",
        ]))
        .unwrap()["issue"]
            .clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn creating_model_is_automatic_immutable_and_visible_without_changing_identity() {
    let f = Fixture::new("capture");
    f.transcript(&[
        json!({"type":"turn_context","payload":{"model":"gpt-older"}}),
        json!({"type":"turn_context","payload":{"model":"gpt-6-astra"}}),
        json!({"type":"response_item","payload":{"type":"function_call","call_id":"create"}}),
    ]);
    let issue = f.create();
    assert_eq!(issue["origin"]["model"], "gpt-6-astra");
    assert_eq!(issue["created_by"], format!("codex:{SESSION}"));
    assert_eq!(issue["origin"]["invocation"]["call_id"], "create");
    f.transcript(&[json!({"type":"turn_context","payload":{"model":"gpt-next"}})]);
    f.run(&["edit", "1", "--title", "Renamed"]);
    let list: Value = serde_json::from_str(&f.run(&["--json", "list"])).unwrap();
    assert_eq!(list["issues"][0]["origin"]["model"], "gpt-6-astra");
    assert!(f.run(&["list"]).contains("created by Codex · gpt-6-astra"));
    assert!(f.run(&["view", "1"]).contains("Creator model: gpt-6-astra"));
}

#[test]
fn missing_metadata_and_explicit_humans_never_inherit_a_model() {
    let f = Fixture::new("missing");
    assert!(f.create()["origin"]["model"].is_null());
    f.transcript(&[json!({"type":"turn_context","payload":{"model":"gpt-6-astra"}})]);
    let human: Value = serde_json::from_str(&f.run(&[
        "--agent",
        "human:boss",
        "--json",
        "create",
        "--title",
        "Human issue",
    ]))
    .unwrap();
    assert!(human["issue"]["origin"]["model"].is_null());
    assert!(human["issue"]["origin"]["session_id"].is_null());
}

#[test]
fn model_never_reads_codex_database_when_turn_context_is_outside_the_tail() {
    let f = Fixture::new("thread");
    let db = rusqlite::Connection::open(f.0.join("codex/state_5.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,model TEXT); INSERT INTO threads VALUES('another-session','wrong-model');").unwrap();
    db.execute("INSERT INTO threads VALUES(?1,'gpt-6-sol')", [SESSION])
        .unwrap();
    // A long turn can put its turn_context outside the bounded transcript tail.
    f.transcript(&[
        json!({"type":"turn_context","payload":{"model":"outside-tail"}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","output":"x".repeat(9 * 1024 * 1024)}}),
    ]);
    assert!(
        f.create()["origin"]["model"].is_null(),
        "Metadata outside the bounded session tail stays unknown; never open Codex SQLite"
    );
    f.transcript(&[json!({"type":"turn_context","payload":{"model":"gpt-turn-override"}})]);
    assert_eq!(f.create()["origin"]["model"], "gpt-turn-override");
    db.execute("DELETE FROM threads WHERE id=?1", [SESSION])
        .unwrap();
    f.transcript(&[]);
    assert!(
        f.create()["origin"]["model"].is_null(),
        "Never borrow another thread's model"
    );
}

#[test]
fn old_thread_schema_uses_archived_transcript_without_requiring_a_model_column() {
    let f = Fixture::new("old-schema");
    let db = rusqlite::Connection::open(f.0.join("codex/state_1.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY);")
        .unwrap();
    f.transcript(&[json!({"type":"turn_context","payload":{"model":"gpt-archived"}})]);
    fs::rename(
        f.0.join("codex/sessions"),
        f.0.join("codex/archived_sessions"),
    )
    .unwrap();
    assert_eq!(f.create()["origin"]["model"], "gpt-archived");
}
