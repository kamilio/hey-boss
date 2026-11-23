use crate::{emit, field, plugins::Plugins};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

pub(super) async fn run(tools: &Plugins<'_>, id: &str, item: &mut Value) -> Result<String> {
    let provider = field(item, "provider")?;
    let mut cmd = if provider == "t3" {
        let mut cmd = Command::new("node");
        cmd.arg(tools.resources.join("hey-t3.mjs"))
            .arg("--conversation");
        cmd
    } else {
        let mut cmd = Command::new(
            std::env::var_os("HEY_BOSS_CLI_PATH").unwrap_or_else(|| "hey-boss".into()),
        );
        cmd.args(["workspace-chat", "--provider", provider]);
        cmd
    };
    let mut child = cmd
        .current_dir(tools.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("Could not launch {provider}"))?;
    // Both bridges own their turn only while this pipe is open (including cancellation).
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(format!("{}\n", json!({"turn":item["request"]})).as_bytes())
        .await?;
    // Hand off the work before the UI queues its spoken description; no speech acknowledgement is needed.
    emit(json!({"ToolStarted":{"id":id,"input":item}}))?;
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    while let Some(line) = lines.next_line().await? {
        let e: Value = serde_json::from_str(&line).context("Invalid subagent event")?;
        if let Some(error) = e["error"].as_str() {
            bail!("{error}");
        }
        if let Some(link) = e["Session"]["link"].as_str() {
            item["link"] = json!(link);
            emit(json!({"ToolStarted":{"id":id,"input":item}}))?;
        }
        if e.get("Approval").is_some() {
            bail!("The child requested approval; no permission was granted.");
        }
        if let Some(turn) = e.get("TurnCompleted") {
            ensure!(
                turn["status"] == "Completed",
                "{}",
                turn["output"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .unwrap_or("The child did not finish.")
            );
            return Ok(field(turn, "output")?.to_owned());
        }
    }
    bail!(
        "Subagent exited without a final response ({})",
        child.wait().await?
    )
}
