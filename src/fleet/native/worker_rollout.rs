//! Installation is incomplete while busy schedulers still execute old code.
use super::{Result, context::Context, control, replica};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

fn request_id(build: &str, id: &str) -> String {
    format!("upgrade-{build}-{}", super::context::hash(&json!(id)))
}

fn snapshot(ctx: &Context, build: &str) -> Result<Vec<Value>> {
    let db = ctx.db()?;
    // Ordinary dashboards are capped at 100 registrations. A rollout must inspect
    // every registration, including a stopped worker with a pending restart.
    let ids: HashSet<String> = db
        .query_collect("SELECT id FROM issue_workers", [], |r| r.get(0))?
        .into_iter()
        .collect();
    let mut workers = ctx.workers_for(Some(&ids))?;
    let signals: HashMap<String, Value> = replica::rows(
        &db,
        "SELECT id,state,result FROM fleet_signals WHERE id LIKE ?",
        &[json!(format!("upgrade-{build}-%"))],
    )?
    .into_iter()
    .map(|row| (row["id"].as_str().unwrap().to_owned(), row))
    .collect();
    for worker in &mut workers {
        let id = request_id(build, worker["id"].as_str().unwrap());
        worker["rollout_signal"] = signals.get(&id).cloned().unwrap_or(Value::Null);
    }
    Ok(workers)
}

fn pending(worker: &Value) -> bool {
    matches!(
        worker["rollout_signal"]["state"].as_str(),
        Some("pending" | "stopping" | "starting")
    )
}

fn classification(worker: &Value, build: &str) -> &'static str {
    if pending(worker) {
        return "restarting";
    }
    let busy = worker["active"].as_u64().unwrap_or(0) > 0
        || worker["job_active"].as_u64().unwrap_or(0) > 0;
    let enabled = worker["config"]["enabled"] == true;
    if !busy && !enabled {
        return "inactive";
    }
    if worker["pid"].is_number() && worker["build"] == build {
        return "current";
    }
    if matches!(worker["intent"].as_str(), Some("pause" | "stop" | "drain")) || !enabled {
        return "paused";
    }
    if matches!(
        worker["rollout_signal"]["state"].as_str(),
        Some("failed" | "superseded" | "acknowledged")
    ) {
        return "failed";
    }
    "outdated"
}

fn report(workers: &[Value], build: &str) -> Value {
    let workers: Vec<_> = workers.iter().map(|w| json!({
        "id":w["id"], "pid":w["pid"], "build":w["build"], "active":w["active"],
        "job_active":w["job_active"], "state":classification(w, build), "signal":w["rollout_signal"]
    })).collect();
    let complete = workers
        .iter()
        .all(|w| matches!(w["state"].as_str(), Some("current" | "inactive")));
    json!({"ok":true,"build":build,"complete":complete,"workers":workers})
}

pub(super) fn run(build: &str, apply: bool) -> Result<Value> {
    if build != env!("HEY_BOSS_BUILD_ID") {
        return Err(replica::invalid(
            "Worker rollout helper does not match the requested build",
        ));
    }
    let ctx = Context::new()?;
    // Serialize with installation as well as other rollout helpers. A competing
    // deployment must not replace the executable halfway through the handoff.
    let _installation = ctx.lock("upgrade.lock", true)?;
    if ctx.build()? != Context::running_build() {
        return Err(replica::invalid(
            "Installation changed before worker rollout",
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let workers = snapshot(&ctx, build)?;
        let result = report(&workers, build);
        if !apply || result["complete"] == true || Instant::now() >= deadline {
            return Ok(result);
        }
        let mut requested = false;
        for worker in &workers {
            if !matches!(classification(worker, build), "outdated" | "restarting") {
                continue;
            }
            let id = worker["id"].as_str().unwrap();
            // Existing durable restart waits for the owned processes to stop.
            // Retrying after a lost reply uses the same identity and cannot
            // launch a second worker or resume an explicitly paused worker.
            control::apply_signal(
                &ctx,
                &json!({"id":request_id(build,id),"worker":id,"signal":"restart","upgrade_build":build}),
            )?;
            requested = true;
        }
        if !requested {
            return Ok(result);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_stopped_and_unknown_workers_cannot_be_misreported_or_resumed() {
        let mut w = json!({"id":"w","pid":12,"build":"old","config":{"enabled":true},"active":1});
        assert_eq!(classification(&w, "new"), "outdated");
        w["intent"] = json!("pause");
        assert_eq!(classification(&w, "new"), "paused");
        assert_eq!(report(&[w.clone()], "new")["complete"], false);
        w["config"]["enabled"] = json!(false);
        w["active"] = json!(0);
        assert_eq!(classification(&w, "new"), "inactive");
        w["rollout_signal"] = json!({"state":"stopping"});
        assert_eq!(classification(&w, "new"), "restarting");
        w["rollout_signal"] = json!({"state":"superseded"});
        w["intent"] = json!("running");
        w["config"]["enabled"] = json!(true);
        assert_eq!(classification(&w, "new"), "failed");
        w["build"] = json!("new");
        assert_eq!(classification(&w, "new"), "current");
        w["pid"] = Value::Null;
        assert_eq!(classification(&w, "new"), "failed");
    }
}
