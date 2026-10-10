//! The editable fleet document and its last valid runtime projection.
use super::{
    Result,
    context::{self, Context},
    replica::invalid,
};
use crate::issues::worker::{Settings, validate_settings};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::Path,
};

const LIMIT: usize = 1024 * 1024;

fn read_text(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        return Err(invalid("Fleet YAML exceeds 1 MiB"));
    }
    Ok(String::from_utf8(bytes)?)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    machines: BTreeMap<String, Machine>,
    #[serde(default)]
    utils: BTreeMap<String, crate::utilities::Definition>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Machine {
    #[serde(default)]
    workers: Vec<Worker>,
    #[serde(default)]
    projects: BTreeMap<String, super::projects::Checkout>,
    #[serde(default = "super::projects::default_workspace")]
    workspace: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Worker {
    id: String,
    intent: String,
    config: Settings,
    #[serde(default)]
    retiring: bool,
}

pub(super) fn is_yaml(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("yaml" | "yml")
    )
}

fn parse(text: &str) -> Result<Value> {
    if text.len() > LIMIT {
        return Err(invalid("Fleet YAML exceeds 1 MiB"));
    }
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(text)?;
    let doc: Document = serde_yaml_ng::from_value(yaml)?;
    for (name, utility) in &doc.utils {
        if name.is_empty()
            || name.len() > 128
            || name == "help"
            || !name.as_bytes()[0].is_ascii_alphanumeric()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(invalid(
                "Utility names must start with a letter or digit and use letters, digits, hyphens or underscores; help is reserved",
            ));
        }
        utility.validate()?;
        if let Some(host) = &utility.destination
            && !doc.machines.contains_key(host)
        {
            return Err(invalid(
                "Utility destination must name a configured machine",
            ));
        }
    }
    let mut machines = serde_json::Map::new();
    let mut ids = HashSet::new();
    for (host, machine) in doc.machines {
        if !context::valid_host(&host) {
            return Err(invalid("Invalid machine name"));
        }
        let mut workers = vec![];
        super::projects::validate_path(&machine.workspace)?;
        for (id, checkout) in &machine.projects {
            if super::projects::identity(&checkout.git)? != *id {
                return Err(invalid("Project ID must match its Git repository"));
            }
            super::projects::validate_path(&checkout.path)?;
        }
        for worker in machine.workers {
            if worker.id.is_empty()
                || worker.id.len() > 128
                || !worker
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
                || !ids.insert(worker.id.clone())
            {
                return Err(invalid(
                    "Worker IDs must be unique across machines and use 1–128 letters, digits, hyphens or underscores",
                ));
            }
            if !matches!(
                worker.intent.as_str(),
                "running" | "pause" | "stop" | "drain"
            ) {
                return Err(invalid(
                    "Worker intent must be running, pause, stop or drain",
                ));
            }
            let mut settings = worker.config;
            if worker.retiring && !matches!(worker.intent.as_str(), "drain" | "stop") {
                return Err(invalid("Removed workers must drain or stop"));
            }
            settings.enabled = worker.intent == "running";
            if !settings.directory.is_empty()
                && (settings.projects.len() != 1
                    || !settings.directories.is_empty()
                    || !Path::new(&settings.directory).is_absolute())
            {
                return Err(invalid(
                    "A single absolute checkout requires exactly one project; use directories for a shared worker",
                ));
            }
            for (project, path) in &settings.directories {
                if !settings.projects.contains(project) || !Path::new(path).is_absolute() {
                    return Err(invalid(
                        "Each absolute checkout must belong to a selected project",
                    ));
                }
            }
            // Remote paths and executables are validated by their owning machine.
            let mut structural = settings.clone();
            structural.enabled = false;
            structural.directory.clear();
            structural.directories.clear();
            validate_settings(&structural)?;
            let mut row = json!({"id":worker.id,"intent":worker.intent,"config":settings});
            if worker.retiring {
                row["retiring"] = json!(true);
            }
            workers.push(row);
        }
        let mut value = json!({"workers":workers});
        if !machine.projects.is_empty() {
            value["projects"] = json!(machine.projects);
        }
        if machine.workspace != super::projects::default_workspace() {
            value["workspace"] = json!(machine.workspace);
        }
        machines.insert(host, value);
    }
    let mut value = json!({"machines":machines});
    if !doc.utils.is_empty() {
        value["utils"] = json!(doc.utils);
    }
    Ok(value)
}

fn validate_transition(old: &Value, new: &Value) -> Result<()> {
    for (host, machine) in new["machines"].as_object().into_iter().flatten() {
        for worker in machine["workers"].as_array().into_iter().flatten() {
            if old["machines"]
                .as_object()
                .into_iter()
                .flatten()
                .any(|(old_host, old_machine)| {
                    old_host != host
                        && old_machine["workers"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|w| w["id"] == worker["id"])
                })
            {
                return Err(invalid(
                    "Use a new worker ID when moving a worker to another machine; remove its old definition to drain it",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn retain_removals(old: &Value, new: &Value) -> Value {
    let mut result = new.clone();
    for (host, machine) in old["machines"].as_object().into_iter().flatten() {
        for worker in machine["workers"].as_array().into_iter().flatten() {
            if result["machines"][host]["workers"]
                .as_array()
                .is_some_and(|ws| ws.iter().any(|w| w["id"] == worker["id"]))
            {
                continue;
            }
            if !result["machines"][host]["workers"].is_array() {
                result["machines"][host] = json!({"workers":[]});
            }
            let mut retired = worker.clone();
            retired["intent"] = json!("drain");
            retired["config"]["enabled"] = json!(false);
            result["machines"][host]["workers"]
                .as_array_mut()
                .unwrap()
                .push(retired);
        }
    }
    result
}

fn compact(value: &Value) -> Value {
    let mut value = value.clone();
    let defaults = json!(Settings::default());
    for machine in value["machines"]
        .as_object_mut()
        .into_iter()
        .flat_map(|m| m.values_mut())
    {
        for worker in machine["workers"].as_array_mut().into_iter().flatten() {
            worker
                .as_object_mut()
                .unwrap()
                .retain(|k, _| matches!(k.as_str(), "id" | "intent" | "config" | "retiring"));
            if let Some(config) = worker["config"].as_object_mut() {
                config.retain(|k, v| k != "enabled" && defaults.get(k) != Some(v));
            }
        }
    }
    value
}

fn initialize(ctx: &Context) -> Result<()> {
    if ctx.desired.exists() {
        return Ok(());
    }
    let cache = ctx.read_json(&ctx.state.join("fleet-config-cache.json"), json!({}))?;
    if cache["document"].is_object() {
        // A missing file must never re-import an obsolete legacy configuration.
        return context::atomic_bytes(
            &ctx.desired,
            serde_yaml_ng::to_string(&compact(&cache["document"]))?.as_bytes(),
        );
    }
    let legacy = ctx.desired.with_extension("json");
    ctx.protect_file(&legacy)?;
    let mut doc = context::read_json(&legacy, json!({"machines":{}}))?;
    let main = ctx.read_json(&ctx.state.join("fleet-main.json"), json!({}))?;
    if !doc["machines"]["local"]["workers"].is_array() {
        doc["machines"]["local"]["workers"] = main.get("workers").cloned().unwrap_or(json!([]));
    }
    for pending in main["workers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w.get("local_revision").is_some())
    {
        let workers = doc["machines"]["local"]["workers"].as_array_mut().unwrap();
        if let Some(old) = workers.iter_mut().find(|w| w["id"] == pending["id"]) {
            *old = pending.clone();
        } else {
            workers.push(pending.clone());
        }
    }
    let known = super::replica::state_get(&ctx.db()?, "machines", json!({}))?;
    for entry in ctx.legacy_inventory()? {
        let host = entry["host"].as_str().unwrap();
        if !doc["machines"][host]["workers"].is_array() {
            doc["machines"][host]["workers"] = known[host]
                .get("desired_workers")
                .cloned()
                .unwrap_or(json!([]));
        }
    }
    // Inventory is now defined by machine keys; old connection metadata is not a second config.
    for machine in doc["machines"]
        .as_object_mut()
        .into_iter()
        .flat_map(|m| m.values_mut())
    {
        machine
            .as_object_mut()
            .unwrap()
            .retain(|k, _| k == "workers");
    }
    let text = serde_yaml_ng::to_string(&compact(&doc))?;
    parse(&text)?;
    context::atomic_bytes(&ctx.desired, text.as_bytes())
}

fn load_locked(ctx: &Context) -> Result<Value> {
    initialize(ctx)?;
    let cache_path = ctx.state.join("fleet-config-cache.json");
    let previous = ctx.read_json(&cache_path, json!({}))?;
    let text = match read_text(&ctx.desired) {
        Ok(text) => text,
        Err(error) if previous["document"].is_object() => {
            let mut previous = previous;
            previous["error"] = json!(error.to_string());
            return Ok(previous);
        }
        Err(error) => return Err(error),
    };
    let revision = context::hash(&json!(text));
    if previous["revision"] == revision {
        return Ok(previous);
    }
    let mut next = previous.clone();
    match parse(&text).and_then(|doc| {
        validate_transition(&previous["runtime"], &doc)?;
        Ok(doc)
    }) {
        Ok(doc) => {
            next = json!({"revision":revision,"document":doc,"runtime":retain_removals(&previous["runtime"], &doc),"error":null});
        }
        Err(error) => {
            if !previous["document"].is_object() {
                return Err(error);
            }
            next["revision"] = json!(revision);
            next["error"] = json!(error.to_string());
        }
    }
    ctx.atomic_json(&cache_path, &next)?;
    Ok(next)
}

pub(super) fn load(ctx: &Context) -> Result<Value> {
    ctx.protect_file(&ctx.desired)?;
    let _lock = ctx.lock("fleet-config-file.lock", true)?;
    load_locked(ctx)
}

pub(super) fn write(ctx: &Context, value: &Value) -> Result<()> {
    let text = serde_yaml_ng::to_string(&compact(value))?;
    let document = parse(&text)?;
    ctx.protect_file(&ctx.desired)?;
    let _lock = ctx.lock("fleet-config-file.lock", true)?;
    initialize(ctx)?;
    let previous = ctx.read_json(&ctx.state.join("fleet-config-cache.json"), json!({}))?;
    validate_transition(&previous["runtime"], &document)?;
    let old = fs::read(&ctx.desired)?;
    if parse(std::str::from_utf8(&old)?)? != parse(&text)? {
        let backup = ctx.desired.with_extension("yaml.previous");
        ctx.protect_file(&backup)?;
        context::atomic_bytes(&backup, &old)?;
        context::atomic_bytes(&ctx.desired, text.as_bytes())?;
    }
    load_locked(ctx)?;
    Ok(())
}

pub(super) fn edit(ctx: &Context, edit: impl FnOnce(&mut Value) -> Result<()>) -> Result<()> {
    ctx.protect_file(&ctx.desired)?;
    let _lock = ctx.lock("fleet-config-file.lock", true)?;
    initialize(ctx)?;
    let current = read_text(&ctx.desired)?;
    let mut doc = parse(&current)?;
    edit(&mut doc)?;
    let text = serde_yaml_ng::to_string(&compact(&doc))?;
    parse(&text)?;
    let previous = ctx.read_json(&ctx.state.join("fleet-config-cache.json"), json!({}))?;
    validate_transition(&previous["runtime"], &doc)?;
    if parse(&current)? != parse(&text)? {
        let backup = ctx.desired.with_extension("yaml.previous");
        ctx.protect_file(&backup)?;
        context::atomic_bytes(&backup, current.as_bytes())?;
        context::atomic_bytes(&ctx.desired, text.as_bytes())?;
    }
    load_locked(ctx)?;
    Ok(())
}

pub(super) fn record_local(id: &str, settings: Option<&Settings>, intent: &str) -> Result<bool> {
    let ctx = Context::new()?;
    let role: String = ctx
        .db()?
        .query_row("SELECT role FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    if role == "agent" || !is_yaml(&ctx.desired) {
        return Ok(false);
    }
    if !ctx.desired.exists() && !ctx.state.join("fleet-main.json").exists() {
        return Ok(true);
    }
    edit(&ctx, |doc| {
        if let Some(worker) = doc["machines"]["local"]["workers"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .find(|w| w["id"] == id)
        {
            worker["intent"] = json!(intent);
            if let Some(settings) = settings {
                worker["config"] = json!(settings);
            }
        }
        Ok(())
    })?;
    Ok(true)
}

pub(super) fn request(ctx: &Context, request: &Value) -> Result<Value> {
    if !is_yaml(&ctx.desired) {
        return Err(invalid("The editor requires a YAML fleet configuration"));
    }
    ctx.protect_file(&ctx.desired)?;
    let _lock = ctx.lock("fleet-config-file.lock", true)?;
    initialize(ctx)?;
    let current = read_text(&ctx.desired)?;
    let revision = context::hash(&json!(current));
    let mut request = request.clone();
    if let Some(update) = request.get("machine_update").cloned() {
        if request.get("text").is_some() || request.get("worker_update").is_some() {
            return Err(invalid("Choose one configuration edit"));
        }
        let mut doc = parse(&current)?;
        machine_update(&mut doc, &update)?;
        request["text"] = json!(serde_yaml_ng::to_string(&compact(&doc))?);
    }
    if let Some(update) = request
        .get("worker_update")
        .cloned()
        .filter(|u| !u.is_null())
    {
        if request.get("text").is_some() {
            return Err(invalid("Choose a worker edit or YAML text, not both"));
        }
        let host = update["host"]
            .as_str()
            .ok_or_else(|| invalid("Choose a machine"))?;
        let id = update["id"]
            .as_str()
            .ok_or_else(|| invalid("Choose a worker"))?;
        let mut doc = parse(&current)?;
        let worker = doc["machines"][host]["workers"]
            .as_array_mut()
            .and_then(|workers| workers.iter_mut().find(|w| w["id"] == id))
            .ok_or_else(|| {
                invalid("This worker is no longer in the configuration; reload the page")
            })?;
        let config = update["config"]
            .as_object()
            .ok_or_else(|| invalid("Expected worker settings"))?;
        for (key, value) in config {
            worker["config"][key] = value.clone();
        }
        worker["intent"] = update["intent"].clone();
        request["text"] = json!(serde_yaml_ng::to_string(&compact(&doc))?);
    }
    if let Some(text) = request.get("text") {
        let text = text.as_str().ok_or_else(|| invalid("Expected YAML text"))?;
        let doc = parse(text)?;
        let previous = ctx.read_json(&ctx.state.join("fleet-config-cache.json"), json!({}))?;
        validate_transition(&previous["runtime"], &doc)?;
        if request["revision"] != revision {
            return Err(invalid(
                "Configuration changed since you opened it. Reload before saving.",
            ));
        }
        if request["save"] == true && text != current {
            let backup = ctx.desired.with_extension("yaml.previous");
            ctx.protect_file(&backup)?;
            context::atomic_bytes(&backup, current.as_bytes())?;
            context::atomic_bytes(&ctx.desired, text.as_bytes())?;
        }
        if request["save"] != true {
            return Ok(
                json!({"ok":true,"valid":true,"revision":revision,"text":text,"changes":changes(&parse(&current).unwrap_or(json!({})), &doc)}),
            );
        }
    }
    let text = read_text(&ctx.desired)?;
    let cached = load_locked(ctx)
        .unwrap_or_else(|e| json!({"revision":context::hash(&json!(text)),"error":e.to_string()}));
    Ok(
        json!({"ok":true,"source":ctx.desired,"text":text,"document":parse(&text).ok().map(|doc| compact(&doc)),"revision":cached["revision"],"error":cached["error"]}),
    )
}

fn machine_update(doc: &mut Value, update: &Value) -> Result<()> {
    let host = update["host"]
        .as_str()
        .filter(|h| context::valid_host(h))
        .ok_or_else(|| invalid("Choose a machine"))?;
    let machine = &mut doc["machines"][host];
    if !machine.is_object() {
        *machine = json!({"workers":[]});
    }
    match update["action"].as_str() {
        Some("add") => {
            let id = update["id"]
                .as_str()
                .ok_or_else(|| invalid("Missing worker ID"))?;
            let project_ids: Vec<_> = if let Some(project) = update.get("project") {
                let project = project
                    .as_str()
                    .filter(|id| machine["projects"].get(*id).is_some())
                    .ok_or_else(|| invalid("Project is no longer configured on this machine"))?;
                vec![project.to_owned()]
            } else {
                machine["projects"]
                    .as_object()
                    .into_iter()
                    .flat_map(|p| p.keys())
                    .cloned()
                    .collect()
            };
            let workers = machine["workers"].as_array_mut().unwrap();
            if workers.iter().any(|w| w["id"] == id) {
                return Err(invalid("Worker ID already exists"));
            }
            let config = if let Some(template) = update["template"].as_str() {
                workers
                    .iter()
                    .find(|w| w["id"] == template && w["retiring"] != true)
                    .ok_or_else(|| invalid("Template worker is no longer available"))?["config"]
                    .clone()
            } else {
                let mut settings = Settings {
                    projects: project_ids,
                    ..Settings::default()
                };
                if let Some(slots) = update["concurrency"].as_u64() {
                    settings.concurrency = slots
                        .try_into()
                        .map_err(|_| invalid("Invalid agent slot count"))?;
                }
                json!(settings)
            };
            workers.push(json!({"id":id,"intent":"running","config":config}));
        }
        Some("remove") => {
            let worker = machine["workers"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|w| w["id"] == update["id"])
                .ok_or_else(|| invalid("Worker is no longer configured"))?;
            worker["intent"] = json!("drain");
            worker["retiring"] = json!(true);
        }
        Some("project" | "edit-project") => {
            let git = update["git"].as_str().unwrap_or("").trim();
            let id = super::projects::identity(git)?;
            if update["action"] == "edit-project"
                && (update["project"] != id || machine["projects"].get(&id).is_none())
            {
                return Err(invalid(
                    "Keep the same repository when editing a checkout; add a project for another repository",
                ));
            }
            let workspace = update["workspace"]
                .as_str()
                .unwrap_or("~/Workspace")
                .trim()
                .trim_end_matches('/');
            super::projects::validate_path(workspace)?;
            let name = id.rsplit('/').next().unwrap();
            let existing = machine["projects"][&id].clone();
            let automatic = update["path"].as_str().is_none_or(|p| p.trim().is_empty());
            let path = update["path"]
                .as_str()
                .filter(|p| !p.trim().is_empty())
                .map(|p| p.trim().to_owned())
                .or_else(|| existing["path"].as_str().map(str::to_owned))
                .unwrap_or_else(|| format!("{workspace}/{name}"));
            super::projects::validate_path(&path)?;
            if let Some(worker_id) = update["worker"].as_str().filter(|s| !s.is_empty()) {
                let worker = machine["workers"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|w| w["id"] == worker_id && w["retiring"] != true)
                    .ok_or_else(|| invalid("Worker is no longer configured"))?;
                let settings: &mut Value = &mut worker["config"];
                // Preserve a single-project explicit checkout when expanding its scope.
                if let Some(directory) = settings["directory"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                {
                    let previous = settings["projects"][0].as_str().unwrap().to_owned();
                    settings["directories"][previous] = json!(directory);
                    settings["directory"] = json!("");
                }
                let selected = settings["projects"].as_array_mut().unwrap();
                if !selected.iter().any(|p| p == &id) {
                    selected.push(json!(id));
                }
                if let Some(directories) = settings["directories"].as_object_mut() {
                    directories.remove(&id);
                }
            }
            machine["workspace"] = json!(workspace);
            machine["projects"][&id] = json!({"git":git,"path":path});
            if automatic && (existing.is_null() || existing["reuse_existing"] == true) {
                machine["projects"][&id]["reuse_existing"] = json!(true);
            }
        }
        _ => return Err(invalid("Unknown machine edit")),
    }
    Ok(())
}

fn changes(old: &Value, new: &Value) -> Vec<Value> {
    let mut changes = vec![];
    for (name, utility) in new["utils"].as_object().into_iter().flatten() {
        if old["utils"].get(name) != Some(utility) {
            changes.push(json!({"utility":name,"host":utility["destination"].as_str().unwrap_or("caller"),"action":if old["utils"].get(name).is_some() {"update"} else {"add"}}));
        }
    }
    for (name, _) in old["utils"].as_object().into_iter().flatten() {
        if new["utils"].get(name).is_none() {
            changes.push(json!({"utility":name,"action":"remove"}));
        }
    }
    let flatten = |doc: &Value| -> BTreeMap<(String, String), Value> {
        doc["machines"]
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(host, m)| {
                m["workers"].as_array().into_iter().flatten().map(move |w| {
                    (
                        (host.clone(), w["id"].as_str().unwrap_or("").to_owned()),
                        w.clone(),
                    )
                })
            })
            .collect()
    };
    let old = flatten(old);
    let new = flatten(new);
    for ((host, id), worker) in &new {
        if old.get(&(host.clone(), id.clone())) != Some(worker) {
            changes.push(json!({"host":host,"worker":id,"action":if old.contains_key(&(host.clone(),id.clone())) {"update"} else {"add"}}));
        }
    }
    for (host, id) in old.keys() {
        if !new.contains_key(&(host.clone(), id.clone())) {
            changes.push(json!({"host":host,"worker":id,"action":"drain"}));
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    #[test]
    fn utilities_round_trip_and_validate_destination() {
        let text = "machines: {local: {}, macbook: {}}\nutils:\n  pbcopy: {command: pbcopy, destination: macbook}\n  check: {command: 'git status --short'}\n";
        let doc = parse(text).unwrap();
        assert_eq!(doc["utils"]["pbcopy"]["destination"], "macbook");
        assert_eq!(changes(&json!({}), &doc).len(), 2);
        assert_eq!(changes(&doc, &json!({}))[0]["action"], "remove");
        assert_eq!(
            parse(&serde_yaml_ng::to_string(&compact(&doc)).unwrap()).unwrap(),
            doc
        );
        assert!(parse(&text.replace("destination: macbook", "destination: missing")).is_err());
        assert!(parse(&text.replace("command: pbcopy", "command: ''")).is_err());
        assert!(parse(&text.replace("  pbcopy:", "  --bad:")).is_err());
    }
    use super::*;
    use serde_json::json;

    #[test]
    fn editing_checkout_preserves_workers_and_rejects_identity_changes() {
        let mut doc = parse("machines: {local: {workers: [{id: w, intent: pause, config: {projects: [github.com/acme/atlas], directory: /custom/atlas}}], projects: {github.com/acme/atlas: {git: 'git@github.com:acme/atlas.git', path: '~/old/atlas'}}}}\n").unwrap();
        let workers = doc["machines"]["local"]["workers"].clone();
        let update = json!({"host":"local","action":"edit-project","project":"github.com/acme/atlas","git":"https://github.com/acme/atlas.git","path":"~/new/atlas","workspace":"~/new"});
        machine_update(&mut doc, &update).unwrap();
        assert_eq!(doc["machines"]["local"]["workers"], workers);
        assert_eq!(
            doc["machines"]["local"]["projects"]["github.com/acme/atlas"]["path"],
            "~/new/atlas"
        );
        let before = doc.clone();
        let mut invalid = update.clone();
        invalid["git"] = json!("https://github.com/acme/other.git");
        assert!(machine_update(&mut doc, &invalid).is_err());
        assert_eq!(doc, before);
    }

    #[test]
    fn adding_a_worker_to_one_project_does_not_include_other_machine_projects() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        fs::write(&ctx.desired, "machines: {local: {workers: [], projects: {github.com/acme/atlas: {git: 'https://github.com/acme/atlas.git', path: '~/Workspace/atlas'}, github.com/acme/tools: {git: 'https://github.com/acme/tools.git', path: '~/Workspace/tools'}}}}\n").unwrap();
        let first = request(ctx, &json!({})).unwrap();
        let saved = request(ctx, &json!({"machine_update":{"host":"local","action":"add","id":"atlas-worker","project":"github.com/acme/atlas","concurrency":5},"revision":first["revision"],"save":true})).unwrap();
        let config = &saved["document"]["machines"]["local"]["workers"][0]["config"];
        assert_eq!(config["projects"], json!(["github.com/acme/atlas"]));
        assert_eq!(config["concurrency"], 5);
        assert!(request(ctx, &json!({"machine_update":{"host":"local","action":"add","id":"missing","project":"github.com/acme/missing"},"revision":saved["revision"],"save":true})).is_err());
    }

    #[test]
    fn machine_controls_add_drain_and_remember_project_paths() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        fs::write(&ctx.desired, "machines: {local: {workers: []}}\n").unwrap();
        let first = request(ctx, &json!({})).unwrap();
        let add = json!({"host":"local","action":"add","id":"new-worker"});
        let preview = request(
            ctx,
            &json!({"machine_update":add,"revision":first["revision"]}),
        )
        .unwrap();
        assert_eq!(preview["changes"][0]["action"], "add");
        assert_eq!(
            request(ctx, &json!({})).unwrap()["revision"],
            first["revision"]
        );
        let saved = request(
            ctx,
            &json!({"machine_update":add,"revision":first["revision"],"save":true}),
        )
        .unwrap();
        assert_eq!(
            saved["document"]["machines"]["local"]["workers"][0]["id"],
            "new-worker"
        );
        assert!(
            request(
                ctx,
                &json!({"machine_update":add,"revision":first["revision"],"save":true})
            )
            .is_err()
        );
        let project = json!({"host":"local","action":"project","git":"git@github.com:acme/my-project.git","workspace":"~/Work","worker":"new-worker"});
        let saved = request(
            ctx,
            &json!({"machine_update":project,"revision":saved["revision"],"save":true}),
        )
        .unwrap();
        let machine = &saved["document"]["machines"]["local"];
        assert_eq!(machine["workspace"], "~/Work");
        assert_eq!(
            machine["projects"]["github.com/acme/my-project"]["path"],
            "~/Work/my-project"
        );
        assert_eq!(
            machine["workers"][0]["config"]["projects"],
            json!(["github.com/acme/my-project"])
        );
        let removed = request(ctx, &json!({"machine_update":{"host":"local","action":"remove","id":"new-worker"},"revision":saved["revision"],"save":true})).unwrap();
        assert_eq!(
            removed["document"]["machines"]["local"]["workers"][0]["intent"],
            "drain"
        );
        assert_eq!(
            removed["document"]["machines"]["local"]["workers"][0]["retiring"],
            true
        );
        let reloaded = request(ctx, &json!({})).unwrap();
        assert_eq!(
            reloaded["document"]["machines"]["local"]["workspace"],
            "~/Work"
        );
    }

    #[test]
    fn project_path_is_automatic_only_when_not_explicitly_chosen() {
        let mut doc = parse("machines: {local: {workers: []}}\n").unwrap();
        let mut update = json!({"host":"local","action":"project","git":"https://github.com/acme/atlas.git","workspace":"~/Work"});
        machine_update(&mut doc, &update).unwrap();
        assert_eq!(
            doc["machines"]["local"]["projects"]["github.com/acme/atlas"]["reuse_existing"],
            true
        );
        update["path"] = json!("~/Work/atlas-separate");
        machine_update(&mut doc, &update).unwrap();
        assert_ne!(
            doc["machines"]["local"]["projects"]["github.com/acme/atlas"]["reuse_existing"],
            true
        );
        update.as_object_mut().unwrap().remove("path");
        machine_update(&mut doc, &update).unwrap();
        assert_eq!(
            doc["machines"]["local"]["projects"]["github.com/acme/atlas"]["path"],
            "~/Work/atlas-separate"
        );
        assert_ne!(
            doc["machines"]["local"]["projects"]["github.com/acme/atlas"]["reuse_existing"],
            true
        );
    }

    struct Fixture {
        ctx: Context,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("hb-yaml-{}", context::id().unwrap()));
            fs::create_dir(&root).unwrap();
            Self {
                ctx: Context {
                    home: root.clone(),
                    state: root.clone(),
                    desired: root.join("fleet.yaml"),
                    path: root.join("issues.db"),
                    binary: std::env::current_exe().unwrap(),
                    node: "test".into(),
                    stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                },
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.ctx.state);
        }
    }

    #[test]
    fn sync_inventory_keeps_registered_companions_without_saved_workers() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        fs::create_dir_all(ctx.home.join(".hey-boss")).unwrap();
        fs::write(
            ctx.home.join(".hey-boss/config.json"),
            r#"{"ssh_hosts":["devbox","peer",{"host":"disabled","enabled":false}]}"#,
        )
        .unwrap();
        let yaml = "machines: {local: {workers: []}, peer: {workers: [{id: saved, intent: pause, config: {}}]}}\n";
        fs::write(&ctx.desired, yaml).unwrap();
        let hosts = ctx.inventory().unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(
            hosts.iter().find(|h| h["host"] == "devbox").unwrap()["workers"],
            json!([])
        );
        assert_eq!(
            hosts.iter().find(|h| h["host"] == "peer").unwrap()["workers"][0]["intent"],
            "pause"
        );
        assert_eq!(fs::read_to_string(&ctx.desired).unwrap(), yaml);

        // Connection inventory changes must take effect without restarting or
        // rewriting the saved-worker document.
        fs::write(
            ctx.home.join(".hey-boss/config.json"),
            r#"{"ssh_hosts":[]}"#,
        )
        .unwrap();
        fs::write(ctx.state.join("companion-hosts"), "devbox\n").unwrap();
        let hosts = ctx.inventory().unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0]["host"], "peer");
    }

    #[test]
    fn structured_edit_preserves_other_settings_and_checks_revision() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        fs::write(&ctx.desired, "machines:\n  local:\n    workers:\n      - id: tools\n        intent: pause\n        config:\n          name: Tools\n          tags: [review]\n          concurrency: 2\n").unwrap();
        let first = request(ctx, &json!({})).unwrap();
        assert_eq!(
            first["document"]["machines"]["local"]["workers"][0]["config"]["name"],
            "Tools"
        );
        let edit = json!({"host":"local","id":"tools","intent":"pause","config":{"name":"Review","concurrency":3}});
        let preview = request(
            ctx,
            &json!({"worker_update":edit,"revision":first["revision"],"save":false}),
        )
        .unwrap();
        assert_eq!(preview["valid"], true);
        assert_eq!(
            request(ctx, &json!({})).unwrap()["revision"],
            first["revision"]
        );
        let saved = request(
            ctx,
            &json!({"worker_update":edit,"revision":first["revision"],"save":true}),
        )
        .unwrap();
        let config = &saved["document"]["machines"]["local"]["workers"][0]["config"];
        assert_eq!(config["name"], "Review");
        assert_eq!(config["concurrency"], 3);
        assert_eq!(config["tags"], json!(["review"]));
        assert!(
            request(
                ctx,
                &json!({"worker_update":edit,"revision":first["revision"],"save":true})
            )
            .is_err()
        );
        assert!(request(ctx, &json!({"worker_update":{"host":"local","id":"missing","intent":"pause","config":{}},"revision":saved["revision"],"save":true})).is_err());
    }

    #[test]
    fn editor_saves_exact_text_rejects_stale_edits_and_retains_previous_file() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let original = "# My machines\nmachines:\n  local:\n    workers: []\n";
        fs::write(&ctx.desired, original).unwrap();
        let first = request(ctx, &json!({})).unwrap();
        let text = "# Still my machines\nmachines:\n  local:\n    workers: []\n  devbox:\n    workers: []\n";
        let preview = request(
            ctx,
            &json!({"text":text,"revision":first["revision"],"save":false}),
        )
        .unwrap();
        assert_eq!(preview["valid"], true);
        assert_eq!(fs::read_to_string(&ctx.desired).unwrap(), original);
        let saved = request(
            ctx,
            &json!({"text":text,"revision":first["revision"],"save":true}),
        )
        .unwrap();
        assert_eq!(saved["text"], text);
        assert_eq!(
            fs::read_to_string(ctx.desired.with_extension("yaml.previous")).unwrap(),
            original
        );
        assert!(
            request(
                ctx,
                &json!({"text":original,"revision":first["revision"],"save":true})
            )
            .unwrap_err()
            .to_string()
            .contains("changed")
        );
        assert!(
            request(
                ctx,
                &json!({"text":"machines: [", "revision":saved["revision"],"save":true})
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&ctx.desired).unwrap(), text);
    }

    #[test]
    fn malformed_manual_edits_keep_last_valid_runtime_and_recover() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let original = "machines:\n  local:\n    workers:\n      - id: one\n        intent: running\n        config: {}\n";
        fs::write(&ctx.desired, original).unwrap();
        let first = load(ctx).unwrap();
        fs::write(&ctx.desired, "machines: [").unwrap();
        let broken = load(ctx).unwrap();
        assert_eq!(broken["runtime"], first["runtime"]);
        assert!(broken["error"].is_string());
        fs::write(&ctx.desired, "machines: {local: {workers: []}}\n").unwrap();
        let removed = load(ctx).unwrap();
        assert!(removed["error"].is_null());
        assert_eq!(
            removed["runtime"]["machines"]["local"]["workers"][0]["intent"],
            "drain"
        );
        assert_eq!(load(ctx).unwrap(), removed);
        assert!(
            removed["document"]["machines"]["local"]["workers"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        fs::write(
            ctx.desired.with_extension("json"),
            first["document"].to_string(),
        )
        .unwrap();
        fs::remove_file(&ctx.desired).unwrap();
        let restored = load(ctx).unwrap();
        assert!(
            restored["document"]["machines"]["local"]["workers"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            restored["runtime"]["machines"]["local"]["workers"][0]["intent"],
            "drain"
        );
    }

    #[test]
    fn yaml_validates_the_entire_fleet_without_requiring_remote_paths() {
        let text = "machines:\n  local:\n    workers: []\n  devbox:\n    workers:\n      - id: tools\n        intent: pause\n        config:\n          concurrency: 2\n          projects: [named:Tools]\n          directory: /remote/checkout\n";
        let config = parse(text).unwrap();
        assert_eq!(
            config["machines"]["devbox"]["workers"][0]["config"]["enabled"],
            false
        );
        assert!(parse(&text.replace("concurrency: 2", "concurrency: 0")).is_err());
        assert!(parse(&text.replace("concurrency: 2", "concurency: 2")).is_err());
        assert!(parse(&text.replace("intent: pause", "intent: restart")).is_err());
        assert!(parse("machines: {local: {workers: []}, local: {workers: []}}").is_err());
    }

    #[test]
    fn removed_workers_drain_and_never_reappear_as_running() {
        let old = json!({"machines":{"local":{"workers":[{"id":"one","intent":"running","config":{"enabled":true}}]},"devbox":{"workers":[{"id":"remote","intent":"running","config":{}}]}}});
        let new = json!({"machines":{"local":{"workers":[]}}});
        let runtime = retain_removals(&old, &new);
        assert_eq!(
            runtime["machines"]["local"]["workers"][0]["intent"],
            "drain"
        );
        assert_eq!(
            runtime["machines"]["devbox"]["workers"][0]["intent"],
            "drain"
        );
        assert_eq!(retain_removals(&runtime, &new), runtime);
        assert_eq!(new["machines"]["local"]["workers"], json!([]));
    }

    #[test]
    fn moving_an_id_between_machines_never_starts_overlapping_instances() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let original =
            "machines: {local: {workers: [{id: shared, intent: running, config: {}}]}}\n";
        fs::write(&ctx.desired, original).unwrap();
        let first = request(ctx, &json!({})).unwrap();
        let moved = original.replace("local:", "remote:");
        assert!(
            request(
                ctx,
                &json!({"text":moved,"revision":first["revision"],"save":true})
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&ctx.desired).unwrap(), original);
        fs::write(&ctx.desired, moved).unwrap();
        let status = load(ctx).unwrap();
        assert!(status["error"].as_str().unwrap().contains("new worker ID"));
        assert!(status["runtime"]["machines"]["remote"].is_null());
        assert_eq!(
            status["runtime"]["machines"]["local"]["workers"][0]["intent"],
            "running"
        );
    }
}
