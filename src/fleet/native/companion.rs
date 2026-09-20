use super::{
    Result,
    context::{Context, atomic_json, now, read_frame, read_json, send},
    control, conversation,
    replica::{self, invalid},
    takeover,
};
use serde_json::{Value, json};
use std::{
    io::BufReader,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
fn local_config(ctx: &Context) -> Result<Vec<Value>> {
    Ok(
        read_json(&ctx.state.join("fleet-agent.json"), json!({}))?["workers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|w| w.get("local_revision").is_some())
            .cloned()
            .collect(),
    )
}
fn reply(output: &Arc<Mutex<std::io::Stdout>>, value: Value) -> Result<()> {
    send(&mut *output.lock().unwrap(), value)
}
pub(super) fn stdio(ctx: Context) -> Result<()> {
    // Register fleet identity before journaling and exporting the first hello.
    ctx.rpc(json!({"action":"whoami"}))?;
    let db = ctx.db()?;
    let role: String = db.query_row("SELECT role FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    if role == "standalone" {
        db.backup(
            "main",
            ctx.state
                .join(format!("fleet-bootstrap-{}.db", now() as i64)),
            None,
        )?;
        replica::install_capture(&db, "agent", &ctx.node)?;
        let tx = db.unchecked_transaction()?;
        for table in [
            "agents",
            "issues",
            "issue_status_updates",
            "issue_subtasks",
            "comments",
            "events",
            "issue_pull_requests",
        ] {
            for row in replica::rows(&db, &format!("SELECT * FROM {table}"), &[])? {
                replica::execute(
                    &db,
                    "INSERT INTO fleet_outbox(table_name,after_json,created_at) VALUES(?,?,?)",
                    &[
                        json!(table),
                        json!(row.to_string()),
                        json!(crate::issues::worker::now()),
                    ],
                )?;
            }
        }
        let max: i64 = db.query_row("SELECT coalesce(max(seq),0) FROM fleet_outbox", [], |r| {
            r.get(0)
        })?;
        replica::state_set(&db, "bootstrap_last_seq", &json!(max))?;
        tx.commit()?;
    }
    replica::install_capture(&db, "agent", &ctx.node)?;
    let output = Arc::new(Mutex::new(std::io::stdout()));
    reply(
        &output,
        json!({"kind":"hello","node":ctx.node,"hostname":crate::issues::identity::host(),"build":ctx.build()?,"projects":replica::rows(&db,"SELECT * FROM projects",&[])?,"local_config":local_config(&ctx)?,"workers":ctx.workers()?,"cursor":replica::state_get(&db,"cursor",Value::Null)?,"revision":replica::state_get(&db,"revision",Value::Null)?,"pending":count(&db,"fleet_outbox")?}),
    )?;
    let (tx, rx) = mpsc::sync_channel::<Value>(100);
    let signals = ctx.clone();
    let signal_output = output.clone();
    std::thread::spawn(move || {
        while let Ok(message) = rx.recv() {
            let result = control::apply_signal(&signals, &message).unwrap_or_else(
                |e| json!({"id":message["id"],"state":"pending","error":e.to_string()}),
            );
            if reply(&signal_output, json!({"kind":"ack","signal":result})).is_err() {
                break;
            }
        }
    });
    let mut input = BufReader::new(std::io::stdin());
    while !ctx.stopped() {
        let Some(message) = read_frame(&mut input)? else {
            break;
        };
        if message["version"] != 1 {
            return Err(invalid("Unsupported fleet protocol version"));
        }
        match message["kind"].as_str() {
            Some("configure") => reply(&output, control::configure_companion(&ctx, &message)?)?,
            Some("pull") => {
                replica::apply_pull(
                    &db,
                    &ctx.node,
                    &message["payload"],
                    message["receipts"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                )?;
                atomic_json(
                    &ctx.state.join("fleet-agent-status.json"),
                    &json!({"connected_at":now(),"last_sync":now()}),
                )?;
                control::reconcile(
                    &ctx,
                    &read_json(&ctx.state.join("fleet-agent.json"), json!({}))?,
                )?;
                reply(
                    &output,
                    json!({"kind":"ack","cursor":replica::state_get(&db,"cursor",Value::Null)?,"pending":count(&db,"fleet_outbox")?}),
                )?;
            }
            Some("signal") => {
                if tx.try_send(message.clone()).is_err() {
                    reply(
                        &output,
                        json!({"kind":"ack","signal":{"id":message["id"],"state":"pending","error":"Worker control queue is busy"}}),
                    )?;
                }
            }
            Some("conversation" | "takeover") => {
                let cursor = message.get("cursor").cloned().unwrap_or(json!(0));
                let result = if message["kind"] == "takeover" {
                    takeover::apply(&ctx, message["run"].as_str().unwrap_or(""))
                } else {
                    conversation::page(&ctx, message["run"].as_str().unwrap_or(""), &cursor)
                }
                .unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()}));
                reply(
                    &output,
                    json!({"kind":message["kind"],"id":message["id"],"result":result}),
                )?;
            }
            Some("ping") => {
                atomic_json(
                    &ctx.state.join("fleet-agent-status.json"),
                    &json!({"connected_at":now(),"last_sync":replica::state_get(&db,"last_sync",Value::Null)?}),
                )?;
                reply(
                    &output,
                    json!({"kind":"heartbeat","at":now(),"workers":ctx.workers()?,"changes":replica::journal(&db,0)?,"cursor":replica::state_get(&db,"cursor",Value::Null)?,"local_config":local_config(&ctx)?,"pending":count(&db,"fleet_outbox")?,"conflicts":replica::rows(&db,"SELECT count(*) count FROM fleet_conflicts WHERE resolved=0",&[])?[0]["count"],"revision":replica::state_get(&db,"revision",Value::Null)?}),
                )?;
            }
            _ => return Err(invalid("Unknown fleet message kind")),
        }
    }
    // Closing the transport leaves detached workers and active agents alive.
    Ok(())
}
fn count(db: &rusqlite::Connection, table: &str) -> Result<i64> {
    Ok(db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
}
pub(super) fn daemon(ctx: Context) -> Result<()> {
    let Some(_lock) = ctx.lock("fleet-agent.lock", false)? else {
        return Err("Fleet companion is already running".into());
    };
    while !ctx.stopped() {
        let interrupted = {
            let _lock = ctx.lock("fleet-worker-control.lock", false)?;
            if _lock.is_some() {
                replica::rows(
                    &ctx.db()?,
                    "SELECT * FROM fleet_signals WHERE host='local' AND state IN ('stopping','starting') ORDER BY created_at",
                    &[],
                )?
            } else {
                vec![]
            }
        };
        for pending in interrupted {
            if let Err(e) = control::apply_signal(&ctx, &pending) {
                atomic_json(
                    &ctx.state.join("fleet-agent-error.json"),
                    &json!({"worker":pending["worker"],"error":e.to_string(),"at":now()}),
                )?;
            }
        }
        let config = read_json(&ctx.state.join("fleet-agent.json"), json!({}))?;
        if config.as_object().is_some_and(|m| !m.is_empty())
            && let Err(e) = control::reconcile(&ctx, &config)
        {
            atomic_json(
                &ctx.state.join("fleet-agent-error.json"),
                &json!({"error":e.to_string(),"at":now()}),
            )?;
        }
        ctx.wait(Duration::from_secs(5));
    }
    Ok(())
}
