use anyhow::{Result, ensure};
use hey_boss_conversation::{agent, notebook::Notebook, plugins::Plugins};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use tokio::sync::mpsc;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out");
    fs::create_dir_all(&out)?;
    let tmp = tempfile::tempdir_in(&out)?;
    let root = tmp.path();
    fs::write(root.join("AGENTS.md"), "")?;
    let proxy = root.join(".hey-boss/proxy");
    fs::create_dir_all(&proxy)?;
    let real = Path::new(&std::env::var("HOME")?).join(".hey-boss/proxy/config.access-keys.json");
    if real.exists() {
        fs::copy(real, proxy.join("config.access-keys.json"))?;
    }
    let script = root.join("hey-t3.mjs");
    fs::write(&script, include_str!("t3-replay.mjs"))?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700))?;
    fs::write(root.join("thread.json"), json!({"messages":[{"role":"assistant","text":"The retry handler assigned a new message ID after reconnecting, so the server treated retries as new messages."}]}).to_string())?;
    let (_, mut acks) = mpsc::unbounded_channel();
    let mut tools = Plugins {
        cwd: root,
        focus: None,
        home: root,
        login: Path::new("/bin/sh"),
        resources: root,
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    let scenarios: Vec<Value> = serde_json::from_str(include_str!("t3-replay.md"))?;
    let state = root.join("session.json");
    let mut report = vec![];
    for scenario in scenarios {
        fs::write(root.join("sent.jsonl"), "")?;
        fs::write(
            root.join("delivery.json"),
            json!({"uncertain":scenario["uncertain"] == true}).to_string(),
        )?;
        let request = json!({"t3_thread":"t3code://threads/00000000-0000-0000-0000-000000000001/simulated","turn":scenario["turn"],"automatic":scenario["automatic"] == true});
        agent::run(
            &state,
            "http://127.0.0.1:8080/v1/responses",
            &request,
            &mut tools,
        )
        .await?;
        let book = Notebook::open(&state, &Value::Null)?;
        let spoken = book.messages.last().unwrap()["content"]
            .as_str()
            .unwrap_or("");
        let sent = fs::read_to_string(root.join("sent.jsonl"))?;
        let words = spoken.split_whitespace().count();
        report.push(json!({"scenario":scenario["name"],"spoken":spoken,"words":words,"sent":sent}));
        fs::write(
            out.join("t3-replay.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        ensure!(
            words <= 55,
            "Too much speech: {} ({words} words)",
            scenario["name"]
        );
        ensure!(
            scenario["silent"] != true || spoken.trim().is_empty(),
            "Should have stayed quiet: {}: {spoken}",
            scenario["name"]
        );
        if let Some(count) = scenario["sends"].as_u64() {
            ensure!(
                sent.lines().count() == count as usize,
                "Unexpected sends in {}: {sent}",
                scenario["name"]
            );
        }
    }
    eprintln!(
        "{} simulated turns passed; transcript: {}",
        report.len(),
        out.join("t3-replay.json").display()
    );
    Ok(())
}
