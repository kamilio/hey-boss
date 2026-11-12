//! JSONL protocol shared by the synthetic Codex workers. No model or remote service is used.
use serde_json::{Value, json};
use std::{
    env,
    io::{self, BufRead},
    process::Command,
};

pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn messages() -> impl Iterator<Item = Value> {
    io::stdin()
        .lock()
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
}
pub fn send(value: Value) {
    println!("{value}");
}
pub fn result(id: &Value, value: Value) {
    send(json!({"id":id,"result":value}));
}
pub fn issue(session: &str, args: &[&str]) -> Value {
    let out = Command::new(env::var_os("HEY_BOSS_TEST_CLI").unwrap())
        .args(["issue", "--json", "--agent", &format!("codex:{session}")])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
pub fn finish(session: &str, turn: &str, summary: &str) {
    send(
        json!({"method":"item/completed","params":{"threadId":session,"turnId":turn,"item":{"type":"agentMessage","text":json!({"status":"completed","summary":summary}).to_string()}}}),
    );
    send(
        json!({"method":"turn/completed","params":{"threadId":session,"turn":{"id":turn,"status":"completed"}}}),
    );
}
