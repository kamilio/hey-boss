use super::{
    Connection, error, local,
    wire::{self, Command, Failure, Reply},
};
use rusqlite::Result;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

fn directory() -> PathBuf {
    PathBuf::from("/tmp").join(format!("hey-boss-db-{}", unsafe { libc::getuid() }))
}
fn identity(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let normalized = path.canonicalize().unwrap_or_else(|_| {
        path.parent()
            .and_then(|p| p.canonicalize().ok())
            .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_owned())
            .join(path.file_name().unwrap_or_default())
    });
    format!(
        "{:x}",
        Sha256::digest(normalized.as_os_str().as_encoded_bytes())
    )[..24]
        .to_owned()
}
pub(super) fn socket_path(path: &Path) -> PathBuf {
    directory().join(format!("{}.sock", identity(path)))
}
fn secure_directory() -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(directory()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(error(e.to_string())),
    }
    let metadata = fs::symlink_metadata(directory()).map_err(|e| error(e.to_string()))?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(error(
            "Database socket directory must be private and owned by the current user",
        ));
    }
    Ok(())
}

fn prepare_parent(path: &Path) -> Result<()> {
    // Resolve the same canonical directory before choosing either lock name,
    // including first use below an existing symlinked parent directory.
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| error(e.to_string()))?;
    }
    Ok(())
}

pub struct Owner {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Owner {
    /// Nonblocking election: another existing service may already host the owner.
    pub fn start(path: &Path) -> Result<Option<Self>> {
        prepare_parent(path)?;
        secure_directory()?;
        let lock_path = directory().join(format!("{}.lock", identity(path)));
        crate::issues::planning::protect_database_paths(path, [&lock_path])
            .map_err(|e| error(e.to_string()))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
            .map_err(|e| error(e.to_string()))?;
        if lock.metadata().map_err(|e| error(e.to_string()))?.nlink() != 1 {
            return Err(error("Database owner lock must not have hard links"));
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = std::io::Error::last_os_error();
            return if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Ok(None)
            } else {
                Err(error(e.to_string()))
            };
        }
        let writer = local(|| crate::issues::Store::open(path))
            .map_err(|e| {
                eprintln!("database startup: {}", serde_json::to_string(&e).unwrap());
                rusqlite::Error::ToSqlConversionFailure(Box::new(e))
            })?
            .into_database()
            .into_local();
        let path = path.canonicalize().map_err(|e| error(e.to_string()))?;
        let socket = socket_path(&path);
        match fs::symlink_metadata(&socket) {
            Ok(metadata)
                if metadata.file_type().is_socket()
                    && metadata.uid() == unsafe { libc::getuid() } =>
            {
                fs::remove_file(&socket).map_err(|e| error(e.to_string()))?
            }
            Ok(_) => return Err(error("Database socket path must be a user-owned socket")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(e.to_string())),
        }
        let listener = UnixListener::bind(&socket).map_err(|e| error(e.to_string()))?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
            .map_err(|e| error(e.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| error(e.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("sqlite-owner".into())
            .spawn(move || serve(listener, socket, path, writer, lock, stopping))
            .map_err(|e| error(e.to_string()))?;
        Ok(Some(Self {
            stop,
            thread: Some(thread),
        }))
    }
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(
    listener: UnixListener,
    socket: PathBuf,
    path: PathBuf,
    writer: rusqlite::Connection,
    lock: File,
    stop: Arc<AtomicBool>,
) {
    let generation = Arc::new(AtomicI64::new(
        writer
            .pragma_query_value(None, "schema_version", |r| r.get(0))
            .unwrap_or(-1),
    ));
    let writer = Arc::new(Writer {
        db: Mutex::new(writer),
        waiters: Mutex::new(VecDeque::new()),
        ready: Condvar::new(),
        next: AtomicUsize::new(0),
    });
    let clients = Arc::new(AtomicUsize::new(0));
    let mut sessions = Vec::new();
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if clients.load(Ordering::Acquire) >= 128 {
                    drop(stream);
                    continue;
                }
                clients.fetch_add(1, Ordering::AcqRel);
                let path = path.clone();
                let writer = writer.clone();
                let stop = stop.clone();
                let clients = clients.clone();
                let generation = generation.clone();
                sessions.push(std::thread::spawn(move || {
                    let _ = session(stream, &path, &writer, &generation, &stop);
                    clients.fetch_sub(1, Ordering::AcqRel);
                }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let mut fd = libc::pollfd {
                    fd: listener.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                unsafe {
                    libc::poll(&mut fd, 1, 250);
                }
            }
            Err(e) => {
                eprintln!("Database owner listener: {e}");
                break;
            }
        }
        sessions.retain(|thread| !thread.is_finished());
    }
    drop(listener);
    for thread in sessions {
        let _ = thread.join();
    }
    let _ = fs::remove_file(socket);
    drop(writer);
    drop(lock);
}

struct Writer {
    db: Mutex<rusqlite::Connection>,
    waiters: Mutex<VecDeque<usize>>,
    ready: Condvar,
    next: AtomicUsize,
}
struct Lease<'a>(Option<MutexGuard<'a, rusqlite::Connection>>, &'a Condvar);
impl std::ops::Deref for Lease<'_> {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &Self::Target {
        self.0.as_deref().unwrap()
    }
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(db) = self.0.take() {
            if !db.is_autocommit() {
                let _ = db.execute_batch("ROLLBACK");
            }
            let _ = db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;");
            drop(db);
        }
        self.1.notify_all();
    }
}
fn acquire<'a>(
    writer: &'a Writer,
    stream: &UnixStream,
    stop: &AtomicBool,
) -> std::io::Result<Lease<'a>> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let ticket = writer.next.fetch_add(1, Ordering::Relaxed);
    let mut waiters = writer.waiters.lock().unwrap();
    waiters.push_back(ticket);
    loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline || disconnected(stream) {
            waiters.retain(|id| *id != ticket);
            writer.ready.notify_all();
            return Err(std::io::Error::other("Database writer wait cancelled"));
        }
        if waiters.front() == Some(&ticket) {
            match writer.db.try_lock() {
                Ok(guard) => {
                    waiters.pop_front();
                    return Ok(Lease(Some(guard), &writer.ready));
                }
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    waiters.pop_front();
                    writer.ready.notify_all();
                    return Err(std::io::Error::other(
                        "Database writer failed; service restart required",
                    ));
                }
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
        }
        waiters = writer
            .ready
            .wait_timeout(waiters, Duration::from_millis(50))
            .unwrap()
            .0;
    }
}
pub(super) fn disconnected(stream: &UnixStream) -> bool {
    let mut byte = 0u8;
    let n = unsafe {
        libc::recv(
            stream.as_raw_fd(),
            (&mut byte as *mut u8).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    n == 0
        || n < 0
            && !matches!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EAGAIN | libc::EINTR)
            )
}
fn session(
    stream: UnixStream,
    path: &Path,
    writer: &Writer,
    generation: &AtomicI64,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    let reader = local(|| crate::issues::Store::open_read_connection(path))
        .map_err(std::io::Error::other)?
        .into_local();
    reader
        .busy_timeout(Duration::from_secs(1))
        .map_err(std::io::Error::other)?;
    reader
        .execute_batch("PRAGMA foreign_keys=ON")
        .map_err(std::io::Error::other)?;
    let mut input = BufReader::new(stream);
    let mut lease: Option<Lease<'_>> = None;
    let mut last_id = 0;
    let mut last_request = Instant::now();
    let mut pinned = false;
    let mut foreign_keys = true;
    loop {
        if stop.load(Ordering::Acquire) && lease.is_none() && reader.is_autocommit()
            || (lease.is_some() || !reader.is_autocommit())
                && last_request.elapsed() > Duration::from_secs(30)
        {
            break;
        }
        match input.fill_buf() {
            Ok([]) => break,
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        }
        input
            .get_mut()
            .set_read_timeout(Some(Duration::from_secs(15)))?;
        let command = match wire::read::<Command>(&mut input) {
            Ok(Some(command)) => command,
            Ok(None) => break,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        input
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(250)))?;
        last_request = Instant::now();
        let repair = matches!(command, Command::CheckSchema)
            && reader
                .pragma_query_value(None, "schema_version", |r| r.get::<_, i64>(0))
                .map_err(std::io::Error::other)?
                != generation.load(Ordering::Acquire)
            && reader
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .map_err(std::io::Error::other)?
                <= crate::issues::Store::schema_version();
        let needs_writer = match &command {
            Command::CheckSchema => repair,
            Command::ExclusiveSession => true,
            Command::Batch { .. } | Command::Execute { .. } => true,
            Command::Query { sql, .. } if lease.is_none() => reader
                .prepare(sql)
                .map(|stmt| !stmt.readonly() || stmt.column_count() == 0)
                .unwrap_or(false),
            _ => false,
        };
        if lease.is_none() && reader.is_autocommit() && needs_writer {
            lease = match acquire(writer, input.get_ref(), stop) {
                Ok(lease) => Some(lease),
                Err(_) => {
                    let reply = Reply { version: wire::VERSION, pid: std::process::id(), error: Some(Failure::capture(rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), Some("Database writer unavailable before execution; retry after service recovery".into())))), ..Reply::default() };
                    wire::write(input.get_mut(), &reply)?;
                    continue;
                }
            };
            lease
                .as_ref()
                .unwrap()
                .pragma_update(None, "foreign_keys", foreign_keys)
                .map_err(std::io::Error::other)?;
            unsafe {
                rusqlite::ffi::sqlite3_set_last_insert_rowid(
                    lease.as_ref().unwrap().handle(),
                    last_id,
                );
            }
        }
        if matches!(command, Command::ExclusiveSession) {
            pinned = true;
        }
        let db = lease.as_deref().unwrap_or(&reader);
        let mut reply = Reply {
            version: wire::VERSION,
            pid: std::process::id(),
            last_id,
            ..Reply::default()
        };
        let result = (|| {
            if repair {
                local(|| crate::issues::Store::open(path))
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                generation.store(
                    db.pragma_query_value(None, "schema_version", |r| r.get(0))?,
                    Ordering::Release,
                );
            }
            execute(db, command, &mut reply, input.get_mut())
        })();
        reply.transaction = !db.is_autocommit();
        if lease.is_some() {
            last_id = db.last_insert_rowid();
            reply.last_id = last_id;
            foreign_keys = db
                .pragma_query_value(None, "foreign_keys", |r| r.get(0))
                .map_err(std::io::Error::other)?;
        }
        if let Err(e) = result {
            reply.error = Some(Failure::capture(e));
        }
        reply.more = false;
        if !reply.transaction && !pinned {
            lease = None;
        }
        wire::write(input.get_mut(), &reply)?;
    }
    Ok(())
}
fn execute(
    db: &rusqlite::Connection,
    command: Command,
    reply: &mut Reply,
    output: &mut UnixStream,
) -> Result<()> {
    match command {
        Command::Hello | Command::ExclusiveSession => {}
        Command::CheckSchema => {
            reply.application = db.pragma_query_value(None, "application_id", |r| r.get(0))?;
            reply.schema = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        }
        Command::ReadTransaction => {
            db.execute_batch("BEGIN DEFERRED")?;
        }
        Command::Prepare { sql } => {
            let stmt = db.prepare(&sql)?;
            reply.parameters = stmt.parameter_count();
            reply.columns = stmt.column_names().into_iter().map(str::to_owned).collect();
        }
        Command::Execute { sql, values } => {
            reply.changes = db.execute(&sql, rusqlite::params_from_iter(values))?;
        }
        Command::Batch { sql } => {
            db.execute_batch(&sql)?;
        }
        Command::Backup { path } => {
            crate::issues::planning::protect_database_paths(Path::new(db.path().unwrap()), [&path])
                .map_err(|e| error(e.to_string()))?;
            db.backup("main", path, None)?;
        }
        Command::Query { sql, values } => {
            let mut stmt = db.prepare(&sql)?;
            reply.columns = stmt.column_names().into_iter().map(str::to_owned).collect();
            let mut cursor = stmt.query(rusqlite::params_from_iter(values))?;
            let mut bytes = 0usize;
            while let Some(row) = cursor.next()? {
                let values = (0..reply.columns.len())
                    .map(|i| {
                        row.get::<_, rusqlite::types::Value>(i)
                            .map(wire::SqlValue::from)
                    })
                    .collect::<Result<Vec<_>>>()?;
                bytes += values
                    .iter()
                    .map(|v| match v {
                        wire::SqlValue::Text(s) => s.len().saturating_mul(6) + 16,
                        wire::SqlValue::Blob(b) => b.len().saturating_mul(4) + 16,
                        _ => 32,
                    })
                    .sum::<usize>();
                reply.rows.push(values);
                if bytes >= 256 * 1024 {
                    reply.more = true;
                    wire::write(output, reply).map_err(|e| error(e.to_string()))?;
                    reply.rows.clear();
                    bytes = 0;
                }
            }
            drop(cursor);
            reply.more = false;
            reply.steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
        }
    }
    Ok(())
}

/// Start an already-installed service when its socket is temporarily unavailable.
/// Bare CLI installations bootstrap the existing companion daemon automatically.
pub(crate) fn ensure(path: &Path) -> Result<()> {
    if Connection::connect(path).is_ok() {
        return Ok(());
    }
    prepare_parent(path)?;
    secure_directory()?;
    let startup_path = directory().join(format!("{}.startup", identity(path)));
    crate::issues::planning::protect_database_paths(path, [&startup_path])
        .map_err(|e| error(e.to_string()))?;
    let startup = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(startup_path)
        .map_err(|e| error(e.to_string()))?;
    if unsafe { libc::flock(startup.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(error(std::io::Error::last_os_error().to_string()));
    }
    if Connection::connect(path).is_ok() {
        return Ok(());
    }
    let service_started = start_installed_service(path);
    if service_started {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if Connection::connect(path).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let binary = std::env::current_exe().map_err(|e| error(e.to_string()))?;
    let log_path = directory().join(format!("{}.log", identity(path)));
    crate::issues::planning::protect_database_paths(path, [&log_path])
        .map_err(|e| error(e.to_string()))?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&log_path)
        .map_err(|e| error(e.to_string()))?;
    let log_start = log.metadata().map_err(|e| error(e.to_string()))?.len();
    let mut command = std::process::Command::new(binary);
    // This daemon already exists in hey-boss. No additional service or settings.
    command
        .args(["fleet", "companion"])
        .env("HEY_BOSS_ISSUE_DB", path)
        .env(
            "HEY_BOSS_FLEET_STATE",
            path.parent()
                .unwrap_or(Path::new("."))
                .join("database-service")
                .join(identity(path)),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|e| error(e.to_string()))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if Connection::connect(path).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(|e| error(e.to_string()))? {
            let mut tail = String::new();
            if let Ok(mut log) = File::open(&log_path) {
                let end = log.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = log.seek(std::io::SeekFrom::Start(
                    log_start.max(end.saturating_sub(16384)),
                ));
                let _ = log.read_to_string(&mut tail);
            }
            if let Some(domain) = tail.lines().rev().find_map(|line| {
                line.strip_prefix("database startup: ")
                    .and_then(|json| serde_json::from_str::<crate::issues::Error>(json).ok())
            }) {
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(domain)));
            }
            return Err(error(format!(
                "Database service exited during startup ({status}): {}",
                tail.trim()
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(error("Database service did not become ready"))
}

fn start_installed_service(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let installed = std::env::current_exe()
        .ok()
        .and_then(|binary| fs::read_to_string(binary.with_file_name("hey-boss.state")).ok())
        .map(|root| PathBuf::from(root.trim()).join("issues.db"));
    let expected = installed.unwrap_or_else(|| home.join(".local/share/hey-boss/issues.db"));
    if identity(&expected) != identity(path) {
        return false;
    }
    for role in ["controller", "agent"] {
        #[cfg(target_os = "macos")]
        {
            let label = format!("local.hey-boss-fleet-{role}");
            if home
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist"))
                .is_file()
            {
                return std::process::Command::new("launchctl")
                    .args([
                        "kickstart",
                        &format!("gui/{}/{label}", unsafe { libc::getuid() }),
                    ])
                    .output()
                    .is_ok_and(|o| o.status.success());
            }
        }
        #[cfg(target_os = "linux")]
        {
            let name = format!("hey-boss-fleet-{role}.service");
            if home.join(".config/systemd/user").join(&name).is_file() {
                return std::process::Command::new("systemctl")
                    .args(["--user", "start", &name])
                    .output()
                    .is_ok_and(|o| o.status.success());
            }
        }
    }
    false
}
