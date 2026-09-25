//! Human planning sessions and one detached, file-authoritative sync owner.
use super::{Error, Operation, Project, Request, Result, Store};
use crate::database::Connection;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{
    fs::{MetadataExt, OpenOptionsExt},
    process::CommandExt,
};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub path: String,
    pub checkout: PathBuf,
    pub machine: String,
    pub host: String,
}
impl Plan {
    pub fn validate(&self) -> Result<()> {
        relative(&self.path)?;
        if !self.checkout.is_absolute() {
            return Err(Error::invalid("Plan checkout must be absolute"));
        }
        super::identifier(&self.machine, "plan machine", 256)?;
        super::identifier(&self.host, "plan host", 256)
    }
    fn file(&self) -> PathBuf {
        self.checkout.join(&self.path)
    }
}
fn relative(path: &str) -> Result<()> {
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::invalid(
            "Plan path must be repository-relative without parent components",
        ));
    }
    Ok(())
}
pub fn validate_template(template: &str) -> Result<()> {
    relative(template)?;
    super::identifier(template, "plan template", 4096)?;
    let expanded = template
        .replace("{timestamp}", "date")
        .replace("{number}", "1")
        .replace("{year}", "2026")
        .replace("{month}", "09")
        .replace("{day}", "19")
        .replace("{hour}", "12")
        .replace("{minute}", "00")
        .replace("{second}", "00");
    if expanded.contains(['{', '}']) {
        return Err(Error::invalid("Unknown plan template placeholder"));
    }
    Ok(())
}
pub fn drafts_allowed(db: &Connection, project: &Project) -> Result<()> {
    let enabled: bool = db.query_row(
        "SELECT coalesce((SELECT drafts_enabled FROM project_settings WHERE project_id=?1),1)",
        [&project.id],
        |r| r.get(0),
    )?;
    if !enabled {
        return Err(Error::invalid("Drafts are disabled in this project"));
    }
    Ok(())
}
pub fn can_draft(
    db: &Connection,
    project: &Project,
    number: i64,
    state: &str,
    assignee: Option<&str>,
) -> Result<()> {
    drafts_allowed(db, project)?;
    let reserved: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL)
         OR EXISTS(SELECT 1 FROM fleet_allocations a
             LEFT JOIN fleet_allocation_deadlines d USING(project_id,issue_number)
             WHERE a.project_id=?1 AND a.issue_number=?2
               AND (d.expires_at IS NULL OR d.expires_at>?3))",
        params![project.id, number, super::worker::now()],
        |r| r.get(0),
    )?;
    if !matches!(state, "open" | "blocked") || assignee.is_some() || reserved {
        return Err(Error::conflict(
            "Only open or blocked, unassigned, unreserved issues can be drafted",
        ));
    }
    Ok(())
}
/// A real ATX level-one heading outside code fences supplies the title.
pub fn parse(text: &str) -> Result<(String, String)> {
    if text.len() > super::BODY_LIMIT {
        return Err(Error::invalid("Plan exceeds 1 MiB"));
    }
    let parser = pulldown_cmark::Parser::new(text).into_offset_iter();
    for (event, range) in parser {
        if matches!(
            event,
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Heading {
                level: pulldown_cmark::HeadingLevel::H1,
                ..
            })
        ) {
            let heading = &text[range.clone()];
            let line = heading.lines().next().unwrap_or("").trim();
            let atx = line == "#" || line.starts_with("# ") || line.starts_with("#\t");
            let mut title = if atx { line[1..].trim() } else { line };
            if atx {
                let without_hashes = title.trim_end_matches('#');
                if without_hashes.ends_with([' ', '\t']) {
                    title = without_hashes.trim_end();
                }
            }
            let title = title.to_owned();
            super::identifier(&title, "plan title", 512)?;
            let body = format!("{}{}", &text[..range.start], &text[range.end..])
                .trim()
                .to_owned();
            return Ok((title, body));
        }
    }
    Err(Error::invalid("Plan needs a first-level # Title heading"))
}
fn lock_path(plan: &Plan) -> PathBuf {
    plan.file().with_extension("hey-boss-sync-lock")
}
fn lock(path: &Path, nonblocking: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let flags = libc::LOCK_EX | if nonblocking { libc::LOCK_NB } else { 0 };
    if unsafe { libc::flock(file.as_raw_fd(), flags) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}
fn protect_database_files(database: &Path, plan: &Plan) -> Result<()> {
    protect_database_paths(
        database,
        [
            plan.file(),
            lock_path(plan),
            plan.file().with_extension("hey-boss-sync-paused"),
        ],
    )
}
pub(crate) fn protect_database_paths<P: AsRef<Path>>(
    database: &Path,
    paths: impl IntoIterator<Item = P>,
) -> Result<()> {
    // Inspect inode identity without opening a raw descriptor: even closing a
    // read-only alias would release this process's SQLite record locks.
    let mut protected = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let mut path = database.as_os_str().to_owned();
        path.push(suffix);
        match fs::metadata(path) {
            Ok(metadata) => protected.push((metadata.dev(), metadata.ino())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for path in paths {
        let path = path.as_ref();
        match fs::metadata(path) {
            Ok(metadata) if protected.contains(&(metadata.dev(), metadata.ino())) => {
                return Err(Error::invalid(format!(
                    "Auxiliary file {} must not alias the active issue database or its sidecars",
                    path.display(),
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn protect_local_database(plan: &Plan) -> Result<()> {
    protect_database_files(&super::database_path()?, plan)
}

fn local_read(db: &Connection, plan: &Plan) -> Result<(String, String)> {
    plan.validate()?;
    if let Some(database) = db.path().filter(|path| !path.is_empty()) {
        protect_database_files(Path::new(database), plan)?;
    }
    let _lock = lock(&lock_path(plan), true).map_err(|_| {
        Error::new(
            "plan_sync_busy",
            "Plan sync is busy. Retry undrafting after reconciliation finishes.",
        )
    })?;
    read(plan).map_err(|error| Error::new("plan_sync_failed", format!("Cannot sync plan {} on {}. Restore or fix the file, then retry undrafting. {error}",plan.path,plan.host)))
}
fn read(plan: &Plan) -> Result<(String, String)> {
    let file = plan.file().canonicalize()?;
    if !file.starts_with(plan.checkout.canonicalize()?) {
        return Err(Error::invalid("Plan must remain inside its saved checkout"));
    }
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(file)?;
    if !f.metadata()?.is_file() {
        return Err(Error::invalid("Plan must be a regular file"));
    }
    let mut text = String::new();
    f.take(super::BODY_LIMIT as u64 + 1)
        .read_to_string(&mut text)?;
    parse(&text)
}
fn owner_host(db: &Connection, plan: &Plan) -> Result<String> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='fleet_state' AND type='table')",
        [],
        |r| r.get(0),
    )?;
    if exists {
        let raw: Option<String> = db
            .query_row(
                "SELECT value FROM fleet_state WHERE key='machines'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = raw {
            let machines: Value = serde_json::from_str(&raw)?;
            if let Some(machine) = machines
                .as_object()
                .and_then(|m| m.values().find(|m| m["node"] == plan.machine))
                && let Some(host) = machine["host"].as_str().filter(|h| *h != "local")
            {
                return Ok(host.to_owned());
            }
        }
    }
    Ok(plan.host.clone())
}
/// Shared CLI/UI final sync. Remote owners are queried synchronously; no stale fallback.
pub fn final_sync(
    db: &Connection,
    _project: &Project,
    title: &mut String,
    body: &mut String,
    plan: Option<&Plan>,
) -> Result<()> {
    let Some(plan) = plan else { return Ok(()) };
    let content = if plan.machine == super::identity::machine()? {
        local_read(db, plan)?
    } else {
        let host = owner_host(db, plan)?;
        if !crate::health::remote::valid_host(&host) {
            return Err(Error::invalid("Invalid plan owner SSH host"));
        }
        let request = Request {
            version: 1,
            project: _project.clone(),
            project_override: None,
            actor: None,
            operation: Operation::ReadPlan { plan: plan.clone() },
            request_id: None,
        };
        let value=super::remote::call(&host, &request).map_err(|error|Error::new("plan_sync_failed",format!("Cannot sync the plan from {}. Reconnect its owning machine and retry undrafting. {error}",plan.host)))?;
        (
            value["title"]
                .as_str()
                .ok_or_else(|| Error::invalid("Invalid plan reply"))?
                .to_owned(),
            value["body"]
                .as_str()
                .ok_or_else(|| Error::invalid("Invalid plan reply"))?
                .to_owned(),
        )
    };
    super::identifier(&content.0, "plan title", 512)?;
    if content.1.len() > super::BODY_LIMIT {
        return Err(Error::invalid("Plan body exceeds 1 MiB"));
    }
    if plan.machine == super::identity::machine()? {
        let pause = plan.file().with_extension("hey-boss-sync-paused");
        if pause.exists() {
            fs::remove_file(pause)?;
        }
    }
    *title = content.0;
    *body = content.1;
    Ok(())
}
pub fn read_plan(db: &Connection, plan: &Plan) -> Result<Value> {
    if plan.machine != super::identity::machine()? {
        return Err(Error::invalid("This machine does not own the plan"));
    }
    let (title, body) = local_read(db, plan)?;
    let pause = plan.file().with_extension("hey-boss-sync-paused");
    if pause.exists() {
        fs::remove_file(pause)?;
    }
    Ok(json!({"ok":true,"title":title,"body":body}))
}
pub fn require_terminal() -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::invalid(
            "Interactive planning requires a human terminal",
        ));
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Sync {
    request: Request,
    host: Option<String>,
}
fn call(sync: &Sync, operation: Operation) -> Result<Value> {
    let mut request = sync.request.clone();
    request.operation = operation;
    request.request_id = None;
    match &sync.host {
        Some(host) => super::remote::call(host, &request),
        None => Store::open(&super::database_path()?)?.execute(&request),
    }
}
fn sync_once(sync: &Sync, issue: &Value, plan: &Plan) -> Result<()> {
    protect_local_database(plan)?;
    let (title, body) = read(plan)?;
    if issue["title"] != title || issue["body"] != body {
        call(
            sync,
            Operation::Edit {
                number: issue["number"].as_i64().unwrap(),
                title: Some(title),
                body: Some(body),
                draft: None,
                add_labels: vec![],
                remove_labels: vec![],
                if_version: None,
            },
        )?;
    }
    Ok(())
}
fn seed(plan: &Plan, issue: &Value) -> Result<()> {
    protect_local_database(plan)?;
    fs::write(
        plan.file(),
        format!(
            "# {}\n\n{}\n",
            issue["title"].as_str().unwrap(),
            issue["body"].as_str().unwrap()
        ),
    )?;
    Ok(())
}
pub fn interactive(
    request: &Request,
    host: Option<&str>,
    value: Value,
    file: Option<&Path>,
    mut pending: Option<Operation>,
) -> Result<Value> {
    let mut sync = Sync {
        request: request.clone(),
        host: host.map(str::to_owned),
    };
    sync.request.project = serde_json::from_value(value["project"].clone())?;
    sync.request.project_override = None;
    let number = value["issue"]["number"].as_i64().unwrap();
    sync.request.operation = Operation::View { number };
    let settings = call(&sync, Operation::ProjectSettings)?;
    if settings["drafts_enabled"] != true {
        return Err(Error::invalid("Drafts are disabled in this project"));
    }
    if value["issue"]["draft"] != true
        && value["issue"]["plan"].is_null()
        && !matches!(
            pending,
            Some(Operation::Edit {
                draft: Some(true),
                ..
            })
        )
    {
        return Err(Error::conflict(
            "Interactive planning requires a draft; use --draft to draft an eligible issue",
        ));
    }
    let checkout = std::env::current_dir()?.canonicalize()?;
    let machine = super::identity::machine()?;
    let saved: Option<Plan> = value["issue"]["plan"]
        .as_object()
        .map(|_| serde_json::from_value(value["issue"]["plan"].clone()))
        .transpose()?;
    if let Some(saved) = &saved
        && (saved.machine != machine || saved.checkout != checkout)
    {
        return Err(Error::conflict(format!(
            "Resume planning on {} in {}",
            saved.host,
            saved.checkout.display()
        )));
    }
    if let Some(saved) = &saved {
        protect_local_database(saved)?;
    }
    // Existing owner blocks here during reconciliation. Hold its content lock until launch.
    let _existing_lock = saved
        .as_ref()
        .map(|p| lock(&lock_path(p), false))
        .transpose()?;
    if let Some(saved) = &saved {
        fs::write(
            saved.file().with_extension("hey-boss-sync-paused"),
            b"Startup reconciliation pending",
        )?;
    }
    let plan = if let Some(file) = file {
        let absolute = file.canonicalize()?;
        let relative = absolute
            .strip_prefix(&checkout)
            .map_err(|_| Error::invalid("Plan file must be inside the project checkout"))?;
        Plan {
            path: relative.to_string_lossy().into(),
            checkout: checkout.clone(),
            machine: machine.clone(),
            host: request.actor.as_ref().unwrap().host.clone(),
        }
    } else if let Some(saved) = &saved {
        saved.clone()
    } else {
        let template = settings["plan_template"].as_str().unwrap();
        validate_template(template)?;
        let date = Command::new("date")
            .args(["-u", "+%Y %m %d %H %M %S"])
            .output()?;
        let date = String::from_utf8_lossy(&date.stdout);
        let parts: Vec<_> = date.split_whitespace().collect();
        if parts.len() != 6 {
            return Err(Error::invalid("Cannot resolve plan timestamp"));
        }
        let timestamp = format!(
            "{}{}{}-{}{}{}",
            parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]
        );
        let mut path = template
            .replace("{timestamp}", &timestamp)
            .replace("{number}", &number.to_string());
        for (key, part) in ["year", "month", "day", "hour", "minute", "second"]
            .iter()
            .zip(parts)
        {
            path = path.replace(&format!("{{{key}}}"), part);
        }
        Plan {
            path,
            checkout: checkout.clone(),
            machine: machine.clone(),
            host: request.actor.as_ref().unwrap().host.clone(),
        }
    };
    plan.validate()?;
    protect_local_database(&plan)?;
    fs::create_dir_all(plan.file().parent().unwrap())?;
    let _new_lock = if saved
        .as_ref()
        .is_none_or(|p| lock_path(p) != lock_path(&plan))
    {
        Some(lock(&lock_path(&plan), false)?)
    } else {
        None
    };
    // No existing file needs reconciliation when generating a new plan.
    if file.is_none()
        && saved.is_none()
        && let Some(operation) = pending.take()
    {
        call(&sync, operation)?;
    }
    let current = call(&sync, Operation::View { number })?;
    if file.is_none() && saved.is_none() {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(plan.file())?;
        write!(
            f,
            "# {}\n\n{}\n",
            current["issue"]["title"].as_str().unwrap(),
            current["issue"]["body"].as_str().unwrap()
        )?;
    }
    let parsed = read(&plan)?;
    if (saved.is_some() || file.is_some())
        && (current["issue"]["title"] != parsed.0 || current["issue"]["body"] != parsed.1)
    {
        println!(
            "hey-boss:\n# {}\n\n{}\n\nfile ({}):\n# {}\n\n{}",
            current["issue"]["title"].as_str().unwrap(),
            current["issue"]["body"].as_str().unwrap(),
            plan.path,
            parsed.0,
            parsed.1
        );
        print!("Choose hey-boss or file (blank cancels): ");
        std::io::stdout().flush()?;
        let mut choice = String::new();
        std::io::stdin().read_line(&mut choice)?;
        if !matches!(choice.trim(), "hey-boss" | "file") {
            return Err(Error::new(
                "cancelled",
                "Planning cancelled; both versions preserved",
            ));
        }
        // Draft eligibility must succeed before reconciliation writes either side.
        // Cancellation above leaves even the issue's lifecycle untouched.
        if let Some(operation) = pending.take() {
            call(&sync, operation)?;
        }
        let current = call(&sync, Operation::View { number })?;
        match choice.trim() {
            "hey-boss" => seed(&plan, &current["issue"])?,
            "file" => {
                sync_once(&sync, &current["issue"], &plan)?;
            }
            _ => {
                return Err(Error::new(
                    "cancelled",
                    "Planning cancelled; both versions preserved",
                ));
            }
        }
    }
    if let Some(operation) = pending.take() {
        call(&sync, operation)?;
    }
    let current = call(&sync, Operation::View { number })?;
    if saved.as_ref() != Some(&plan) {
        call(
            &sync,
            Operation::BindPlan {
                number,
                plan: plan.clone(),
                if_version: current["issue"]["version"].as_i64().unwrap(),
            },
        )?;
    }
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["issue", "plan-sync"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&sync)?)?;
    if current["issue"]["assignee"].is_string() {
        eprintln!("Warning: this issue is assigned; plan edits continue updating it.");
    }
    if let Some(saved) = &saved {
        let pause = saved.file().with_extension("hey-boss-sync-paused");
        if pause.exists() {
            fs::remove_file(pause)?;
        }
    }
    let pause = plan.file().with_extension("hey-boss-sync-paused");
    if pause.exists() {
        fs::remove_file(pause)?;
    }
    drop(_new_lock);
    drop(_existing_lock);
    let mut codex = crate::codex_permissions::apply(&mut Command::new("codex"))
        .current_dir(&checkout)
        .arg(format!("We are planning in {}", plan.path))
        .spawn()?;
    let mut assigned = current["issue"]["assignee"].is_string();
    let mut checked = std::time::Instant::now();
    let status = loop {
        if let Some(status) = codex.try_wait()? {
            break status;
        }
        if checked.elapsed() >= Duration::from_secs(10) {
            if let Ok(value) = call(&sync, Operation::View { number }) {
                let now_assigned = value["issue"]["assignee"].is_string();
                if now_assigned && !assigned {
                    eprintln!("Warning: this issue is assigned; plan edits continue updating it.");
                }
                assigned = now_assigned;
            }
            checked = std::time::Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !status.success() {
        return Err(Error::new(
            "planning_interrupted",
            "Codex did not exit normally; the issue status is preserved",
        ));
    }
    call(&sync, Operation::Undraft { number })
}
pub fn daemon() -> Result<()> {
    let sync: Sync = serde_json::from_reader(std::io::stdin().take(super::WIRE_LIMIT as u64))?;
    let number = sync.request.operation.number();
    // Creation operations have no number; the parent supplies the resolved View operation.
    let number = number.ok_or_else(|| Error::invalid("Sync needs an issue number"))?;
    let state = super::database_path()?.parent().unwrap().join("plan-sync");
    fs::create_dir_all(&state)?;
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    sync.request.project.id.hash(&mut h);
    sync.request.project_override.hash(&mut h);
    let _owner = match lock(&state.join(format!("{}-{number}.lock", h.finish())), true) {
        Ok(lock) => lock,
        Err(_) => return Ok(()),
    };
    _owner.set_len(0)?;
    writeln!(&_owner, "{}", std::process::id())?;
    loop {
        let result = (|| -> Result<()> {
            let value = call(&sync, Operation::View { number })?;
            let Some(plan) = value["issue"]["plan"].as_object() else {
                return Ok(());
            };
            let plan: Plan = serde_json::from_value(json!(plan))?;
            if plan.machine != super::identity::machine()? {
                return Err(Error::invalid("Plan moved to another machine"));
            }
            protect_local_database(&plan)?;
            let _lock = lock(&lock_path(&plan), false)?;
            if plan.file().with_extension("hey-boss-sync-paused").exists() {
                return Ok(());
            }
            // Fetch again after waiting for startup reconciliation.
            let value = call(&sync, Operation::View { number })?;
            if value["issue"]["plan"] != serde_json::to_value(&plan)? {
                return Ok(());
            }
            if value["issue"]["deleted_at"].is_null() {
                sync_once(&sync, &value["issue"], &plan)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Plan sync: {error}");
        }
        std::thread::sleep(Duration::from_secs(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn title_heading_and_remaining_markdown() {
        assert_eq!(
            parse("# Feature\n\n## Context\nBody\n").unwrap(),
            ("Feature".into(), "## Context\nBody".into())
        );
        assert_eq!(
            parse("```md\n# Example\n```\n\n# Real\n\nBody").unwrap(),
            ("Real".into(), "```md\n# Example\n```\n\n\nBody".into())
        );
        assert_eq!(parse("# C#\n\nBody").unwrap().0, "C#");
        assert_eq!(parse("# C# ###\n\nBody").unwrap().0, "C#");
        assert!(parse("## No title").is_err());
    }
    #[test]
    fn templates_stay_inside_checkout() {
        assert!(validate_template("plans/{year}/{timestamp}-{number}.md").is_ok());
        assert!(validate_template("../plan.md").is_err());
        assert!(validate_template("/tmp/plan.md").is_err());
        assert!(validate_template("plans/{unknown}.md").is_err());
    }
}
