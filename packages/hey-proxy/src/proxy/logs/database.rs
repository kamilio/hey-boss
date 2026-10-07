use super::{
    pricing,
    store::{Entry, now_ms},
};
use crate::config::Logging;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

pub struct Event {
    pub entry: Entry,
    pub kind: Option<String>,
    pub details: Value,
    pub timestamp_ms: u64,
}
enum Command {
    Event(Box<Event>),
    Flush(tokio::sync::oneshot::Sender<()>),
}
#[derive(Default)]
struct Health {
    ready: AtomicBool,
    enqueued: AtomicU64,
    committed: AtomicU64,
    pending: AtomicUsize,
    dropped: AtomicU64,
    write_errors: AtomicU64,
    last_commit_ms: AtomicU64,
    last_recorded_ms: AtomicU64,
    busy_since_ms: AtomicU64,
    last_drop_ms: AtomicU64,
    commit_duration_us: AtomicU64,
    last_error: Mutex<Option<String>>,
}
pub struct Database {
    pub path: PathBuf,
    sender: SyncSender<Command>,
    health: Arc<Health>,
    capacity: usize,
    readers: Arc<tokio::sync::Semaphore>,
    closed: Arc<AtomicBool>,
    started_ms: u64,
    worker: std::thread::Thread,
    batch_size: usize,
}

// Only pending and streaming requests lack ended_ms. Naming the states lets
// SQLite use requests_state_time instead of scanning every logged request.
pub(super) const OPEN_REQUESTS: &str = "state IN ('pending','streaming') AND ended_ms IS NULL";

impl Database {
    pub fn open(path: PathBuf, config: &Logging, session_id: &str) -> Result<Self> {
        Self::open_with_initializer(path, config, session_id, initialize)
    }

    fn open_with_initializer(
        path: PathBuf,
        config: &Logging,
        session_id: &str,
        initialize: impl FnOnce(&std::path::Path, &str, &Health) -> Result<(Connection, File)>
        + Send
        + 'static,
    ) -> Result<Self> {
        let health = Arc::new(Health::default());
        let (sender, receiver) = mpsc::sync_channel(config.queue_capacity);
        let worker_health = health.clone();
        let closed = Arc::new(AtomicBool::new(false));
        let worker_closed = closed.clone();
        let config = config.clone();
        let capacity = config.queue_capacity;
        let batch_size = config.batch_size;
        let worker_path = path.clone();
        let session_id = session_id.to_owned();
        let worker = std::thread::Builder::new()
            .name("proxy-log-writer".into())
            .spawn(move || {
                match initialize(&worker_path, &session_id, &worker_health) {
                    Ok((connection, _database_lock)) => {
                        worker_health.ready.store(true, Ordering::Release);
                        writer(connection, receiver, worker_health, config, worker_closed);
                    }
                    Err(error) => {
                        worker_health.write_errors.fetch_add(1, Ordering::Relaxed);
                        *worker_health
                            .last_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) =
                            Some(format!("Logging initialization failed: {error:#}"));
                        eprintln!(
                            "Logging initialization failed; proxy remains available: {error:#}"
                        );
                        // Keep consuming so failed persistence remains bounded and every
                        // lost event is counted. Flush senders are dropped (never acknowledged).
                        while let Ok(command) = receiver.recv() {
                            if matches!(command, Command::Event(_)) {
                                worker_health.pending.fetch_sub(1, Ordering::Relaxed);
                                worker_health.dropped.fetch_add(1, Ordering::Relaxed);
                                worker_health
                                    .last_drop_ms
                                    .store(now_ms(), Ordering::Relaxed);
                            }
                        }
                    }
                }
            })
            .context("Start logging writer")?
            .thread()
            .clone();
        Ok(Self {
            path,
            sender,
            health,
            capacity,
            readers: Arc::new(tokio::sync::Semaphore::new(2)),
            closed,
            started_ms: now_ms(),
            worker,
            batch_size,
        })
    }
    /// The forwarding path never blocks on a queue slot, SQLite, or disk.
    pub fn enqueue(&self, event: Event) {
        let pending = self.health.pending.fetch_add(1, Ordering::Relaxed) + 1;
        if pending == 1 {
            // Start a fresh busy period after idle time. Otherwise the next
            // request's lag would include the entire idle interval.
            self.health
                .busy_since_ms
                .store(event.timestamp_ms, Ordering::Relaxed);
        }
        match self.sender.try_send(Command::Event(Box::new(event))) {
            Ok(()) => {
                self.health.enqueued.fetch_add(1, Ordering::Relaxed);
                // Wake for the first event or a full batch, not every lifecycle update.
                if pending == 1 || pending.is_multiple_of(self.batch_size) {
                    self.worker.unpark();
                }
            }
            Err(_) => {
                self.health.pending.fetch_sub(1, Ordering::Relaxed);
                self.health.dropped.fetch_add(1, Ordering::Relaxed);
                self.health.last_drop_ms.store(now_ms(), Ordering::Relaxed);
            }
        }
    }
    pub async fn flush(&self) -> Result<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut command = Command::Flush(sender);
            loop {
                match self.sender.try_send(command) {
                    Ok(()) => {
                        self.worker.unpark();
                        break;
                    }
                    Err(mpsc::TrySendError::Full(returned)) => {
                        command = returned;
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => bail!("Logging writer stopped"),
                }
            }
            receiver.await.context("Logging writer stopped")
        })
        .await
        .context("Logging flush timed out")??;
        Ok(())
    }
    pub fn health(&self) -> Value {
        let h = &self.health;
        let last_commit = h.last_commit_ms.load(Ordering::Relaxed);
        let last_recorded = h.last_recorded_ms.load(Ordering::Relaxed);
        let busy_since = h.busy_since_ms.load(Ordering::Relaxed);
        let pending = h.pending.load(Ordering::Relaxed);
        let last_error = h
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        json!({"enabled":true,"storage":"sqlite","journal_mode":"WAL","synchronous":"FULL",
            "queue_capacity":self.capacity,"pending_events":pending,"enqueued_events":h.enqueued.load(Ordering::Relaxed),
            "committed_events":h.committed.load(Ordering::Relaxed),"dropped_events":h.dropped.load(Ordering::Relaxed),
            "last_drop_ms":h.last_drop_ms.load(Ordering::Relaxed),"write_errors":h.write_errors.load(Ordering::Relaxed),
            "last_error":last_error,"last_commit_ms":last_commit,"commit_duration_us":h.commit_duration_us.load(Ordering::Relaxed),
            "lag_ms":if pending > 0 {now_ms().saturating_sub(last_recorded.max(busy_since).max(self.started_ms))} else {0},
            "retention":"all","status":if last_error.is_some(){"error"}else if !h.ready.load(Ordering::Acquire){"initializing"}else if h.dropped.load(Ordering::Relaxed)>0{"gaps"}else if pending>self.capacity/2{"lagging"}else{"healthy"}})
    }
    pub async fn read<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        if !self.health.ready.load(Ordering::Acquire) {
            bail!("Logging database is initializing or unavailable; retry shortly");
        }
        let permit = self
            .readers
            .clone()
            .try_acquire_owned()
            .context("Historical reports are busy; retry shortly")?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let connection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            connection.busy_timeout(Duration::from_millis(500))?;
            connection.execute_batch(
                "PRAGMA query_only=ON; PRAGMA temp_store=FILE; PRAGMA cache_size=-4096;",
            )?;
            let started = Instant::now();
            connection.progress_handler(
                10_000,
                Some(move || started.elapsed() > Duration::from_secs(10)),
            )?;
            connection.execute_batch("BEGIN")?;
            let result = operation(&connection);
            let _ = connection.execute_batch("ROLLBACK");
            result
        })
        .await
        .context("Historical query worker stopped")?
    }
}

// All filesystem access, SQLite setup, repairs and recovery run on the writer.
fn initialize(
    path: &std::path::Path,
    session_id: &str,
    health: &Health,
) -> Result<(Connection, File)> {
    let parent = path.parent().context("Database path must have a parent")?;
    if !parent.exists() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
    }
    let private_file = |path: &std::path::Path| -> Result<File> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(file)
    };
    let lock = private_file(&path.with_extension("sqlite3.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context(
        "Logging database is already owned by another proxy; use a separate database for previews",
    )?;
    private_file(path)?;
    let mut connection = Connection::open(path).context("Open logging database")?;
    connection.busy_timeout(Duration::from_millis(250))?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA wal_autocheckpoint=1000;")?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > 1 {
        bail!("Logging database schema {version} is newer than this executable supports");
    }
    connection.execute_batch(SCHEMA)?;
    repair_completed_disconnects(&mut connection)?;
    backfill_claude_prices(&mut connection)?;
    super::dashboard::initialize(&mut connection)?;
    let transaction = connection.transaction()?;
    let timestamp = integer(now_ms());
    transaction.execute(&format!("INSERT INTO request_events(request_id,timestamp_ms,kind,details)
            SELECT request_id,?1,'interrupted','{{\"source\":\"process_restart\",\"error_code\":\"proxy_process_ended\"}}' FROM requests WHERE {OPEN_REQUESTS}"),[timestamp])?;
    transaction.execute(&format!("UPDATE requests SET state='interrupted', ended_ms=?1, updated_ms=?1,
            total_duration_ms=NULL, error_code='proxy_process_ended',
            record=json_set(record,'$.state','interrupted','$.ended_ms',?1,'$.updated_ms',?1,
                '$.total_duration_ms',NULL,'$.error_code','proxy_process_ended','$.outcome_source','process_restart')
            WHERE {OPEN_REQUESTS}"), [timestamp])?;
    transaction.execute(
        "INSERT INTO sessions(session_id,started_ms) VALUES(?1,?2)",
        params![session_id, timestamp],
    )?;
    transaction.commit()?;
    let historical_drops = connection
        .query_row(
            "SELECT value FROM metadata WHERE key='dropped_events'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    health
        .dropped
        .fetch_add(historical_drops, Ordering::Relaxed);
    Ok((connection, lock))
}

// Previously native Messages usage was retained but never priced. Only fill
// missing estimates; historical prices already recorded remain unchanged. The
// migration runs off the listener thread and holds at most 256 records in RAM.
fn backfill_claude_prices(connection: &mut Connection) -> Result<()> {
    let done: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='claude_prices_2026_10_01')",
        [],
        |r| r.get(0),
    )?;
    if done {
        return Ok(());
    }
    let mut cursor = 0i64;
    loop {
        let rows = {
            let mut query = connection.prepare("SELECT seq,record FROM requests WHERE seq>?1
                AND cost_nano_usd IS NULL AND input_tokens IS NOT NULL AND output_tokens IS NOT NULL
                AND path IN ('/v1/messages','/v1/messages/','/v1/custom/messages','/custom/v1/messages',
                    '/v1/custom/chat/completions','/custom/v1/chat/completions')
                ORDER BY seq LIMIT 256")?;
            query
                .query_map([cursor], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rows.is_empty() {
            break;
        }
        let transaction = connection.transaction()?;
        for (seq, record) in rows {
            cursor = seq;
            let Ok(entry) = serde_json::from_str::<Entry>(&record) else {
                continue;
            };
            let price = pricing::price(&entry);
            let Some(cost) = price.cost_nano_usd else {
                continue;
            };
            transaction.execute(
                "UPDATE requests SET cost_nano_usd=?2,price_model=?3,price_version=?4,
                record=json_set(record,'$.cost_nano_usd',?2,'$.price_model',?3,'$.price_version',?4,
                    '$.estimated_cost_usd',?5) WHERE seq=?1 AND cost_nano_usd IS NULL",
                params![
                    seq,
                    cost,
                    price.price_model,
                    price.price_version,
                    cost as f64 / 1e9
                ],
            )?;
        }
        transaction.commit()?;
    }
    connection.execute(
        "INSERT INTO metadata(key,value) VALUES('claude_prices_2026_10_01','1')",
        [],
    )?;
    Ok(())
}

// Version-one logging mistook Codex's close after response.completed for a
// cancelled generation. Repair only records whose terminal event proves success,
// leaving an audit event and preserving all original events, usage and costs.
fn repair_completed_disconnects(connection: &mut Connection) -> Result<()> {
    let done: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='completed_disconnect_fix_v1')",
        [],
        |r| r.get(0),
    )?;
    if done {
        return Ok(());
    }
    let transaction = connection.transaction()?;
    let condition="r.state='cancelled' AND r.error_code='client_disconnected' AND r.status<400
        AND json_extract(r.record,'$.streaming')=1
        AND (SELECT json_extract(e.details,'$.type') FROM request_events e
            WHERE e.request_id=r.request_id AND e.kind='response_event'
            AND json_extract(e.details,'$.type') IN ('response.completed','proxy.stream.done','response.failed','response.incomplete','error')
            ORDER BY e.seq DESC LIMIT 1) IN ('response.completed','proxy.stream.done')";
    transaction.execute(&format!("INSERT INTO request_events(request_id,timestamp_ms,kind,details)
        SELECT r.request_id,?1,'classification_corrected','{{\"previous_state\":\"cancelled\",\"state\":\"succeeded\",\"reason\":\"client_closed_after_completed_event\"}}' FROM requests r WHERE {condition}"),[integer(now_ms())])?;
    let corrected=transaction.execute(&format!("UPDATE requests AS r SET state='succeeded',error_code=NULL,
        record=json_set(record,'$.state','succeeded','$.error_code',NULL,'$.outcome_source','responses_event') WHERE {condition}"),[])?;
    transaction.execute(
        "INSERT INTO metadata(key,value) VALUES('completed_disconnect_fix_v1',?1)",
        [corrected.to_string()],
    )?;
    transaction.commit()?;
    if corrected > 0 {
        eprintln!(
            "Corrected {corrected} completed responses previously classified as client cancellations"
        );
    }
    Ok(())
}

impl Drop for Database {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
        self.worker.unpark();
    }
}

fn writer(
    mut connection: Connection,
    receiver: Receiver<Command>,
    health: Arc<Health>,
    config: Logging,
    closed: Arc<AtomicBool>,
) {
    let mut batch = Vec::with_capacity(config.batch_size);
    let mut flushes = Vec::new();
    let interval = Duration::from_millis(config.flush_interval_ms);
    let mut disconnected = false;
    let mut failure_since = None;
    let mut recorded_drops = health.dropped.load(Ordering::Relaxed);
    let mut deadline = Instant::now() + interval;
    loop {
        while batch.len() < config.batch_size && !disconnected && flushes.is_empty() {
            match receiver.try_recv() {
                Ok(Command::Event(event)) => batch.push(event),
                Ok(Command::Flush(sender)) => flushes.push(sender),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => disconnected = true,
            }
        }
        let drops = health.dropped.load(Ordering::Relaxed);
        if !disconnected
            && flushes.is_empty()
            && batch.len() < config.batch_size
            && recorded_drops == drops
            && (batch.is_empty() || Instant::now() < deadline)
        {
            std::thread::park_timeout(if batch.is_empty() {
                interval
            } else {
                deadline.saturating_duration_since(Instant::now())
            });
            if batch.is_empty() {
                deadline = Instant::now() + interval;
            }
            continue;
        }
        if !batch.is_empty() || recorded_drops != drops {
            let started = Instant::now();
            match commit(&mut connection, &batch, drops) {
                Ok(()) => {
                    health.pending.fetch_sub(batch.len(), Ordering::Relaxed);
                    health
                        .committed
                        .fetch_add(batch.len() as u64, Ordering::Relaxed);
                    health.last_commit_ms.store(now_ms(), Ordering::Relaxed);
                    if let Some(timestamp) = batch.iter().map(|event| event.timestamp_ms).max() {
                        health.last_recorded_ms.store(timestamp, Ordering::Relaxed);
                    }
                    health
                        .commit_duration_us
                        .store(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                    *health.last_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    failure_since = None;
                    recorded_drops = drops;
                    batch.clear();
                }
                Err(error) => {
                    health.write_errors.fetch_add(1, Ordering::Relaxed);
                    *health.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(error.to_string());
                    let since = failure_since.get_or_insert_with(Instant::now);
                    if (disconnected || closed.load(Ordering::Relaxed))
                        && since.elapsed() > Duration::from_secs(5)
                    {
                        eprintln!(
                            "Logging writer could not flush {} events: {error}",
                            batch.len()
                        );
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
        }
        for sender in flushes.drain(..) {
            let _ = sender.send(());
        }
        if disconnected {
            break;
        }
        deadline = Instant::now() + interval;
    }
    let _ = connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
}

fn commit(connection: &mut Connection, batch: &[Box<Event>], drops: u64) -> Result<()> {
    #[derive(serde::Serialize)]
    struct Record<'a> {
        #[serde(flatten)]
        entry: &'a Entry,
        #[serde(flatten)]
        price: &'a pricing::Price,
        estimated_cost_usd: Option<f64>,
    }
    let transaction = connection.transaction()?;
    {
        let mut request = transaction.prepare_cached(UPSERT)?;
        let mut event = transaction.prepare_cached(
            "INSERT INTO request_events(request_id,timestamp_ms,kind,details) VALUES(?1,?2,?3,?4)",
        )?;
        // Only the last snapshot of each request is needed, but keep every timeline event.
        let mut latest = std::collections::HashMap::new();
        for item in batch {
            latest.insert(&item.entry.request_id, item);
        }
        for item in latest.values() {
            let entry = &item.entry;
            let price = pricing::price(entry);
            let record = serde_json::to_string(&Record {
                entry,
                price: &price,
                estimated_cost_usd: price.cost_nano_usd.map(|n| n as f64 / 1e9),
            })?;
            request.execute(params![
                entry.request_id,
                entry.session_id,
                integer(entry.timestamp_ms),
                integer(entry.updated_ms),
                entry.ended_ms.map(integer),
                entry.requested_model,
                entry.routed_model,
                entry.project,
                entry.path,
                entry.method,
                entry.transport,
                entry.mode,
                entry.state,
                entry.status,
                entry.retries,
                entry.input_tokens.map(integer),
                entry.output_tokens.map(integer),
                entry.cached_input_tokens.map(integer),
                entry.cache_write_tokens.map(integer),
                entry.reasoning_tokens.map(integer),
                entry.duration_ms.map(integer),
                entry.total_duration_ms.map(integer),
                entry.first_byte_ms.map(integer),
                entry.first_output_ms.map(integer),
                integer(entry.request_bytes),
                integer(entry.response_bytes),
                entry.error_code,
                price.cost_nano_usd,
                price.price_version,
                price.price_model,
                record
            ])?;
        }
        for item in batch.iter().filter(|item| item.kind.is_some()) {
            event.execute(params![
                item.entry.request_id,
                integer(item.timestamp_ms),
                item.kind.as_deref(),
                serde_json::to_string(&item.details)?
            ])?;
        }
    }
    transaction.execute("INSERT INTO metadata(key,value) VALUES('dropped_events',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[drops.to_string()])?;
    transaction.commit()?;
    Ok(())
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS requests (
    seq INTEGER PRIMARY KEY, request_id TEXT NOT NULL UNIQUE, session_id TEXT NOT NULL,
    timestamp_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL, ended_ms INTEGER,
    requested_model TEXT, routed_model TEXT, project TEXT, path TEXT NOT NULL, method TEXT NOT NULL,
    transport TEXT NOT NULL, mode TEXT NOT NULL, state TEXT NOT NULL, status INTEGER, retries INTEGER NOT NULL,
    input_tokens INTEGER, output_tokens INTEGER, cached_input_tokens INTEGER, cache_write_tokens INTEGER,
    reasoning_tokens INTEGER, duration_ms INTEGER, total_duration_ms INTEGER, first_byte_ms INTEGER,
    first_output_ms INTEGER, request_bytes INTEGER, response_bytes INTEGER, error_code TEXT,
    cost_nano_usd INTEGER, price_version TEXT, price_model TEXT, record TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS requests_time ON requests(timestamp_ms DESC,request_id DESC);
CREATE INDEX IF NOT EXISTS requests_requested_time ON requests(requested_model,timestamp_ms);
CREATE INDEX IF NOT EXISTS requests_routed_time ON requests(routed_model,timestamp_ms);
CREATE INDEX IF NOT EXISTS requests_project_time ON requests(project,timestamp_ms);
CREATE INDEX IF NOT EXISTS requests_state_time ON requests(state,timestamp_ms);
CREATE TABLE IF NOT EXISTS request_events(seq INTEGER PRIMARY KEY, request_id TEXT NOT NULL REFERENCES requests(request_id), timestamp_ms INTEGER NOT NULL, kind TEXT NOT NULL, details TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS events_request ON request_events(request_id,seq);
CREATE TABLE IF NOT EXISTS sessions(session_id TEXT PRIMARY KEY,started_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
PRAGMA user_version=1;";

const UPSERT: &str = "INSERT INTO requests(request_id,session_id,timestamp_ms,updated_ms,ended_ms,
requested_model,routed_model,project,path,method,transport,mode,state,status,retries,input_tokens,output_tokens,
cached_input_tokens,cache_write_tokens,reasoning_tokens,duration_ms,total_duration_ms,first_byte_ms,first_output_ms,
request_bytes,response_bytes,error_code,cost_nano_usd,price_version,price_model,record)
VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31)
ON CONFLICT(request_id) DO UPDATE SET updated_ms=excluded.updated_ms,ended_ms=excluded.ended_ms,
requested_model=excluded.requested_model,routed_model=excluded.routed_model,project=excluded.project,path=excluded.path,
transport=excluded.transport,state=excluded.state,status=excluded.status,retries=excluded.retries,input_tokens=excluded.input_tokens,
output_tokens=excluded.output_tokens,cached_input_tokens=excluded.cached_input_tokens,cache_write_tokens=excluded.cache_write_tokens,
reasoning_tokens=excluded.reasoning_tokens,duration_ms=excluded.duration_ms,total_duration_ms=excluded.total_duration_ms,
first_byte_ms=excluded.first_byte_ms,first_output_ms=excluded.first_output_ms,request_bytes=excluded.request_bytes,
response_bytes=excluded.response_bytes,error_code=excluded.error_code,cost_nano_usd=excluded.cost_nano_usd,
price_version=excluded.price_version,price_model=excluded.price_model,record=excluded.record";

fn integer(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_history_backfill_prices_only_missing_estimates_and_is_idempotent() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        for (id, model, existing) in [
            (1, "claude-sonnet-4-6", None),
            (2, "claude-sonnet-4-6", Some(123i64)),
            (3, "unknown-model", None),
        ] {
            let entry = Entry {
                request_id: format!("old-{id}"),
                session_id: "old".into(),
                mode: "standalone".into(),
                method: "POST".into(),
                path: "/v1/messages".into(),
                state: "succeeded".into(),
                requested_model: Some(model.into()),
                routed_model: Some(model.into()),
                input_tokens: Some(1000),
                output_tokens: Some(100),
                ..Entry::default()
            };
            let mut record = serde_json::to_value(&entry).unwrap();
            for field in [
                "response_model",
                "cache_write_1h_tokens",
                "speed",
                "inference_geo",
            ] {
                record.as_object_mut().unwrap().remove(field);
            }
            connection.execute("INSERT INTO requests(request_id,session_id,timestamp_ms,updated_ms,path,method,
                transport,mode,state,retries,input_tokens,output_tokens,cost_nano_usd,record)
                VALUES(?1,'old',0,0,'/v1/messages','POST','HTTP','standalone','succeeded',0,1000,100,?2,?3)",
                params![entry.request_id,existing,record.to_string()]).unwrap();
        }
        backfill_claude_prices(&mut connection).unwrap();
        backfill_claude_prices(&mut connection).unwrap();
        let actual = connection
            .prepare("SELECT cost_nano_usd FROM requests ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get::<_, Option<i64>>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(actual, vec![Some(4_500_000), Some(123), None]);
        let record: String = connection
            .query_row(
                "SELECT record FROM requests WHERE request_id='old-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let record: Value = serde_json::from_str(&record).unwrap();
        assert_eq!(record["price_model"], "claude-sonnet-4-6");
        assert_eq!(record["estimated_cost_usd"], 0.0045);
    }

    #[tokio::test]
    async fn delayed_initialization_keeps_startup_and_enqueue_nonblocking() {
        let dir = tempfile::tempdir().unwrap();
        let (release, gate) = mpsc::channel();
        let config = Logging {
            queue_capacity: 128,
            batch_size: 32,
            ..Logging::default()
        };
        let mut database = Database::open_with_initializer(
            dir.path().join("requests.sqlite3"),
            &config,
            "startup-test",
            move |path, session, health| {
                // A slow filesystem or recovery must not hold up the caller.
                gate.recv_timeout(Duration::from_secs(10))?;
                initialize(path, session, health)
            },
        )
        .unwrap();
        // A long idle interval before the first event is not writer backlog.
        database.started_ms = now_ms().saturating_sub(60_000);
        assert_eq!(database.health()["status"], "initializing");
        assert!(
            database
                .read::<()>(|_| panic!("read ran before initialization"))
                .await
                .is_err()
        );
        for id in 0..129 {
            database.enqueue(Event {
                entry: Entry {
                    request_id: format!("startup-test-{id}"),
                    session_id: "startup-test".into(),
                    state: "succeeded".into(),
                    ended_ms: Some(now_ms()),
                    ..Entry::default()
                },
                kind: Some("completed".into()),
                details: json!({}),
                timestamp_ms: now_ms(),
            });
        }
        assert_eq!(database.health()["pending_events"], 128);
        assert_eq!(database.health()["dropped_events"], 1);
        assert!(database.health()["lag_ms"].as_u64().unwrap() < 1000);
        assert!(!database.path.exists());
        release.send(()).unwrap();
        database.flush().await.unwrap();
        assert_eq!(database.health()["status"], "gaps");
        assert_eq!(database.health()["committed_events"], 128);
        assert_eq!(database.health()["pending_events"], 0);
        let (requests, events, drops) = database
            .read(|c| {
                Ok((
                    c.query_row("SELECT count(*) FROM requests", [], |r| r.get::<_, i64>(0))?,
                    c.query_row("SELECT count(*) FROM request_events", [], |r| {
                        r.get::<_, i64>(0)
                    })?,
                    c.query_row(
                        "SELECT value FROM metadata WHERE key='dropped_events'",
                        [],
                        |r| r.get::<_, String>(0),
                    )?,
                ))
            })
            .await
            .unwrap();
        assert_eq!((requests, events, drops.as_str()), (128, 128, "1"));
    }

    #[tokio::test]
    async fn http_stream_and_dashboard_respond_before_database_initializes() {
        use crate::proxy::{Options, router_with};
        use axum::{Router, body::Body};
        use std::convert::Infallible;
        async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            (url, task)
        }
        let dir = tempfile::tempdir().unwrap();
        let (release, gate) = mpsc::channel();
        let database = Arc::new(
            Database::open_with_initializer(
                dir.path().join("requests.sqlite3"),
                &Logging::default(),
                "http-startup-test",
                move |path, session, health| {
                    gate.recv_timeout(Duration::from_secs(10))?;
                    initialize(path, session, health)
                },
            )
            .unwrap(),
        );
        let mut store = super::super::Store::default();
        store.database = Some(database.clone());
        let store = Arc::new(store);
        let (upstream_url, upstream) = serve(Router::new().fallback(|| async {
            let chunks = futures_util::stream::iter([
                Ok::<_, Infallible>("data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n"),
                Ok("data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"),
            ]);
            ([("content-type", "text/event-stream")], Body::from_stream(chunks))
        })).await;
        let config = crate::config::Config {
            upstream_url,
            ..crate::config::Config::test_fixture()
        };
        let (url, proxy) = serve(
            router_with(
                config,
                Options {
                    logs: Some(store.clone()),
                    ..Options::default()
                },
            )
            .unwrap(),
        )
        .await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let response = client
            .post(format!("{url}/v1/responses"))
            .json(&json!({"model":"gpt-4.1","input":"synthetic","stream":true}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("response.completed")
        );
        let dashboard: Value = client
            .get(format!("{url}/logs/api/dashboard"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(dashboard["rpm"], 1);
        assert_eq!(dashboard["logging"]["status"], "initializing");
        assert!(dashboard["spend"].is_null());
        assert!(!database.path.exists());
        release.send(()).unwrap();
        store.flush().await.unwrap();
        assert_eq!(database.health()["dropped_events"], 0);
        assert_eq!(database.health()["pending_events"], 0);
        proxy.abort();
        upstream.abort();
    }
}
