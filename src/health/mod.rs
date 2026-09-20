//! Local machine maintenance. Unknown processes and uncertain worktrees are preserved.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod linux_harvest;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub automatic: bool,
    pub harvest_processes: bool,
    pub clean_worktrees: bool,
    pub interval_seconds: u64,
    pub process_min_age_seconds: u64,
    pub browser_min_age_seconds: u64,
    pub observation_seconds: u64,
    pub worktree_min_age_days: u64,
    pub workspace_roots: Vec<PathBuf>,
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
            harvest_processes: true,
            clean_worktrees: true,
            interval_seconds: 300,
            process_min_age_seconds: 3600,
            browser_min_age_seconds: 600,
            observation_seconds: 300,
            worktree_min_age_days: 14,
            workspace_roots,
        }
    }
}
impl Config {
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
    pub last_cleanup_at: Option<u64>,
    pub metrics: Metrics,
    pub config: Config,
    pub processes: Vec<Item>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_inventory: Option<Vec<processes::DisplayProcess>>,
    pub worktrees: Vec<Item>,
    pub harvested_processes: usize,
    pub removed_worktrees: usize,
    pub errors: Vec<String>,
    #[serde(default)]
    pub activity: Vec<Activity>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub phase: String,
}
impl Snapshot {
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
        s.config = self.config()?;
        s.metrics = system::metrics();
        s.refresh_process_inventory();
        if s.running && self.lock().is_ok() {
            s.running = false;
            s.phase = "Previous check was interrupted".into();
            s.errors.push("Previous maintenance process exited before finishing; the next scheduled check will retry".into());
        }
        Ok(s)
    }
    fn checkpoint(&self, state: &mut State, snapshot: &Snapshot) -> io::Result<()> {
        state.snapshot = snapshot.clone();
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
        let config = self.config()?;
        let mut state = self.state()?;
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
        if state.snapshot.running {
            state.processes.clear();
            state.worktrees.clear();
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
                "Inspection started; no processes or worktrees will be removed"
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
                snapshot.errors.push(format!("Process inventory: {e}"));
                snapshot.record(
                    "error",
                    format!("Process inspection failed; no cleanup: {e}"),
                );
                snapshot.running = false;
                snapshot.phase = "Check failed".into();
                self.checkpoint(&mut state, &snapshot)?;
                return Ok(snapshot);
            }
        };
        match processes::harvest(
            &process_table,
            &config,
            &mut state.processes,
            apply && config.harvest_processes,
        ) {
            Ok((items, count)) => {
                snapshot.processes = items;
                snapshot.harvested_processes = count;
            }
            Err(e) => {
                state.processes.clear();
                snapshot.errors.push(format!("Process harvester: {e}"));
            }
        }
        for item in snapshot.processes.clone() {
            snapshot.record("process", format!("{} — {}", item.name, item.detail));
        }
        snapshot.record("scan", format!("Inspected {} processes; {} candidate groups; stopped {} processes. Codex and normal services are protected.", process_table.len(), snapshot.processes.len(), snapshot.harvested_processes));
        snapshot.phase = "Inspecting worktrees".into();
        snapshot.record(
            "scan",
            "Checking worktree age, open files, agent activity, Git state, and merge status",
        );
        self.checkpoint(&mut state, &snapshot)?;
        match worktrees::clean(
            &config,
            &process_table,
            &mut state.worktrees,
            apply && config.clean_worktrees,
        ) {
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
        for error in snapshot.errors.clone() {
            snapshot.record("error", error);
        }
        if apply {
            snapshot.last_cleanup_at = Some(snapshot.observed_at);
        }
        snapshot.running = false;
        snapshot.phase = if snapshot.errors.is_empty() {
            "Waiting for the next check"
        } else {
            "Finished with inspection errors"
        }
        .into();
        snapshot.record("scan", format!("Finished: {} worktrees checked; {} processes stopped; {} worktrees removed; {} errors.", snapshot.worktrees.len(), snapshot.harvested_processes, snapshot.removed_worktrees, snapshot.errors.len()));
        self.checkpoint(&mut state, &snapshot)?;
        snapshot.refresh_process_inventory();
        Ok(snapshot)
    }
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
            data: &mut Vec<u8>,
            eof: &mut bool,
            limit: usize,
        ) -> io::Result<()> {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        *eof = true;
                        return Ok(());
                    }
                    Ok(n) => {
                        data.extend_from_slice(&buffer[..n]);
                        if data.len() > limit {
                            return Err(io::Error::other("Inspection output exceeds limit"));
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        let deadline = Instant::now() + timeout;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (mut out_eof, mut err_eof) = (false, false);
        let mut status = None;
        loop {
            drain(&mut stdout, &mut out, &mut out_eof, stdout_limit)?;
            drain(&mut stderr, &mut err, &mut err_eof, 8 * 1024 * 1024)?;
            if status.is_none() {
                status = child.try_wait()?;
            }
            if out_eof
                && err_eof
                && let Some(status) = status
            {
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("Inspection command timed out"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
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
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
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
        let restored: Snapshot = serde_json::from_value(old).unwrap();
        assert!(restored.activity.is_empty() && !restored.running);
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
