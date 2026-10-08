//! Proxy-owned accounting. One bounded enqueue per completed request; all pricing,
//! SQLite writes, retention and legacy compaction happen off the forwarding path.
mod database;
mod report;
#[cfg(test)]
mod tests;
use crate::proxy::logs::{pricing, store::Entry};
use anyhow::{Context, Result};
use database::{initialize, open_writer};
pub use report::{SpendReport, generate_spend_report, render_spend_report};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 24 * HOUR_MS;
const QUEUE_SIZE: usize = 4096;
const MAX_DB_BYTES: usize = 64 * 1024 * 1024;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub fn default_db_path(config: Option<&Path>) -> PathBuf {
    if let Some(path) = std::env::var_os("HEY_PROXY_SPEND_DB").filter(|p| !p.is_empty()) {
        return path.into();
    }
    config
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".hey-proxy")
        })
        .join("spend.sqlite3")
}

#[derive(Default)]
struct Health {
    ready: AtomicBool,
    queued: AtomicU64,
    dropped: AtomicU64,
    failed: AtomicU64,
}
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct WriterHealth {
    pub ready: bool,
    pub queued_records: u64,
    pub dropped_records: u64,
    pub failed_records: u64,
}
pub struct SpendTracker {
    sender: SyncSender<Command>,
    health: Arc<Health>,
}
enum Command {
    Record(Box<Record>),
    Flush(tokio::sync::oneshot::Sender<bool>),
}

// Only small accounting metadata crosses threads. Never copy bodies or credentials.
struct Record {
    timestamp: i64,
    provider: String,
    account: String,
    label: String,
    billing: String,
    model: String,
    input: Option<u64>,
    output: Option<u64>,
    cached: u64,
    writes: u64,
    writes_1h: u64,
    reasoning: u64,
    speed: Option<String>,
    geography: Option<String>,
}
impl Record {
    fn from_entry(e: &Entry) -> Option<Self> {
        if e.mode == "client" || !e.is_final() || e.provider.is_none() {
            return None;
        }
        // Exclude setup, token-counting, probes and non-inference endpoints.
        if !matches!(e.method.as_str(), "POST" | "SEND") || e.path.contains("count_tokens") {
            return None;
        }
        let inference = [
            "/responses",
            "/responses/compact",
            "/chat/completions",
            "/completions",
            "/messages",
        ]
        .iter()
        .any(|p| e.path.trim_end_matches('/').ends_with(p))
            || e.path.contains(":generateContent")
            || e.path.contains(":streamGenerateContent");
        if !inference {
            return None;
        }
        Some(Self {
            timestamp: e.ended_ms.unwrap_or(e.timestamp_ms).min(i64::MAX as u64) as i64,
            provider: bounded(e.provider.as_deref().unwrap_or("unknown")),
            account: bounded(e.account_id.as_deref().unwrap_or("unknown")),
            label: bounded(
                e.account_label
                    .as_deref()
                    .or(e.account_id.as_deref())
                    .unwrap_or("unknown"),
            ),
            billing: bounded(e.billing.as_deref().unwrap_or("unknown")),
            // A known requested alias must never price an unknown returned model.
            model: bounded(
                e.response_model
                    .as_deref()
                    .or(e.routed_model.as_deref())
                    .unwrap_or("unknown"),
            ),
            input: e.input_tokens,
            output: e.output_tokens,
            cached: e.cached_input_tokens.unwrap_or(0),
            writes: e.cache_write_tokens.unwrap_or(0),
            writes_1h: e.cache_write_1h_tokens.unwrap_or(0),
            reasoning: e.reasoning_tokens.unwrap_or(0),
            speed: e.speed.clone(),
            geography: e.inference_geo.clone(),
        })
    }
    fn price(&self) -> pricing::Price {
        pricing::price(&Entry {
            method: "POST".into(),
            path: "/v1/responses".into(),
            response_model: Some(self.model.clone()),
            input_tokens: self.input,
            output_tokens: self.output,
            cached_input_tokens: Some(self.cached),
            cache_write_tokens: Some(self.writes),
            cache_write_1h_tokens: Some(self.writes_1h),
            speed: self.speed.clone(),
            inference_geo: self.geography.clone(),
            ..Entry::default()
        })
    }
}
fn bounded(s: &str) -> String {
    s.chars().take(128).collect()
}
impl SpendTracker {
    pub fn open(path: PathBuf) -> Result<Arc<Self>> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_SIZE);
        let health = Arc::new(Health::default());
        let worker_health = health.clone();
        std::thread::Builder::new()
            .name("proxy-accounting".into())
            .spawn(move || {
                loop {
                    match open_writer(&path).and_then(|mut conn| {
                        initialize(&mut conn)?;
                        Ok(conn)
                    }) {
                        Ok(mut conn) => {
                            worker_health.ready.store(true, Ordering::Relaxed);
                            writer_loop(&mut conn, receiver, &worker_health);
                            return;
                        }
                        Err(_) => {
                            // Retry initialization after transient locks/disk failures, without
                            // holding an unbounded queue or making forwarding wait.
                            let deadline = std::time::Instant::now() + Duration::from_secs(5);
                            while std::time::Instant::now() < deadline {
                                match receiver.recv_timeout(
                                    deadline.saturating_duration_since(std::time::Instant::now()),
                                ) {
                                    Ok(Command::Record(_)) => {
                                        worker_health.queued.fetch_sub(1, Ordering::Relaxed);
                                        worker_health.failed.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Ok(Command::Flush(done)) => {
                                        let _ = done.send(false);
                                    }
                                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                                }
                            }
                        }
                    }
                }
            })
            .context("Cannot start accounting worker")?;
        Ok(Arc::new(Self { sender, health }))
    }
    pub fn record_entry(&self, entry: &Entry) {
        if let Some(record) = Record::from_entry(entry) {
            self.health.queued.fetch_add(1, Ordering::Relaxed);
            if self
                .sender
                .try_send(Command::Record(Box::new(record)))
                .is_err()
            {
                self.health.queued.fetch_sub(1, Ordering::Relaxed);
                self.health.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    pub fn health(&self) -> WriterHealth {
        WriterHealth {
            ready: self.health.ready.load(Ordering::Relaxed),
            queued_records: self.health.queued.load(Ordering::Relaxed),
            dropped_records: self.health.dropped.load(Ordering::Relaxed),
            failed_records: self.health.failed.load(Ordering::Relaxed),
        }
    }
    pub async fn flush(&self) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .try_send(Command::Flush(tx))
            .map_err(|_| anyhow::anyhow!("Accounting queue is full or unavailable"))?;
        anyhow::ensure!(
            tokio::time::timeout(Duration::from_secs(10), rx).await??,
            "Accounting write failed"
        );
        Ok(())
    }
}
fn writer_loop(conn: &mut Connection, rx: Receiver<Command>, health: &Health) {
    let mut last_hour = -1;
    let mut persisted_losses = 0;
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        // A short batching window limits write amplification without delaying forwarding.
        let deadline = std::time::Instant::now() + Duration::from_millis(250);
        while batch.len() < 256 && !matches!(batch.last(), Some(Command::Flush(_))) {
            match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                Ok(cmd) => batch.push(cmd),
                Err(_) => break,
            }
        }
        let count = batch
            .iter()
            .filter(|c| matches!(c, Command::Record(_)))
            .count() as u64;
        let hour = now_ms() / HOUR_MS;
        let losses = health.dropped.load(Ordering::Relaxed) + health.failed.load(Ordering::Relaxed);
        let result = (|| -> Result<()> {
            let tx = conn.transaction()?;
            if last_hour != hour {
                database::prune(&tx, now_ms())?;
            }
            for cmd in &batch {
                if let Command::Record(rec) = cmd {
                    database::record(&tx, rec)?;
                }
            }
            tx.execute("UPDATE accounting_meta SET value=CAST(value AS INTEGER)+?1 WHERE key='lost_records'",[losses.saturating_sub(persisted_losses) as i64])?;
            tx.commit()?;
            Ok(())
        })();
        health.ready.store(result.is_ok(), Ordering::Relaxed);
        health.queued.fetch_sub(count, Ordering::Relaxed);
        if result.is_err() {
            health.failed.fetch_add(count, Ordering::Relaxed);
        } else {
            last_hour = hour;
            persisted_losses = losses;
        }
        for cmd in batch {
            if let Command::Flush(done) = cmd {
                let _ = done.send(result.is_ok());
            }
        }
    }
}
