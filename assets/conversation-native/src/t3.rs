use crate::{field, plugins::Plugins};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

pub fn schemas() -> Vec<Value> {
    vec![
        json!({"type":"function","name":"t3_read","description":include_str!("../../conversation-tools/t3_read.md"),"parameters":{"type":"object","properties":{},"additionalProperties":false}}),
        json!({"type":"function","name":"t3_send","description":include_str!("../../conversation-tools/t3_send.md"),"parameters":{"type":"object","properties":{"text":{"type":"string"},"queue":{"type":"boolean"}},"required":["text"],"additionalProperties":false}}),
    ]
}

pub async fn call(tools: &Plugins<'_>, link: &str, name: &str, args: &Value) -> Result<Value> {
    ensure!(
        link.starts_with("t3code://threads/"),
        "Invalid attached thread"
    );
    let script = tools.resources.join("hey-t3.mjs");
    let script = if script.exists() {
        script
    } else {
        tools.home.join(".local/bin/hey-t3")
    };
    let sending = name == "t3_send";
    let mut command = Command::new(script);
    command
        .arg(if sending {
            if args["queue"] == true {
                "queue"
            } else {
                "send"
            }
        } else {
            "show"
        })
        .arg(link);
    if sending {
        command.arg("-");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    if sending {
        let text = field(args, "text")?;
        ensure!(!text.trim().is_empty(), "Message cannot be empty");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .await?;
    }
    let output = timeout(Duration::from_secs(45), child.wait_with_output())
        .await
        .context(if sending {
            "T3 delivery is uncertain; check the thread before sending again"
        } else {
            "T3 read timed out"
        })??;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(json!(String::from_utf8(output.stdout)?))
}
