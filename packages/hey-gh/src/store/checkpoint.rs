//! Keep passive checkpoint I/O out of the FIFO cache writer.
use super::{Connection, Result, storage};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use tracing::instrument::WithSubscriber;

// Match SQLite's existing automatic checkpoint threshold. This is a frame
// count, not the physical WAL file length (SQLite reuses the latter).
const PAGES: i64 = 1000;

struct Status {
    busy: bool,
    frames: i64,
    copied: i64,
}

impl Status {
    fn pending(&self) -> bool {
        self.busy || self.frames >= PAGES && self.copied < self.frames
    }
    fn incomplete(&self) -> bool {
        self.busy || self.frames >= 0 && self.copied < self.frames
    }
}

fn status(conn: &Connection, passive: bool) -> rusqlite::Result<Status> {
    conn.prepare_cached(if passive {
        "PRAGMA wal_checkpoint(PASSIVE)"
    } else {
        "PRAGMA wal_checkpoint(NOOP)"
    })?
    .query_row([], |row| {
        Ok(Status {
            busy: row.get::<_, i64>(0)? != 0,
            frames: row.get(1)?,
            copied: row.get(2)?,
        })
    })
}

pub(super) fn pending(conn: &Connection) -> bool {
    // NOOP reads the WAL counters without copying or syncing pages. An active
    // checkpointer can own the checkpoint lock; remember a follow-up instead
    // of waiting for it or failing a successfully committed cache write.
    status(conn, false).map_or(true, |status| status.pending())
}

pub(super) struct Checkpointer {
    connection: Arc<Mutex<Connection>>,
    // 0 idle, 1 running, 2 running with one coalesced follow-up. No queue of
    // payloads, thread-per-write, or lifetime tied to a cancelled read caller.
    state: AtomicU8,
}

impl Checkpointer {
    pub fn open(path: &str) -> Result<Arc<Self>> {
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(storage)?;
        connection.busy_timeout(Duration::ZERO).map_err(storage)?;
        Ok(Arc::new(Self {
            connection: Arc::new(Mutex::new(connection)),
            state: AtomicU8::new(0),
        }))
    }

    pub fn request(self: &Arc<Self>) {
        let previous = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| match state {
                0 => Some(1),
                1 => Some(2),
                _ => None,
            });
        if previous != Ok(0) {
            return;
        }
        let checkpoint = self.clone();
        // Capture the guard before spawning: shutdown can discard a future
        // before its first poll, but must still release maintenance ownership.
        let running = Running {
            checkpoint: checkpoint.clone(),
            armed: true,
        };
        tokio::spawn(
            async move {
                let mut running = running;
                loop {
                    let connection = checkpoint.connection.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        let started = Instant::now();
                        let result = connection.lock().map_err(storage).and_then(|conn| {
                            let before = status(&conn, false).map_err(storage)?;
                            if before.pending() {
                                status(&conn, true).map_err(storage)
                            } else {
                                Ok(before)
                            }
                        });
                        let elapsed = started.elapsed();
                        if elapsed >= Duration::from_millis(250) || result.is_err() {
                            tracing::info!(
                                elapsed_ms = elapsed.as_millis() as u64,
                                succeeded = result.is_ok(),
                                error_code = result
                                    .as_ref()
                                    .err()
                                    .map_or("none", crate::Error::diagnostic_code),
                                frames = result.as_ref().ok().map(|s| s.frames),
                                checkpointed_frames = result.as_ref().ok().map(|s| s.copied),
                                "Cache passive checkpoint finished"
                            );
                        }
                        result
                    })
                    .await;
                    // Long readers and independent SDK checkpointers can prevent
                    // progress. Bound retries without holding any connection lock
                    // or blocking thread; retry only if a write requested it.
                    if !matches!(result, Ok(Ok(ref status)) if !status.incomplete()) {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    if checkpoint
                        .state
                        .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        running.armed = false;
                        break;
                    }
                    checkpoint.state.store(1, Ordering::Release);
                }
            }
            .with_current_subscriber(),
        );
    }
}

struct Running {
    checkpoint: Arc<Checkpointer>,
    armed: bool,
}
impl Drop for Running {
    fn drop(&mut self) {
        // Runtime shutdown or an unexpected task panic must not leave future
        // maintenance permanently marked as running. A still-finishing blocking
        // operation remains serialized by the checkpoint connection's mutex.
        if self.armed {
            self.checkpoint.state.store(0, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_before_first_poll_releases_checkpoint_ownership() {
        let checkpoint = Arc::new(Checkpointer {
            connection: Arc::new(Mutex::new(Connection::open_in_memory().unwrap())),
            state: AtomicU8::new(0),
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        {
            let _entered = runtime.enter();
            checkpoint.request();
            assert_eq!(checkpoint.state.load(Ordering::Acquire), 1);
        }
        drop(runtime);
        assert_eq!(checkpoint.state.load(Ordering::Acquire), 0);
    }
}
