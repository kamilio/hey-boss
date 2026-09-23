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
        let store = Store::open(&self.path)?;
        let mut workers = store.fleet_workers()?;
        for w in &mut workers {
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
