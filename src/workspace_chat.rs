//! JSON lines between the native conversation window and its owned Codex thread.
use hey_boss::agent_runtime::{AgentSession, Event, Launch, Provider, SessionRef, TurnStatus};
use serde::Deserialize;
use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Input {
    Turn { turn: String },
    Decision { decision: String, allow: bool },
}

pub fn run(resume: Option<String>) -> io::Result<()> {
    let (tx, rx) = mpsc::sync_channel(4);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut session = AgentSession::launch(Launch {
        provider: Provider::Codex,
        cwd: std::env::current_dir()?,
        binary: None,
        model: None,
        resume: resume.map(|id| SessionRef {
            provider: Provider::Codex,
            id,
            path: None,
            model: None,
        }),
        env: Default::default(),
        output_schema: None,
        codex_execution: Default::default(),
    })?;
    let mut out = io::stdout().lock();
    let mut active = false;
    let mut pending_session = None;
    loop {
        let input = if active {
            rx.try_recv()
        } else {
            rx.recv().map_err(|_| mpsc::TryRecvError::Disconnected)
        };
        match input {
            Ok(line) => match serde_json::from_str::<Input>(&line?)? {
                Input::Turn { turn } => {
                    session.prompt(&turn, None)?;
                    active = true;
                    pending_session = session.state().session;
                }
                Input::Decision { decision, allow } => session.decide(&decision, allow)?,
            },
            Err(mpsc::TryRecvError::Disconnected) => {
                if active {
                    let _ = session.interrupt();
                }
                return session.stop();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if let Some(event) = session.receive(Duration::from_millis(40))? {
            let recorded = matches!(&event, Event::Other(v) if v["method"] == "item/completed" && v["params"]["item"]["type"] == "userMessage");
            match event {
                Event::TurnCompleted { ref status, .. } => {
                    active = false;
                    if status != &TurnStatus::Completed {
                        pending_session = None;
                    }
                }
                Event::TextDelta { .. }
                | Event::Message { .. }
                | Event::Approval { .. }
                | Event::RequestCancelled { .. }
                | Event::ToolStarted { .. }
                | Event::ToolCompleted { .. } => {}
                Event::Input { .. } => {
                    return Err(io::Error::other(
                        "This tool requires an interactive input that voice chat cannot provide.",
                    ));
                }
                _ if recorded => {}
                _ => continue,
            }
            // A thread ID from turn/start can still point at an empty rollout.
            // A user item or actual reply/tool output proves this turn started.
            if let Some(reference) = pending_session.take() {
                let mut notice = serde_json::to_value(Event::Session(reference))?;
                notice["Session"]["accepted"] = true.into();
                serde_json::to_writer(&mut out, &notice)?;
                writeln!(out)?;
            }
            if !recorded {
                serde_json::to_writer(&mut out, &event)?;
                writeln!(out)?;
            }
            out.flush()?;
        }
    }
}
