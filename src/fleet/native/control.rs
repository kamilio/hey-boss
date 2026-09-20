//! Worker lifecycle is independently owned: service exit never signals workers.
use super::{
    Result,
    context::{Context, atomic_json, hash, now, read_json},
    replica::{self, invalid},
};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn ensure_worker(ctx: &Context, worker: &Value) -> Result<()> {
    if !ctx.rpc(json!({"action":"workers","worker_id":null}))?["workers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["id"] == worker["id"])
    {
        let created=ctx.rpc(json!({"action":"configure_worker","worker_id":null,"config":worker["config"],"if_version":null}))?;
        let db = ctx.db()?;
        // Update references as one transaction, retaining supervisor-issued identity.
        let tx = db.unchecked_transaction()?;
        replica::execute(
            &db,
            "UPDATE issue_workers SET id=? WHERE id=?",
            &[worker["id"].clone(), created["worker_id"].clone()],
        )?;
        tx.commit()?;
    }
    Ok(())
}
pub(super) fn start_worker(ctx: &Context, worker: &Value) -> Result<u32> {
    ensure_worker(ctx, worker)?;
    let id = worker["id"]
        .as_str()
        .ok_or_else(|| invalid("Invalid worker ID"))?;
    let logfile = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(ctx.state.join(format!(
            "fleet-worker-{}.log",
            &format!("{:x}", sha2::Sha256::digest(id.as_bytes()))[..24]
        )))?;
    let mut command = Command::new(&ctx.binary);
    command
        .args(["worker", "--id", id, "--json"])
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .env("HEY_BOSS_FLEET_MANAGED", "1")
        .env("HEY_BOSS_ISSUE_DB", &ctx.path)
        .current_dir(
            worker["config"]["directory"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(std::path::Path::new)
                .unwrap_or(&ctx.home),
        )
        .stdin(Stdio::null())
        .stdout(logfile.try_clone()?)
        .stderr(logfile);
    // Detach from launchd/systemd and the SSH session, just as the old companion did.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait()?.is_some() {
            return Err(
                "Replacement worker exited during startup; inspect its fleet-worker log".into(),
            );
        }
        let status = ctx.rpc(json!({"action":"workers","worker_id":id}))?;
        if status["workers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == id && w["pid"].as_u64() == Some(child.id() as u64))
        {
            return Ok(child.id());
        }
        if Instant::now() >= deadline || ctx.stopped() {
            // Leave independently registered workers alive. A retry checks their
            // durable PID before launching, so uncertainty cannot kill an agent.
            return Err(
                "Replacement worker has not registered; inspect its fleet-worker log".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
use sha2::Digest;
pub(super) fn control(ctx: &Context, id: &Value, command: &str) -> Result<Value> {
    ctx.rpc(json!({"action":"control_worker","worker_id":id,"command":command,"run_id":null}))
}
pub(super) fn configure_workers(ctx: &Context, workers: &[Value]) -> Result<Vec<String>> {
    let db = ctx.db()?;
    let changing = replica::rows(
        &db,
        "SELECT worker FROM fleet_signals WHERE state IN ('stopping','starting')",
        &[],
    )?;
    let mut failures = vec![];
    for desired in workers {
        if changing.iter().any(|r| r["worker"] == desired["id"]) {
            failures.push(format!(
                "{}: Worker restart in progress; configuration will retry",
                desired["id"]
            ));
            continue;
        }
        let result = (|| -> Result<()> {
            ensure_worker(ctx, desired)?;
            let row = replica::rows(
                &db,
                "SELECT config,version FROM issue_workers WHERE id=?",
                &[desired["id"].clone()],
            )?;
            let mut config = desired["config"].clone();
            config["enabled"] = json!(desired["intent"].as_str().unwrap_or("running") == "running");
            if let Some(row) = row.first() {
                let previous: Value = serde_json::from_str(row["config"].as_str().unwrap())?;
                if previous != config {
                    ctx.rpc(json!({"action":"configure_worker","worker_id":desired["id"],"config":config,"if_version":row["version"]}))?;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            failures.push(format!(
                "{}: {e}",
                desired["id"].as_str().unwrap_or("worker")
            ));
        }
    }
    Ok(failures)
}
pub(super) fn configure_companion(ctx: &Context, message: &Value) -> Result<Value> {
    let Some(_lock) = ctx.lock("fleet-worker-control.lock", false)? else {
        return Ok(
            json!({"kind":"ack","configuration_error":"Worker restart in progress; configuration will retry"}),
        );
    };
    let previous = read_json(&ctx.state.join("fleet-agent.json"), json!({}))?;
    let workers = message.get("workers").unwrap_or(&previous["workers"]);
    let configured = json!({"role":"agent","controller":message["controller"],"revision":message["revision"],"workers":workers});
    let failures = configure_workers(
        ctx,
        workers
            .as_array()
            .ok_or_else(|| invalid("Invalid worker configuration"))?,
    )?;
    if !failures.is_empty() {
        return Ok(json!({"kind":"ack","configuration_error":failures.join("; ")}));
    }
    atomic_json(&ctx.state.join("fleet-agent.json"), &configured)?;
    replica::state_set(&ctx.db()?, "revision", &message["revision"])?;
    Ok(json!({"kind":"ack","revision":message["revision"]}))
}
pub(super) fn reconcile(ctx: &Context, config: &Value) -> Result<()> {
    let Some(_lock) = ctx.lock("fleet-worker-control.lock", false)? else {
        return Ok(());
    };
    let known = ctx.workers()?;
    let db = ctx.db()?;
    let changing = replica::rows(
        &db,
        "SELECT worker FROM fleet_signals WHERE state IN ('stopping','starting')",
        &[],
    )?;
    for desired in config["workers"].as_array().into_iter().flatten() {
        if changing.iter().any(|r| r["worker"] == desired["id"]) {
            continue;
        }
        let worker = known.iter().find(|w| w["id"] == desired["id"]);
        let intent =
            desired["intent"]
                .as_str()
                .unwrap_or(if desired["config"]["enabled"] == true {
                    "running"
                } else {
                    "pause"
                });
        let result = match (intent, worker) {
            ("running", None) => start_worker(ctx, desired).map(|_| ()),
            ("running", Some(w)) if w["pid"].is_null() => start_worker(ctx, desired).map(|_| ()),
            ("pause", Some(w)) if !w["pid"].is_null() && w["config"]["enabled"] == true => {
                control(ctx, &desired["id"], "pause").map(|_| ())
            }
            ("stop", Some(w)) if !w["pid"].is_null() => {
                control(ctx, &desired["id"], "stop_worker").map(|_| ())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            atomic_json(
                &ctx.state.join("fleet-agent-error.json"),
                &json!({"worker":desired["id"],"error":e.to_string(),"at":now()}),
            )?;
        }
    }
    Ok(())
}
pub(super) fn apply_signal(ctx: &Context, message: &Value) -> Result<Value> {
    let Some(_lock) = ctx.lock("fleet-worker-control.lock", true)? else {
        unreachable!()
    };
    let db = ctx.db()?;
    let old = replica::rows(
        &db,
        "SELECT * FROM fleet_signals WHERE id=?",
        &[message["id"].clone()],
    )?;
    if let Some(old) = old.first() {
        if old["worker"] != message["worker"] || old["signal"] != message["signal"] {
            return Err(invalid("Signal ID already has a different payload"));
        }
        if matches!(
            old["state"].as_str(),
            Some("acknowledged" | "superseded" | "failed")
        ) {
            return Ok(serde_json::from_str(old["result"].as_str().unwrap())?);
        }
    }
    let progress = old
        .first()
        .and_then(|r| r["result"].as_str())
        .map(serde_json::from_str::<Value>)
        .transpose()?
        .unwrap_or(json!({}));
    if progress["retry_at"].as_f64().unwrap_or(0.0) > now() {
        return Ok(json!({"id":message["id"],"state":"pending","error":progress["error"]}));
    }
    let result = apply_signal_locked(ctx, &db, message, old.first(), &progress);
    match result {
        Ok(result) => Ok(result),
        Err(e) => {
            let invalid = e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::InvalidInput);
            let receipt = json!({"id":message["id"],"state":if invalid{"failed"}else{"pending"},"signal":message["signal"],"worker":message["worker"],"error":e.to_string()});
            if invalid {
                replica::execute(
                    &db,
                    "UPDATE fleet_signals SET state='failed',result=? WHERE id=?",
                    &[json!(receipt.to_string()), message["id"].clone()],
                )?;
            } else {
                let rows = replica::rows(
                    &db,
                    "SELECT result FROM fleet_signals WHERE id=? AND state IN ('stopping','starting')",
                    &[message["id"].clone()],
                )?;
                if let Some(row) = rows.first() {
                    let mut current: Value =
                        serde_json::from_str(row["result"].as_str().unwrap_or("{}"))?;
                    let failures = progress["failures"].as_u64().unwrap_or(0) + 1;
                    current["failures"] = json!(failures);
                    current["retry_at"] =
                        json!(now() + (5 * 2u64.pow(failures.min(6) as u32)).min(300) as f64);
                    current["error"] = json!(e.to_string());
                    replica::execute(
                        &db,
                        "UPDATE fleet_signals SET result=? WHERE id=?",
                        &[json!(current.to_string()), message["id"].clone()],
                    )?;
                }
            }
            Ok(receipt)
        }
    }
}
fn apply_signal_locked(
    ctx: &Context,
    db: &rusqlite::Connection,
    message: &Value,
    old: Option<&Value>,
    progress: &Value,
) -> Result<Value> {
    let action = message["signal"]
        .as_str()
        .ok_or_else(|| invalid("Unknown signal"))?;
    if !matches!(action, "pause" | "resume" | "stop" | "restart") {
        return Err(invalid("Unknown signal"));
    }
    let mut worker = ctx
        .workers()?
        .into_iter()
        .find(|w| w["id"] == message["worker"])
        .ok_or_else(|| invalid("Worker not found on this machine"))?;
    replica::execute(
        db,
        "INSERT OR IGNORE INTO fleet_signals VALUES(?,?,?,?,'pending',NULL,?)",
        &[
            message["id"].clone(),
            json!("local"),
            message["worker"].clone(),
            message["signal"].clone(),
            json!(now()),
        ],
    )?;
    for previous in replica::rows(
        db,
        "SELECT id,signal FROM fleet_signals WHERE worker=? AND id<>? AND state IN ('stopping','starting')",
        &[message["worker"].clone(), message["id"].clone()],
    )? {
        let receipt = json!({"id":previous["id"],"state":"superseded","signal":previous["signal"],"worker":worker["id"],"superseded_by":message["id"]});
        replica::execute(
            db,
            "UPDATE fleet_signals SET state='superseded',result=? WHERE id=?",
            &[json!(receipt.to_string()), previous["id"].clone()],
        )?;
    }
    let phase = old.and_then(|v| v["state"].as_str()).unwrap_or("pending");
    let prior = progress.get("prior_pid").unwrap_or(&worker["pid"]).clone();
    let already_started = action == "restart"
        && phase == "starting"
        && !worker["pid"].is_null()
        && worker["pid"] != prior;
    let role: String = db.query_row("SELECT role FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    let config_path = ctx.state.join(if role == "controller" {
        "fleet-main.json"
    } else {
        "fleet-agent.json"
    });
    let mut saved = read_json(&config_path, json!({}))?;
    set_intent(
        &mut saved,
        &worker["id"],
        if matches!(action, "stop" | "restart") {
            "stop"
        } else if action == "pause" {
            "pause"
        } else {
            "running"
        },
    );
    atomic_json(&config_path, &saved)?;
    if matches!(action, "stop" | "restart") && !already_started {
        if phase != "starting" {
            replica::execute(
                db,
                "UPDATE fleet_signals SET state='stopping',result=? WHERE id=?",
                &[
                    json!(json!({"prior_pid":prior}).to_string()),
                    message["id"].clone(),
                ],
            )?;
            control(ctx, &worker["id"], "stop_worker")?;
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            worker = ctx
                .workers()?
                .into_iter()
                .find(|w| w["id"] == message["worker"])
                .ok_or_else(|| invalid("Worker disappeared"))?;
            if worker["pid"].is_null() && worker["active"] == 0 {
                break;
            }
            if phase == "starting" || Instant::now() >= deadline {
                return Err(
                    "Previous worker or owned agents have not stopped; no duplicate was launched"
                        .into(),
                );
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    if action == "pause" {
        control(ctx, &worker["id"], "pause")?;
    } else if action == "resume" && !worker["pid"].is_null() {
        control(ctx, &worker["id"], "start")?;
    } else if matches!(action, "resume" | "restart") && !already_started {
        if worker["active"].as_u64().unwrap_or(0) > 0 {
            return Err("Owned sessions are still active; no duplicate was launched".into());
        }
        replica::execute(
            db,
            "UPDATE fleet_signals SET state='starting',result=? WHERE id=?",
            &[
                json!(json!({"prior_pid":prior}).to_string()),
                message["id"].clone(),
            ],
        )?;
        let pid = start_worker(ctx, &worker)?;
        replica::execute(
            db,
            "UPDATE fleet_signals SET result=? WHERE id=?",
            &[
                json!(json!({"prior_pid":prior,"replacement_pid":pid}).to_string()),
                message["id"].clone(),
            ],
        )?;
    }
    set_intent(
        &mut saved,
        &worker["id"],
        if matches!(action, "resume" | "restart") {
            "running"
        } else {
            action
        },
    );
    atomic_json(&config_path, &saved)?;
    let result =
        json!({"id":message["id"],"state":"acknowledged","signal":action,"worker":worker["id"]});
    replica::execute(
        db,
        "UPDATE fleet_signals SET state='acknowledged',result=? WHERE id=?",
        &[json!(result.to_string()), message["id"].clone()],
    )?;
    Ok(result)
}
fn set_intent(saved: &mut Value, id: &Value, intent: &str) {
    for worker in saved["workers"].as_array_mut().into_iter().flatten() {
        if &worker["id"] == id {
            worker["intent"] = json!(intent);
        }
    }
}
pub(super) fn revision(node: &str, workers: &Value) -> String {
    hash(&json!({"controller":node,"workers":workers}))
}
