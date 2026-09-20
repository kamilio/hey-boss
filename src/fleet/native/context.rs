use super::{
    Result,
    replica::{self, invalid},
};
use crate::issues::{Actor, Operation, Project, Request, Store};
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, Read, Write},
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct Context {
    pub home: PathBuf,
    pub state: PathBuf,
    pub desired: PathBuf,
    pub binary: PathBuf,
    pub path: PathBuf,
    pub node: String,
    pub stop: Arc<AtomicBool>,
}
impl Context {
    pub fn new() -> Result<Self> {
        let home =
            PathBuf::from(std::env::var_os("HOME").ok_or_else(|| invalid("HOME is missing"))?);
        let state = std::env::var_os("HEY_BOSS_FLEET_STATE")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share/hey-boss"));
        let desired = std::env::var_os("HEY_BOSS_FLEET_DESIRED")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".hey-boss/fleet.json"));
        let binary = std::env::var_os("HEY_BOSS_FLEET_BINARY")
            .map(PathBuf::from)
            .unwrap_or(std::env::current_exe()?)
            .canonicalize()?;
        let path = crate::issues::database_path_for_installation(&binary)?;
        let node = crate::issues::identity::machine()?;
        drop(Store::open(&path)?);
        fs::create_dir_all(&state)?;
        let ctx = Self {
            home,
            state,
            desired,
            binary,
            path,
            node,
            stop: Arc::new(AtomicBool::new(false)),
        };
        replica::ensure_metadata(&ctx.db()?)?;
        Ok(ctx)
    }
    pub fn db(&self) -> Result<Connection> {
        let db = Connection::open(&self.path)?;
        db.busy_timeout(Duration::from_secs(10))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
        Ok(db)
    }
    pub fn actor(&self) -> Result<Actor> {
        Ok(crate::issues::identity::resolve(
            Some("human:fleet"),
            &self.node,
            &self.home,
        )?)
    }
    pub fn rpc(&self, operation: Value) -> Result<Value> {
        self.rpc_store(&mut Store::open(&self.path)?, operation)
    }
    fn rpc_store(&self, store: &mut Store, operation: Value) -> Result<Value> {
        let operation: Operation = serde_json::from_value(operation)?;
        let actor = if operation.needs_actor() {
            Some(self.actor()?)
        } else {
            None
        };
        Ok(store.execute(&Request {
            version: 1,
            project: Project {
                id: "named:Fleet".into(),
                name: "Fleet".into(),
            },
            project_override: None,
            actor,
            operation,
            request_id: None,
        })?)
    }
    pub fn workers(&self) -> Result<Vec<Value>> {
        let mut store = Store::open(&self.path)?;
        let status = self.rpc_store(&mut store, json!({"action":"workers","worker_id":null}))?;
        let mut workers = status["workers"]
            .as_array()
            .ok_or_else(|| invalid("Invalid worker overview"))?
            .clone();
        for w in &mut workers {
            let selected =
                self.rpc_store(&mut store, json!({"action":"workers","worker_id":w["id"]}))?;
            for k in ["active", "free", "eligible", "runs", "upgrading"] {
                if let Some(v) = selected.get(k) {
                    w[k] = v.clone();
                }
            }
            if let Some(pid) = w["pid"].as_u64()
                && !alive(pid as u32)
            {
                w["pid"] = Value::Null;
            }
        }
        Ok(workers)
    }
    pub fn build(&self) -> Result<String> {
        let output = Command::new(&self.binary).arg("--version").output()?;
        if !output.status.success() {
            return Err(invalid("CLI build lookup failed"));
        }
        Ok(String::from_utf8(output.stdout)?.trim().into())
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
    pub fn wait(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while !self.stopped() && Instant::now() < deadline {
            std::thread::sleep(
                Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    pub fn lock(&self, name: &str, wait: bool) -> Result<Option<Lock>> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.state.join(name))?;
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Some(Lock(file)));
            }
            let error = std::io::Error::last_os_error();
            if !matches!(error.raw_os_error(), Some(libc::EWOULDBLOCK)) {
                return Err(error.into());
            }
            if !wait {
                return Ok(None);
            }
            if self.stopped() || Instant::now() >= deadline {
                return Err("Worker lifecycle is busy; retry the same signal ID".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    pub fn inventory(&self) -> Result<Vec<Value>> {
        let config = std::env::var_os("HEY_BOSS_FLEET_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(".hey-boss/config.json"));
        let value = read_json(&config, json!({}))?;
        let mut hosts = vec![];
        for entry in value["ssh_hosts"].as_array().into_iter().flatten() {
            let entry = if entry.is_string() {
                json!({"host":entry})
            } else {
                entry.clone()
            };
            if !valid_host(entry["host"].as_str().unwrap_or("")) {
                return Err(invalid("Invalid configured SSH host"));
            }
            if entry["enabled"] != false {
                hosts.push(entry);
            }
        }
        if hosts.is_empty() {
            match fs::read_to_string(self.state.join("companion-hosts")) {
                Ok(s) => {
                    hosts = s
                        .lines()
                        .filter(|h| valid_host(h))
                        .map(|h| json!({"host":h}))
                        .collect()
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let overrides = read_json(&self.desired, json!({}))?;
        for entry in &mut hosts {
            let host = entry["host"].as_str().unwrap().to_owned();
            if let Some(m) = overrides["machines"][&host].as_object() {
                entry.as_object_mut().unwrap().extend(m.clone());
            }
        }
        Ok(hosts)
    }
}
pub(super) struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}
pub(super) fn alive(pid: u32) -> bool {
    unsafe {
        libc::kill(pid as i32, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}
pub(super) fn now() -> f64 {
    crate::issues::worker::now() as f64 / 1000.0
}
pub(super) fn id() -> Result<String> {
    Ok(crate::issues::worker::random_id()?)
}
pub(super) fn hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))[..16].into()
}
pub(super) fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-_@".contains(&c))
}
pub(super) fn read_json(path: &Path, default: Value) -> Result<Value> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.new", id()?));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        serde_json::to_writer(&mut file, value)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
pub(super) fn read_frame(reader: &mut impl std::io::BufRead) -> Result<Option<Value>> {
    let mut bytes = vec![];
    let length = reader
        .take(crate::issues::WIRE_LIMIT as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    if length == 0 {
        return Ok(None);
    }
    if length > crate::issues::WIRE_LIMIT {
        return Err(invalid("Fleet frame exceeds 16 MiB"));
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}
pub(super) fn send(writer: &mut impl Write, mut frame: Value) -> Result<()> {
    frame["version"] = json!(1);
    let bytes = serde_json::to_vec(&frame)?;
    if bytes.len() + 1 > crate::issues::WIRE_LIMIT {
        return Err(invalid("Fleet frame exceeds 16 MiB"));
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

// Keep the fingerprint identical to build.rs and the source updater. Normal
// daemon heartbeats do not spawn an interpreter just to detect source changes.
pub(super) fn source_build(source: &Path) -> Result<String> {
    fn collect(source: &Path, path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                collect(source, &entry?.path(), files)?;
            }
        } else if path.is_file() {
            files.push(path.strip_prefix(source)?.to_path_buf());
        } else {
            return Err(invalid("Missing upgrade source"));
        }
        Ok(())
    }
    let mut files = vec![];
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "src",
        "skills/hey-boss",
        "tools/upgrade_hey_boss.py",
        "tools/drain_github_issues.py",
        "hey_boss_daemon.swift",
        "package_hey_boss.swift",
        "setup_hey_boss.swift",
        "assets",
    ] {
        collect(source, &source.join(name), &mut files)?;
    }
    files.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    let mut fingerprint = 0xcbf29ce484222325u64;
    for name in files {
        let label = name.to_string_lossy();
        for byte in label
            .bytes()
            .chain([0])
            .chain(fs::read(source.join(&name))?)
            .chain([0])
        {
            fingerprint = (fingerprint ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("{fingerprint:016x}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_source_fingerprint_matches_the_cli_build() {
        assert_eq!(
            source_build(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap(),
            env!("HEY_BOSS_BUILD_ID")
        );
    }
    #[test]
    fn bounded_frames_reject_oversize_requests_and_truncated_json() {
        let mut reader = std::io::Cursor::new(vec![b'x'; crate::issues::WIRE_LIMIT + 1]);
        assert!(read_frame(&mut reader).is_err());
        assert!(read_frame(&mut std::io::Cursor::new(b"{\n")).is_err());
        assert_eq!(
            read_frame(&mut std::io::Cursor::new(b"{\"version\":1}\n")).unwrap(),
            Some(json!({"version":1}))
        );
    }
}
