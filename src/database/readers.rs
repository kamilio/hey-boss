//! Read connections outlive short client sessions, retaining SQLite's caches.
use super::{local, wire::Command};
use std::{ops::Deref, path::PathBuf, sync::Mutex, time::Duration};

const MAX_IDLE: usize = 16;

pub(super) struct Readers {
    path: PathBuf,
    idle: Mutex<Vec<rusqlite::Connection>>,
}

impl Readers {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            idle: Mutex::new(Vec::new()),
        }
    }

    pub fn take(&self) -> std::io::Result<Reader<'_>> {
        // Do not hold the pool lock while opening SQLite or serving a client.
        let cached = self.idle.lock().unwrap().pop();
        let db = match cached {
            Some(db) => db,
            None => {
                let db = local(|| crate::issues::Store::open_read_connection(&self.path))
                    .map_err(std::io::Error::other)?
                    .into_local();
                db.busy_timeout(Duration::from_secs(1))
                    .map_err(std::io::Error::other)?;
                db.execute_batch("PRAGMA foreign_keys=ON")
                    .map_err(std::io::Error::other)?;
                db
            }
        };
        Ok(Reader {
            db: Some(db),
            pool: self,
            reusable: true,
        })
    }
}

pub(super) struct Reader<'a> {
    db: Option<rusqlite::Connection>,
    pool: &'a Readers,
    reusable: bool,
}
impl Deref for Reader<'_> {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &Self::Target {
        self.db.as_ref().unwrap()
    }
}

impl Reader<'_> {
    pub fn observe(&mut self, command: &Command) {
        // PRAGMAs can change connection state during preparation, before the
        // readonly check routes execution to the writer. Only ordinary queries
        // are safe to reuse across clients; unusual SQL keeps a private reader.
        // Batch/Execute always use the writer, except inside a read transaction.
        match command {
            Command::Prepare { sql } | Command::Query { sql, .. } => {
                let keyword = sql
                    .trim_start()
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("");
                self.reusable &=
                    keyword.eq_ignore_ascii_case("SELECT") || keyword.eq_ignore_ascii_case("WITH");
            }
            Command::Batch { sql } | Command::Execute { sql, .. } if !self.is_autocommit() => {
                // A batch may contain several statements. Only an exact
                // transaction terminator is known to leave settings untouched.
                let sql = sql.trim().trim_end_matches(';').trim_end();
                self.reusable &= ["COMMIT", "END", "ROLLBACK"]
                    .iter()
                    .any(|end| sql.eq_ignore_ascii_case(end));
            }
            _ => {}
        }
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        let db = self.db.take().unwrap();
        if !self.reusable || (!db.is_autocommit() && db.execute_batch("ROLLBACK").is_err()) {
            return;
        }
        let mut idle = self.pool.idle.lock().unwrap();
        if idle.len() < MAX_IDLE {
            idle.push(db);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hb-readers-{}-{}.db",
                std::process::id(),
                crate::issues::worker::random_id().unwrap()
            ));
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE counters(value INTEGER); INSERT INTO counters VALUES(1)").unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            for suffix in ["-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }

    #[test]
    fn short_sessions_reuse_schema_and_prepared_statements() {
        let fixture = Fixture::new();
        let readers = Readers::new(fixture.0.clone());
        for iteration in 0..128 {
            let reader = readers.take().unwrap();
            if iteration > 0 {
                // A new SQLite connection has no prepared statements. Check the
                // actual cache, not allocator addresses or elapsed wall time.
                assert!(
                    !unsafe {
                        rusqlite::ffi::sqlite3_next_stmt(reader.handle(), std::ptr::null_mut())
                    }
                    .is_null(),
                    "Short client sessions discarded the compiled query and schema cache"
                );
            }
            assert_eq!(
                reader
                    .prepare_cached("SELECT value FROM counters")
                    .unwrap()
                    .query_row([], |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }

    #[test]
    fn returning_a_reader_ends_its_snapshot_and_refreshes_changed_schema() {
        let fixture = Fixture::new();
        let readers = Readers::new(fixture.0.clone());
        let writer = rusqlite::Connection::open(&fixture.0).unwrap();
        let reader = readers.take().unwrap();
        reader.execute_batch("BEGIN").unwrap();
        assert_eq!(
            reader
                .prepare_cached("SELECT * FROM counters")
                .unwrap()
                .query_row([], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        writer.execute_batch("UPDATE counters SET value=2; ALTER TABLE counters ADD COLUMN added INTEGER DEFAULT 42").unwrap();
        assert_eq!(
            reader
                .prepare_cached("SELECT * FROM counters")
                .unwrap()
                .query_row([], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(reader);
        let reader = readers.take().unwrap();
        assert!(reader.is_autocommit());
        assert_eq!(
            reader
                .prepare_cached("SELECT * FROM counters")
                .unwrap()
                .query_row([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
                .unwrap(),
            (2, 42)
        );
    }

    #[test]
    fn preparation_side_effects_cannot_leak_to_the_next_client() {
        let fixture = Fixture::new();
        let readers = Readers::new(fixture.0.clone());
        for sql in [
            "PRAGMA foreign_keys=OFF",
            "/* settings */ PRAGMA foreign_keys=OFF",
            "EXPLAIN PRAGMA foreign_keys=OFF",
        ] {
            let mut reader = readers.take().unwrap();
            reader.observe(&Command::Prepare { sql: sql.into() });
            reader.prepare(sql).unwrap();
            drop(reader);
            let reader = readers.take().unwrap();
            assert!(
                reader
                    .db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY)
                    .unwrap()
            );
        }
        let mut reader = readers.take().unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let sql = "SELECT 1; PRAGMA cache_size=123";
        reader.observe(&Command::Batch { sql: sql.into() });
        reader.execute_batch(sql).unwrap();
        drop(reader);
        let reader = readers.take().unwrap();
        assert_ne!(
            reader
                .pragma_query_value(None, "cache_size", |r| r.get::<_, i64>(0))
                .unwrap(),
            123
        );
    }

    #[test]
    fn idle_readers_are_bounded_and_active_readers_are_independent() {
        let fixture = Fixture::new();
        let readers = Readers::new(fixture.0.clone());
        let mut active = Vec::new();
        for _ in 0..MAX_IDLE * 2 {
            let reader = readers.take().unwrap();
            assert!(
                active
                    .iter()
                    .all(|other: &Reader<'_>| unsafe { other.handle() != reader.handle() })
            );
            active.push(reader);
        }
        drop(active);
        assert_eq!(readers.idle.lock().unwrap().len(), MAX_IDLE);
    }

    #[test]
    #[ignore = "manual comparison of cold and pooled reads against the full issue schema"]
    fn profile_short_session_reads() {
        let fixture = Fixture::new();
        std::fs::remove_file(&fixture.0).unwrap();
        local(|| crate::issues::Store::open(&fixture.0)).unwrap();
        let readers = Readers::new(fixture.0.clone());
        let query = |pool: &Readers| {
            let reader = pool.take().unwrap();
            reader
                .prepare_cached("SELECT count(*) FROM projects")
                .unwrap()
                .query_row([], |row| row.get::<_, i64>(0))
                .unwrap();
        };
        let started = std::time::Instant::now();
        for _ in 0..100 {
            query(&Readers::new(fixture.0.clone()));
        }
        let cold = started.elapsed();
        let started = std::time::Instant::now();
        for _ in 0..100 {
            query(&readers);
        }
        eprintln!(
            "100 short database sessions: cold {cold:?}; pooled {:?}",
            started.elapsed()
        );
    }
}
