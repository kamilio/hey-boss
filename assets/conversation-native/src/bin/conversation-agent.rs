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
    let (cwd, login) = context::settings(&home)?;
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
        while let Some(req) = turns.recv().await {
            let mut plugins = Plugins {
                focus: None,
                home: &home,
                cwd: &cwd,
                login: &login,
                resources: &resources,
                http: &http,
                acks: &mut acks,
            };
            if let Err(e) = agent::run(
                Path::new(&state),
                "http://127.0.0.1:8080/v1/responses",
                &req,
                &mut plugins,
            )
            .await
            {
                emit(json!({"error":format!("{e:#}")}))?;
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
