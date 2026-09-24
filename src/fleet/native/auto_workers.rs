//! Run this machine's saved fleet configuration without creating fresh identities.
use super::{Result, context::Context, control, replica};
use crate::issues::worker::{Settings, validate_settings};
use serde_json::{Value, json};
use std::collections::HashSet;

fn definitions(value: &Value) -> Result<Vec<Value>> {
    let rows = value
        .as_array()
        .ok_or_else(|| replica::invalid("Worker configuration must contain a workers array"))?;
    let mut ids = HashSet::new();
    rows.iter()
        .map(|row| {
            let id = row["id"]
                .as_str()
                .filter(|id| !id.trim().is_empty() && id.len() <= 128)
                .ok_or_else(|| replica::invalid("Every configured worker needs a stable id"))?;
            if !ids.insert(id) {
                return Err(replica::invalid(&format!("Duplicate worker id: {id}")));
            }
            let settings: Settings = serde_json::from_value(row["config"].clone())?;
            let intent = row["intent"].as_str().unwrap_or(if settings.enabled {
                "running"
            } else {
                "pause"
            });
            if !matches!(intent, "running" | "pause" | "stop" | "drain") {
                return Err(replica::invalid(&format!(
                    "Invalid worker intent: {intent}"
                )));
            }
            // Retired checkouts may have been deleted; tombstones still prevent
            // their workers from being resurrected after reconnecting.
            if !matches!(intent, "stop" | "drain") {
                validate_settings(&settings)?;
            }
            let mut result = row.clone();
            result["config"] = json!(settings);
            result["intent"] = json!(intent);
            Ok(result)
        })
        .collect()
}

fn configuration(ctx: &Context) -> Result<(std::path::PathBuf, Vec<Value>, &'static str)> {
    let agent_path = ctx.state.join("fleet-agent.json");
    let agent = ctx.read_json(&agent_path, Value::Null)?;
    let role: String =
        ctx.db()?
            .query_row("SELECT role FROM fleet_meta WHERE id=1", [], |row| {
                row.get(0)
            })?;
    let (source, config, role) = if role == "agent" {
        (agent_path, agent["workers"].clone(), "agent")
    } else {
        let saved = ctx.read_json(&ctx.desired, Value::Null)?;
        let workers = saved["machines"]["local"]["workers"].clone();
        let main_path = ctx.state.join("fleet-main.json");
        let main = ctx.read_json(&main_path, Value::Null)?;
        if workers.is_null()
            || main["workers"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|w| w.get("local_revision").is_some()))
        {
            (main_path, main["workers"].clone(), "controller")
        } else {
            (ctx.desired.clone(), workers, "controller")
        }
    };
    if config.is_null() {
        return Err(replica::invalid(&format!(
            "No saved worker configuration. Add machines.local.workers to {}, or run hey-boss fleet setup to adopt existing workers.",
            ctx.desired.display()
        )));
    }
    let workers = config
        .as_array()
        .ok_or_else(|| replica::invalid("Worker configuration must contain a workers array"))?
        .clone();
    Ok((source, workers, role))
}

// Ownership is local and explicit. Fleet discovery must never enroll workers in
// this dashboard, including workers added later from a separate terminal.
fn owned_ids(ctx: &Context) -> Result<HashSet<String>> {
    let saved = ctx.read_json(
        &ctx.state.join("auto-workers.json"),
        json!({"worker_ids":[]}),
    )?;
    let ids: Vec<String> = serde_json::from_value(saved["worker_ids"].clone())?;
    Ok(ids.into_iter().collect())
}
fn save_owned_ids(ctx: &Context, ids: &HashSet<String>) -> Result<()> {
    let mut ids: Vec<_> = ids.iter().collect();
    ids.sort();
    ctx.atomic_json(
        &ctx.state.join("auto-workers.json"),
        &json!({"worker_ids":ids}),
    )
}

pub(super) fn run(apply: bool, config_only: bool) -> Result<Value> {
    let ctx = Context::new()?;
    // Read, persist intent and reconcile under the same control lock as removal
    // and fleet configuration, so a launch cannot revive a concurrent removal.
    let control_lock = if apply {
        ctx.lock("fleet-worker-control.lock", true)?
    } else {
        None
    };
    let (fleet_source, mut all_definitions, role) = configuration(&ctx)?;
    let ids = owned_ids(&ctx)?;
    let selected: Vec<_> = all_definitions
        .iter()
        .filter(|w| w["id"].as_str().is_some_and(|id| ids.contains(id)))
        .cloned()
        .collect();
    let mut definitions = definitions(&json!(selected))?;
    let source = ctx.state.join("auto-workers.json");
    if config_only {
        return Ok(
            json!({"ok":true,"machine":ctx.node,"source":source,"fleet_source":fleet_source,"workers":definitions}),
        );
    }
    if apply {
        let mut resumed = Vec::new();
        for worker in &mut definitions {
            if worker["intent"] == "pause" {
                worker["intent"] = json!("running");
                worker["config"]["enabled"] = json!(true);
                // Enabling pickup also requires an available agent executable.
                validate_settings(&serde_json::from_value(worker["config"].clone())?)?;
                resumed.push(worker["id"].as_str().unwrap().to_owned());
            }
        }
        if !resumed.is_empty() {
            for selected in &definitions {
                if let Some(existing) = all_definitions
                    .iter_mut()
                    .find(|w| w["id"] == selected["id"])
                {
                    *existing = selected.clone();
                }
            }
            save_changes(&ctx, role, all_definitions, &resumed)?;
        }
        let failures = control::apply_workers_locked(&ctx, &definitions)?;
        if !failures.is_empty() {
            return Err(replica::invalid(&failures.join("; ")));
        }
    }
    drop(control_lock);
    let mut workers = ctx.workers_for(Some(&ids))?;
    workers.retain(|worker| {
        definitions.iter().any(|definition| {
            definition["id"] == worker["id"]
                && ((!matches!(definition["intent"].as_str(), Some("stop" | "drain")))
                    || !worker["pid"].is_null()
                    || worker["active"].as_u64().unwrap_or(0) > 0)
        })
    });
    for worker in &mut workers {
        worker["intent"] = definitions
            .iter()
            .find(|d| d["id"] == worker["id"])
            .unwrap()["intent"]
            .clone();
    }
    let db = ctx.db()?;
    let projects = replica::rows(
        &db,
        "SELECT id,name FROM projects WHERE hidden_at IS NULL ORDER BY name,id",
        &[],
    )?;
    let connection = crate::fleet::worker_connection_path(role, db.path());
    Ok(
        json!({"ok":true,"project_tabs":true,"source":source,"machine":ctx.node,
        "store":{"host":crate::issues::identity::host()},"workers":workers,"projects":projects,
        "fleet":{"role":role,"supervisor_connection":connection}}),
    )
}

pub(super) fn add(settings: &Settings, requested_id: Option<&str>) -> Result<Value> {
    validate_settings(settings)?;
    let ctx = Context::new()?;
    let Some(_lock) = ctx.lock("fleet-worker-control.lock", true)? else {
        unreachable!()
    };
    let (_, mut workers, role) = configuration(&ctx)?;
    let id = match requested_id {
        Some(id)
            if !id.is_empty()
                && id.len() <= 128
                && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') =>
        {
            id.to_owned()
        }
        Some(_) => {
            return Err(replica::invalid(
                "Worker ID must use 1–128 letters, digits or hyphens",
            ));
        }
        None => format!("auto-{}", crate::issues::worker::random_id()?),
    };
    let worker = json!({"id":id,"config":settings,"intent":"running"});
    let mut ids = owned_ids(&ctx)?;
    let existing = workers.iter().find(|w| w["id"] == id);
    let registered: bool = ctx.db()?.query_row(
        "SELECT EXISTS(SELECT 1 FROM issue_workers WHERE id=?1)",
        [&id],
        |row| row.get(0),
    )?;
    if (existing.is_some() || registered) && !ids.contains(&id) {
        return Err(replica::invalid(
            "Worker belongs to another launcher; choose a new ID",
        ));
    }
    if existing.is_some_and(|w| w["config"] != worker["config"] || w["intent"] != "running") {
        return Err(replica::invalid(
            "Worker ID already has different settings; use a new ID for a different worker",
        ));
    }
    let is_new = existing.is_none();
    // Register each checkout's project through the normal store path.
    let mut store = crate::issues::Store::open(&ctx.path)?;
    for directory in std::iter::once(&settings.directory)
        .filter(|s| !s.is_empty())
        .chain(settings.directories.values())
    {
        let project = crate::issues::identity::project(std::path::Path::new(directory), &ctx.node)?;
        store.execute(&crate::issues::Request {
            version: 1,
            project,
            project_override: None,
            actor: Some(ctx.actor()?),
            operation: crate::issues::Operation::Projects {
                include_hidden: false,
            },
            request_id: None,
        })?;
    }
    if is_new {
        workers.push(worker.clone());
        ids.insert(id.clone());
        save_owned_ids(&ctx, &ids)?;
        save_changes(&ctx, role, workers, std::slice::from_ref(&id))?;
    }
    let failures = control::configure_workers(&ctx, std::slice::from_ref(&worker))?;
    if !failures.is_empty() {
        return Err(replica::invalid(&failures.join("; ")));
    }
    if !ctx
        .workers()?
        .iter()
        .any(|w| w["id"] == id && !w["pid"].is_null())
    {
        control::start_worker(&ctx, &worker)?;
    }
    Ok(json!({"ok":true,"worker_id":id}))
}

pub(super) fn remove(id: &str) -> Result<Value> {
    let ctx = Context::new()?;
    let Some(_lock) = ctx.lock("fleet-worker-control.lock", true)? else {
        unreachable!()
    };
    if !owned_ids(&ctx)?.contains(id) {
        return Err(replica::invalid("Worker is not owned by auto-workers"));
    }
    let (_, mut workers, role) = configuration(&ctx)?;
    let worker = workers
        .iter_mut()
        .find(|w| w["id"] == id)
        .ok_or_else(|| replica::invalid("Configured worker was not found"))?;
    worker["intent"] = json!("drain");
    worker["config"]["enabled"] = json!(false);
    let definition = worker.clone();
    save_changes(&ctx, role, workers, &[id.to_owned()])?;
    // Configuration disables pickup without setting stop_requested on live runs.
    let failures = control::configure_workers(&ctx, &[definition])?;
    if !failures.is_empty() {
        return Err(replica::invalid(&failures.join("; ")));
    }
    Ok(json!({"ok":true,"worker_id":id,"state":"draining"}))
}

fn save_changes(ctx: &Context, role: &str, workers: Vec<Value>, changed: &[String]) -> Result<()> {
    let path = ctx.state.join(if role == "agent" {
        "fleet-agent.json"
    } else {
        "fleet-main.json"
    });
    let mut saved = ctx.read_json(&path, json!({"role":role}))?;
    let base = saved["revision"].clone();
    saved["workers"] = json!(workers);
    for worker in saved["workers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|w| changed.iter().any(|id| w["id"] == *id))
    {
        worker["local_revision"] = json!(crate::issues::worker::now());
        worker["base_revision"] = base.clone();
    }
    ctx.atomic_json(&path, &saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_per_checkout_and_multi_project_workers() {
        let root = std::env::temp_dir().join(format!(
            "auto-worker-paths-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        for path in ["a", "b", "one", "two"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        // Layout validation does not need a locally installed agent CLI. The
        // process integration tests cover running workers with a fixture agent.
        let mut input = json!([
            {"id":"checkout-a","config":{"directory":"/work/a","projects":["named:One"],"enabled":false}},
            {"id":"checkout-b","config":{"directory":"/work/b","projects":["named:One"],"enabled":false}},
            {"id":"shared","config":{"projects":["named:One","named:Two"],"directories":{"named:One":"/work/one","named:Two":"/work/two"},"concurrency":2,"enabled":false}}
        ]);
        input[0]["config"]["directory"] = json!(root.join("a"));
        input[1]["config"]["directory"] = json!(root.join("b"));
        input[2]["config"]["directories"] =
            json!({"named:One":root.join("one"),"named:Two":root.join("two")});
        let rows = definitions(&input).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0]["config"]["directory"],
            input[0]["config"]["directory"]
        );
        assert_eq!(
            rows[1]["config"]["directory"],
            input[1]["config"]["directory"]
        );
        assert_eq!(
            rows[2]["config"]["directories"],
            input[2]["config"]["directories"]
        );
        assert_eq!(rows[2]["config"]["concurrency"], 2);
        assert_eq!(definitions(&json!(rows)).unwrap(), rows);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rejects_duplicate_ids_and_invalid_settings_before_applying() {
        assert!(
            definitions(&json!([{"id":"same","config":{}},{"id":"same","config":{}}])).is_err()
        );
        assert!(definitions(&json!([{"id":"one","config":{"concurrency":0}}])).is_err());
        assert!(definitions(&json!([{"id":"one","config":{},"intent":"restart"}])).is_err());
        assert!(definitions(&json!({})).is_err());
    }
    #[test]
    fn paused_and_stopped_workers_do_not_become_running() {
        let rows = definitions(&json!([
            {"id":"paused","config":{"enabled":false}},
            {"id":"stopped","config":{"enabled":true},"intent":"stop"}
        ]))
        .unwrap();
        assert_eq!(rows[0]["intent"], "pause");
        assert_eq!(rows[1]["intent"], "stop");
    }
}
