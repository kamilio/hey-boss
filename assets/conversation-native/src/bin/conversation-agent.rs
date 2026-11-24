use anyhow::{Context, Result};
use hey_boss_conversation::{agent, context, emit, plugins::Plugins};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};

async fn serve() -> Result<()> {
    let state = std::env::args_os()
        .nth(1)
        .context("Expected conversation state path")?;
    let home = PathBuf::from(std::env::var_os("HOME").context("Missing HOME")?);
    let (cwd, login) = context::settings(&home, None)?;
    let resources = std::env::current_exe()?
        .parent()
        .context("Missing desktop resources")?
        .to_owned();
    let http = reqwest::Client::new();
    let (tx, mut turns) = mpsc::unbounded_channel();
    let (ack, mut acks) = mpsc::unbounded_channel();
    let mut term = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let read = async {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Some(line) = lines.next_line().await? {
            let mut value: Value = serde_json::from_str(&line)?;
            if let Some(a) = value.get_mut("mediaResult") {
                ack.send(a.take())?;
            } else {
                tx.send(value)?;
            }
        }
        Ok::<(), anyhow::Error>(())
    };
    let work = async {
        let mut queued = std::collections::VecDeque::new();
        loop {
            let req = match queued.pop_front() {
                Some(req) => req,
                None => match turns.recv().await {
                    Some(req) => req,
                    None => break,
                },
            };
            if req["cancel"] == true {
                queued.clear();
                continue;
            }
            let mut plugins = Plugins {
                focus: None,
                home: &home,
                cwd: &cwd,
                login: &login,
                resources: &resources,
                http: &http,
                acks: &mut acks,
            };
            let (follow_tx, mut follow_rx) = mpsc::unbounded_channel();
            let mut stop_queue = false;
            let cancelled = {
                let run = agent::run_with_input(
                    Path::new(&state),
                    "http://127.0.0.1:8080/v1/responses",
                    &req,
                    &mut plugins,
                    &mut follow_rx,
                );
                tokio::pin!(run);
                loop {
                    tokio::select! {
                        biased;
                        next = turns.recv() => {
                            let Some(next) = next else { return Ok(()); };
                            if next["cancel"] == true { queued.clear(); stop_queue = true; break true; }
                            match next["mode"].as_str().unwrap_or("queue") {
                                "interrupt" => { queued.push_front(next); break true; }
                                "follow-up" => { follow_tx.send(next)?; }
                                "queue" => queued.push_back(next),
                                _ => { emit(json!({"error":"Unknown input mode"}))?; }
                            }
                        }
                        result = &mut run => {
                            if let Err(e) = result { emit(json!({"error":format!("{e:#}")}))?; }
                            break false;
                        }
                    }
                }
            };
            // Inputs arriving at the terminal boundary become new turns, never disappear.
            let remaining: Vec<_> = std::iter::from_fn(|| follow_rx.try_recv().ok()).collect();
            if !stop_queue {
                if cancelled {
                    let replacement = queued.pop_front();
                    for request in remaining.into_iter().rev() {
                        queued.push_front(request);
                    }
                    if let Some(replacement) = replacement {
                        queued.push_front(replacement);
                    }
                } else {
                    for request in remaining.into_iter().rev() {
                        queued.push_front(request);
                    }
                }
            }
            if cancelled {
                emit(json!({"TurnCompleted":{"status":"Cancelled"}}))?;
            }
        }
        Ok(())
    };
    tokio::select! {r=read=>r,r=work=>r,_=term.recv()=>Ok(()),_=interrupt.recv()=>Ok(())}
}
fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = rt.block_on(serve());
    rt.shutdown_background();
    if let Err(e) = result {
        let _ = emit(json!({"error":format!("{e:#}")}));
        std::process::exit(1)
    }
}
