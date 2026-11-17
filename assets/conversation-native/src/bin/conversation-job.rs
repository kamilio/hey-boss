// One worker-owned turn. Keep stdin open so a lost owner stops the job.
use anyhow::{Context, Result};
use hey_boss_conversation::{agent, context, emit, plugins::Plugins};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let state = PathBuf::from(std::env::args_os().nth(1).context("Missing job notebook")?);
    let home = PathBuf::from(std::env::var_os("HOME").context("Missing HOME")?);
    let exe = std::env::current_exe()?;
    let resources = exe.parent().context("Missing agent resources")?;
    let (cwd, shell) = context::settings(&home, Some(std::env::current_dir()?))?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut request: Value = serde_json::from_str(
        &lines
            .next_line()
            .await?
            .context("Missing job instructions")?,
    )?;
    request["job"] = true.into();
    let (_tx, mut acks) = tokio::sync::mpsc::unbounded_channel();
    let mut tools = Plugins {
        cwd: &cwd,
        home: &home,
        login: &shell,
        resources,
        http: &reqwest::Client::new(),
        focus: None,
        acks: &mut acks,
    };
    tokio::select! {
        r = agent::run(&state, "http://127.0.0.1:8080/v1/responses", &request, &mut tools) => {
            if let Err(e) = r { emit(json!({"error":format!("{e:#}")}))?; }
        }
        _ = lines.next_line() => {}
    }
    Ok(())
}
