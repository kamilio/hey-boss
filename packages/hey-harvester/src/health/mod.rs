//! Local machine maintenance. Unknown processes and uncertain worktrees are preserved.
mod codex;
mod databases;
mod sweep;
pub use sweep::Statistics as CacheProgress;

mod caches;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod linux_harvest;
mod logs;
pub mod processes;
pub mod remote;
mod system;
pub mod worktrees;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug)]
struct Preserved(&'static str);
impl std::fmt::Display for Preserved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Preserved {}
fn preserved(reason: &'static str) -> io::Error {
    io::Error::other(Preserved(reason))
}
fn is_preserved(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Preserved>())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub automatic: bool,
    /// Expire disposable files/worktrees after 24 hours and reap stale developer workloads.
    pub aggressive: bool,
    pub harvest_processes: bool,
    pub clean_worktrees: bool,
    pub clean_caches: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub trim_worker_logs: bool,
    pub interval_seconds: u64,
    pub process_min_age_seconds: u64,
    pub browser_min_age_seconds: u64,
    pub observation_seconds: u64,
    pub worktree_min_age_days: u64,
    pub workspace_roots: Vec<PathBuf>,
}
fn is_false(value: &bool) -> bool {
    !*value
}
impl Default for Config {
    fn default() -> Self {
        let workspace = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Workspace");
        let codex = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default()
                    .join(".codex")
            })
            .join("worktrees");
        let workspace_roots = [workspace, codex]
            .into_iter()
            .filter(|p| p.is_dir())
            .collect();
        Self {
            automatic: false,
            aggressive: false,
            harvest_processes: true,
            clean_worktrees: true,
            clean_caches: true,
            trim_worker_logs: false,
            interval_seconds: 300,
            process_min_age_seconds: 3600,
            browser_min_age_seconds: 600,
            observation_seconds: 300,
            worktree_min_age_days: 7,
            workspace_roots,
        }
    }
}
impl Config {
    fn for_observations(&self, previous: &Snapshot) -> Self {
        // Timers skip overlapping checks. A large inventory makes the actual
        // cadence slower than the configured interval, even with no interruption.
        let duration = if previous.running {
            0
        } else {
            // Read the last completion event when migrating older stored snapshots.
            // A manual action with a newer observed_at cannot revive old observations.
            let recorded = previous
                .activity
                .iter()
                .rev()
                .find(|event| event.category == "scan" && event.message.starts_with("Finished:"))
                .map_or(0, |event| event.at.saturating_sub(previous.observed_at));
            previous.cycle_duration_seconds.max(recorded).min(86400)
        };
        Self {
            interval_seconds: self.interval_seconds.saturating_add(duration),
            ..self.clone()
        }
    }
    pub fn validate(&self) -> io::Result<()> {
        if self.interval_seconds < 60
            || self.interval_seconds > 86400
            || self.observation_seconds < 60
            || self.observation_seconds > 604800
            || self.process_min_age_seconds < 600
            || self.browser_min_age_seconds < 600
            || self.worktree_min_age_days < 1
            || self.worktree_min_age_days > 3650
        {
            return Err(io::Error::other(
                "Health intervals must be at least 60 seconds, process age 10 minutes, and worktree age 1–3650 days",
            ));
        }
        if self.workspace_roots.len() > 32
            || self
                .workspace_roots
                .iter()
                .any(|p| !p.is_absolute() || p.parent().is_none())
        {
            return Err(io::Error::other(
                "Use at most 32 absolute workspace roots, never the filesystem root",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub disk_path: String,
    pub disk_total_bytes: Option<u64>,
    pub disk_available_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub memory_pressure: String,
    pub swap_used_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub name: String,
    pub detail: String,
    pub eligible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<worktrees::Details>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub at: u64,
    pub category: String,
    pub message: String,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub observed_at: u64,
    #[serde(default)]
    pub cycle_duration_seconds: u64,
    pub last_cleanup_at: Option<u64>,
    pub metrics: Metrics,
    pub config: Config,
    pub processes: Vec<Item>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_inventory: Option<Vec<processes::DisplayProcess>>,
    pub worktrees: Vec<Item>,
    pub harvested_processes: usize,
    pub removed_worktrees: usize,
    #[serde(default)]
    pub caches: Vec<Item>,
    #[serde(default)]
    pub removed_caches: usize,
    #[serde(default)]
    pub cache_progress: CacheProgress,
    #[serde(default)]
    pub trimmed_logs: usize,
    /// Net volume free-space change, including concurrent filesystem activity.
    #[serde(default)]
    pub disk_available_change_bytes: Option<i64>,
    pub errors: Vec<String>,
    #[serde(default)]
    pub activity: Vec<Activity>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub phase: String,
}
impl Snapshot {
    fn collect_item_errors(&mut self) {
        for item in self
            .processes
            .iter()
            .chain(&self.caches)
            .chain(&self.worktrees)
        {
            if let Some(error) = &item.error {
                let message = format!("{}: {error}", item.name);
                if !self.errors.contains(&message) {
                    self.errors.push(message);
                }
            }
        }
    }
    fn refresh_process_inventory(&mut self) {
        self.process_inventory = match processes::display_inventory() {
            Ok(rows) => Some(rows),
            Err(error) => {
                self.errors.push(format!("Process list: {error}"));
                None
            }
        };
    }
    pub fn record(&mut self, category: &str, message: impl Into<String>) {
        self.activity.push(Activity {
            at: now(),
            category: category.into(),
            message: message.into().chars().take(1500).collect(),
        });
        if self.activity.len() > 1000 {
            self.activity.drain(..self.activity.len() - 1000);
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub first_seen: u64,
    pub last_seen: u64,
    pub cpu_seconds: f64,
    #[serde(default)]
    pub activity_fingerprint: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
pub struct State {
    pub processes: BTreeMap<String, Observation>,
    pub worktrees: BTreeMap<String, Observation>,
    #[serde(default)]
    pub caches: BTreeMap<String, Observation>,
    #[serde(default)]
    pub sweep: sweep::Progress,
    pub snapshot: Snapshot,
}

pub struct Store {
    pub directory: PathBuf,
}
impl Store {
    pub fn standard() -> io::Result<Self> {
        let directory = if let Some(path) = std::env::var_os("HEY_BOSS_HEALTH_DIR") {
            PathBuf::from(path)
        } else {
            PathBuf::from(
                std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is unavailable"))?,
            )
            .join(".local/share/hey-boss/health")
        };
        Self::new(directory)
    }
    pub fn new(directory: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&directory)?;
        let metadata = fs::symlink_metadata(&directory)?;
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::other(
                "Health state must be an owned directory, not a symlink",
            ));
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        Ok(Self { directory })
    }
    pub fn lock(&self) -> io::Result<File> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.directory.join("maintenance.lock"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            return Err(if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Machine maintenance is already running",
                )
            } else {
                error
            });
        }
        Ok(file)
    }
    pub fn config(&self) -> io::Result<Config> {
        let c: Config = self.read("config.json")?;
        c.validate()?;
        Ok(c)
    }
    pub fn state(&self) -> io::Result<State> {
        self.read("state.json")
    }
    fn read<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> io::Result<T> {
        let path = self.directory.join(name);
        let mut f = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
            Err(e) => return Err(e),
        };
        if f.metadata()?.len() > 4 * 1024 * 1024 {
            return Err(io::Error::other("Health state exceeds size limit"));
        }
        let mut data = Vec::new();
        f.read_to_end(&mut data)?;
        serde_json::from_slice(&data).map_err(io::Error::other)
    }
    pub fn save<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        use std::io::Write;
        let data = serde_json::to_vec(value)?;
        if data.len() > 4 * 1024 * 1024 {
            return Err(io::Error::other("Health runtime state exceeds size limit"));
        }
        let temporary = self
            .directory
            .join(format!(".{name}.{}", std::process::id()));
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let result = (|| {
            f.write_all(&data)?;
            f.sync_all()?;
            fs::rename(&temporary, self.directory.join(name))
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
    pub fn status(&self) -> io::Result<Snapshot> {
        let mut s = self.state()?.snapshot;
        self.reconcile_running_snapshot(&mut s)?;
        s.config = self.config()?;
        s.metrics = system::metrics();
        s.refresh_process_inventory();
        Ok(s)
    }
    fn reconcile_running_snapshot(&self, s: &mut Snapshot) -> io::Result<()> {
        if s.running
            && let Ok(_lock) = self.lock()
        {
            // The worker may have finished since the first read. Keep the lock
            // through the re-read and classification, before collecting live metrics.
            *s = self.state()?.snapshot;
            if s.running {
                s.running = false;
                s.phase = "Previous check was interrupted".into();
                s.errors.push("Previous maintenance process exited before finishing; the next scheduled check will retry".into());
            }
        }
        Ok(())
    }
    fn checkpoint(&self, state: &mut State, snapshot: &Snapshot) -> io::Result<()> {
        state.snapshot = snapshot.clone();
        // Early phases have a fresh snapshot, but traversal progress spans cycles.
        state.snapshot.cache_progress = state.sweep.stats.clone();
        self.save("state.json", state)
    }
    pub fn record_setting(&self, message: &str) -> io::Result<()> {
        let mut state = self.state()?;
        state.snapshot.record("settings", message);
        self.save("state.json", &state)
    }
    pub fn remove_worktree(&self, path: &Path) -> io::Result<Snapshot> {
        let _lock = self.lock()?;
        let mut state = self.state()?;
        state.snapshot.config = self.config()?;
        state.snapshot.errors.clear();
        state.snapshot.running = true;
        state.snapshot.phase = "Checking selected worktree before removal".into();
        state
            .snapshot
            .record("worktree", format!("Requested removal: {}", path.display()));
        self.save("state.json", &state)?;
        match worktrees::remove_one(path) {
            Ok(()) => {
                state
                    .snapshot
                    .worktrees
                    .retain(|item| item.worktree.as_ref().is_none_or(|w| w.path != path));
                state.snapshot.removed_worktrees += 1;
                state.snapshot.last_cleanup_at = Some(now());
                state
                    .worktrees
                    .retain(|key, _| !key.starts_with(&format!("{}:", path.display())));
                state.snapshot.record(
                    "worktree",
                    format!("Removed {}; branch retained", path.display()),
                );
                state.snapshot.phase = "Worktree removed; branch retained".into();
            }
            Err(error) => {
                let message = format!("{}: {error}", path.display());
                state.snapshot.record("error", &message);
                state.snapshot.errors.push(message);
                state.snapshot.phase = "Worktree preserved".into();
            }
        }
        state.snapshot.running = false;
        state.snapshot.observed_at = now();
        self.save("state.json", &state)?;
        state.snapshot.refresh_process_inventory();
        Ok(state.snapshot)
    }
    pub fn cycle(&self, apply: bool) -> io::Result<Snapshot> {
        let _lock = self.lock()?;
        let started = Instant::now();
        let config = self.config()?;
        let mut state = self.state()?;
        let observation_config = config.for_observations(&state.snapshot);
        let mut snapshot = Snapshot {
            observed_at: now(),
            metrics: system::metrics(),
            config: config.clone(),
            last_cleanup_at: state.snapshot.last_cleanup_at,
            activity: state.snapshot.activity.clone(),
            running: true,
            phase: "Inspecting processes".into(),
            ..Snapshot::default()
        };
        let disk_before = snapshot.metrics.disk_available_bytes;
        if state.snapshot.running {
            state.processes.clear();
            state.worktrees.clear();
            state.caches.clear();
            snapshot.record(
                "error",
                "Previous check was interrupted; quiet observations reset before retrying",
            );
        }
        snapshot.record(
            "scan",
            if apply {
                "Cleanup check started"
            } else {
                "Inspection started; no processes, worktrees, or caches will be removed"
            },
        );
        snapshot.record(
            "scan",
            "Checking process owners, identities, connections, and quiet observations",
        );
        self.checkpoint(&mut state, &snapshot)?;
        let process_table = match processes::inventory() {
            Ok(table) => table,
            Err(e) => {
                state.processes.clear();
                state.worktrees.clear();
                state.caches.clear();
                snapshot.errors.push(format!("Process inventory: {e}"));
                snapshot.record(
                    "error",
                    format!("Process inspection failed; disk cleanup continues: {e}"),
                );
                BTreeMap::new()
            }
        };
        inspect_processes(
            &process_table,
            &observation_config,
            &mut state.processes,
            &mut snapshot,
            apply,
        );
        // Caches are cheap to inspect; do not put them behind hundreds of Git checks.
        snapshot.phase = "Inspecting disposable caches".into();
        self.checkpoint(&mut state, &snapshot)?;
        match if config.aggressive {
            sweep::clean(&config, &mut state.sweep, apply && config.clean_caches)
        } else {
            caches::clean(
                &observation_config,
                &mut state.caches,
                apply && config.clean_caches,
            )
        } {
            Ok((items, count)) => {
                snapshot.caches = items;
                snapshot.removed_caches = count;
            }
            Err(error) => {
                state.caches.clear();
                snapshot.errors.push(format!("Cache cleaner: {error}"));
            }
        }
        for item in snapshot.caches.clone() {
            snapshot.record("cache", format!("{} — {}", item.name, item.detail));
        }
        snapshot.cache_progress = state.sweep.stats.clone();
        if let Some(home) = std::env::var_os("HOME") {
            match logs::clean(&PathBuf::from(home), apply && config.trim_worker_logs) {
                Ok((mut items, count)) => {
                    snapshot.trimmed_logs = count;
                    for item in &mut items {
                        if !config.trim_worker_logs {
                            item.eligible = false;
                            item.detail = format!("Worker-log trimming disabled; {}", item.detail);
                        }
                        snapshot.record("log", format!("{} — {}", item.name, item.detail));
                    }
                    snapshot.caches.extend(items);
                }
                Err(error) => snapshot.errors.push(format!("Worker log cleanup: {error}")),
            }
        }
        snapshot.phase = "Inspecting worktrees".into();
        snapshot.record(
            "scan",
            "Checking worktree age, open files, agent activity, Git state, and merge status",
        );
        self.checkpoint(&mut state, &snapshot)?;
        // Keep a single maintenance owner, but do not let hundreds of Git checks
        // postpone the next process observation for tens of minutes. The disk
        // scanner owns only its worktree observations; this thread owns state
        // publication and every process check. Both finish before releasing the lock.
        let mut worktree_observations = state.worktrees.clone();
        let result = during_worktree_scan(
            Duration::from_secs(config.interval_seconds),
            || {
                worktrees::clean(
                    &observation_config,
                    &process_table,
                    &mut worktree_observations,
                    apply && config.clean_worktrees,
                )
            },
            || {
                snapshot.record(
                    "scan",
                    "Checking processes while the worktree scan continues",
                );
                match processes::inventory() {
                    Ok(table) => inspect_processes(
                        &table,
                        &observation_config,
                        &mut state.processes,
                        &mut snapshot,
                        apply,
                    ),
                    Err(error) => {
                        state.processes.clear();
                        snapshot.processes.clear();
                        let message = format!("Process inventory: {error}");
                        snapshot.errors.push(message.clone());
                        snapshot.record("error", message);
                    }
                }
                snapshot.metrics = system::metrics();
                self.checkpoint(&mut state, &snapshot)
            },
        )?;
        state.worktrees = worktree_observations;
        match result {
            Ok((items, count)) => {
                snapshot.worktrees = items;
                snapshot.removed_worktrees = count;
            }
            Err(e) => {
                state.worktrees.clear();
                snapshot.errors.push(format!("Worktree cleaner: {e}"));
            }
        }
        for item in snapshot.worktrees.clone() {
            snapshot.record("worktree", format!("{} — {}", item.name, item.detail));
        }
        if apply {
            let after = system::disk_available_bytes();
            snapshot.disk_available_change_bytes = disk_before.zip(after).map(|(before, after)| {
                (i128::from(after) - i128::from(before))
                    .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            });
            snapshot.metrics.disk_available_bytes = after;
            if let Some(change) = snapshot.disk_available_change_bytes {
                snapshot.record("disk", format!("Net available disk-space change: {change:+} bytes (includes concurrent writes and APFS shared blocks)"));
            }
        }
        snapshot.collect_item_errors();
        for error in snapshot.errors.clone() {
            snapshot.record("error", error);
        }
        if apply {
            snapshot.last_cleanup_at = Some(snapshot.observed_at);
        }
        snapshot.running = false;
        snapshot.cycle_duration_seconds = started.elapsed().as_secs();
        snapshot.phase = if snapshot.errors.is_empty() {
            "Waiting for the next check"
        } else {
            "Finished with inspection errors"
        }
        .into();
        snapshot.record("scan", format!("Finished: {} worktrees checked; {} processes stopped; {} worktrees removed; {} caches removed; {} errors.", snapshot.worktrees.len(), snapshot.harvested_processes, snapshot.removed_worktrees, snapshot.removed_caches, snapshot.errors.len()));
        self.checkpoint(&mut state, &snapshot)?;
        snapshot.refresh_process_inventory();
        Ok(snapshot)
    }
}

fn inspect_processes(
    table: &processes::Table,
    config: &Config,
    observations: &mut BTreeMap<String, Observation>,
    snapshot: &mut Snapshot,
    apply: bool,
) {
    match if config.aggressive {
        processes::aggressive_harvest(
            table,
            &snapshot.metrics.memory_pressure,
            config,
            observations,
            apply && config.harvest_processes,
        )
    } else {
        processes::harvest(
            table,
            config,
            observations,
            apply && config.harvest_processes,
        )
    } {
        Ok((items, count)) => {
            snapshot.processes = items;
            snapshot.harvested_processes += count;
            for item in snapshot.processes.clone() {
                snapshot.record("process", format!("{} — {}", item.name, item.detail));
            }
            snapshot.record("scan", format!("Inspected {} processes; {} candidate groups; stopped {count} processes. Database services and maintenance ancestry are protected.", table.len(), snapshot.processes.len()));
        }
        Err(error) => {
            observations.clear();
            snapshot.processes.clear();
            let message = format!("Process harvester: {error}");
            snapshot.errors.push(message.clone());
            snapshot.record("error", message);
        }
    }
}

fn during_worktree_scan<T: Send>(
    interval: Duration,
    scan: impl FnOnce() -> T + Send,
    mut check_processes: impl FnMut() -> io::Result<()>,
) -> io::Result<T> {
    std::thread::scope(|scope| {
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        scope.spawn(move || {
            let _ = send.send(scan());
        });
        loop {
            match receive.recv_timeout(interval) {
                Ok(result) => return Ok(result),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => check_processes()?,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("Worktree scan interrupted"));
                }
            }
        }
    })
}

/// Bound runtime and output; no subprocess left behind on timeout or read failure.
pub(crate) fn output(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    output_with_limit(command, timeout, 8 * 1024 * 1024)
}

fn output_with_limit(
    command: &mut Command,
    timeout: Duration,
    stdout_limit: usize,
) -> io::Result<Output> {
    let mut stdout = Vec::new();
    let mut result = stream_output(command, timeout, |bytes| {
        if stdout.len().saturating_add(bytes.len()) > stdout_limit {
            return Err(io::Error::other("Inspection output exceeds limit"));
        }
        stdout.extend_from_slice(bytes);
        Ok(true)
    })?
    .ok_or_else(|| io::Error::other("Inspection unexpectedly stopped"))?;
    result.stdout = stdout;
    Ok(result)
}

/// Visit NUL-delimited Git paths without retaining a whole checkout's listing.
/// False means the visitor proved it could stop early; true requires clean EOF
/// and successful exit. A truncated or failed inspection can never authorize deletion.
fn visit_nul_output(
    command: &mut Command,
    timeout: Duration,
    mut visit: impl FnMut(&[u8]) -> io::Result<bool>,
) -> io::Result<bool> {
    let mut pending = Vec::new();
    let result = stream_output(command, timeout, |mut bytes| {
        while let Some(end) = bytes.iter().position(|b| *b == 0) {
            pending.extend_from_slice(&bytes[..end]);
            if !pending.is_empty() && !visit(&pending)? {
                return Ok(false);
            }
            pending.clear();
            bytes = &bytes[end + 1..];
        }
        pending.extend_from_slice(bytes);
        if pending.len() > 1024 * 1024 {
            return Err(io::Error::other("Inspection record exceeds limit"));
        }
        Ok(true)
    })?;
    let Some(result) = result else {
        return Ok(false);
    };
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(io::Error::other("Git path inspection failed"));
    }
    if !pending.is_empty() {
        return Err(io::Error::other("Incomplete Git path record"));
    }
    Ok(true)
}

fn stream_output(
    command: &mut Command,
    timeout: Duration,
    mut consume: impl FnMut(&[u8]) -> io::Result<bool>,
) -> io::Result<Option<Output>> {
    use std::os::unix::process::CommandExt;
    command
        .process_group(0)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let result = (|| {
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        fn drain(
            reader: &mut impl Read,
            eof: &mut bool,
            consume: &mut impl FnMut(&[u8]) -> io::Result<bool>,
        ) -> io::Result<bool> {
            let mut buffer = [0u8; 8192];
            // A continuously writing subprocess must not starve the deadline
            // check or its stderr pipe.
            for _ in 0..16 {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        *eof = true;
                        return Ok(true);
                    }
                    Ok(n) => {
                        if !consume(&buffer[..n])? {
                            return Ok(false);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(true),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok(true)
        }
        let deadline = Instant::now() + timeout;
        let mut err = Vec::new();
        let (mut out_eof, mut err_eof) = (false, false);
        let mut status = None;
        loop {
            if !drain(&mut stdout, &mut out_eof, &mut consume)? {
                return Ok(None);
            }
            drain(&mut stderr, &mut err_eof, &mut |bytes: &[u8]| {
                err.extend_from_slice(bytes);
                if err.len() > 8 * 1024 * 1024 {
                    return Err(io::Error::other("Inspection stderr exceeds limit"));
                }
                Ok(true)
            })?;
            if status.is_none() {
                status = child.try_wait()?;
            }
            if out_eof
                && err_eof
                && let Some(status) = status
            {
                return Ok(Some(Output {
                    status,
                    stdout: Vec::new(),
                    stderr: err,
                }));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("Inspection command timed out"));
            }
            // Wait only when neither pipe is ready. Sleeping after every bounded
            // read adds avoidable delays to large lsof/Git inventories.
            let mut ready = [
                libc::pollfd {
                    fd: if out_eof { -1 } else { stdout.as_raw_fd() },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if err_eof { -1 } else { stderr.as_raw_fd() },
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, 100) as i32;
            if unsafe { libc::poll(ready.as_mut_ptr(), ready.len() as libc::nfds_t, wait) } < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    })();
    if !matches!(result, Ok(Some(_))) {
        // This group is created solely for this inspection, never an existing user's group.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
pub(crate) fn text(command: &mut Command) -> io::Result<String> {
    text_within(command, Duration::from_secs(15))
}
pub(crate) fn text_within(command: &mut Command, timeout: Duration) -> io::Result<String> {
    let o = output(command, timeout)?;
    if !o.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&o.stderr).trim().to_owned(),
        ));
    }
    String::from_utf8(o.stdout).map_err(io::Error::other)
}
pub fn readable_bytes(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

pub(crate) fn under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    #[test]
    fn failed_items_count_as_errors_but_protected_items_do_not() {
        let mut snapshot = super::Snapshot::default();
        for (detail, error) in [
            ("Inspection failed", Some("Cannot read index".to_owned())),
            ("SQLite database or sidecar preserved", None),
            ("Running Codex session uses this worktree; preserved", None),
        ] {
            snapshot.worktrees.push(super::Item {
                name: detail.into(),
                detail: detail.into(),
                error,
                eligible: false,
                worktree: None,
            });
        }
        snapshot.collect_item_errors();
        snapshot.collect_item_errors();
        assert_eq!(snapshot.errors.len(), 1);
        assert!(snapshot.errors[0].contains("Cannot read index"));
        let legacy: super::Item =
            serde_json::from_str(r#"{"name":"old","detail":"protected","eligible":false}"#)
                .unwrap();
        assert!(legacy.error.is_none());
    }
    #[test]
    fn ready_inspection_output_is_not_artificially_throttled() {
        let mut bytes = 0;
        let result = stream_output(
            Command::new("sh").args([
                "-c",
                "dd if=/dev/zero bs=1048576 count=32 2>/dev/null; printf complete >&2",
            ]),
            Duration::from_secs(2),
            |chunk| {
                bytes += chunk.len();
                Ok(true)
            },
        )
        .unwrap()
        .unwrap();
        assert!(result.status.success());
        assert_eq!(bytes, 32 * 1024 * 1024);
        assert_eq!(result.stderr, b"complete");
    }

    #[test]
    fn streamed_records_have_no_total_output_cap_and_reject_incomplete_results() {
        let mut count = 0;
        assert!(
            super::visit_nul_output(
                std::process::Command::new("sh").args([
                    "-c",
                    "awk 'BEGIN { for (i=0;i<100000;i++) printf \"%0100d%c\", i, 0 }'"
                ]),
                std::time::Duration::from_secs(20),
                |_| {
                    count += 1;
                    Ok(true)
                },
            )
            .unwrap()
        );
        assert_eq!(count, 100000);
        for script in ["printf unterminated", "printf 'entry\\0'; exit 1"] {
            assert!(
                super::visit_nul_output(
                    std::process::Command::new("sh").args(["-c", script]),
                    std::time::Duration::from_secs(5),
                    |_| Ok(true),
                )
                .is_err()
            );
        }
    }
    #[test]
    fn streamed_records_can_stop_early_and_reap_the_inspector() {
        let started = std::time::Instant::now();
        assert!(
            !super::visit_nul_output(
                std::process::Command::new("sh").args(["-c", "printf 'recent\\0'; sleep 30"]),
                std::time::Duration::from_secs(5),
                |_| Ok(false),
            )
            .unwrap()
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn process_checks_continue_until_slow_worktree_scan_finishes() {
        let (send, receive) = std::sync::mpsc::channel();
        let mut checks = 0;
        let result = during_worktree_scan(
            Duration::from_millis(10),
            move || {
                for _ in 0..3 {
                    receive.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                "disk scan complete"
            },
            || {
                checks += 1;
                let _ = send.send(());
                Ok(())
            },
        )
        .unwrap();
        assert!(checks >= 3);
        assert_eq!(result, "disk scan complete");
    }

    #[test]
    fn quick_worktree_scans_do_not_trigger_extra_process_checks() {
        assert_eq!(
            during_worktree_scan(
                Duration::from_secs(1),
                || 42,
                || { panic!("process interval has not elapsed") }
            )
            .unwrap(),
            42
        );
    }

    #[test]
    fn process_checkpoint_failure_is_reported_and_scan_is_joined() {
        let finished = std::sync::atomic::AtomicBool::new(false);
        let completed = &finished;
        let (send, receive) = std::sync::mpsc::channel();
        let error = during_worktree_scan(
            Duration::from_millis(5),
            move || {
                receive.recv_timeout(Duration::from_secs(5)).unwrap();
                completed.store(true, Ordering::SeqCst);
            },
            || {
                send.send(()).unwrap();
                Err(io::Error::other("checkpoint failed"))
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "checkpoint failed");
        assert!(finished.load(Ordering::SeqCst));
    }

    #[test]
    fn settings_lock_and_corrupt_state_fail_closed() {
        let root = std::env::temp_dir().join(format!(
            "hb-health-store-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let store = Store::new(root.clone()).unwrap();
        let lock = store.lock().unwrap();
        assert_eq!(store.lock().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        drop(lock);
        let config = Config::default();
        store.save("config.json", &config).unwrap();
        assert!(!store.config().unwrap().automatic);
        fs::write(root.join("state.json"), "broken").unwrap();
        assert!(store.state().is_err());
        fs::remove_file(root.join("state.json")).unwrap();
        std::os::unix::fs::symlink(root.join("config.json"), root.join("state.json")).unwrap();
        assert!(store.state().is_err());
        assert!(!config.automatic);
        assert_eq!(config.process_min_age_seconds, 3600);
        assert!(
            Config {
                interval_seconds: 0,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                workspace_roots: vec![PathBuf::from("/")],
                ..config
            }
            .validate()
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn activity_is_bounded_and_old_state_remains_readable() {
        let mut s = Snapshot::default();
        for i in 0..1100 {
            s.record("scan", format!("event {i}"));
        }
        assert_eq!(s.activity.len(), 1000);
        assert_eq!(s.activity.first().unwrap().message, "event 100");
        let mut old = serde_json::to_value(&s).unwrap();
        let fields = old.as_object_mut().unwrap();
        fields.remove("activity");
        fields.remove("running");
        fields.remove("phase");
        fields.remove("cycle_duration_seconds");
        fields.remove("caches");
        fields.remove("removed_caches");
        fields.remove("disk_available_change_bytes");
        let restored: Snapshot = serde_json::from_value(old).unwrap();
        assert!(restored.activity.is_empty() && !restored.running);
        assert_eq!(restored.cycle_duration_seconds, 0);
        assert!(
            restored.caches.is_empty()
                && restored.removed_caches == 0
                && restored.disk_available_change_bytes.is_none()
        );
        let old: State = serde_json::from_value(
            serde_json::json!({"processes": {}, "worktrees": {}, "snapshot": restored}),
        )
        .unwrap();
        assert!(old.caches.is_empty());
        let config: Config =
            serde_json::from_value(serde_json::json!({"automatic": true})).unwrap();
        assert!(config.clean_caches && config.worktree_min_age_days == 7);
        assert!(!config.trim_worker_logs);
        let serialized = serde_json::to_value(&config).unwrap();
        assert!(serialized.get("trim_worker_logs").is_none());
        let enabled = Config {
            trim_worker_logs: true,
            ..config
        };
        assert_eq!(
            serde_json::to_value(enabled).unwrap()["trim_worker_logs"],
            true
        );
    }
    #[test]
    fn checkpoints_preserve_cache_progress_before_cache_inspection_finishes() {
        let root = std::env::temp_dir().join(format!("hb-health-progress-{}", std::process::id()));
        let store = Store::new(root.clone()).unwrap();
        let _lock = store.lock().unwrap();
        let mut state = State::default();
        state.sweep.stats = CacheProgress {
            pass_started_at: 200,
            last_completed_at: Some(190),
            last_pass_seconds: Some(90),
            visited_this_pass: 1234,
            removed_this_pass: 567,
            roots_pending: 2,
            discovery_pending: true,
            ..CacheProgress::default()
        };
        let expected = serde_json::to_value(&state.sweep.stats).unwrap();
        for phase in ["Inspecting processes", "Inspecting disposable caches"] {
            let snapshot = Snapshot {
                running: true,
                phase: phase.into(),
                ..Snapshot::default()
            };
            store.checkpoint(&mut state, &snapshot).unwrap();
            let status = store.status().unwrap();
            assert_eq!(status.phase, phase);
            assert_eq!(
                serde_json::to_value(status.cache_progress).unwrap(),
                expected
            );
            assert_eq!(
                serde_json::to_value(store.state().unwrap().sweep.stats).unwrap(),
                expected
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn status_recognizes_an_interrupted_check() {
        let root =
            std::env::temp_dir().join(format!("hb-health-interrupted-{}", std::process::id()));
        let store = Store::new(root.clone()).unwrap();
        let mut state = State::default();
        state.snapshot.running = true;
        state.snapshot.phase = "Inspecting processes".into();
        store.save("state.json", &state).unwrap();
        let lock = store.lock().unwrap();
        let live = store.status().unwrap();
        assert!(live.running);
        assert_eq!(live.phase, "Inspecting processes");
        assert!(live.errors.is_empty());
        assert!(live.processes.is_empty());
        assert!(
            live.process_inventory
                .unwrap()
                .iter()
                .any(|p| p.pid == std::process::id())
        );
        assert!(store.state().unwrap().snapshot.process_inventory.is_none());
        drop(lock);
        let status = store.status().unwrap();
        assert!(!status.running && status.phase.contains("interrupted"));
        assert_eq!(status.errors.len(), 1);
        // Status is a read-only projection, including interruption detection.
        assert!(store.state().unwrap().snapshot.running);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn status_reloads_a_worker_that_finished_after_the_initial_read() {
        let root = std::env::temp_dir().join(format!("hb-health-finished-{}", std::process::id()));
        let store = Store::new(root.clone()).unwrap();
        let lock = store.lock().unwrap();
        let mut state = State::default();
        state.snapshot.running = true;
        state.snapshot.phase = "Inspecting processes".into();
        store.save("state.json", &state).unwrap();
        let mut stale = store.state().unwrap().snapshot;

        // Complete between status's first read and lock attempt, even within
        // the same timestamp. The entire completed snapshot must be returned.
        state.snapshot.running = false;
        state.snapshot.phase = "Finished".into();
        state.snapshot.removed_caches = 7;
        state.snapshot.errors.push("Existing cleanup error".into());
        state.snapshot.record("scan", "Finished: removed 7 caches");
        store.save("state.json", &state).unwrap();
        drop(lock);

        store.reconcile_running_snapshot(&mut stale).unwrap();
        assert_eq!(
            serde_json::to_value(&stale).unwrap(),
            serde_json::to_value(&state.snapshot).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn status_does_not_hide_an_unreadable_state_after_the_initial_read() {
        let root =
            std::env::temp_dir().join(format!("hb-health-status-corrupt-{}", std::process::id()));
        let store = Store::new(root.clone()).unwrap();
        let mut stale = Snapshot {
            running: true,
            ..Snapshot::default()
        };
        fs::write(root.join("state.json"), "broken").unwrap();
        assert!(store.reconcile_running_snapshot(&mut stale).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn inspection_timeout_reaps_its_command_tree() {
        let start = Instant::now();
        assert!(
            output(
                Command::new("sh").args(["-c", "sleep 30 & wait"]),
                Duration::from_millis(150)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        let start = Instant::now();
        assert!(
            output(
                Command::new("sh").args(["-c", "sleep 30 &"]),
                Duration::from_millis(150)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
