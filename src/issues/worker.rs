//! Durable issue workers, each backed by an owned Codex app-server process.
use super::{Actor, Error, Project, Result, Store, identity};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_PROMPT: &str = "Claim and implement `{{issue_command}}`.";
/// Stored as ordinary labels so task intent uses the existing durable fleet wire format.
pub(crate) fn artifact_task(issue: &Value) -> Option<&'static str> {
    let labels = issue["labels"].as_array()?;
    if labels.iter().any(|label| label == "task:research") {
        Some("research")
    } else if labels.iter().any(|label| label == "task:plan") {
        Some("plan")
    } else {
        None
    }
}
pub const DEFAULT_WORKTREE_PROMPT: &str = "Work in a dedicated Git worktree for this issue. Create it before editing files and keep unrelated changes intact.";
pub const DEFAULT_CHECKOUT_PROMPT: &str = "Work in the project's existing checkout.";
pub const DEFAULT_MAIN_PROMPT: &str =
    "Commit your changes. If a Git remote is configured, push to main.";
pub const DEFAULT_PRS_PROMPT: &str = "Commit your changes, push a branch, open a pull request, and attach every PR with `hey-boss issue pr add {{number}} '<pr-url>'`.";
pub(crate) const PR_HANDOFF_PROMPT: &str = "PR handoff: Keep the issue open until the actual fix PR is merged. CI passing and a ready-for-review handoff are not a merge. Continue in this session until required CI and reviews are complete, code feedback and findings are addressed, and conflicts are resolved. Only when the fix is fully merge-ready, record the verification and remaining merge step in an issue comment, then hand it to Boss with `hey-boss issue assign-to-boss {{number}}` and report completed. Do not close the issue at this handoff or merge automatically. If work remains, continue working; if blocked, report blocked rather than completed or assigning it to Boss. Review attachment purposes: supporting evidence PRs do not all need to merge. An explicitly requested source/group closure may still close normally.";
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PromptOverrides {
    pub worktree: Option<String>,
    pub checkout: Option<String>,
    pub prs: Option<String>,
    pub main: Option<String>,
}
impl PromptOverrides {
    pub fn validate(&self) -> Result<()> {
        for text in [&self.worktree, &self.checkout, &self.prs, &self.main]
            .into_iter()
            .flatten()
        {
            if text.trim().is_empty() || text.len() > 32000 {
                return Err(Error::invalid(
                    "Workflow prompts must contain 1–32000 bytes, or null to use the default",
                ));
            }
        }
        Ok(())
    }
}
// Old saved templates still load, but delivery instructions now belong to the workflow.
pub(crate) fn base_prompt(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("}}") else {
            output.push_str(&rest[start..]);
            return output.trim_end().to_owned();
        };
        if rest[start + 2..start + end].trim() != "commit_instruction" {
            output.push_str(&rest[start..start + end + 2]);
        }
        rest = &rest[start + end + 2..];
    }
    output.push_str(rest);
    output.trim_end().to_owned()
}
pub const DEFAULT_CLAIM_TIMEOUT_SECONDS: u32 = 600;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub name: String,
    pub concurrency: u32,
    pub tags: Vec<String>,
    pub projects: Vec<String>,
    pub directory: String,
    /// Explicit checkouts keyed by project; empty for legacy/discovered workers.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub directories: std::collections::BTreeMap<String, String>,
    pub prompt: Option<String>,
    pub prs_enabled: Option<bool>,
    pub worktree_enabled: Option<bool>,
    pub prompt_overrides: Option<PromptOverrides>,
    pub use_goal: bool,
    pub reservation_seconds: u32,
    pub enabled: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            name: "Worker".into(),
            concurrency: 1,
            tags: vec![],
            projects: vec![],
            directory: String::new(),
            directories: Default::default(),
            prompt: None,
            prs_enabled: None,
            worktree_enabled: None,
            prompt_overrides: None,
            use_goal: false,
            reservation_seconds: DEFAULT_CLAIM_TIMEOUT_SECONDS,
            enabled: false,
        }
    }
}
pub fn validate_settings(c: &Settings) -> Result<()> {
    super::identifier(&c.name, "worker name", 128)?;
    if !(1..=1024).contains(&c.concurrency) {
        return Err(Error::invalid(
            "Worker concurrency must be between 1 and 1024",
        ));
    }
    if !(5..=3600).contains(&c.reservation_seconds) {
        return Err(Error::invalid(
            "Claim timeout must be between 5 and 3600 seconds",
        ));
    }
    if c.tags.len() > 50 {
        return Err(Error::invalid("Use at most 50 worker tags"));
    }
    for tag in &c.tags {
        super::identifier(tag, "tag", 64)?;
    }
    for p in &c.projects {
        super::identifier(p, "project", 8192)?;
    }
    if let Some(prompt) = &c.prompt
        && (prompt.trim().is_empty() || prompt.len() > 32000)
    {
        return Err(Error::invalid("Prompt must contain 1–32000 bytes"));
    }
    if let Some(overrides) = &c.prompt_overrides {
        overrides.validate()?;
    }
    if !c.directory.is_empty()
        && (!Path::new(&c.directory).is_absolute() || !Path::new(&c.directory).is_dir())
    {
        return Err(Error::invalid(
            "Choose an existing absolute checkout directory",
        ));
    }
    if !c.directory.is_empty() && c.projects.len() != 1 {
        return Err(Error::invalid(
            "A checkout directory requires one project; leave it empty to discover directories for multiple projects",
        ));
    }
    if !c.directory.is_empty() && !c.directories.is_empty() {
        return Err(Error::invalid(
            "Use either a single checkout or per-project checkouts",
        ));
    }
    for (project, path) in &c.directories {
        if !c.projects.contains(project) {
            return Err(Error::invalid(
                "Each checkout must belong to a selected project",
            ));
        }
        if !Path::new(path).is_absolute() || !Path::new(path).is_dir() {
            return Err(Error::invalid(
                "Choose an existing absolute checkout directory",
            ));
        }
        let actual = identity::project(Path::new(path), &identity::machine()?)?;
        if !project.starts_with("named:") && actual.id != *project {
            return Err(Error::invalid(
                "A worker checkout belongs to a different project",
            ));
        }
    }
    if c.enabled {
        codex_binary()?;
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    pub prompt: String,
    pub cwd: String,
    pub concurrency: u32,
    pub labels: Vec<String>,
    pub use_goal: bool,
    pub enabled: bool,
    pub prs_enabled: bool,
    pub worktree_enabled: bool,
    pub prompt_overrides: PromptOverrides,
}
impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            prompt: DEFAULT_PROMPT.into(),
            cwd: String::new(),
            concurrency: 1,
            labels: vec![],
            use_goal: false,
            enabled: false,
            prs_enabled: false,
            worktree_enabled: false,
            prompt_overrides: PromptOverrides::default(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Job {
    pub id: String,
    #[serde(default)]
    pub worker_id: String,
    #[serde(default)]
    pub resume_session: Option<String>,
    pub project: Project,
    pub issue: Value,
    pub comments: Vec<Value>,
    pub config: ProjectConfig,
    pub actor: Actor,
    pub owner_pid: u32,
    pub owner_start: String,
    pub machine: String,
}
impl Job {
    pub(crate) fn requires_pr(&self) -> bool {
        self.config.prs_enabled && artifact_task(&self.issue).is_none()
    }
    pub fn number(&self) -> i64 {
        self.issue["number"].as_i64().unwrap()
    }
}
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub(crate) fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub fn validate_config(c: &ProjectConfig, p: &Project) -> Result<()> {
    c.prompt_overrides.validate()?;
    if c.prompt.trim().is_empty() || c.prompt.len() > 32_000 {
        return Err(Error::invalid("Worker prompt must contain 1–32000 bytes"));
    }
    if !(1..=1024).contains(&c.concurrency) {
        return Err(Error::invalid(
            "Worker concurrency must be between 1 and 1024",
        ));
    }
    if c.labels.len() > 50 {
        return Err(Error::invalid("Use at most 50 required labels"));
    }
    for tag in &c.labels {
        super::identifier(tag, "label", 64)?;
    }
    if c.cwd.is_empty() && !c.enabled {
        return Ok(());
    }
    let path = Path::new(&c.cwd);
    if !path.is_absolute() || !path.is_dir() {
        return Err(Error::invalid(
            "Choose an existing absolute project directory",
        ));
    }
    let actual = identity::project(&path.canonicalize()?, &identity::machine()?)?;
    if !p.id.starts_with("named:") && actual.id != p.id {
        return Err(Error::invalid(
            "The worker directory belongs to a different project. Choose this repository's checkout or worktree.",
        ));
    }
    if c.enabled {
        codex_binary()?;
    }
    Ok(())
}

pub fn codex_binary() -> Result<PathBuf> {
    crate::agent_runtime::Provider::Codex
        .binary()
        .map_err(|error| {
            if std::env::var_os("HEY_BOSS_CODEX").is_some() {
                Error::invalid(error.to_string())
            } else {
                Error::new("worker_error", error.to_string())
            }
        })
}

pub struct Worker {
    pub stop: Arc<AtomicBool>,
    pub upgrading: Arc<AtomicBool>,
    reload: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

/// Lifecycle writes must finish even when another SQLite writer outlasts the
/// connection's busy timeout. Retry only contention, preserving other errors.
pub fn retry_database_busy<T>(mut operation: impl FnMut() -> Result<T>) -> Result<T> {
    loop {
        match operation() {
            Err(error) if error.code == "database_busy" => {
                crate::worker_tui::diagnostics::report(format_args!(
                    "Worker database temporarily busy: {error}; waiting for the writer"
                ));
                thread::sleep(Duration::from_millis(200));
            }
            result => return result,
        }
    }
}

impl Worker {
    pub fn start(path: PathBuf) -> Result<Self> {
        Self::start_for(path, None)
    }
    pub fn start_for(path: PathBuf, worker_id: Option<String>) -> Result<Self> {
        let machine = identity::machine()?;
        let mut store = retry_database_busy(|| Store::open(&path))?;
        // Never free a slot until the old owned process has actually stopped.
        retry_database_busy(|| recover(&mut store, &machine))?;
        let stop = Arc::new(AtomicBool::new(false));
        let upgrading = Arc::new(AtomicBool::new(false));
        let reload = Arc::new(AtomicBool::new(false));
        let executable = std::env::current_exe()?;
        let original_executable = executable_identity(&executable);
        let upgrade_requested = upgrading.clone();
        let reload_ready = reload.clone();
        let stopped = stop.clone();
        let handle = thread::spawn(move || {
            let drain_marker = path.with_added_extension("drain-for-update");
            let mut update_pending = false;
            let mut handles: Vec<(String, thread::JoinHandle<()>)> = vec![];
            let mut chiefs: Vec<thread::JoinHandle<()>> = vec![];
            let mut last_chief = Instant::now() - Duration::from_secs(5);
            let mut abandoned = Vec::new();
            let mut last_recovery = Instant::now();
            while !stopped.load(Ordering::Relaxed) {
                if let Some(id) = &worker_id
                    && store.worker_shutdown_requested(id).unwrap_or(false)
                {
                    stopped.store(true, Ordering::Relaxed);
                    break;
                }
                let mut active = Vec::new();
                for (id, handle) in handles.drain(..) {
                    if handle.is_finished() {
                        let _ = handle.join();
                        abandoned.push(id);
                    } else {
                        active.push((id, handle));
                    }
                }
                handles = active;
                chiefs.retain_mut(|handle| !handle.is_finished());
                abandoned.retain(|id| {
                    if let Err(e) = finalize_abandoned(&mut store, &machine, id) {
                        crate::worker_tui::diagnostics::report(format_args!(
                            "Worker recovery: {e}"
                        ));
                        true
                    } else {
                        false
                    }
                });
                if worker_id.is_some()
                    && executable_identity(&executable)
                        .is_some_and(|current| Some(current) != original_executable)
                {
                    update_pending = true;
                }
                // Routine replacement never suspends pickup. The old process
                // loads the new CLI at a natural idle point; only an explicit
                // emergency marker stops new work while existing agents finish.
                let draining = update_pending && drain_marker.is_file();
                if draining != upgrade_requested.load(Ordering::Relaxed) {
                    if let Some(id) = &worker_id
                        && let Err(e) = store.worker_set_upgrading(id, draining)
                    {
                        crate::worker_tui::diagnostics::report(format_args!(
                            "Worker upgrade status: {e}"
                        ));
                        thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                    upgrade_requested.store(draining, Ordering::Relaxed);
                }
                if update_pending {
                    if handles.is_empty()
                        && chiefs.is_empty()
                        && abandoned.is_empty()
                        && worker_id
                            .as_deref()
                            .is_some_and(|id| store.worker_reload_allowed(id).unwrap_or(false))
                    {
                        reload_ready.store(true, Ordering::Relaxed);
                        if stopped
                            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                            .is_err()
                        {
                            reload_ready.store(false, Ordering::Relaxed);
                        }
                        break;
                    }
                    if draining {
                        thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                }
                if last_recovery.elapsed() >= Duration::from_secs(5) {
                    if let Err(e) = recover(&mut store, &machine) {
                        crate::worker_tui::diagnostics::report(format_args!(
                            "Worker recovery: {e}"
                        ));
                    }
                    last_recovery = Instant::now();
                }
                if last_chief.elapsed() >= Duration::from_secs(5) {
                    match store.reserve_chief(&machine, worker_id.as_deref()) {
                        Ok(Some(job)) => {
                            let path = path.clone();
                            let stop = stopped.clone();
                            chiefs.push(thread::spawn(move || {
                                super::chief::execute(path, job, stop)
                            }));
                        }
                        Ok(None) => {}
                        Err(error) => crate::worker_tui::diagnostics::report(format_args!(
                            "Chief scheduler: {error}"
                        )),
                    }
                    last_chief = Instant::now();
                }
                match store.worker_reserve(&machine, worker_id.as_deref()) {
                    Ok(Some(job)) => {
                        let path = path.clone();
                        let stop = stopped.clone();
                        handles.push((
                            job.id.clone(),
                            thread::spawn(move || execute_job(&path, job, stop)),
                        ));
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => crate::worker_tui::diagnostics::report(format_args!(
                        "Worker scheduler: {e}"
                    )),
                }
                for _ in 0..5 {
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(200));
                }
            }
            for (id, handle) in handles {
                let _ = handle.join();
                if let Err(e) =
                    retry_database_busy(|| finalize_abandoned(&mut store, &machine, &id))
                {
                    crate::worker_tui::diagnostics::report(format_args!("Worker recovery: {e}"));
                }
            }
            for handle in chiefs {
                let _ = handle.join();
            }
        });
        Ok(Self {
            stop,
            upgrading,
            reload,
            handle: Some(handle),
        })
    }
}
pub(crate) fn executable_identity(path: &Path) -> Option<(u64, u64, i64, i64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some((
        metadata.dev(),
        metadata.ino(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.len(),
    ))
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
pub fn install_signals(stop: Arc<AtomicBool>) -> Result<()> {
    install_signals_for_upgrade(stop, None)
}
pub(crate) fn install_signals_for_upgrade(
    stop: Arc<AtomicBool>,
    reload: Option<Arc<AtomicBool>>,
) -> Result<()> {
    ctrlc::set_handler(move || {
        if let Some(reload) = &reload {
            reload.store(false, Ordering::Relaxed);
        }
        stop.store(true, Ordering::Relaxed);
    })
    .map_err(|e| Error::new("worker_error", e.to_string()))
}
pub fn serve() -> Result<()> {
    let worker = Worker::start(super::database_path()?)?;
    install_signals(worker.stop.clone())?;
    println!(
        "Hey Boss issue workers · monitoring enabled managed workers. Ctrl+C stops owned sessions."
    );
    while !worker.stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}
fn alive(pid: u32, start: &str) -> bool {
    crate::agents::process_identity(pid).as_deref() == Some(start)
}
pub(super) fn stop_group(pid: u32, start: &str) -> Result<()> {
    if !alive(pid, start) {
        return Ok(());
    }
    if unsafe { libc::kill(-(pid as i32), libc::SIGTERM) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ESRCH) {
            return Err(e.into());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    while alive(pid, start) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    if alive(pid, start) && unsafe { libc::kill(-(pid as i32), libc::SIGKILL) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ESRCH) {
            return Err(e.into());
        }
    }
    Ok(())
}
fn finalize_abandoned(store: &mut Store, machine: &str, id: &str) -> Result<()> {
    for (job, pid, start) in store.worker_orphans(machine)? {
        if job.id != id {
            continue;
        }
        if let (Some(pid), Some(start)) = (pid, start) {
            stop_group(pid, &start)?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while alive(pid, &start) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            if alive(pid, &start) {
                return Err(Error::new(
                    "worker_error",
                    "Failed worker process is still alive; capacity retained",
                ));
            }
        }
        store.worker_finish(
            &job,
            "failed",
            "Worker exited before saving its result. Review the saved session before retrying.",
        )?;
    }
    Ok(())
}
pub(crate) fn recover(store: &mut Store, machine: &str) -> Result<()> {
    store.prune_workers(machine)?;
    for (job, pid, start) in store.worker_orphans(machine)? {
        if alive(job.owner_pid, &job.owner_start) {
            continue;
        }
        if let (Some(pid), Some(start)) = (pid, start) {
            stop_group(pid, &start)?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while alive(pid, &start) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            if alive(pid, &start) {
                return Err(Error::new(
                    "worker_error",
                    "An orphaned Codex process could not be stopped. Its issue claim and capacity were retained.",
                ));
            }
        }
        store.worker_finish(&job,"interrupted","The worker service stopped unexpectedly. The saved Codex session and issue history are retained. Review or retry this issue.")?;
    }
    store.release_stale_claims(machine, now())?;
    Ok(())
}

struct Codex {
    process: crate::agent_process::Process,
    pending: VecDeque<Value>,
    next_id: u64,
    session: Option<String>,
    native_goal: bool,
    approvals: super::worker_approvals::Approvals,
    approval_items: VecDeque<(String, Value)>,
}
impl Codex {
    fn spawn(path: &Path, job: &Job) -> Result<Self> {
        let binary = codex_binary()?;
        let mut paths = vec![
            binary.parent().unwrap().to_path_buf(),
            std::env::current_exe()?.parent().unwrap().to_path_buf(),
        ];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let mut command = Command::new(binary);
        crate::codex_permissions::apply(&mut command);
        command
            .args(["app-server", "--listen", "stdio://"])
            .current_dir(&job.config.cwd)
            .env("HEY_BOSS_ISSUE_DB", path)
            .env("HEY_BOSS_ISSUE_PROJECT", &job.project.id)
            .env(
                "PATH",
                std::env::join_paths(paths)
                    .map_err(|e| Error::new("worker_error", e.to_string()))?,
            )
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_AGENT_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CLAUDE_SESSION_ID")
            .env_remove("PI_SESSION_ID")
            .env_remove("CLAUDECODE");
        let process = crate::agent_process::Process::spawn(&mut command)?;
        Ok(Self {
            process,
            pending: VecDeque::new(),
            next_id: 0,
            session: None,
            native_goal: false,
            approvals: Default::default(),
            approval_items: VecDeque::new(),
        })
    }
    fn suspend_goal(&mut self, state: &str) -> Result<Option<Value>> {
        let Some(session) = self.session.clone().filter(|_| self.native_goal) else {
            return Ok(None);
        };
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"id":id,"method":"thread/goal/set","params":{"threadId":session,"status":state}}))?;
        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline {
            if let Some(value) = self.receive()?
                && value.get("id") == Some(&json!(id))
                && value.get("method").is_none()
            {
                return Ok(value.get("result").and_then(|r| r.get("goal")).cloned());
            }
        }
        Ok(None)
    }
    fn send(&mut self, value: Value) -> Result<()> {
        self.process.send(&value).map_err(Error::from)
    }
    fn receive(&mut self) -> Result<Option<Value>> {
        self.process
            .receive(Duration::from_millis(200))
            .map_err(Error::from)
    }
    fn check(store: &Store, job: &Job, stop: &AtomicBool) -> Result<()> {
        if store.worker_model_expired(job)? {
            return Err(Error::new(
                "startup_timeout",
                "Codex did not begin model work within 15 minutes. The session was stopped and the issue can retry.",
            ));
        }
        if store.worker_claim_expired(job)? {
            return Err(Error::new(
                "claim_timeout",
                "Codex did not claim the issue before its reservation expired. The session was stopped; review or retry the issue.",
            ));
        }
        if stop.load(Ordering::Relaxed) || store.worker_cancelled(job)? {
            return Err(Error::new(
                "cancelled",
                "Worker stopped, claim deadline expired, or issue ownership changed",
            ));
        }
        Ok(())
    }
    fn rpc(
        &mut self,
        method: &str,
        params: Value,
        store: &mut Store,
        job: &Job,
        stop: &AtomicBool,
    ) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"id":id,"method":method,"params":params}))?;
        let mut deadline = Instant::now() + Duration::from_secs(45);
        loop {
            Self::check(store, job, stop)?;
            self.poll_approvals(store, job, stop)?;
            if self.approvals.is_pending() {
                deadline = Instant::now() + Duration::from_secs(45);
            }
            if Instant::now() > deadline {
                return Err(Error::new(
                    "worker_error",
                    format!("Codex timed out acknowledging {method}"),
                ));
            }
            let Some(value) = self.receive()? else {
                continue;
            };
            self.observe_approvals(&value);
            if value.get("id") == Some(&json!(id)) && value.get("method").is_none() {
                if let Some(error) = value.get("error") {
                    return Err(Error::new(
                        "worker_error",
                        format!("Codex {method}: {}", error["message"]),
                    ));
                }
                return value.get("result").cloned().ok_or_else(|| {
                    Error::new("worker_error", "Codex returned an invalid acknowledgement")
                });
            }
            if value.get("id").is_some() && value.get("method").is_some() {
                self.request_input(value, store, job, stop)?;
                continue;
            }
            if self.pending.len() > 4096 {
                return Err(Error::new(
                    "worker_error",
                    "Too many Codex events before acknowledgement",
                ));
            }
            self.pending.push_back(value);
        }
    }
    fn observe_approvals(&mut self, value: &Value) {
        // An owned server may still report other threads; never mix their requests.
        if value["params"]["threadId"]
            .as_str()
            .is_some_and(|id| self.session.as_deref() != Some(id))
        {
            return;
        }
        self.approvals.observe(value);
        if value["method"] == "item/started" && value["params"]["item"]["type"] == "fileChange" {
            let item = &value["params"]["item"];
            if let Some(id) = item["id"].as_str() {
                self.approval_items.retain(|(saved, _)| saved != id);
                let preview = if serde_json::to_vec(item).is_ok_and(|bytes| bytes.len() <= 65536) {
                    item.clone()
                } else {
                    json!({"previewTooLarge":true})
                };
                self.approval_items.push_back((id.into(), preview));
                if self.approval_items.len() > 64 {
                    self.approval_items.pop_front();
                }
            }
        }
    }
    fn request_input(
        &mut self,
        value: Value,
        store: &mut Store,
        job: &Job,
        stop: &AtomicBool,
    ) -> Result<()> {
        Self::check(store, job, stop)?;
        if value["params"]["threadId"].as_str() != self.session.as_deref() || self.session.is_none()
        {
            return Err(Error::new(
                "blocked",
                "Codex needs input or approval: request came from an unowned session; no decision was sent",
            ));
        }
        let preview = self
            .approval_items
            .iter()
            .rev()
            .find(|(id, _)| Some(id.as_str()) == value["params"]["itemId"].as_str())
            .map(|(_, item)| item);
        match self.approvals.start(&value, job, preview) {
            Ok(true) => {
                store.worker_event(
                    &job.id,
                    "Waiting for Codex approval in Hey Boss Inbox",
                    None,
                )?;
                Ok(())
            }
            Ok(false) => self.needs_input(value),
            Err(error) => {
                // Fail closed while keeping the original request in the saved session.
                let _: Result<()> = self.needs_input(value);
                Err(Error::new(
                    "blocked",
                    format!(
                        "Codex needs input or approval: {error}. Resume the saved session or retry after connecting Hey Boss."
                    ),
                ))
            }
        }
    }
    fn poll_approvals(&mut self, store: &mut Store, job: &Job, stop: &AtomicBool) -> Result<()> {
        for (response, cancelled) in self.approvals.poll().map_err(|e| {
            Error::new(
                "blocked",
                format!("Codex needs input or approval: Inbox bridge unavailable: {e}. Review the saved session."),
            )
        })? {
            Self::check(store, job, stop)?;
            self.send(response)?;
            if cancelled {
                return Err(Error::new(
                    "blocked",
                    "Codex needs input or approval: request cancelled or answered without a supported decision. Explicit retry is required.",
                ));
            }
            store.worker_event(
                &job.id,
                "Codex approval answered in Hey Boss; continuing session",
                None,
            )?;
        }
        Ok(())
    }
    fn needs_input<T>(&mut self, value: Value) -> Result<T> {
        let method = value["method"].as_str().unwrap_or("input");
        // Unsupported input and unavailable Inbox fail closed with a saved session.
        let response = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                json!({"decision":"cancel"})
            }
            "item/permissions/requestApproval" => json!({"permissions":{},"scope":"turn"}),
            "item/tool/requestUserInput" | "tool/requestUserInput" => json!({"answers":{}}),
            "mcpServer/elicitation/request" => json!({"action":"cancel","content":null}),
            _ => json!({}),
        };
        self.send(json!({"id":value["id"],"result":response}))?;
        let detail = value["params"]["reason"]
            .as_str()
            .or_else(|| value["params"]["command"].as_str())
            .unwrap_or(method);
        Err(Error::new(
            "blocked",
            format!(
                "Codex needs input or approval: {detail}. Resume the saved session to continue."
            ),
        ))
    }
}
impl Drop for Codex {
    fn drop(&mut self) {
        let _ = self.process.stop();
        // The server is stopped before its remaining Inbox questions are cancelled.
        self.approvals = Default::default();
    }
}

fn execute_job(path: &Path, mut job: Job, stop: Arc<AtomicBool>) {
    let mut store = match retry_database_busy(|| Store::open(path)) {
        Ok(s) => s,
        Err(e) => {
            crate::worker_tui::diagnostics::report(format_args!("Worker {}: {e}", job.id));
            return;
        }
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_codex(path, &mut store, &mut job, &stop)
    }));
    let (state, summary) = match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => (
            match error.code.as_str() {
                "cancelled" => "cancelled",
                "claim_timeout" => "claim_timeout",
                "blocked" => "blocked",
                _ => "failed",
            }
            .into(),
            error.message,
        ),
        Err(_) => (
            "failed".into(),
            "Worker encountered an internal error. Review the saved session before retrying."
                .into(),
        ),
    };
    if let Err(e) = retry_database_busy(|| store.worker_finish(&job, &state, &summary)) {
        crate::worker_tui::diagnostics::report(format_args!(
            "Worker {} could not finalize: {e}",
            job.id
        ))
    }
}
fn template(text: &str, job: &Job) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("}}") else {
            output.push_str(&rest[start..]);
            return output;
        };
        let key = rest[start + 2..start + end].trim();
        let value = match key {
            "issue_command" => format!("hey-boss issue view {}", number_text(job)),
            "create_issue_command" => create_issue_command(None),
            "project" => job.project.id.clone(),
            "project_arg" => format!("'{}'", job.project.id.replace('\'', "'\\''")),
            "number" => number_text(job),
            "title" => job.issue["title"].as_str().unwrap().into(),
            "body" => job.issue["body"].as_str().unwrap().into(),
            "commit_instruction" => String::new(),
            _ => match key.split_once(char::is_whitespace) {
                Some(("create_issue_command", project)) if !project.trim().is_empty() => {
                    create_issue_command(Some(project.trim()))
                }
                _ => rest[start..start + end + 2].into(),
            },
        };
        output.push_str(&value);
        rest = &rest[start + end + 2..];
    }
    output.push_str(rest);
    output
}
fn create_issue_command(project: Option<&str>) -> String {
    let target = project
        .map(|value| format!(" --project '{}'", value.replace('\'', "'\\''")))
        .unwrap_or_default();
    format!("hey-boss issue create{target} --title '<title>' --body '<markdown>'")
}
fn number_text(job: &Job) -> String {
    job.issue["number"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| job.number().to_string())
}
pub(crate) fn preview(
    config: &ProjectConfig,
    project: &Project,
    issue: Value,
) -> (String, bool, String) {
    let job = Job {
        id: String::new(),
        worker_id: String::new(),
        resume_session: None,
        project: project.clone(),
        issue,
        comments: vec![],
        config: config.clone(),
        actor: Actor {
            id: String::new(),
            kind: String::new(),
            session_id: None,
            machine: String::new(),
            host: String::new(),
            pid: None,
            process_start: None,
            cwd: config.cwd.clone().into(),
            source: String::new(),
            invocation: None,
            creation_run: None,
        },
        owner_pid: 0,
        owner_start: String::new(),
        machine: String::new(),
    };
    prompt(&job)
}
fn prompt(job: &Job) -> (String, bool, String) {
    prompt_with_config(job, &job.config)
}
fn prompt_with_config(job: &Job, config: &ProjectConfig) -> (String, bool, String) {
    let rendered = template(&base_prompt(&config.prompt), job);
    let after_goal = rendered
        .trim_start()
        .strip_prefix("/goal")
        .filter(|rest| rest.chars().next().is_none_or(char::is_whitespace));
    let goal = after_goal.is_some();
    let instructions = if let Some(rest) = after_goal {
        if rest.trim().is_empty() {
            template(DEFAULT_PROMPT, job)
        } else {
            rest.trim_start().to_owned()
        }
    } else {
        rendered
    };
    if let Some(kind) = artifact_task(&job.issue) {
        // Task intent replaces saved implementation instructions, including custom
        // delivery/workspace branches. Goal mode still follows the saved /goal prefix.
        let focus = if kind == "plan" {
            "Produce a concrete plan with scope, design, tradeoffs, implementation steps, and verification criteria."
        } else {
            "Investigate the question and document findings, sources, uncertainties, tradeoffs, and recommendations."
        };
        let instructions = template(
            &format!(
                "Claim and {kind} `{{{{issue_command}}}}`.\n\n{focus}\n\nDeliver artifacts only; do not implement code, commit, push, or deploy. Save the result with `hey-boss artifact create --title '<title>' --body '<markdown>' --issue {{{{number}}}} --project {{{{project_arg}}}}`. Link every output artifact to this issue. When several related artifacts or topics benefit from an overview, organize and link them in a project mindmap using `hey-boss mm --project {{{{project_arg}}}}`. If the result identifies actionable work, create draft follow-up issues with `hey-boss issue create --draft --title '<title>' --body '<markdown>' --project {{{{project_arg}}}}` and reference the source artifact and this issue; do not start those issues. If drafts are unavailable, record proposed follow-ups in the artifact. Finish with links to the saved artifacts, any mindmap, and follow-up issues."
            ),
            job,
        );
        let instructions = if let Some(path) = job.issue["plan"]["path"].as_str() {
            format!("{instructions}\n\nPlan document: {path}")
        } else {
            instructions
        };
        let objective = instructions.trim().chars().take(4000).collect();
        return (instructions, goal, objective);
    }
    let overrides = &config.prompt_overrides;
    let workspace = if config.worktree_enabled {
        overrides
            .worktree
            .as_deref()
            .unwrap_or(DEFAULT_WORKTREE_PROMPT)
    } else {
        overrides
            .checkout
            .as_deref()
            .unwrap_or(DEFAULT_CHECKOUT_PROMPT)
    };
    let delivery = if config.prs_enabled {
        overrides.prs.as_deref().unwrap_or(DEFAULT_PRS_PROMPT)
    } else {
        overrides.main.as_deref().unwrap_or(DEFAULT_MAIN_PROMPT)
    };
    let instructions = format!(
        "{}\n\n{}\n\n{}",
        instructions.trim_end(),
        template(workspace, job),
        template(delivery, job)
    );
    // Lifecycle rules also apply to saved/custom delivery prompts. Preview,
    // claims, new sessions and resumed sessions all use this assembly path.
    let instructions = if config.prs_enabled {
        format!("{instructions}\n\n{}", template(PR_HANDOFF_PROMPT, job))
    } else {
        instructions
    };
    let instructions = if let Some(path) = job.issue["plan"]["path"].as_str() {
        format!("{instructions}\n\nPlan document: {path}")
    } else {
        instructions
    };
    let objective = instructions.trim().chars().take(4000).collect();
    (instructions, goal, objective)
}
fn turn_params(session: &str, text: &str) -> Value {
    json!({"threadId":session,"input":[{"type":"text","text":text}],"outputSchema":{"type":"object","properties":{"status":{"type":"string","enum":["completed","blocked"]},"summary":{"type":"string"}},"required":["status","summary"],"additionalProperties":false}})
}
fn run_codex(
    path: &Path,
    store: &mut Store,
    job: &mut Job,
    stop: &AtomicBool,
) -> Result<(String, String)> {
    validate_config(&job.config, &job.project)?;
    Codex::check(store, job, stop)?;
    let mut c = Codex::spawn(path, job)?;
    store.worker_process(&job.id, c.process.pid())?;
    store.worker_event(&job.id, "Launching Codex", None)?;
    let outcome = run_thread(&mut c, store, job, stop);
    if let Err(error) = &outcome {
        let state = if matches!(error.code.as_str(), "cancelled" | "claim_timeout") {
            "paused"
        } else {
            "blocked"
        };
        if let Ok(Some(goal)) = c.suspend_goal(state) {
            let _ = store.worker_event(&job.id, &format!("Goal: {state}"), Some(&goal));
        }
    }
    outcome
}
fn run_thread(
    c: &mut Codex,
    store: &mut Store,
    job: &mut Job,
    stop: &AtomicBool,
) -> Result<(String, String)> {
    c.rpc("initialize",json!({"clientInfo":{"name":"hey_boss_worker","title":"Hey Boss issue worker","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),store,job,stop)?;
    c.send(json!({"method":"initialized","params":{}}))?;
    let (method, params) = if let Some(session) = &job.resume_session {
        store.worker_event(&job.id, &format!("Resuming Codex session {session}"), None)?;
        (
            "thread/resume",
            // Codex retains the full history; the worker only needs the session
            // metadata. Large rollouts otherwise exceed the transport limit.
            json!({"threadId":session,"cwd":job.config.cwd,"excludeTurns":true}),
        )
    } else {
        (
            "thread/start",
            json!({"cwd":job.config.cwd,"ephemeral":false}),
        )
    };
    let result = c.rpc(
        method,
        crate::codex_permissions::thread(params),
        store,
        job,
        stop,
    )?;
    let session = result["thread"]["id"]
        .as_str()
        .ok_or_else(|| Error::new("worker_error", "Codex did not return a session ID"))?
        .to_owned();
    if job
        .resume_session
        .as_ref()
        .is_some_and(|saved| saved != &session)
    {
        return Err(Error::new(
            "worker_error",
            "Codex resumed a different session than the saved issue session",
        ));
    }
    c.session = Some(session.clone());
    store.worker_attach(job, &session)?;
    store.worker_event(&job.id, &format!("Codex session {session}"), None)?;
    let (text, goal_enabled, objective) = prompt(job);
    store.worker_prompt(job, &text)?;
    let result = c.rpc("turn/start", turn_params(&session, &text), store, job, stop)?;
    let mut turn = result["turn"]["id"]
        .as_str()
        .ok_or_else(|| Error::new("worker_error", "Codex did not return a turn ID"))?
        .to_owned();
    if goal_enabled {
        c.native_goal = true;
        let result = c.rpc(
            "thread/goal/set",
            json!({"threadId":session,"objective":objective,"status":"active"}),
            store,
            job,
            stop,
        )?;
        if result["goal"]["status"] != "active" {
            return Err(Error::new(
                "worker_error",
                "Codex did not activate the goal",
            ));
        }
        store.worker_event(&job.id, "/goal activated", Some(&result["goal"]))?;
    }
    let mut final_text = String::new();
    let mut claim_window_started = false;
    let mut activity_text = String::new();
    let mut last_log = Instant::now() - Duration::from_secs(2);
    let mut applied_prompt = text;
    let mut last_prompt_check = Instant::now();
    loop {
        Codex::check(store, job, stop)?;
        c.poll_approvals(store, job, stop)?;
        if last_prompt_check.elapsed() >= Duration::from_secs(2) {
            last_prompt_check = Instant::now();
            let config = store.worker_prompt_config(job)?;
            let next_prompt = prompt_with_config(job, &config).0;
            if next_prompt != applied_prompt {
                let steering = prompt_update(&next_prompt);
                match c.rpc("turn/steer", json!({"threadId":session,"expectedTurnId":turn,"input":[{"type":"text","text":steering}]}), store, job, stop) {
                    Ok(ack) if ack["turnId"] == turn => {
                        applied_prompt = next_prompt;
                        job.config = config;
                        store.worker_prompt(job, &applied_prompt)?;
                        store.worker_event(&job.id, "Updated instructions delivered to the running agent", None)?;
                    }
                    // A turn may have ended while the settings were being read.
                    // Keep the update pending; completion below starts a follow-up
                    // in the same thread before accepting a completion report.
                    result => {
                        Codex::check(store, job, stop)?;
                        let detail = result.err().map(|e| e.to_string()).unwrap_or_else(|| "Unexpected steering acknowledgement".into());
                        store.worker_event(&job.id, &format!("Instruction update pending; retrying: {detail}"), None)?;
                    }
                }
            }
        }
        let Some(value) = (if let Some(pending) = c.pending.pop_front() {
            Some(pending)
        } else {
            c.receive()?
        }) else {
            continue;
        };
        c.observe_approvals(&value);
        if value.get("id").is_some() && value.get("method").is_some() {
            c.request_input(value, store, job, stop)?;
            continue;
        }
        let method = value["method"].as_str().unwrap_or("");
        let params = &value["params"];
        if params["threadId"].as_str().is_some_and(|id| id != session) {
            continue;
        }
        if !claim_window_started
            && matches!(
                method,
                "item/started"
                    | "item/agentMessage/delta"
                    | "item/reasoning/textDelta"
                    | "item/reasoning/summaryTextDelta"
                    | "item/completed"
            )
            && params["item"]["type"] != "userMessage"
        {
            store.worker_begin_claim(&job.id)?;
            claim_window_started = true;
        }
        match method {
            "thread/goal/updated" => {
                store.worker_event(
                    &job.id,
                    &format!(
                        "Goal: {}",
                        params["goal"]["status"].as_str().unwrap_or("updated")
                    ),
                    Some(&params["goal"]),
                )?;
            }
            "item/agentMessage/delta" => {
                if let Some(text) = params["delta"].as_str() {
                    activity_text.push_str(text);
                    if activity_text.len() > 4000 {
                        activity_text = activity_text
                            .chars()
                            .rev()
                            .take(2000)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                    }
                    if last_log.elapsed() > Duration::from_secs(1) {
                        store.worker_event(&job.id, &activity_text, None)?;
                        last_log = Instant::now();
                    }
                }
            }
            "item/started" => {
                let item = &params["item"];
                let kind = item["type"].as_str().unwrap_or("working");
                if kind == "agentMessage" {
                    activity_text.clear();
                }
                let detail = item["command"]
                    .as_str()
                    .or_else(|| item["text"].as_str())
                    .unwrap_or(kind);
                store.worker_event(&job.id, detail, None)?;
            }
            "item/completed" if params["item"]["type"] == "agentMessage" => {
                final_text = params["item"]["text"].as_str().unwrap_or("").to_owned();
                store.worker_event(&job.id, &final_text, None)?;
            }
            "turn/started" => {
                if let Some(id) = params["turn"]["id"].as_str() {
                    turn = id.into();
                }
            }
            "turn/completed" if params["turn"]["id"] == turn => {
                if params["turn"]["status"] != "completed" {
                    return Err(Error::new(
                        "worker_error",
                        format!(
                            "Codex turn {}: {}",
                            params["turn"]["status"], params["turn"]["error"]
                        ),
                    ));
                }
                let config = store.worker_prompt_config(job)?;
                let next_prompt = prompt_with_config(job, &config).0;
                if next_prompt != applied_prompt {
                    let result = c.rpc(
                        "turn/start",
                        turn_params(&session, &prompt_update(&next_prompt)),
                        store,
                        job,
                        stop,
                    )?;
                    turn = result["turn"]["id"]
                        .as_str()
                        .ok_or_else(|| {
                            Error::new("worker_error", "Missing instruction update turn")
                        })?
                        .into();
                    applied_prompt = next_prompt;
                    job.config = config;
                    store.worker_prompt(job, &applied_prompt)?;
                    store.worker_event(
                        &job.id,
                        "Updated instructions delivered in the same agent session",
                        None,
                    )?;
                    final_text.clear();
                    continue;
                }
                let report = serde_json::from_str::<Value>(final_text.trim())
                    .ok()
                    .filter(|v| {
                        matches!(v["status"].as_str(), Some("completed" | "blocked"))
                            && v["summary"].as_str().is_some_and(|s| !s.trim().is_empty())
                    });
                if let Some(report) = report {
                    if goal_enabled {
                        let state = if report["status"] == "completed" {
                            "complete"
                        } else {
                            "blocked"
                        };
                        let goal = c.rpc(
                            "thread/goal/set",
                            json!({"threadId":session,"status":state}),
                            store,
                            job,
                            stop,
                        )?;
                        store.worker_event(
                            &job.id,
                            &format!("Goal: {state}"),
                            Some(&goal["goal"]),
                        )?;
                    }
                    return Ok((
                        report["status"].as_str().unwrap().into(),
                        report["summary"].as_str().unwrap().into(),
                    ));
                }
                if goal_enabled {
                    let result = c.rpc(
                        "thread/goal/get",
                        json!({"threadId":session}),
                        store,
                        job,
                        stop,
                    )?;
                    store.worker_event(&job.id, "Checking goal progress", Some(&result["goal"]))?;
                    match result["goal"]["status"].as_str() {
                        Some("complete") if !final_text.trim().is_empty() => {
                            return Ok(("completed".into(), final_text));
                        }
                        Some("active") => {
                            let result=c.rpc("turn/start",turn_params(&session,"Continue pursuing the saved goal and the assigned issue. Return the required JSON status and summary only after completing and verifying the issue or identifying a blocker."),store,job,stop)?;
                            turn = result["turn"]["id"]
                                .as_str()
                                .ok_or_else(|| {
                                    Error::new("worker_error", "Missing continuation turn")
                                })?
                                .into();
                            final_text.clear();
                        }
                        _ => {
                            return Err(Error::new(
                                "blocked",
                                format!(
                                    "Codex goal stopped with status {}. {final_text}",
                                    result["goal"]["status"]
                                ),
                            ));
                        }
                    }
                } else {
                    return Err(Error::new(
                        "blocked",
                        format!(
                            "Codex ended without a completion report. Review the saved session.\n\n{final_text}"
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
}

fn prompt_update(instructions: &str) -> String {
    format!(
        "Project instructions have changed. Apply the updated instructions below to your current assigned issue, preserving your progress and session. These replace the previous project instructions. Continue the task and verify it against this update before reporting completion. Your existing workspace, delivery mode, and goal lifecycle remain in effect.\n\n{instructions}"
    )
}

pub fn print_status(v: &Value, redraw: bool) {
    print_status_with_history(v, redraw, 3);
}
pub fn print_status_with_history(v: &Value, redraw: bool, history_limit: usize) {
    if redraw {
        print!("\x1b[H\x1b[2J");
    }
    println!(
        "{} · {}",
        v["config"]["name"].as_str().unwrap_or("Workers"),
        v["worker_id"].as_str().unwrap_or("new")
    );
    println!(
        "Slots: {} free · {} busy / {} · {}",
        v["free"],
        v["active"],
        v["config"]["concurrency"],
        if v["config"]["enabled"] == true && v["upgrading"] == true {
            "emergency drain for CLI update"
        } else if v["config"]["enabled"] == true {
            "pickup on"
        } else {
            "paused"
        }
    );
    if let Some(database) = v["store"]["database"].as_str() {
        println!(
            "Queue: {} · {database}",
            v["store"]["host"].as_str().unwrap_or("local")
        );
    }
    println!("Projects: {}", v["config"]["projects"]);
    if let Some(directories) = v["config"]["directories"].as_object() {
        for (project, path) in directories {
            println!(
                "Checkout: {project} · {}",
                path.as_str().unwrap_or_default()
            );
        }
    }
    if matches!(v["fleet"]["role"].as_str(), Some("companion" | "agent")) {
        println!(
            "Fleet replica · {} local changes waiting to synchronize · workers can continue reserved work offline",
            v["fleet"]["pending_changes"]
        );
    }
    if let Some(queue) = v["queue"].as_object() {
        println!(
            "Queue issues: {} open · {} assigned · {} eligible · {} excluded by tags · {} waiting",
            queue["open"],
            queue["assigned"],
            queue["eligible"],
            queue["tag_filtered"],
            queue["waiting"]
        );
        if queue["waiting"].as_i64().unwrap_or(0) > 0 {
            println!(
                "Waiting issues have active reservations, unfinished subtasks, retry holds or fleet allocation requirements."
            );
        }
    }
    println!(
        "Pipeline: refresh issue order → scan visible projects → filter tags {} → {} eligible → reserve → launch Codex → manual claim → implement → finish",
        v["config"]["tags"], v["eligible"]
    );
    if let Some(runs) = v["runs"].as_array() {
        let active = runs.iter().filter(|run| run["finished_at"].is_null());
        let recent = runs
            .iter()
            .filter(|run| !run["finished_at"].is_null())
            .take(history_limit);
        println!("Active agents ({})", v["active"]);
        let mut printed_history = false;
        for run in active.chain(recent) {
            if !run["finished_at"].is_null() && !printed_history {
                println!("Recent attempts (history; these do not use slots)");
                printed_history = true;
            }
            let end = run["finished_at"].as_i64().unwrap_or_else(now);
            let seconds = (end - run["started_at"].as_i64().unwrap_or(end)).max(0) / 1000;
            let claim = run["reservation_expires"]
                .as_i64()
                .filter(|_| run["finished_at"].is_null())
                .map(|t| format!(" · claim in {}s", ((t - now()).max(0) + 999) / 1000))
                .unwrap_or_default();
            println!(
                "{} #{} · {} · {}m{:02}s{} · Codex {} · {}",
                run["project_name"].as_str().unwrap_or(""),
                run["number"],
                run["state"].as_str().unwrap_or(""),
                seconds / 60,
                seconds % 60,
                claim,
                run["session_id"]
                    .as_str()
                    .unwrap_or(if run["finished_at"].is_null() {
                        "launching"
                    } else {
                        "not launched"
                    }),
                run["last_event"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| run["summary"].as_str().unwrap_or(""))
            );
        }
    }
}
pub fn serve_instance(
    settings: Settings,
    id: Option<&str>,
    project: Project,
    json_output: bool,
) -> Result<()> {
    serve_instance_with_history(settings, id, project, json_output, 3)
}
pub fn serve_instance_with_history(
    settings: Settings,
    id: Option<&str>,
    project: Project,
    json_output: bool,
    history_limit: usize,
) -> Result<()> {
    use std::io::IsTerminal;
    // Linux current_exe() gains " (deleted)" after an atomic replacement.
    // Retain the installed path while it still names the running executable.
    let reload_executable = std::env::current_exe()?.canonicalize()?;
    let path = super::database_path()?;
    let machine = identity::machine()?;
    let mut store = retry_database_busy(|| Store::open(&path))?;
    // Ensure the caller's project exists before registration/selection.
    retry_database_busy(|| {
        store.execute(&super::Request {
            version: 1,
            project: project.clone(),
            project_override: None,
            actor: None,
            operation: super::Operation::Projects {
                include_hidden: true,
            },
            request_id: None,
        })
    })?;
    let id = retry_database_busy(|| store.register_worker(id, &settings, &machine))?;
    crate::fleet::record_local_worker(&id, Some(&settings), "running")?;
    struct Registration {
        path: PathBuf,
        id: String,
    }
    impl Drop for Registration {
        fn drop(&mut self) {
            if let Ok(store) = retry_database_busy(|| Store::open(&self.path)) {
                let _ = retry_database_busy(|| store.unregister_worker(&self.id));
            }
        }
    }
    let _registration = Registration {
        path: path.clone(),
        id: id.clone(),
    };
    let worker = Worker::start_for(path, Some(id.clone()))?;
    install_signals_for_upgrade(worker.stop.clone(), Some(worker.reload.clone()))?;
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal() && !json_output;
    if tty {
        use crate::worker_tui::{backend::Client, runtime};
        let exit = runtime::run(
            runtime::Options {
                client: Client {
                    binary: reload_executable.clone(),
                    host: None,
                    directory: Some(std::env::current_dir()?),
                    timeout: Duration::from_secs(10),
                },
                id: Some(id.clone()),
                history: history_limit > 0,
                owned_worker: true,
            },
            worker.stop.clone(),
        )
        .map_err(|e| Error::new("worker_error", e.to_string()))?;
        if exit == runtime::Exit::Quit {
            worker.reload.store(false, Ordering::Relaxed);
        }
    }
    let mut last = String::new();
    let mut heartbeat = Instant::now() - Duration::from_secs(30);
    while !tty && !worker.stop.load(Ordering::Relaxed) {
        let value = store.execute(&super::Request {
            version: 1,
            project: project.clone(),
            project_override: None,
            actor: None,
            operation: super::Operation::Workers {
                worker_id: Some(id.clone()),
            },
            request_id: None,
        });
        let mut value = match value {
            Ok(value) => value,
            Err(error) if error.code == "database_busy" => {
                eprintln!(
                    "Worker status temporarily unavailable: {error}; retrying without stopping sessions"
                );
                thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(error) => return Err(error),
        };
        value["store"] = json!({"host":identity::host(), "database":super::database_path()?});
        value["upgrading"] =
            json!(worker.upgrading.load(Ordering::Relaxed) || value["upgrading"] == true);
        let signature = serde_json::to_string(&value)?;
        if signature != last || heartbeat.elapsed() > Duration::from_secs(15) {
            if json_output {
                println!("{value}");
            } else {
                print_status_with_history(&value, false, history_limit);
                println!("Ctrl+C stops this worker's Codex sessions.");
            }
            std::io::stdout().flush()?;
            last = signature;
            heartbeat = Instant::now();
        }
        for _ in 0..5 {
            if worker.stop.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    let reload = worker.reload.load(Ordering::Relaxed) && !store.worker_shutdown_requested(&id)?;
    drop(worker);
    if !reload && !store.worker_shutdown_requested(&id)? {
        crate::fleet::record_local_worker(&id, None, "stop")?;
    }
    retry_database_busy(|| store.unregister_worker(&id))?;
    if reload {
        drop(_registration);
        let mut command = Command::new(reload_executable);
        command.args([
            "worker",
            "--id",
            &id,
            "--history",
            &history_limit.to_string(),
        ]);
        if json_output {
            command.arg("--json");
        }
        return Err(command.exec().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn artifact_tasks_replace_implementation_and_delivery_prompts() {
        for kind in ["plan", "research"] {
            for base in [
                DEFAULT_PROMPT,
                "/goal",
                "/goal Implement and deploy everything",
            ] {
                let config = ProjectConfig {
                    prompt: base.into(),
                    prs_enabled: true,
                    worktree_enabled: true,
                    ..Default::default()
                };
                let mut task = issue();
                task["labels"] = json!([format!("task:{kind}"), "ready"]);
                task["plan"] = json!({"path":"/tmp/task-notes.md"});
                let (text, goal, objective) = preview(&config, &project(), task);
                assert!(text.contains(&format!("Claim and {kind}")), "{text}");
                assert!(text.contains("hey-boss artifact create"), "{text}");
                assert!(text.contains("--issue 7"));
                assert!(text.contains("mindmap"));
                assert!(text.contains("draft follow-up issues"));
                assert!(text.contains("Plan document: /tmp/task-notes.md"));
                assert!(!text.contains("Commit your changes"));
                assert!(!text.contains("pull request"));
                assert!(!text.contains("dedicated Git worktree"));
                assert!(!text.contains("Implement and deploy everything"));
                assert_eq!(goal, base.starts_with("/goal"));
                assert!(objective.starts_with(&format!("Claim and {kind}")));
            }
        }
    }
    #[test]
    fn ordinary_labels_do_not_change_task_behavior() {
        let mut task = issue();
        task["labels"] = json!(["plan", "research", "task:unknown"]);
        assert_eq!(
            preview(&ProjectConfig::default(), &project(), task).0,
            preview(&ProjectConfig::default(), &project(), issue()).0
        );
    }
    #[test]
    fn pr_handoff_instructions_survive_custom_delivery_prompts() {
        for custom in [None, Some("Publish custom PR {{number}}.".into())] {
            let config = ProjectConfig {
                prs_enabled: true,
                prompt_overrides: PromptOverrides {
                    prs: custom,
                    ..Default::default()
                },
                ..Default::default()
            };
            let (text, _, _) = preview(&config, &project(), issue());
            assert!(text.contains("Keep the issue open until the actual fix PR is merged"));
            assert!(text.contains("hey-boss issue assign-to-boss 7"));
            assert!(text.contains("CI passing and a ready-for-review handoff are not a merge"));
            assert!(text.contains("supporting evidence PRs"));
        }
        let (text, _, _) = preview(&ProjectConfig::default(), &project(), issue());
        assert!(!text.contains("assign-to-boss"));
    }
    fn project() -> Project {
        Project {
            id: "named:a'b $(touch nope)".into(),
            name: "Prompt test".into(),
        }
    }
    fn with_workflow(base: &str) -> String {
        format!(
            "{base}\n\nWork in the project's existing checkout.\n\nCommit your changes. If a Git remote is configured, push to main."
        )
    }
    fn issue() -> Value {
        json!({"number":7,"title":"Literal {{body}}","body":"Literal {{title}}"})
    }
    #[test]
    fn issue_commands_use_the_worker_project_and_expand_once() {
        let c = ProjectConfig {
            prompt: "{{issue_command}} | {{title}} | {{body}} | {{unknown}}".into(),
            ..ProjectConfig::default()
        };
        let (text, _, _) = preview(&c, &project(), issue());
        assert_eq!(
            text,
            with_workflow(
                "hey-boss issue view 7 | Literal {{body}} | Literal {{title}} | {{unknown}}"
            )
        );
    }
    #[test]
    fn create_commands_target_projects_quote_arguments_and_expand_once() {
        let config = ProjectConfig {
            prompt: "/goal If safe-bash fails, report with `{{create_issue_command poe-code}}`. {{create_issue_command}} | {{ create_issue_command  Team's project $(touch nope); x }} | {{unknown}} | {{title}}".into(),
            ..ProjectConfig::default()
        };
        let (text, goal, objective) = preview(&config, &project(), issue());
        assert!(goal);
        assert_eq!(objective, text);
        assert_eq!(
            text,
            with_workflow(
                "If safe-bash fails, report with `hey-boss issue create --project 'poe-code' --title '<title>' --body '<markdown>'`. hey-boss issue create --title '<title>' --body '<markdown>' | hey-boss issue create --project 'Team'\\''s project $(touch nope); x' --title '<title>' --body '<markdown>' | {{unknown}} | Literal {{body}}"
            )
        );
    }
    #[test]
    fn slash_goal_only_is_not_an_empty_turn() {
        for prompt in ["/goal", "/goal Implement issue {{number}}"] {
            let c = ProjectConfig {
                prompt: prompt.into(),
                use_goal: false,
                ..ProjectConfig::default()
            };
            let (text, goal, objective) = preview(&c, &project(), issue());
            assert!(goal);
            assert!(!text.trim().is_empty());
            assert!(!objective.trim().is_empty());
            if prompt != "/goal" {
                assert_eq!(text, with_workflow("Implement issue 7"));
                assert_eq!(objective, text);
            }
        }
    }
    #[test]
    fn slash_goal_preserves_multiline_instructions_and_requires_command_boundary() {
        for source in [
            "/goal First sentence.\nSecond sentence.",
            " \t/goal\tFirst sentence.\nSecond sentence.",
            "/goal\r\nFirst sentence.\nSecond sentence.",
        ] {
            let config = ProjectConfig {
                prompt: source.into(),
                ..Default::default()
            };
            let (text, goal, objective) = preview(&config, &project(), issue());
            assert_eq!(text, with_workflow("First sentence.\nSecond sentence."));
            assert_eq!(objective, text);
            assert!(goal);
        }
        for source in [
            "/goals Implement it",
            "Implement /goal later",
            "Assign and implement {{issue_command}}.",
        ] {
            let config = ProjectConfig {
                prompt: source.into(),
                use_goal: true,
                ..Default::default()
            };
            let (_, goal, _) = preview(&config, &project(), issue());
            assert!(!goal, "Only a leading /goal command enables a goal");
        }
    }
    #[test]
    fn preview_placeholder_does_not_rewrite_literal_commands() {
        let config = ProjectConfig {
            prompt: "{{issue_command}}. Keep issue view 123 literal.".into(),
            ..Default::default()
        };
        let (text, _, _) = preview(
            &config,
            &project(),
            json!({"number":"<number>","title":"t","body":"b"}),
        );
        assert_eq!(
            text,
            with_workflow("hey-boss issue view <number>. Keep issue view 123 literal.")
        );
    }
    #[test]
    fn stale_pid_identity_cannot_signal_a_reused_process() {
        let pid = std::process::id();
        assert!(!alive(pid, "an unrelated process start"));
        stop_group(pid, "an unrelated process start").unwrap();
        assert!(crate::agents::process_identity(pid).is_some());
    }
}
