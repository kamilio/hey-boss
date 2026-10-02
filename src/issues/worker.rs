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

#[path = "worker_prompts.rs"]
mod prompts;
#[path = "worker_providers.rs"]
mod providers;
pub use prompts::*;
pub const DEFAULT_PROMPT: &str = include_str!("prompts/worker.md").trim_ascii_end();
/// Stored as ordinary labels so task intent uses the existing durable fleet wire format.
pub(crate) fn artifact_task(issue: &Value) -> Option<&'static str> {
    let labels = issue["labels"].as_array()?;
    // Old Research tasks remain artifact-only work and now use the Plan prompt.
    if labels
        .iter()
        .any(|label| label == "task:plan" || label == "task:research")
    {
        Some("plan")
    } else {
        None
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
    pub provider: crate::agent_runtime::Provider,
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
            provider: Default::default(),
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
    validate_settings_structure(c)?;
    if !c.directory.is_empty() && !Path::new(&c.directory).is_dir() {
        return Err(Error::invalid(
            "Choose an existing absolute checkout directory",
        ));
    }
    for (project, path) in &c.directories {
        if !Path::new(path).is_dir() {
            return Err(Error::invalid(
                "Choose an existing absolute checkout directory",
            ));
        }
        if !checkout_matches_project(Path::new(path), project)? {
            return Err(Error::invalid(
                "A worker checkout belongs to a different project",
            ));
        }
    }
    if c.enabled {
        c.provider.binary()?;
    }
    Ok(())
}

/// Validate saved data without inspecting checkouts or installed executables.
pub(crate) fn validate_settings_structure(c: &Settings) -> Result<()> {
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
    if !c.directory.is_empty() && !Path::new(&c.directory).is_absolute() {
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
        if !Path::new(path).is_absolute() {
            return Err(Error::invalid(
                "Choose an existing absolute checkout directory",
            ));
        }
    }
    Ok(())
}

fn checkout_matches_project(path: &Path, project: &str) -> Result<bool> {
    let machine = identity::machine()?;
    let actual = identity::project(path, &machine)?;
    // Adding an origin does not invalidate the saved identity of this same
    // local checkout. Its existing issues still belong to that local project.
    let local = format!("local:{machine}:{}", path.canonicalize()?.display());
    Ok(project.starts_with("named:") || actual.id == project || local == project)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    pub provider: crate::agent_runtime::Provider,
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
            provider: Default::default(),
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
    #[serde(default)]
    pub session_ref: Option<crate::agent_runtime::SessionRef>,
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
    if !checkout_matches_project(path, &p.id)? {
        return Err(Error::invalid(
            "The worker directory belongs to a different project. Choose this repository's checkout or worktree.",
        ));
    }
    if c.enabled {
        c.provider.binary()?;
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

/// Cancellable startup, before a scheduler or any agent has been launched.
pub struct Startup {
    stop: Arc<AtomicBool>,
    reload: Arc<AtomicBool>,
}
impl Startup {
    pub fn new() -> Result<Self> {
        let startup = Self {
            stop: Arc::new(AtomicBool::new(false)),
            reload: Arc::new(AtomicBool::new(false)),
        };
        install_signals_for_upgrade(startup.stop.clone(), Some(startup.reload.clone()))?;
        Ok(startup)
    }

    // Callers must reconcile uncertain writes or supply a stable request ID.
    fn retry<T>(&self, mut operation: impl FnMut() -> Result<T>) -> Result<T> {
        let mut delay = Duration::from_secs(1);
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Err(Error::new(
                    "cancelled",
                    "Worker stopped while waiting for connection",
                ));
            }
            match operation() {
                Err(error) if startup_unavailable(&error) => {
                    eprintln!(
                        "Worker idle: {error}; waiting for connection, retrying in {}s. Ctrl+C stops this worker.",
                        delay.as_secs()
                    );
                    let deadline = Instant::now() + delay;
                    while !self.stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(100));
                    }
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
                result => return result,
            }
        }
    }

    pub fn open(&self, path: &Path) -> Result<Store> {
        self.retry(|| Store::open(path))
    }

    pub fn execute(
        &self,
        store: &mut Store,
        path: &Path,
        request: &super::Request,
    ) -> Result<Value> {
        let mut request = request.clone();
        if request.operation.writes() && request.request_id.is_none() {
            request.request_id = Some(format!("worker-startup-{}", random_id()?));
        }
        let mut reconnect = false;
        self.retry(|| {
            if reconnect {
                *store = Store::open(path)?;
            }
            let result = store.execute(&request);
            reconnect = result.as_ref().is_err_and(startup_unavailable);
            result
        })
    }

    fn register(
        &self,
        store: &mut Store,
        path: &Path,
        id: &str,
        settings: &Settings,
        machine: &str,
    ) -> Result<()> {
        let mut reconcile = false;
        self.retry(|| {
            if reconcile {
                *store = Store::open(path)?;
                if store.worker_registered_here(id, settings, machine)? {
                    return Ok(());
                }
            }
            let result = store
                .register_worker(Some(id), settings, machine)
                .map(|_| ());
            reconcile = result.as_ref().is_err_and(startup_unavailable);
            result
        })
    }
}

fn startup_unavailable(error: &Error) -> bool {
    error.code == "database_busy"
        || super::worker_infrastructure::database_unavailable(&error.message)
}

/// Bound local contention retries so an outage cannot prevent worker shutdown.
/// The scheduler reconciles saved results later with its own backoff.
pub fn retry_database_busy<T>(mut operation: impl FnMut() -> Result<T>) -> Result<T> {
    for attempt in 0..3 {
        match operation() {
            Err(error) if error.code == "database_busy" && attempt < 2 => {
                crate::worker_tui::diagnostics::report(format_args!(
                    "Worker database temporarily busy: {error}; waiting for the writer"
                ));
                thread::sleep(Duration::from_millis(200 << attempt));
            }
            result => return result,
        }
    }
    unreachable!()
}

fn reconnect_store(store: &mut Store, path: &Path, error: &Error) {
    if super::worker_infrastructure::database_unavailable(&error.message) {
        match Store::open(path) {
            Ok(fresh) => *store = fresh,
            Err(error) => {
                crate::worker_tui::diagnostics::report(format_args!("Database reconnect: {error}"))
            }
        }
    }
}

impl Worker {
    pub fn start(path: PathBuf) -> Result<Self> {
        Self::start_for(path, None)
    }
    pub fn start_for(path: PathBuf, worker_id: Option<String>) -> Result<Self> {
        Self::start_for_control(
            path,
            worker_id,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
    }
    fn start_for_control(
        path: PathBuf,
        worker_id: Option<String>,
        stop: Arc<AtomicBool>,
        reload: Arc<AtomicBool>,
    ) -> Result<Self> {
        let machine = identity::machine()?;
        let mut store = retry_database_busy(|| Store::open(&path))?;
        // Never free a slot until the old owned process has actually stopped.
        retry_database_busy(|| recover(&mut store, &machine))?;
        retry_database_busy(|| store.recover_chiefs(&machine, worker_id.as_deref()))?;
        let upgrading = Arc::new(AtomicBool::new(false));
        let executable = std::env::current_exe()?;
        let original_executable = executable_identity(&executable);
        let upgrade_requested = upgrading.clone();
        let reload_ready = reload.clone();
        let stopped = stop.clone();
        let handle = thread::spawn(move || {
            let drain_marker = path.with_added_extension("drain-for-update");
            let mut update_pending = false;
            let mut handles: Vec<(String, thread::JoinHandle<()>)> = vec![];
            let mut chiefs: Vec<super::chief::Task> = vec![];
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
                        abandoned.push((id, Instant::now(), Duration::from_secs(1)));
                    } else {
                        active.push((id, handle));
                    }
                }
                handles = active;
                let mut chief_finished = false;
                chiefs.retain_mut(|task| match task.poll(&store) {
                    Ok(finished) => {
                        chief_finished |= finished;
                        !finished
                    }
                    Err(error) => {
                        reconnect_store(&mut store, &path, &error);
                        crate::worker_tui::diagnostics::report(format_args!(
                            "Chief result: {error}"
                        ));
                        true
                    }
                });
                abandoned.retain_mut(|(id, next, delay)| {
                    if Instant::now() < *next {
                        return true;
                    }
                    if let Err(e) = finalize_abandoned(&mut store, &machine, id) {
                        reconnect_store(&mut store, &path, &e);
                        crate::worker_tui::diagnostics::report(format_args!(
                            "Worker recovery: {e}"
                        ));
                        *next = Instant::now() + *delay;
                        *delay = (*delay * 2).min(Duration::from_secs(30));
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
                        reconnect_store(&mut store, &path, &e);
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
                if chief_finished || last_chief.elapsed() >= Duration::from_secs(5) {
                    loop {
                        match store.reserve_chief(&machine, worker_id.as_deref()) {
                            Ok(Some(job)) => {
                                chiefs.push(super::chief::Task::start(
                                    path.clone(),
                                    job,
                                    stopped.clone(),
                                ));
                            }
                            Ok(None) => break,
                            Err(error) => {
                                crate::worker_tui::diagnostics::report(format_args!(
                                    "Chief scheduler: {error}"
                                ));
                                break;
                            }
                        }
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
                    Err(e) => {
                        reconnect_store(&mut store, &path, &e);
                        crate::worker_tui::diagnostics::report(format_args!(
                            "Worker scheduler: {e}"
                        ));
                    }
                }
                thread::park_timeout(Duration::from_secs(1));
            }
            for (id, handle) in handles {
                let _ = handle.join();
                // Finalization already retries and retains its durable result
                // on contention. Nesting another retry loop here multiplies
                // the shutdown delay while the same writer remains locked.
                if let Err(e) = finalize_abandoned(&mut store, &machine, &id) {
                    crate::worker_tui::diagnostics::report(format_args!("Worker recovery: {e}"));
                }
            }
            for mut task in chiefs {
                task.join();
                if let Err(error) = retry_database_busy(|| task.poll(&store)) {
                    crate::worker_tui::diagnostics::report(format_args!("Chief result: {error}"));
                }
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
pub(crate) fn stop_group(pid: u32, start: &str) -> Result<()> {
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
        if store.worker_attempt_held(&job)? {
            continue;
        }
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
        finish_recovered(
            store,
            &job,
            "failed",
            "Worker exited before saving its result. Review the saved session before retrying.",
        )?;
    }
    Ok(())
}
pub(crate) fn recover(store: &mut Store, machine: &str) -> Result<()> {
    store.prune_workers(machine)?;
    if let Some(path) = store.worker_database_path() {
        for result in super::worker_results::pending(&path)? {
            if result.job.machine == machine
                && !alive(result.job.owner_pid, &result.job.owner_start)
            {
                finish_job(&path, store, &result.job, &result.state, &result.summary)?;
            }
        }
    }
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
        finish_recovered(
            store,
            &job,
            "failed",
            "The worker service stopped unexpectedly. The saved Codex session and issue history are retained for automatic retry.",
        )?;
    }
    store.release_stale_claims(machine, now())?;
    store.worker_release_automatic_holds(machine)?;
    Ok(())
}

fn check_job(store: &Store, job: &Job, stop: &AtomicBool) -> Result<()> {
    if store.worker_model_expired(job)? {
        return Err(Error::new(
            "startup_timeout",
            "Agent did not begin model work within 15 minutes. The session was stopped and the issue can retry.",
        ));
    }
    if store.worker_claim_expired(job)? {
        return Err(Error::new(
            "claim_timeout",
            "Agent did not claim the issue before its reservation expired. The session was stopped; review or retry the issue.",
        ));
    }
    if stop.load(Ordering::Relaxed) || store.worker_cancelled(job)? {
        return Err(Error::new(
            "cancelled",
            "Worker stopped or issue ownership changed",
        ));
    }
    Ok(())
}

struct Codex {
    process: crate::agent_process::Process,
    protection: (PathBuf, Job),
    pending: VecDeque<Value>,
    next_id: u64,
    session: Option<String>,
    native_goal: bool,
    approvals: super::worker_approvals::Approvals,
    approval_items: VecDeque<(String, Value)>,
    infrastructure_outage: Option<String>,
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
        if yolo(&job.issue) {
            crate::codex_permissions::apply_yolo(&mut command);
        } else {
            crate::codex_permissions::apply(&mut command);
        }
        command
            .args(["app-server", "--listen", "stdio://"])
            .current_dir(&job.config.cwd)
            .env("HEY_BOSS_ISSUE_DB", path)
            .env("HEY_BOSS_ISSUE_PROJECT", &job.project.id)
            .env(
                "HEY_BOSS_ISSUE_NUMBER",
                job.issue["number"].as_i64().unwrap_or_default().to_string(),
            )
            .env(
                "PATH",
                std::env::join_paths(paths)
                    .map_err(|e| Error::new("worker_error", e.to_string()))?,
            )
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_AGENT_ID")
            .env_remove("HEY_BOSS_WORKER_RUN")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CLAUDE_SESSION_ID")
            .env_remove("PI_SESSION_ID")
            .env_remove("CLAUDECODE");
        let process = crate::agent_process::Process::spawn(&mut command)?;
        Ok(Self {
            process,
            protection: (path.to_path_buf(), job.clone()),
            pending: VecDeque::new(),
            next_id: 0,
            session: None,
            native_goal: false,
            approvals: Default::default(),
            approval_items: VecDeque::new(),
            infrastructure_outage: None,
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
            check_job(store, job, stop)?;
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
        check_job(store, job, stop)?;
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
            check_job(store, job, stop)?;
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
        let (path, job) = self.protection.clone();
        let protected = move || {
            Store::open(&path)
                .and_then(|s| s.worker_attempt_held(&job))
                .unwrap_or(true)
        };
        if protected() {
            self.process.preserve_until(protected);
        } else {
            let _ = self.process.stop();
        }
        // The server is stopped before its remaining Inbox questions are cancelled.
        self.approvals = Default::default();
    }
}

fn execute_job(path: &Path, mut job: Job, stop: Arc<AtomicBool>) {
    let mut store = match retry_database_busy(|| Store::open(path)) {
        Ok(s) => s,
        Err(e) => {
            crate::worker_tui::diagnostics::report(format_args!("Worker {}: {e}", job.id));
            if let Err(error) =
                super::worker_results::save(path, &job, "startup_failed", &e.message)
            {
                crate::worker_tui::diagnostics::report(format_args!(
                    "Saving startup failure: {error}"
                ));
            }
            return;
        }
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::environment::check_worker(Path::new(&job.config.cwd))
            .map_err(|e| Error::new("infrastructure_blocked", e.to_string()))?;
        if job.config.provider == crate::agent_runtime::Provider::Codex {
            run_codex(path, &mut store, &mut job, &stop)
        } else {
            providers::run(path, &mut store, &mut job, &stop)
        }
    }));
    let (state, summary) = match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => (
            match error.code.as_str() {
                "cancelled" => "cancelled",
                "claim_timeout" => "claim_timeout",
                "blocked" => "blocked",
                "startup_failed" => "startup_failed",
                "infrastructure_blocked" => "infrastructure_blocked",
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
    if let Err(e) = finish_job(path, &mut store, &job, &state, &summary) {
        crate::worker_tui::diagnostics::report(format_args!(
            "Worker {} could not finalize: {e}",
            job.id
        ))
    }
}

pub(super) fn finish_job(
    path: &Path,
    store: &mut Store,
    job: &Job,
    state: &str,
    summary: &str,
) -> Result<()> {
    let saved = super::worker_results::save(path, job, state, summary)?;
    let mut reconnect = false;
    for attempt in 0..2 {
        let result = retry_database_busy(|| {
            if reconnect {
                *store = Store::open(path)?;
                reconnect = false;
            }
            if store.worker_attempt_held(job)? {
                return store.worker_finish(&saved.job, &saved.state, &saved.summary);
            }
            let child = store.worker_process_identity(&job.id)?;
            if let Some((Some(pid), Some(start))) = child {
                stop_group(pid, &start)?;
                if alive(pid, &start) {
                    return Err(Error::new(
                        "worker_error",
                        "Agent process is still alive; its result is saved and finalization will retry after it stops",
                    ));
                }
            }
            // worker_finish begins an atomic transaction and reads finished_at
            // before writing anything. On a fresh connection this reconciles a
            // lost COMMIT reply: committed results are returned without replay,
            // and disconnected, rolled-back transactions can safely finish.
            store.worker_finish(&saved.job, &saved.state, &saved.summary)
        });
        match result {
            Err(error)
                if attempt == 0
                    && super::worker_infrastructure::database_unavailable(&error.message) =>
            {
                crate::worker_tui::diagnostics::report(format_args!(
                    "Worker {} finalization awaiting database recovery; result retained, next attempt reconciles the saved run: {error}",
                    job.id
                ));
                reconnect = true;
            }
            Ok(()) => return super::worker_results::remove(path, &job.id),
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

fn finish_recovered(store: &mut Store, job: &Job, state: &str, summary: &str) -> Result<()> {
    if let Some(path) = store.worker_database_path() {
        finish_job(&path, store, job, state, summary)
    } else {
        store.worker_finish(job, state, summary)
    }
}
// Use portable Git branch / directory characters and bound component lengths.
// Issue numbers distinguish tasks whose shortened titles happen to match.
fn worktree_slug(text: &str, limit: usize, fallback: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() && slug.len() < limit {
                slug.push('-');
            }
            if slug.len() == limit {
                break;
            }
            slug.push((byte as char).to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        fallback.into()
    } else {
        slug.into()
    }
}
fn worktree_name(job: &Job) -> String {
    format!(
        "{}-{}-{}",
        worktree_slug(&job.project.name, 60, "project"),
        worktree_slug(job.issue["title"].as_str().unwrap_or(""), 15, "issue"),
        worktree_slug(&number_text(job), 20, "number"),
    )
}
fn worktree_path(job: &Job) -> String {
    Path::new(&job.config.cwd)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new(".."))
        .join(worktree_name(job))
        .to_string_lossy()
        .into_owned()
}
fn template(text: &str, job: &Job) -> String {
    template_with(text, job, &[])
}
fn template_with(text: &str, job: &Job, values: &[(&str, &str)]) -> String {
    prompts::render(text, |key| {
        if let Some((_, value)) = values.iter().find(|(name, _)| *name == key) {
            return Some((*value).into());
        }
        Some(match key {
            "issue_command" => format!("hey-boss issue view {}", number_text(job)),
            "create_issue_command" => create_issue_command(None),
            "project" => job.project.id.clone(),
            "project_arg" => format!("'{}'", job.project.id.replace('\'', "'\\''")),
            "number" => number_text(job),
            "title" => job.issue["title"].as_str().unwrap_or("").into(),
            "body" => job.issue["body"].as_str().unwrap_or("").into(),
            "worktree_name" => worktree_name(job),
            "worktree_path" => worktree_path(job),
            "plan_path" => job.issue["plan"]["path"].as_str().unwrap_or("").into(),
            "subtask_position" => value_text(&job.issue["subtask_context"]["position"]),
            "subtask_total" => value_text(&job.issue["subtask_context"]["total"]),
            "parent_number" => value_text(&job.issue["subtask_context"]["parent"]["number"]),
            "parent_title" => job.issue["subtask_context"]["parent"]["title"]
                .as_str()
                .unwrap_or("")
                .into(),
            "parent_state" => job.issue["subtask_context"]["parent"]["state"]
                .as_str()
                .unwrap_or("")
                .into(),
            "previous_subtask" => task_summary(&job.issue["subtask_context"]["previous"]),
            "next_subtask" => task_summary(&job.issue["subtask_context"]["next"]),
            "dependencies" => job.issue["dependency_context"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|dependency| {
                    format!(
                        "{} PRs: {}",
                        task_summary(dependency),
                        dependency["pull_requests"]
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            "commit_instruction" => String::new(),
            _ => match key.split_once(char::is_whitespace) {
                Some(("create_issue_command", project)) if !project.trim().is_empty() => {
                    create_issue_command(Some(project.trim()))
                }
                _ => return None,
            },
        })
    })
}
fn value_text(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    })
}
fn task_summary(task: &Value) -> String {
    if task.is_object() {
        format!(
            "#{} {} [{}]",
            value_text(&task["number"]),
            task["title"].as_str().unwrap_or(""),
            task["state"].as_str().unwrap_or("")
        )
    } else {
        "none".into()
    }
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
        session_ref: None,
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
            model: None,
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
    let (task, goal) = task_prompt(job, config);
    let artifact = artifact_task(&job.issue).is_some();
    let dependencies = job.issue["dependency_context"].as_array();
    let has_dependency_prs = dependencies.is_some_and(|deps| {
        deps.iter().any(|d| {
            d["pull_requests"]
                .as_array()
                .is_some_and(|prs| !prs.is_empty())
        })
    });
    let section = |key: &str, enabled: bool| {
        if enabled {
            template(config.prompt_overrides.get(key), job)
        } else {
            String::new()
        }
    };
    let values = [
        ("task", task),
        (
            "workspace",
            section(
                if config.worktree_enabled {
                    "worktree"
                } else {
                    "checkout"
                },
                !artifact,
            ),
        ),
        (
            "delivery",
            section(if config.prs_enabled { "prs" } else { "main" }, !artifact),
        ),
        // Empty legacy layout slots: these details come from fetching the task.
        ("plan_document", String::new()),
        ("subtask", String::new()),
        ("dependencies", String::new()),
        ("github", String::new()),
        (
            "handoff",
            section(
                "handoff",
                !artifact
                    && (config.prs_enabled || has_dependency_prs)
                    && (job.issue["dependency_ready_state"] != "closed" || has_dependency_prs),
            ),
        ),
        ("resume", section("resume", job.resume_session.is_some())),
    ];
    let values: Vec<_> = values
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    // Remove only empty standalone slots and their preceding separator. Keep
    // authored spacing and never expand variables inside substituted content.
    let mut lines = Vec::new();
    for line in config.prompt_overrides.get("layout").lines() {
        let empty_slot = line
            .trim()
            .strip_prefix("{{")
            .and_then(|s| s.strip_suffix("}}"))
            .is_some_and(|key| {
                values
                    .iter()
                    .any(|(name, value)| *name == key.trim() && value.is_empty())
            });
        if empty_slot {
            if lines
                .last()
                .is_some_and(|line: &&str| line.trim().is_empty())
            {
                lines.pop();
            }
        } else {
            lines.push(line);
        }
    }
    let instructions = template_with(&lines.join("\n"), job, &values)
        .trim()
        .to_owned();
    let objective = instructions.chars().take(4000).collect();
    (instructions, goal, objective)
}
fn task_prompt(job: &Job, config: &ProjectConfig) -> (String, bool) {
    let rendered = template(&base_prompt(&config.prompt), job);
    let goal = strip_goal(&rendered).is_some();
    let (rendered, fallback) = if artifact_task(&job.issue).is_some() {
        (
            template(config.prompt_overrides.get("plan"), job),
            DEFAULT_PLAN_PROMPT,
        )
    } else {
        (rendered, DEFAULT_PROMPT)
    };
    let after_goal = strip_goal(&rendered);
    let goal = goal || after_goal.is_some();
    let instructions = match after_goal {
        Some(rest) if rest.trim().is_empty() => template(fallback, job),
        Some(rest) => rest.trim_start().to_owned(),
        None => rendered,
    };
    (instructions.trim_end().to_owned(), goal)
}
fn strip_goal(text: &str) -> Option<&str> {
    text.trim_start()
        .strip_prefix("/goal")
        .filter(|rest| rest.chars().next().is_none_or(char::is_whitespace))
}
fn turn_params(session: &str, text: &str) -> Value {
    json!({"threadId":session,"input":[{"type":"text","text":text}],"outputSchema":{"type":"object","properties":{"status":{"type":"string","enum":["completed","blocked"]},"summary":{"type":"string"}},"required":["status","summary"],"additionalProperties":false}})
}
fn yolo(issue: &Value) -> bool {
    issue["labels"]
        .as_array()
        .is_some_and(|labels| labels.iter().any(|label| label.as_str() == Some("yolo")))
}

fn run_codex(
    path: &Path,
    store: &mut Store,
    job: &mut Job,
    stop: &AtomicBool,
) -> Result<(String, String)> {
    validate_config(&job.config, &job.project)?;
    check_job(store, job, stop)?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut delay = Duration::from_secs(1);
    let mut c = loop {
        check_job(store, job, stop)?;
        let mut c = Codex::spawn(path, job)?;
        store.worker_process(&job.id, c.process.pid())?;
        store.worker_event(
            &job.id,
            if yolo(&job.issue) {
                "Launching Codex in YOLO mode (no sandbox or approvals)"
            } else {
                "Launching Codex"
            },
            None,
        )?;
        match c.rpc("initialize",json!({"clientInfo":{"name":"hey_boss_worker","title":"Hey Boss issue worker","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),store,job,stop) {
            Ok(_) => break c,
            Err(error) if error.message.contains("failed to initialize sqlite state runtime") => {
                // No model turn has started. Keep the reservation and the saved
                // session, rather than spending the issue's agent retry budget.
                drop(c);
                if Instant::now() >= deadline {
                    return Err(Error::new("startup_failed", error.message));
                }
                store.worker_event(&job.id, &format!("Codex state is temporarily unavailable; retrying startup: {}", error.message), None)?;
                let retry_at = (Instant::now() + delay).min(deadline);
                while Instant::now() < retry_at {
                    check_job(store, job, stop)?;
                    thread::sleep(Duration::from_millis(100));
                }
                delay = (delay * 2).min(Duration::from_secs(8));
            }
            Err(error) => return Err(error),
        }
    };
    let outcome = run_thread(&mut c, store, job, stop);
    // Let the agent recover within its current turn. Only an unfinished result
    // becomes a hold; a successful repair/completion remains successful.
    let outcome = match (outcome, c.infrastructure_outage.as_deref()) {
        (Ok((state, summary)), Some(detail)) if state == "blocked" => Ok((
            super::worker_infrastructure::STATE.into(),
            format!("{summary}\n\n{detail}"),
        )),
        (Err(error), Some(detail)) if matches!(error.code.as_str(), "worker_error" | "blocked") => {
            Err(Error::new(
                super::worker_infrastructure::STATE,
                format!("{}\n\n{detail}", error.message),
            ))
        }
        (outcome, _) => outcome,
    };
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
        if yolo(&job.issue) {
            crate::codex_permissions::yolo_thread(params)
        } else {
            crate::codex_permissions::thread(params)
        },
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
        check_job(store, job, stop)?;
        c.poll_approvals(store, job, stop)?;
        if last_prompt_check.elapsed() >= Duration::from_secs(2) {
            last_prompt_check = Instant::now();
            let config = store.worker_prompt_config(job)?;
            if let Some(instruction) = store.worker_steering(&job.id)?
                && store.worker_steering_result(
                    instruction["request_id"].as_str().unwrap(),
                    "sending",
                    None,
                )?
            {
                let request = instruction["request_id"].as_str().unwrap();
                let text = steering_text(&instruction, job, &config);
                // Persist before sending. A worker crash must never replay an
                // instruction whose delivery cannot be established.
                match c.rpc("turn/steer", json!({"threadId":session,"expectedTurnId":turn,"input":[{"type":"text","text":text}]}), store, job, stop) {
                    Ok(ack) if ack["turnId"] == turn => {
                        store.worker_steering_result(request, "delivered", None)?;
                        if let Some(body) = instruction["issue_body"].as_str() {
                            job.issue["body"] = json!(body);
                            store.worker_prompt(job, &applied_prompt)?;
                        }
                        store.worker_event(&job.id, &format!("Steering delivered: {}", instruction["text"].as_str().unwrap()), None)?;
                    }
                    Err(error) if error.message.starts_with("Codex turn/steer:") => {
                        let ended = c.pending.iter().any(|event| event["method"] == "turn/completed" && event["params"]["turn"]["id"] == turn && event["params"]["turn"]["status"] == "completed");
                        store.worker_steering_result(request, if ended {"queued"} else {"rejected"}, Some(&error.message))?;
                        store.worker_event(&job.id, &format!("Steering {}: {}", if ended {"awaiting the next turn"} else {"rejected"}, error.message), None)?;
                    }
                    result => {
                        let error = result.err().map(|e| e.message).unwrap_or_else(|| "Unexpected steering acknowledgement".into());
                        store.worker_steering_result(request, "uncertain", Some(&error))?;
                        store.worker_event(&job.id, &format!("Steering delivery unconfirmed; not resent: {error}"), None)?;
                    }
                }
            }
            let next_prompt = prompt_with_config(job, &config).0;
            if next_prompt != applied_prompt {
                let steering = prompt_update(&next_prompt, job, &config);
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
                        check_job(store, job, stop)?;
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
        if params["turnId"].as_str().is_some_and(|id| id != turn) {
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
            "error" if params["willRetry"] == false => {
                return Err(Error::new(
                    "worker_error",
                    format!(
                        "Codex reported a terminal error: {}",
                        params["error"]["message"]
                            .as_str()
                            .unwrap_or("Unknown agent error")
                    ),
                ));
            }
            "item/completed" if params["item"]["type"] != "agentMessage" => {
                if let Some(detail) = super::worker_infrastructure::tool_failure(&params["item"]) {
                    store.worker_event(&job.id, "Infrastructure unavailable; preserving continuation state if this attempt cannot finish", None)?;
                    c.infrastructure_outage = Some(detail);
                }
            }
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
            // Only our acknowledged turn/start response changes the active turn.
            // A delayed notification must not replace it with an earlier turn.
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
                if let Some(instruction) = store.worker_steering(&job.id)?
                    && store.worker_steering_result(
                        instruction["request_id"].as_str().unwrap(),
                        "sending",
                        None,
                    )?
                {
                    let request = instruction["request_id"].as_str().unwrap();
                    let result = c.rpc(
                        "turn/start",
                        turn_params(&session, &steering_text(&instruction, job, &config)),
                        store,
                        job,
                        stop,
                    );
                    match result {
                        Ok(result) if result["turn"]["id"].is_string() => {
                            turn = result["turn"]["id"].as_str().unwrap().into();
                            store.worker_steering_result(request, "delivered", None)?;
                            if let Some(body) = instruction["issue_body"].as_str() {
                                job.issue["body"] = json!(body);
                                store.worker_prompt(job, &applied_prompt)?;
                            }
                            store.worker_event(
                                &job.id,
                                &format!(
                                    "Steering delivered in the same session: {}",
                                    instruction["text"].as_str().unwrap()
                                ),
                                None,
                            )?;
                            final_text.clear();
                            continue;
                        }
                        result => {
                            let error = result
                                .err()
                                .map(|e| e.message)
                                .unwrap_or_else(|| "Missing steering turn acknowledgement".into());
                            store.worker_steering_result(request, "uncertain", Some(&error))?;
                            return Err(Error::new("worker_error", error));
                        }
                    }
                }
                let next_prompt = prompt_with_config(job, &config).0;
                if next_prompt != applied_prompt {
                    let result = c.rpc(
                        "turn/start",
                        turn_params(&session, &prompt_update(&next_prompt, job, &config)),
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
                job.config = config;
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
                        Some("complete") => {
                            return Err(Error::new(
                                "worker_error",
                                format!(
                                    "Codex completed its goal without a valid completion report.\n\n{final_text}"
                                ),
                            ));
                        }
                        Some("active") => {
                            let result = c.rpc(
                                "turn/start",
                                turn_params(
                                    &session,
                                    &template(
                                        job.config.prompt_overrides.get("goal_continue"),
                                        job,
                                    ),
                                ),
                                store,
                                job,
                                stop,
                            )?;
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
                        "worker_error",
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

fn steering_text(instruction: &Value, job: &Job, config: &ProjectConfig) -> String {
    if instruction["request_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("github:"))
    {
        return prompt_with_config(job, config).0;
    }
    let key = if instruction["scope"] == "dependency" {
        "dependency_update"
    } else {
        "steering"
    };
    template_with(
        config.prompt_overrides.get(key),
        job,
        &[
            ("scope", instruction["scope"].as_str().unwrap_or("")),
            ("instruction", instruction["text"].as_str().unwrap_or("")),
        ],
    )
}

fn prompt_update(instructions: &str, job: &Job, config: &ProjectConfig) -> String {
    template_with(
        config.prompt_overrides.get("prompt_update"),
        job,
        &[("instructions", instructions)],
    )
}

pub fn print_status(v: &Value, redraw: bool) {
    print_status_with_history(v, redraw, 3);
}
/// Limit output history without treating paused or approval-waiting live work as finished.
/// Store snapshots order finished attempts newest first; keep all unfinished attempts.
pub fn limit_status_history(value: &mut Value, history_limit: usize) {
    let mut finished = 0;
    if let Some(runs) = value["runs"].as_array_mut() {
        runs.retain(|run| {
            if run["finished_at"].is_null() {
                true
            } else {
                finished += 1;
                finished <= history_limit
            }
        });
    }
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
    let projects = v["config"]["projects"]
        .as_array()
        .map(|projects| {
            projects
                .iter()
                .map(|project| {
                    crate::worker_tui::project_name(project.as_str().unwrap_or_default(), v)
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    println!(
        "Projects: {}",
        if projects.is_empty() {
            "All projects"
        } else {
            &projects
        }
    );
    if let Some(directories) = v["config"]["directories"].as_object() {
        for (project, path) in directories {
            println!(
                "Checkout: {} · {}",
                crate::worker_tui::project_name(project, v),
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
            "Queue issues: {} open · {} assigned · {} eligible · {} excluded by tags · {} reserved or held for pickup",
            queue["open"],
            queue["assigned"],
            queue["eligible"],
            queue["tag_filtered"],
            queue["waiting"]
        );
        if queue["waiting"].as_i64().unwrap_or(0) > 0 {
            println!(
                "Pickup holds are active reservations, retry delays or fleet allocation requirements. Unfinished dependencies appear as Blocked in issue lists."
            );
        }
    }
    println!(
        "Pipeline: refresh issue order → scan visible projects → filter tags {} → {} eligible → reserve → launch agent → manual claim → implement → finish",
        v["config"]["tags"], v["eligible"]
    );
    if let Some(chiefs) = v["chiefs"].as_array() {
        for chief in chiefs {
            println!(
                "{} Chief · {} · no issue slots · Codex {} · {}",
                chief["project_name"].as_str().unwrap_or(""),
                chief["state"].as_str().unwrap_or(""),
                chief["session_id"].as_str().unwrap_or("launching"),
                chief["last_event"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| chief["summary"].as_str().unwrap_or(""))
            );
        }
    }
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
            let seconds = end
                .saturating_sub(run["started_at"].as_i64().unwrap_or(end))
                .max(0)
                / 1000;
            let claim = run["reservation_expires"]
                .as_i64()
                .filter(|_| run["finished_at"].is_null())
                .map(|t| format!(" · claim in {}s", ((t - now()).max(0) + 999) / 1000))
                .unwrap_or_default();
            println!(
                "{} #{} · {} · {}{} · {} {} · {}",
                run["project_name"].as_str().unwrap_or(""),
                run["number"],
                run["state"].as_str().unwrap_or(""),
                crate::worker_tui::duration::format_runtime(seconds),
                claim,
                match run["actor_id"].as_str().unwrap_or("").split(':').next() {
                    Some("claude") => "Claude",
                    Some("pi") => "Pi",
                    _ => "Codex",
                },
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
    let result = Startup::new()?.serve_instance(settings, id, project, json_output, history_limit);
    match result {
        Err(error) if error.code == "cancelled" => Ok(()),
        result => result,
    }
}

impl Startup {
    pub fn serve_instance(
        &self,
        settings: Settings,
        id: Option<&str>,
        project: Project,
        json_output: bool,
        history_limit: usize,
    ) -> Result<()> {
        use std::io::IsTerminal;
        validate_settings(&settings)?;
        // Linux current_exe() gains " (deleted)" after an atomic replacement.
        // Retain the installed path while it still names the running executable.
        let reload_executable = std::env::current_exe()?.canonicalize()?;
        let path = super::database_path()?;
        let machine = identity::machine()?;
        let mut store = self.open(&path)?;
        // Ensure the caller's project exists before registration/selection.
        self.execute(
            &mut store,
            &path,
            &super::Request {
                version: 1,
                project: project.clone(),
                project_override: None,
                actor: None,
                operation: super::Operation::Projects {
                    include_hidden: true,
                },
                request_id: None,
            },
        )?;
        // Keep the same ID if registration committed but its acknowledgment was lost.
        let id = id.map(str::to_owned).map(Ok).unwrap_or_else(random_id)?;
        self.register(&mut store, &path, &id, &settings, &machine)?;
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
        crate::fleet::record_local_worker(&id, Some(&settings), "running")?;
        let worker = self.retry(|| {
            Worker::start_for_control(
                path.clone(),
                Some(id.clone()),
                self.stop.clone(),
                self.reload.clone(),
            )
        });
        let worker = match worker {
            Ok(worker) => worker,
            Err(error) => {
                // Cancelling startup must not leave intent that restarts it later.
                let _ = crate::fleet::record_local_worker(&id, None, "stop");
                return Err(error);
            }
        };
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
                    project_tabs: false,
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
                Err(error)
                    if error.code == "database_busy"
                        || super::worker_infrastructure::database_unavailable(&error.message) =>
                {
                    reconnect_store(&mut store, &path, &error);
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
            limit_status_history(&mut value, history_limit);
            let signature = serde_json::to_string(&value)?;
            if signature != last || heartbeat.elapsed() > Duration::from_secs(15) {
                if json_output {
                    println!("{value}");
                } else {
                    print_status_with_history(&value, false, history_limit);
                    println!("Ctrl+C stops this worker's agent sessions.");
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
        let reload =
            worker.reload.load(Ordering::Relaxed) && !store.worker_shutdown_requested(&id)?;
        drop(worker);
        if !reload && !store.worker_shutdown_requested(&id)? {
            crate::fleet::record_local_worker(&id, None, "stop")?;
        }
        if let Err(error) = retry_database_busy(|| store.unregister_worker(&id)) {
            if error.code != "database_busy"
                && !super::worker_infrastructure::database_unavailable(&error.message)
            {
                return Err(error);
            }
            crate::worker_tui::diagnostics::report(format_args!(
                "Worker stopped; database registration cleanup will be reconciled after recovery: {error}"
            ));
        }
        if reload {
            drop(_registration);
            let mut command = Command::new(reload_executable);
            command.args([
                "worker",
                "run",
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_steering_uses_the_normal_task_prompt_without_evidence() {
        let mut job: Job = serde_json::from_value(json!({
            "id":"run", "worker_id":"worker", "project":project(), "issue":issue(),
            "comments":[], "owner_pid":0, "owner_start":"", "machine":"test",
            "config":ProjectConfig::default(), "actor":{ "id":"test", "kind":"agent", "cwd":"/tmp", "machine":"test", "host":"test", "source":"test" }
        }))
        .unwrap();
        let input =
            json!({"request_id":"github:run:new","scope":"session","text":"large GitHub evidence"});
        let text = steering_text(&input, &job, &job.config);
        assert_eq!(text, prompt_with_config(&job, &job.config).0);
        assert!(!text.contains("large GitHub evidence"));
        job.config.prompt_overrides.github = Some("Check {{number}} in {{project}}".into());
        assert_eq!(
            steering_text(&input, &job, &job.config),
            prompt_with_config(&job, &job.config).0
        );
    }

    #[test]
    fn startup_registration_reconciles_lost_commit_acknowledgments() {
        for committed in [false, true] {
            let root = crate::admin::Temporary::new().unwrap();
            let path = root.0.join("issues.db");
            let mut store = Store::open(&path).unwrap();
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            let (connection, transport) =
                crate::database::tests::lose_commit_response(&path, committed);
            store.replace_connection_for_test(connection);
            let startup = Startup {
                stop: Arc::new(AtomicBool::new(false)),
                reload: Arc::new(AtomicBool::new(false)),
            };
            let settings = Settings::default();
            startup
                .register(&mut store, &path, "retry-worker", &settings, "fixture")
                .unwrap();
            transport.join().unwrap();
            let (count, version): (i64, i64) = crate::database::Connection::open(&path)
                .unwrap()
                .query_row("SELECT count(*),version FROM issue_workers", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap();
            assert_eq!((count, version), (1, 1));
            assert!(
                store
                    .worker_registered_here("retry-worker", &settings, "fixture")
                    .unwrap()
            );
            assert_eq!(
                startup
                    .register(&mut store, &path, "retry-worker", &settings, "fixture")
                    .unwrap_err()
                    .code,
                "conflict"
            );
            owner.stop();
        }
    }

    #[test]
    fn startup_metadata_retries_reuse_the_actor_and_request_id() {
        for committed in [false, true] {
            let root = crate::admin::Temporary::new().unwrap();
            let path = root.0.join("issues.db");
            let mut store = Store::open(&path).unwrap();
            let actor = identity::resolve(Some("test:startup"), "fixture", &root.0).unwrap();
            let mut request = super::super::Request {
                version: 1,
                project: Project {
                    id: "named:Startup".into(),
                    name: "Startup".into(),
                },
                project_override: None,
                actor: Some(actor),
                operation: super::super::Operation::Projects {
                    include_hidden: true,
                },
                request_id: None,
            };
            store.execute(&request).unwrap();
            request.operation = super::super::Operation::ConfigureWorker {
                worker_id: None,
                config: Settings::default(),
                if_version: None,
            };
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            let (connection, transport) =
                crate::database::tests::lose_commit_response(&path, committed);
            store.replace_connection_for_test(connection);
            let startup = Startup {
                stop: Arc::new(AtomicBool::new(false)),
                reload: Arc::new(AtomicBool::new(false)),
            };
            let response = startup.execute(&mut store, &path, &request).unwrap();
            transport.join().unwrap();
            assert!(response["worker_id"].is_string());
            assert_eq!(
                crate::database::Connection::open(&path)
                    .unwrap()
                    .query_row("SELECT count(*) FROM issue_workers", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            owner.stop();
        }
    }

    #[test]
    fn startup_never_retries_invalid_settings_or_permission_denials() {
        let startup = Startup {
            stop: Arc::new(AtomicBool::new(false)),
            reload: Arc::new(AtomicBool::new(false)),
        };
        for error in [
            Error::invalid("Choose an existing absolute checkout directory"),
            Error::new(
                "database_error",
                "Database service unavailable: Permission denied",
            ),
        ] {
            let mut attempts = 0;
            let result: Result<()> = startup.retry(|| {
                attempts += 1;
                Err(error.clone())
            });
            assert_eq!(result.unwrap_err().code, error.code);
            assert_eq!(attempts, 1);
        }
    }

    #[test]
    fn local_checkout_identity_survives_adding_a_remote() {
        let root = std::env::temp_dir().join(format!("hb-worker-origin-{}", random_id().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let machine = identity::machine().unwrap();
        let original = identity::project(&root, &machine).unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec![
                "remote",
                "add",
                "origin",
                "https://github.com/example/new-origin.git",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        assert_ne!(identity::project(&root, &machine).unwrap().id, original.id);
        let mut settings = Settings {
            projects: vec![original.id.clone()],
            directories: [(original.id.clone(), root.to_string_lossy().into_owned())].into(),
            ..Default::default()
        };
        assert!(validate_settings(&settings).is_ok());
        let config = ProjectConfig {
            cwd: root.to_string_lossy().into_owned(),
            ..Default::default()
        };
        assert!(validate_config(&config, &original).is_ok());
        for unrelated in [
            format!("local:another-machine:{}", root.display()),
            format!("local:{machine}:/unrelated"),
            "github.com/example/other-repo".into(),
        ] {
            settings.projects = vec![unrelated.clone()];
            settings.directories = [(unrelated.clone(), config.cwd.clone())].into();
            assert!(validate_settings(&settings).is_err());
            assert!(
                validate_config(
                    &config,
                    &Project {
                        id: unrelated,
                        name: "other".into()
                    }
                )
                .is_err()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn database_contention_is_bounded_and_nonbusy_failures_are_not_replayed() {
        for code in ["database_busy", "database_error"] {
            let mut attempts = 0;
            let started = Instant::now();
            let result: Result<()> = retry_database_busy(|| {
                attempts += 1;
                assert!(
                    attempts <= 3,
                    "Contention must not prevent shutdown forever"
                );
                Err(Error::new(code, "synthetic unavailable store"))
            });
            assert!(result.is_err());
            assert_eq!(attempts, if code == "database_busy" { 3 } else { 1 });
            assert!(started.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    fn worktree_variables_are_repeatable_safe_and_unique_per_issue() {
        let config = ProjectConfig {
            cwd: "/workspace/hey-boss".into(),
            prompt: "{{worktree_name}} | {{worktree_path}}".into(),
            ..Default::default()
        };
        let project = Project {
            id: "repo:test".into(),
            name: "Hey Boss".into(),
        };
        let task = json!({"number":79,"title":"Fix: Worktree / Naming!!!","body":""});
        let first = preview(&config, &project, task.clone()).0;
        assert!(
            first.starts_with(
                "hey-boss-fix-worktree-na-79 | /workspace/hey-boss-fix-worktree-na-79"
            ),
            "{first}"
        );
        assert_eq!(first, preview(&config, &project, task.clone()).0);
        let mut other = task;
        other["number"] = json!(80);
        assert_ne!(first, preview(&config, &project, other).0);
        for title in [
            "../../$(touch nope)",
            "你好 🌳",
            "---",
            "a very very very long title",
            "éÉ Fix",
        ] {
            let task = json!({"number":79,"title":title,"body":""});
            let text = preview(&config, &project, task).0;
            let name = text.split(" | ").next().unwrap();
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            );
            assert!(name.starts_with("hey-boss-") && name.ends_with("-79"));
            assert!(
                name.trim_start_matches("hey-boss-")
                    .trim_end_matches("-79")
                    .len()
                    <= 15
            );
        }
    }
    #[test]
    fn default_worktree_prompt_instructs_agents_to_reuse_the_same_path_and_branch() {
        let config = ProjectConfig {
            cwd: "/workspace/repo".into(),
            worktree_enabled: true,
            ..Default::default()
        };
        let text = preview(&config, &project(), issue()).0;
        assert!(
            text.contains("/workspace/prompt-test-literal-body-7"),
            "{text}"
        );
        assert!(
            text.contains("Reuse")
                && text.contains("branch")
                && text.contains("prompt-test-literal-body-7")
        );
        assert!(!text.contains("{{worktree_"));
    }
    #[test]
    fn artifact_tasks_replace_implementation_and_delivery_prompts() {
        for label in ["task:plan", "task:research"] {
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
                task["labels"] = json!([label, "ready"]);
                task["plan"] = json!({"path":"/tmp/task-notes.md"});
                let (text, goal, objective) = preview(&config, &project(), task);
                assert!(text.contains("Claim and plan"), "{text}");
                assert!(text.contains("hey-boss artifact create"), "{text}");
                assert!(text.contains("--issue 7"));
                assert!(!text.contains("/tmp/task-notes.md"));
                assert!(!text.contains("Commit your changes"));
                assert!(!text.contains("pull request"));
                assert!(!text.contains("dedicated Git worktree"));
                assert!(!text.contains("Implement and deploy everything"));
                assert_eq!(goal, base.starts_with("/goal"));
                assert!(
                    objective
                        .trim_start_matches("- ")
                        .starts_with("Claim and plan")
                );
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
    fn github_evidence_is_fetched_on_demand_instead_of_embedded_in_prompts() {
        let mut task = issue();
        task["github_status"] = json!({"event":"event-1","prs":{
            "https://github.com/o/r/pull/1":{"evidence":{"checks":[{"details":"large evidence"}]}}
        }});
        let text = preview(&ProjectConfig::default(), &project(), task).0;
        assert!(!text.contains("large evidence"), "{text}");
        assert!(!text.contains("github_status"), "{text}");
        assert_eq!(
            text,
            preview(&ProjectConfig::default(), &project(), issue()).0
        );
    }
    #[test]
    fn subtasks_and_github_updates_use_the_normal_task_prompt() {
        for layout in [
            None,
            Some("{{task}}\n\n{{subtask}}\n\n{{github}}".to_owned()),
        ] {
            let config: ProjectConfig = serde_json::from_value(json!({
                "prompt":"Task {{number}}.",
                "prompt_overrides":{
                    "layout":layout,
                    "subtask":"Old subtask instructions.",
                    "github":"Old GitHub instructions."
                }
            }))
            .unwrap();
            let normal = preview(&config, &project(), issue()).0;
            let mut task = issue();
            task["subtask_context"] =
                json!({"position":2,"total":3,"parent":{"number":1,"title":"Parent"}});
            task["github_status"] = json!({"event":"finding","prs":{}});
            assert_eq!(preview(&config, &project(), task).0, normal);
            assert!(!normal.contains("{{subtask}}"));
            assert!(!normal.contains("{{github}}"));
        }
        for key in ["subtask", "github"] {
            assert!(prompt_defaults().get(key).is_none());
            assert!(
                !prompt_sections()
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["key"] == key)
            );
        }
    }
    #[test]
    fn task_details_are_fetched_instead_of_injected_into_prompts() {
        let mut task = issue();
        task["plan"] = json!({"path":"/tmp/task-plan.md"});
        task["dependency_context"] =
            json!([{"number":3,"title":"Dependency detail","state":"ready","pull_requests":[]}]);
        for layout in [
            None,
            Some("{{task}}\n\n{{dependencies}}\n\n{{plan_document}}".to_owned()),
        ] {
            let config: ProjectConfig = serde_json::from_value(json!({
                "prompt":"Task {{number}}.",
                "prompt_overrides":{
                    "layout":layout,
                    "dependencies":"Old prerequisite instructions: {{dependencies}}",
                    "plan_document":"Old plan instructions: {{plan_path}}"
                }
            }))
            .unwrap();
            let text = preview(&config, &project(), task.clone()).0;
            assert!(!text.contains("Dependency detail"), "{text}");
            assert!(!text.contains("task-plan.md"), "{text}");
            assert!(!text.contains("Old prerequisite"), "{text}");
            assert!(!text.contains("Old plan"), "{text}");
            assert!(!text.contains("{{dependencies}}"), "{text}");
            assert!(!text.contains("{{plan_document}}"), "{text}");
        }
        for key in ["dependencies", "plan_document"] {
            assert!(prompt_defaults().get(key).is_none());
            assert!(
                !prompt_sections()
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["key"] == key)
            );
        }
    }
    #[test]
    fn contextual_prompts_are_overridable_with_single_pass_variables() {
        let config: ProjectConfig = serde_json::from_value(json!({
            "prs_enabled":true,
            "prompt_overrides":{
                "handoff":"Handoff {{number}}: {{project_arg}}"
            }
        }))
        .unwrap();
        let mut task = issue();
        task["plan"] = json!({"path":"/tmp/{{number}}.md"});
        task["github_status"] = json!({"prs":{}});
        task["dependency_context"] = json!([{"number":3,"title":"Dependency {{number}}","state":"ready","pull_requests":[]}]);
        let text = preview(&config, &project(), task).0;
        assert!(
            text.contains("Handoff 7: 'named:a'\\''b $(touch nope)'"),
            "{text}"
        );

        assert!(!text.contains("Dependencies unblock at Ready"), "{text}");
        assert!(!text.contains("Plan document:"), "{text}");
    }
    #[test]
    fn layout_controls_sequence_and_omission_without_recursive_expansion() {
        let mut config = ProjectConfig {
            prompt: "Task {{title}}".into(),
            prs_enabled: true,
            prompt_overrides: PromptOverrides {
                layout: Some("{{delivery}}\n\n{{github}}\n\n{{task}}".into()),
                prs: Some("Ship {{number}}".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let (text, _, objective) = preview(&config, &project(), issue());
        assert_eq!(text, "Ship 7\n\nTask Literal {{body}}");
        assert_eq!(objective, text);
        config.prompt_overrides.layout = None;
        let text = preview(&config, &project(), issue()).0;
        assert!(
            text.starts_with("Task Literal {{body}}\n\nWork in"),
            "{text}"
        );
        assert!(text.contains("Dependencies unblock at Ready"));
    }
    #[test]
    fn configured_prompts_keep_their_instructions_and_add_pr_handoff_context() {
        for prs_enabled in [false, true] {
            for worktree_enabled in [false, true] {
                let config = ProjectConfig {
                    prompt: "Task {{number}}.".into(),
                    prs_enabled,
                    worktree_enabled,
                    prompt_overrides: PromptOverrides {
                        checkout: Some("Checkout {{number}}.".into()),
                        worktree: Some("Worktree {{number}}.".into()),
                        main: Some("Main {{number}}.".into()),
                        prs: Some("PR {{number}}.".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let (text, goal, objective) = preview(&config, &project(), issue());
                let base = format!(
                    "Task 7.\n\n{} 7.\n\n{} 7.",
                    if worktree_enabled {
                        "Worktree"
                    } else {
                        "Checkout"
                    },
                    if prs_enabled { "PR" } else { "Main" }
                );
                assert!(text.starts_with(&base));
                if prs_enabled {
                    assert!(text.contains("stacked PRs"));
                    assert!(text.contains("Dependencies unblock at Ready, before merge.\n\n- "));
                    assert!(text.contains(
                        "hey-boss issue ready 7 --project 'named:a'\\''b $(touch nope)'"
                    ));
                } else {
                    assert_eq!(text, base);
                }
                assert!(!goal);
                assert_eq!(objective, text);
            }
        }
    }
    #[test]
    fn authored_extra_instructions_are_preserved() {
        let config = ProjectConfig {
            prompt: "Task {{number}}.\n\nRecord delivery and verification evidence.".into(),
            prompt_overrides: PromptOverrides {
                main: Some("PR handoff: My own instructions.".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            preview(&config, &project(), issue()).0,
            "Task 7.\n\nRecord delivery and verification evidence.\n\nWork in the project's existing checkout.\n\nPR handoff: My own instructions."
        );
    }
    fn project() -> Project {
        Project {
            id: "named:a'b $(touch nope)".into(),
            name: "Prompt test".into(),
        }
    }
    fn with_workflow(base: &str) -> String {
        format!(
            "{base}\n\nWork in the project's existing checkout.\n\n{}",
            DEFAULT_MAIN_PROMPT.replace("{{number}}", "7")
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
                .replace("issue close 7", "issue close <number>")
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
