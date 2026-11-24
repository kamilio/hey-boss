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
    let (delegates, mut completions) = hey_boss_conversation::delegate::Delegates::open(&state)?;
    let mut tools = Plugins {
        delegates: Some(&delegates),
        cwd: &cwd,
        home: &home,
        login: &shell,
        resources,
        http: &reqwest::Client::new(),
        focus: None,
        acks: &mut acks,
    };
    let mut queued = std::collections::VecDeque::new();
    loop {
        let (follow, mut pending) = tokio::sync::mpsc::unbounded_channel();
        {
            let run = agent::run_with_input(
                &state,
                "http://127.0.0.1:8080/v1/responses",
                &request,
                &mut tools,
                &mut pending,
            );
            tokio::pin!(run);
            loop {
                tokio::select! {
                    result = &mut run => { if let Err(error) = result { emit(json!({"error":format!("{error:#}")}))?; return Ok(()); } break; }
                    item = completions.recv() => { follow.send(hey_boss_conversation::delegate::follow_up(item.context("Delegate delivery closed")?))?; }
                    _ = lines.next_line() => return Ok(()),
                }
            }
        }
        queued.extend(std::iter::from_fn(|| pending.try_recv().ok()));
        let next = queued.pop_front();
        if !delegates.pending() && next.is_none() {
            break;
        }
        request = if let Some(next) = next {
            next
        } else {
            tokio::select! {
                item = completions.recv() => hey_boss_conversation::delegate::follow_up(item.context("Delegate delivery closed")?),
                _ = lines.next_line() => return Ok(()),
            }
        };
        request["job"] = true.into();
    }
    Ok(())
}
