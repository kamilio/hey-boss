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
        let db = Store::open_connection(&self.path)?;
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
        let store = Store::open(&self.path)?;
        let mut workers = store.fleet_workers()?;
        for w in &mut workers {
            if let Some(pid) = w["pid"].as_u64()
                && !alive(pid as u32)
            {
                w["pid"] = Value::Null;
                w["build"] = Value::Null;
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
    pub fn running_build() -> &'static str {
        concat!(
            "hey-boss ",
            env!("CARGO_PKG_VERSION"),
            " (build ",
            env!("HEY_BOSS_BUILD_ID"),
            ")"
        )
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
        let path = self.state.join(name);
        self.protect_file(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?;
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
    pub fn protect_file(&self, path: &Path) -> Result<()> {
        Ok(crate::issues::planning::protect_database_paths(
            &self.path,
            [path],
        )?)
    }
    pub fn read_json(&self, path: &Path, default: Value) -> Result<Value> {
        self.protect_file(path)?;
        read_json(path, default)
    }
    pub fn atomic_json(&self, path: &Path, value: &Value) -> Result<()> {
        self.protect_file(path)?;
        atomic_json(path, value)
    }
    pub fn inventory(&self) -> Result<Vec<Value>> {
        let config = std::env::var_os("HEY_BOSS_FLEET_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(".hey-boss/config.json"));
        let value = self.read_json(&config, json!({}))?;
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
            let path = self.state.join("companion-hosts");
            self.protect_file(&path)?;
            match fs::read_to_string(path) {
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
        let overrides = self.read_json(&self.desired, json!({}))?;
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
struct FrameBuffer {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl FrameBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: vec![],
            limit,
            exceeded: false,
        }
    }
}
impl Write for FrameBuffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "JSON byte limit exceeded",
            ));
        }
        let required = self.bytes.len() + data.len();
        if required > self.bytes.capacity() {
            // Keep ordinary geometric growth, without doubling past the quota.
            let capacity = required
                .max(self.bytes.capacity().saturating_mul(2))
                .max(8)
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
// None means the EOF-delimited response exceeds the encoded-byte budget.
pub(in crate::fleet) fn read_control_body(
    reader: &mut impl Read,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut frame = FrameBuffer::new(crate::issues::WIRE_LIMIT);
    // Larger socket reads avoid thousands of syscalls for a near-limit body.
    let mut chunk = [0; 65536];
    loop {
        // Preserve the old limit+1 maximum read, including the final EOF probe.
        let remaining = frame.limit - frame.bytes.len();
        let length = chunk.len().min(remaining + 1);
        let length = match reader.read(&mut chunk[..length]) {
            Ok(length) => length,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if length == 0 {
            return Ok(Some(frame.bytes));
        }
        if length > remaining {
            return Ok(None);
        }
        frame.write_all(&chunk[..length])?;
    }
}
pub(super) fn read_frame(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut frame = FrameBuffer::new(crate::issues::WIRE_LIMIT);
    loop {
        let data = match reader.fill_buf() {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if data.is_empty() {
            break;
        }
        // Inspect at most the remaining budget plus one overflow byte, matching
        // the previous bounded read. Leave prefetched subsequent frames intact.
        let data = &data[..data.len().min(frame.limit - frame.bytes.len() + 1)];
        let newline = memchr::memchr(b'\n', data);
        let length = newline.map_or(data.len(), |position| position + 1);
        frame
            .write_all(&data[..length])
            .map_err(|_| invalid("Fleet frame exceeds 16 MiB"))?;
        reader.consume(length);
        if newline.is_some() {
            break;
        }
    }
    if frame.bytes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::from_slice(&frame.bytes)?))
    }
}
// None means the JSON body exceeds its byte budget. SSH reserves one byte for
// the newline; local control responses are delimited by EOF instead.
pub(super) fn encode_frame(frame: &Value, limit: usize) -> Result<Option<Vec<u8>>> {
    let mut buffer = FrameBuffer::new(limit);
    // Match serde_json's ordinary small-frame allocation; reads stay lazy.
    buffer.bytes.reserve_exact(limit.min(128));
    let result = serde_json::to_writer(&mut buffer, frame);
    if buffer.exceeded {
        Ok(None)
    } else {
        result?;
        Ok(Some(buffer.bytes))
    }
}
pub(super) fn send(writer: &mut impl Write, mut frame: Value) -> Result<()> {
    frame["version"] = json!(1);
    let bytes = encode_frame(&frame, crate::issues::WIRE_LIMIT - 1)?
        .ok_or_else(|| invalid("Fleet frame exceeds 16 MiB"))?;
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

// Keep the fingerprint identical to build.rs and the source updater. Normal
// daemon heartbeats do not spawn an interpreter just to detect source changes.
#[cfg(test)]
fn source_build(source: &Path) -> Result<String> {
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
// Read committed blobs in one Git batch; cache by immutable tree commit rather
// than re-reading the checkout on each supervisor heartbeat.
pub(super) fn development_install_active(
    state: &Path,
    source: &Path,
    installed: &str,
) -> Result<bool> {
    let receipt = read_json(&state.join("upgrade-receipt.json"), Value::Null)?;
    let provenance = &receipt["source"];
    if provenance["kind"] != "development"
        || !provenance["build"]
            .as_str()
            .is_some_and(|build| installed.contains(&format!("build {build})")))
    {
        return Ok(false);
    }
    let Some(commit) = provenance["commit"].as_str() else {
        return Ok(true);
    };
    let result = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "refs/heads/main"])
        .output()?;
    Ok(result.status.success() && String::from_utf8_lossy(&result.stdout).trim() == commit)
}

pub(super) fn committed_source_build(source: &Path) -> Result<String> {
    static CACHE: std::sync::Mutex<Option<(PathBuf, String, String)>> = std::sync::Mutex::new(None);
    fn git(source: &Path, args: &[&str]) -> Result<Vec<u8>> {
        let result = Command::new("git")
            .arg("-C")
            .arg(source)
            .args(args)
            .output()?;
        if !result.status.success() {
            return Err(invalid(&String::from_utf8_lossy(&result.stderr)));
        }
        Ok(result.stdout)
    }
    let commit = String::from_utf8(git(source, &["rev-parse", "refs/heads/main"])?)?
        .trim()
        .to_owned();
    let mut cache = CACHE.lock().unwrap();
    if let Some((path, cached_commit, build)) = &*cache
        && path == source
        && cached_commit == &commit
    {
        return Ok(build.clone());
    }
    let inputs = [
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
    ];
    let mut args = vec!["ls-tree", "-r", "--name-only", &commit, "--"];
    args.extend(inputs);
    let listing = String::from_utf8(git(source, &args)?)?;
    let mut names: Vec<_> = listing.lines().collect();
    names.sort();
    for input in inputs {
        if !names
            .iter()
            .any(|name| *name == input || name.starts_with(&format!("{input}/")))
        {
            return Err(invalid("Committed main is missing upgrade inputs"));
        }
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["cat-file", "--batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let requests = names
        .iter()
        .map(|name| format!("{commit}:{name}\n"))
        .collect::<String>();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(requests.as_bytes()));
    let result = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| invalid("Git batch writer panicked"))??;
    if !result.status.success() {
        return Err(invalid("Cannot read committed build inputs"));
    }
    let mut data = std::io::Cursor::new(result.stdout);
    let mut fingerprint = 0xcbf29ce484222325u64;
    for name in names {
        let mut header = String::new();
        data.read_line(&mut header)?;
        let parts: Vec<_> = header.split_whitespace().collect();
        if parts.len() != 3 || parts[1] != "blob" {
            return Err(invalid("Invalid committed input"));
        }
        let length: u64 = parts[2].parse()?;
        for byte in name.bytes().chain([0]) {
            fingerprint = (fingerprint ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        let mut remaining = length;
        let mut buffer = [0u8; 8192];
        while remaining > 0 {
            let size = remaining.min(buffer.len() as u64) as usize;
            data.read_exact(&mut buffer[..size])?;
            for byte in &buffer[..size] {
                fingerprint = (fingerprint ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
            }
            remaining -= size as u64;
        }
        fingerprint = fingerprint.wrapping_mul(0x100000001b3);
        let mut newline = [0];
        data.read_exact(&mut newline)?;
        if newline != *b"\n" {
            return Err(invalid("Truncated committed input"));
        }
    }
    let build = format!("{fingerprint:016x}");
    *cache = Some((source.to_owned(), commit, build.clone()));
    Ok(build)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::BufReader;
    #[test]
    fn sqlite_lock_probe() {
        let Some(path) = std::env::var_os("HEY_BOSS_FLEET_LOCK_PROBE_DB") else {
            return;
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK as _;
        lock.l_whence = libc::SEEK_SET as _;
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) },
            -1,
            "Fleet auxiliary file close released another SQLite connection's locks"
        );
        assert!(matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EACCES | libc::EAGAIN)
        ));
    }
    pub(in crate::fleet::native) fn assert_sqlite_locked(path: &Path) {
        let probe = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "fleet::native::context::tests::sqlite_lock_probe",
                "--nocapture",
            ])
            .env("HEY_BOSS_FLEET_LOCK_PROBE_DB", path)
            .output()
            .unwrap();
        assert!(
            probe.status.success(),
            "{}{}",
            String::from_utf8_lossy(&probe.stdout),
            String::from_utf8_lossy(&probe.stderr)
        );
    }
    pub(in crate::fleet::native) fn test_context() -> (PathBuf, Context, Store) {
        let root =
            std::env::temp_dir().join(format!("hey-boss-fleet-lock-alias-{}", id().unwrap()));
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        let path = root.join("issues.db");
        let store = Store::open(&path).unwrap();
        let ctx = Context {
            home: root.clone(),
            state,
            desired: root.join("fleet.json"),
            binary: std::env::current_exe().unwrap(),
            path: path.clone(),
            node: "test".into(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        (root, ctx, store)
    }
    #[test]
    fn direct_fleet_connections_reject_database_name_aliases() {
        let (root, mut ctx, store) = test_context();
        let original = ctx.path.clone();
        let alias = root.join("second.db");
        fs::hard_link(&original, &alias).unwrap();
        ctx.path = alias.clone();
        let error = ctx
            .db()
            .expect_err("Fleet accepted hard-linked database name");
        assert!(error.to_string().contains("hard link"), "{error}");
        assert!(!root.join("second.db-wal").exists());
        assert!(!root.join("second.db-shm").exists());
        assert_sqlite_locked(&original);
        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&original, &alias).unwrap();
        assert!(ctx.db().is_err(), "Fleet accepted symbolic database name");
        fs::remove_file(alias).unwrap();
        ctx.path = root.join("missing.db");
        assert!(
            ctx.db().is_err(),
            "Fleet silently recreated a missing database"
        );
        assert!(!ctx.path.exists());
        assert_sqlite_locked(&original);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fleet_json_reads_reject_aliases_without_releasing_sqlite_locks() {
        let (root, ctx, store) = test_context();
        let alias = ctx.state.join("config.json");
        for suffix in ["", "-wal", "-shm"] {
            for symbolic in [false, true] {
                let target = root.join(format!("issues.db{suffix}"));
                if symbolic {
                    std::os::unix::fs::symlink(&target, &alias).unwrap();
                } else {
                    fs::hard_link(&target, &alias).unwrap();
                }
                let result = ctx.read_json(&alias, json!({}));
                assert_sqlite_locked(&ctx.path);
                assert!(result.unwrap_err().to_string().contains("must not alias"));
                fs::remove_file(&alias).unwrap();
            }
        }
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fleet_json_writes_cannot_replace_the_live_database_inode() {
        use std::os::unix::fs::MetadataExt;
        let (root, ctx, store) = test_context();
        let before = fs::metadata(&ctx.path).unwrap().ino();
        let result = ctx.atomic_json(&ctx.path, &json!({"workers":[]}));
        assert_eq!(
            fs::metadata(&ctx.path).unwrap().ino(),
            before,
            "Configuration replaced the live database inode"
        );
        assert!(result.unwrap_err().to_string().contains("must not alias"));
        assert_sqlite_locked(&ctx.path);
        let value = json!({"workers":[],"future":{"text":"é\n"}});
        ctx.atomic_json(&ctx.desired, &value).unwrap();
        assert_eq!(ctx.read_json(&ctx.desired, Value::Null).unwrap(), value);
        let link = ctx.state.join("ordinary-config.json");
        std::os::unix::fs::symlink(&ctx.desired, &link).unwrap();
        assert_eq!(ctx.read_json(&link, Value::Null).unwrap(), value);
        assert_eq!(
            ctx.read_json(&ctx.state.join("missing.json"), json!({"default":true}))
                .unwrap(),
            json!({"default":true})
        );
        assert_sqlite_locked(&ctx.path);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fleet_lock_aliases_are_rejected_without_releasing_sqlite_locks() {
        let (root, ctx, store) = test_context();
        let path = ctx.path.clone();
        let lock = ctx.state.join("fleet-worker-control.lock");
        for suffix in ["", "-wal", "-shm"] {
            for symbolic in [false, true] {
                let target = root.join(format!("issues.db{suffix}"));
                if symbolic {
                    std::os::unix::fs::symlink(&target, &lock).unwrap();
                } else {
                    fs::hard_link(&target, &lock).unwrap();
                }
                let result = ctx.lock("fleet-worker-control.lock", false);
                let rejected = result.is_err();
                drop(result);
                assert_sqlite_locked(&path);
                assert!(
                    rejected,
                    "Fleet lock accepted database alias {suffix}, symbolic={symbolic}"
                );
                fs::remove_file(&lock).unwrap();
            }
        }
        let held = ctx
            .lock("fleet-worker-control.lock", false)
            .unwrap()
            .unwrap();
        assert!(
            ctx.lock("fleet-worker-control.lock", false)
                .unwrap()
                .is_none()
        );
        drop(held);
        drop(
            ctx.lock("fleet-worker-control.lock", false)
                .unwrap()
                .unwrap(),
        );
        assert_sqlite_locked(&path);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    struct Fragmented<R>(R);
    impl<R: Read> Read for Fragmented<R> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let length = bytes.len().min(8192);
            self.0.read(&mut bytes[..length])
        }
    }
    #[test]
    fn fragmented_eof_control_body_keeps_buffer_within_its_byte_budget() {
        let limit = crate::issues::WIRE_LIMIT;
        let mut body = vec![b'x'; limit];
        body[0] = b'"';
        body[limit - 1] = b'"';
        let mut reader = Fragmented(std::io::Cursor::new(body));
        let (result, allocated) =
            crate::test_allocations::measure(|| read_control_body(&mut reader));
        let bytes = result.unwrap().unwrap();
        eprintln!("exact-limit EOF response: {allocated} requested Rust allocation bytes");
        assert_eq!(bytes.len(), limit);
        assert!(bytes.capacity() <= limit, "capacity {}", bytes.capacity());
        assert!(allocated < 40 * 1024 * 1024, "{allocated} allocation bytes");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)
                .unwrap()
                .as_str()
                .unwrap()
                .len(),
            limit - 2
        );
    }
    #[test]
    fn oversized_eof_control_body_rejects_without_growing_past_the_quota() {
        let mut reader = Fragmented(std::io::Cursor::new(vec![
            b'x';
            crate::issues::WIRE_LIMIT + 1
        ]));
        let (result, allocated) =
            crate::test_allocations::measure(|| read_control_body(&mut reader));
        eprintln!("oversized EOF response: {allocated} requested Rust allocation bytes");
        assert!(result.unwrap().is_none());
        assert!(allocated < 40 * 1024 * 1024, "{allocated} allocation bytes");
    }
    #[test]
    fn eof_control_body_preserves_complete_bytes_and_lazy_empty_input() {
        let value = json!({"text":"é\n\\\"","version":1});
        let bytes = format!("{}\n\n", serde_json::to_string_pretty(&value).unwrap());
        let actual = read_control_body(&mut std::io::Cursor::new(bytes.as_bytes()))
            .unwrap()
            .unwrap();
        assert_eq!(actual, bytes.as_bytes());
        assert_eq!(serde_json::from_slice::<Value>(&actual).unwrap(), value);
        let (empty, allocated) =
            crate::test_allocations::measure(|| read_control_body(&mut std::io::empty()));
        assert!(empty.unwrap().unwrap().is_empty());
        assert_eq!(allocated, 0);
    }
    #[test]
    fn eof_control_body_retries_interrupted_and_propagates_partial_input_errors() {
        struct Input {
            interrupted: bool,
            bytes: std::io::Cursor<&'static [u8]>,
            final_error: bool,
        }
        impl Read for Input {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                if std::mem::take(&mut self.interrupted) {
                    Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
                } else if self.bytes.position() == self.bytes.get_ref().len() as u64
                    && self.final_error
                {
                    Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
                } else {
                    self.bytes.read(output)
                }
            }
        }
        for final_error in [false, true] {
            let mut input = Input {
                interrupted: true,
                bytes: std::io::Cursor::new(b"{\"version\":1}"),
                final_error,
            };
            let result = read_control_body(&mut input);
            if final_error {
                assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
            } else {
                assert_eq!(result.unwrap().unwrap(), b"{\"version\":1}");
            }
        }
    }
    #[test]
    #[ignore = "Profiles EOF control reads against the previous Vec read_to_end"]
    fn profile_eof_control_reads() {
        fn receive(size: usize, bounded: bool) -> Duration {
            let (mut reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
            reader
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            writer
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let producing = std::thread::spawn(move || {
                let chunk = [b'x'; 65536];
                let mut remaining = size;
                while remaining != 0 {
                    let length = remaining.min(chunk.len());
                    writer.write_all(&chunk[..length]).unwrap();
                    remaining -= length;
                }
            });
            let started = Instant::now();
            let bytes = if bounded {
                read_control_body(&mut reader).unwrap().unwrap()
            } else {
                let mut bytes = vec![];
                reader
                    .take(crate::issues::WIRE_LIMIT as u64 + 1)
                    .read_to_end(&mut bytes)
                    .unwrap();
                bytes
            };
            let elapsed = started.elapsed();
            assert_eq!(bytes.len(), size);
            assert!(bytes.iter().all(|byte| *byte == b'x'));
            producing.join().unwrap();
            elapsed
        }
        for size in [128, 512 * 1024, crate::issues::WIRE_LIMIT] {
            let mut old = vec![];
            let mut bounded = vec![];
            for _ in 0..9 {
                old.push(receive(size, false));
                bounded.push(receive(size, true));
            }
            old.sort();
            bounded.sort();
            eprintln!(
                "EOF socket {size} bytes: old median {:?}, bounded median {:?}",
                old[4], bounded[4]
            );
        }
    }
    #[test]
    fn fragmented_oversized_frame_reading_has_bounded_allocation_work() {
        let mut reader = BufReader::with_capacity(
            8192,
            std::io::Cursor::new(vec![b'x'; crate::issues::WIRE_LIMIT + 1]),
        );
        let (result, allocated) = crate::test_allocations::measure(|| read_frame(&mut reader));
        eprintln!("oversized fragmented frame read: {allocated} requested Rust allocation bytes");
        assert!(result.unwrap_err().to_string().contains("exceeds 16 MiB"));
        assert!(allocated < 40 * 1024 * 1024, "{allocated} allocation bytes");
    }
    #[test]
    fn fragmented_frame_reading_preserves_utf8_next_frames_and_eof() {
        let first = json!({"version":1,"text":"é\n\"\\"});
        let second = json!({"version":1,"kind":"heartbeat"});
        let bytes = format!("{first}\n{second}");
        for capacity in [1, 7, 8192] {
            let mut reader =
                BufReader::with_capacity(capacity, std::io::Cursor::new(bytes.as_bytes()));
            assert_eq!(read_frame(&mut reader).unwrap(), Some(first.clone()));
            assert_eq!(read_frame(&mut reader).unwrap(), Some(second.clone()));
            assert_eq!(read_frame(&mut reader).unwrap(), None);
        }
    }
    #[test]
    fn frame_reading_counts_newline_in_exact_byte_limit() {
        let limit = crate::issues::WIRE_LIMIT;
        let text = "x".repeat(limit - 3);
        let bytes = format!("\"{text}\"\n");
        assert_eq!(bytes.len(), limit);
        let mut reader = BufReader::new(std::io::Cursor::new(bytes));
        assert_eq!(read_frame(&mut reader).unwrap(), Some(json!(text)));
        assert_eq!(read_frame(&mut reader).unwrap(), None);

        let text = "x".repeat(limit - 2);
        let bytes = format!("\"{text}\"");
        assert_eq!(bytes.len(), limit);
        let mut reader = BufReader::new(std::io::Cursor::new(bytes));
        assert_eq!(read_frame(&mut reader).unwrap(), Some(json!(text)));

        let bytes = format!("\"{text}\"\n");
        let mut reader = BufReader::new(std::io::Cursor::new(bytes));
        assert!(
            read_frame(&mut reader)
                .unwrap_err()
                .to_string()
                .contains("exceeds 16 MiB")
        );
    }
    #[test]
    fn frame_reading_retries_interrupted_input_and_propagates_other_io_errors() {
        struct Input {
            error: Option<std::io::ErrorKind>,
            bytes: std::io::Cursor<&'static [u8]>,
        }
        impl Read for Input {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                if let Some(kind) = self.error.take() {
                    Err(std::io::Error::from(kind))
                } else {
                    self.bytes.read(output)
                }
            }
        }
        let mut interrupted = BufReader::new(Input {
            error: Some(std::io::ErrorKind::Interrupted),
            bytes: std::io::Cursor::new(b"{\"version\":1}\n"),
        });
        assert_eq!(
            read_frame(&mut interrupted).unwrap(),
            Some(json!({"version":1}))
        );
        let mut broken = BufReader::new(Input {
            error: Some(std::io::ErrorKind::BrokenPipe),
            bytes: std::io::Cursor::new(b""),
        });
        assert_eq!(
            read_frame(&mut broken)
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }
    #[test]
    fn oversized_frame_encoding_has_bounded_allocation_work() {
        let frame = json!({"parts": vec![json!({"text":"x".repeat(4096)});12288]});
        let (encoded, allocated) = crate::test_allocations::measure(|| {
            encode_frame(&frame, crate::issues::WIRE_LIMIT).unwrap()
        });
        eprintln!("oversized JSON encoding: {allocated} requested Rust allocation bytes");
        assert!(encoded.is_none());
        assert!(allocated < 40 * 1024 * 1024, "{allocated} allocation bytes");
    }
    #[test]
    fn oversized_string_frame_encoding_stops_before_copying_text() {
        let frame = json!({"payload":"x".repeat(crate::issues::WIRE_LIMIT + 1)});
        let (encoded, allocated) = crate::test_allocations::measure(|| {
            encode_frame(&frame, crate::issues::WIRE_LIMIT).unwrap()
        });
        eprintln!("oversized string encoding: {allocated} requested Rust allocation bytes");
        assert!(encoded.is_none());
        assert!(allocated < 1024 * 1024, "{allocated} allocation bytes");
    }
    #[test]
    fn valid_large_frame_encoding_keeps_buffer_capacity_within_its_budget() {
        let frame = json!({"payload":"x".repeat(9 * 1024 * 1024)});
        let encoded = encode_frame(&frame, crate::issues::WIRE_LIMIT)
            .unwrap()
            .unwrap();
        assert!(
            encoded.capacity() <= crate::issues::WIRE_LIMIT,
            "{} capacity bytes",
            encoded.capacity()
        );
        assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), frame);
    }
    #[test]
    fn frame_encoding_uses_encoded_byte_limits_for_both_delimiters() {
        for frame in [
            json!({"version":1,"text":"é\n\"\\"}),
            json!([true, null, 42]),
            json!({}),
        ] {
            let expected = serde_json::to_vec(&frame).unwrap();
            assert_eq!(
                encode_frame(&frame, expected.len()).unwrap(),
                Some(expected.clone())
            );
            assert!(encode_frame(&frame, expected.len() - 1).unwrap().is_none());
            assert!(encode_frame(&frame, 0).unwrap().is_none());
        }
    }
    #[test]
    fn send_never_emits_an_oversized_partial_frame_and_preserves_version_and_text() {
        let mut output = vec![];
        let error = send(
            &mut output,
            json!({"payload":"x".repeat(crate::issues::WIRE_LIMIT + 1)}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("exceeds 16 MiB"));
        assert!(output.is_empty());
        send(&mut output, json!({"version":8,"text":"é\n\"\\"})).unwrap();
        assert_eq!(output.last(), Some(&b'\n'));
        assert_eq!(
            read_frame(&mut std::io::Cursor::new(output)).unwrap(),
            Some(json!({"version":1,"text":"é\n\"\\"}))
        );
    }
    #[test]
    fn native_source_fingerprint_matches_the_cli_build() {
        assert_eq!(
            source_build(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap(),
            env!("HEY_BOSS_BUILD_ID")
        );
    }
    #[test]
    fn automatic_rollout_fingerprint_ignores_dirty_changes_and_advances_on_commit() {
        let root = std::env::temp_dir().join(format!("hey-boss-106-main-{}", id().unwrap()));
        fs::create_dir(&root).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        for name in [
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "src/file",
            "skills/hey-boss/file",
            "tools/upgrade_hey_boss.py",
            "tools/drain_github_issues.py",
            "hey_boss_daemon.swift",
            "package_hey_boss.swift",
            "setup_hey_boss.swift",
            "assets/file",
        ] {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name).unwrap();
        }
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "initial",
        ]);
        let committed = committed_source_build(&root).unwrap();
        assert_eq!(committed, source_build(&root).unwrap());
        let head = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        atomic_json(&root.join("upgrade-receipt.json"), &json!({"source":{
            "kind":"development", "commit":String::from_utf8_lossy(&head.stdout).trim(), "build":"dev-build"
        }})).unwrap();
        assert!(development_install_active(&root, &root, "hey-boss (build dev-build)").unwrap());
        assert!(!development_install_active(&root, &root, "hey-boss (build other-build)").unwrap());
        fs::write(root.join("src/file"), "dirty changes").unwrap();
        assert_eq!(committed, committed_source_build(&root).unwrap());
        assert_ne!(committed, source_build(&root).unwrap());
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "next",
        ]);
        assert_ne!(committed, committed_source_build(&root).unwrap());
        assert!(!development_install_active(&root, &root, "hey-boss (build dev-build)").unwrap());
        assert_eq!(
            committed_source_build(&root).unwrap(),
            source_build(&root).unwrap()
        );
    }
    #[test]
    #[ignore = "Profiles an explicitly supplied database through a private backup"]
    fn profile_machine_activity_poll() {
        use std::os::unix::fs::PermissionsExt;
        let source = PathBuf::from(
            std::env::var_os("HEY_BOSS_PROFILE_DB").expect("Set HEY_BOSS_PROFILE_DB"),
        );
        let root = std::env::temp_dir().join(format!(
            "hb-machine-poll-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("issues.db");
        let db = Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
        db.backup("main", &path, None).unwrap();
        drop(db);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let ctx = Context {
            home: root.clone(),
            state: root.clone(),
            desired: root.join("desired.json"),
            binary: std::env::current_exe().unwrap(),
            path,
            node: "profile".into(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let repeated_status = || {
            let mut store = Store::open(&ctx.path).unwrap();
            let status = ctx
                .rpc_store(&mut store, json!({"action":"workers","worker_id":null}))
                .unwrap();
            let mut workers = status["workers"].as_array().unwrap().clone();
            for worker in &mut workers {
                let detail = ctx
                    .rpc_store(
                        &mut store,
                        json!({"action":"workers","worker_id":worker["id"]}),
                    )
                    .unwrap();
                for key in ["active", "free", "eligible", "runs", "chiefs", "upgrading"] {
                    worker[key] = detail[key].clone();
                }
                if let Some(pid) = worker["pid"].as_u64()
                    && !alive(pid as u32)
                {
                    worker["pid"] = Value::Null;
                    worker["build"] = Value::Null;
                }
            }
            workers
        };
        let expected = repeated_status();
        assert_eq!(ctx.workers().unwrap(), expected);
        let mut old = Vec::new();
        let mut batched = Vec::new();
        for _ in 0..7 {
            let start = Instant::now();
            assert_eq!(repeated_status(), expected);
            old.push(start.elapsed());
            let start = Instant::now();
            assert_eq!(ctx.workers().unwrap(), expected);
            batched.push(start.elapsed());
        }
        old.sort();
        batched.sort();
        eprintln!(
            "{} workers: repeated public status median {:?}; coherent poll median {:?}",
            expected.len(),
            old[3],
            batched[3]
        );
        fs::remove_dir_all(root).unwrap();
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
