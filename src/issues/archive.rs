//! Cold payloads are published durably before a hot record can reference them.

mod runs;
pub(crate) use runs::{archive_runs, worker_event_tails, worker_payload};
mod history;
pub(crate) use history::{
    archive_issue, cleanup_history, history_connection, issue_body, mutation_targets,
    restore_issue, search_bodies,
};
#[cfg(test)]
mod history_tests;

use super::{Error, Result, Store};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
    time::Duration,
};

const APPLICATION_ID: i64 = 0x48424152;
const FORMAT_VERSION: i64 = 1;
const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const GRACE_MS: i64 = 5 * 24 * 60 * 60 * 1000;

/// A separate connection is intentional: neither reads nor publication require
/// a transaction spanning the hot and cold stores. Objects are immutable and
/// content addressed, so a crash after publication can only leave an extra copy.
pub(crate) struct Archive {
    db: Connection,
}

fn unavailable(message: impl Into<String>) -> Error {
    Error::new("archive_unavailable", message)
}

impl Archive {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        Store::create_database_if_missing(path)?;
        let archive = Self::connect(path, false)?;
        let tx = archive.db.unchecked_transaction()?;
        let app: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if app == 0 && version == 0 {
            let empty: bool =
                tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM sqlite_master)", [], |r| {
                    r.get(0)
                })?;
            if !empty {
                return Err(unavailable(
                    "Refusing to initialize an unrelated archive database",
                ));
            }
            tx.execute_batch("CREATE TABLE objects(key TEXT PRIMARY KEY,kind TEXT NOT NULL,format INTEGER NOT NULL,bytes INTEGER NOT NULL,content BLOB NOT NULL) WITHOUT ROWID;")?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", FORMAT_VERSION)?;
        } else if app != APPLICATION_ID || version != FORMAT_VERSION {
            return Err(unavailable("Unknown archive database format"));
        }
        tx.commit()?;
        let journal: String = archive
            .db
            .pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        if journal != "wal" {
            archive.db.pragma_update(None, "journal_mode", "WAL")?;
        }
        archive.db.pragma_update(None, "synchronous", "FULL")?;
        history::initialize(&archive.db)?;
        Ok(archive)
    }

    pub(crate) fn read(path: &Path) -> Result<Self> {
        let archive = Self::connect(path, true)?;
        let app: i64 = archive
            .db
            .pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = archive
            .db
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if app != APPLICATION_ID || version != FORMAT_VERSION {
            return Err(unavailable("Unknown archive database format"));
        }
        Ok(archive)
    }

    fn connect(path: &Path, read: bool) -> Result<Self> {
        let path = Store::validate_database_path(path)
            .map_err(|e| unavailable(format!("Cannot access archive: {e}")))?;
        let mode = if read {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        let db = Connection::open_with_flags(
            path,
            mode | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_millis(250))?;
        db.create_scalar_function(
            "archive_record",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC
                | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
            |context| {
                let record: String = context.get(0)?;
                let expected: String = context.get(1)?;
                if format!("{:x}", Sha256::digest(record.as_bytes())) != expected {
                    return Err(rusqlite::Error::UserFunctionError(Box::new(
                        std::io::Error::other("Archived history checksum does not match"),
                    )));
                }
                Ok(record)
            },
        )?;
        Ok(Self { db })
    }

    pub(crate) fn put(&self, kind: &str, value: &Value) -> Result<String> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > MAX_OBJECT_BYTES {
            return Err(unavailable("Archive object exceeds size limit"));
        }
        let key = object_key(kind, &bytes);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&bytes)?;
        let compressed = encoder.finish()?;
        self.db.execute("INSERT INTO objects(key,kind,format,bytes,content) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(key) DO NOTHING", params![key,kind,FORMAT_VERSION,bytes.len() as i64,compressed])?;
        // An existing row is not evidence of a valid copy. Verify what actually
        // committed before permitting a caller to remove the hot payload.
        if self.get(kind, &key)? != *value {
            return Err(unavailable("Archive verification failed"));
        }
        Ok(key)
    }

    pub(crate) fn get(&self, kind: &str, key: &str) -> Result<Value> {
        let saved: Option<(String, i64, i64, Vec<u8>)> = self
            .db
            .query_row(
                "SELECT kind,format,bytes,content FROM objects WHERE key=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((stored_kind, format, size, compressed)) = saved else {
            return Err(unavailable("Archive object is missing"));
        };
        if stored_kind != kind
            || format != FORMAT_VERSION
            || !(0..=MAX_OBJECT_BYTES as i64).contains(&size)
        {
            return Err(unavailable(
                "Archive object has an invalid kind, format or size",
            ));
        }
        let mut bytes = Vec::new();
        ZlibDecoder::new(compressed.as_slice())
            .take(size as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| unavailable(format!("Cannot decode archive object: {e}")))?;
        if bytes.len() as i64 != size || object_key(kind, &bytes) != key {
            return Err(unavailable("Archive object integrity check failed"));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| unavailable(format!("Invalid archive JSON: {e}")))
    }
}

fn object_key(kind: &str, bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"archive-v1\0");
    digest.update(kind.as_bytes());
    digest.update([0]);
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn archive_path(db: &crate::database::Connection) -> Result<std::path::PathBuf> {
    db.path()
        .filter(|path| !path.is_empty())
        .map(|path| {
            let mut name = std::ffi::OsString::from(path);
            name.push(".archive.db");
            name.into()
        })
        .ok_or_else(|| unavailable("Archival requires a persistent database"))
}

pub(crate) fn receipt_response(
    db: &crate::database::Connection,
    response: &str,
    key: Option<&str>,
) -> Result<Value> {
    match key {
        Some(key) => Archive::read(&archive_path(db)?)?.get("receipt", key),
        None => Ok(serde_json::from_str(response)?),
    }
}

/// The cold commit precedes the conditional hot update. Neither a concurrent
/// pass nor a failed hot commit can lose a receipt or repeat its side effects.
pub(crate) fn archive_receipts(db: &crate::database::Connection, now: i64) -> Result<usize> {
    if !db.is_autocommit() {
        return Err(unavailable(
            "Archive maintenance cannot run inside a hot transaction",
        ));
    }
    let cutoff = now.saturating_sub(GRACE_MS);
    let candidates = db.query_collect(
        "SELECT project_id,actor,request_id,created_at FROM requests WHERE archive_key IS NULL AND created_at<=?1 ORDER BY created_at LIMIT 8", [cutoff],
        |r| -> rusqlite::Result<_> { Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?)) }
    )?;
    if candidates.is_empty() {
        return Ok(0);
    }
    let archive = Archive::open(&archive_path(db)?)?;
    let started = std::time::Instant::now();
    let mut moved = 0;
    for (project, actor, request, created) in candidates {
        // Read one payload at a time: a batch of large responses must not hold
        // eight times the wire limit in memory while compressing its first row.
        let response: Option<String> = db.query_row("SELECT response FROM requests WHERE project_id=?1 AND actor=?2 AND request_id=?3 AND created_at=?4 AND archive_key IS NULL", params![project,actor,request,created], |r| r.get(0)).optional()?;
        let Some(response) = response else {
            continue;
        };
        let value = serde_json::from_str(&response)?;
        let key = archive.put("receipt", &value)?;
        moved += db.execute("UPDATE requests SET response='',archive_key=?6 WHERE project_id=?1 AND actor=?2 AND request_id=?3 AND response=?4 AND created_at=?5 AND archive_key IS NULL", params![project,actor,request,response,created,key])?;
        if started.elapsed() >= Duration::from_millis(25) {
            break;
        }
    }
    Ok(moved)
}

pub(crate) fn maintain(db: &crate::database::Connection, now: i64) -> Result<usize> {
    let (_, version) = db.check_schema()?;
    if version != Store::schema_version() {
        return Ok(0);
    }
    Ok(archive_receipts(db, now)? + archive_runs(db, now)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "hb-archive-{}-{}",
                std::process::id(),
                crate::issues::worker::random_id().unwrap()
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("issues.db.archive.db")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn committed_archive_survives_reopen_and_duplicate_copy() {
        let f = Fixture::new();
        let value =
            json!({"body":"日本語 🦀\n".repeat(10_000),"empty":"","null":null,"nested":[1,true]});
        let key = {
            let archive = Archive::open(&f.path()).unwrap();
            let key = archive.put("receipt", &value).unwrap();
            assert_eq!(archive.put("receipt", &value).unwrap(), key);
            assert_eq!(archive.get("receipt", &key).unwrap(), value);
            assert_ne!(archive.put("issue", &value).unwrap(), key);
            key
        };
        let archive = Archive::read(&f.path()).unwrap();
        assert_eq!(archive.get("receipt", &key).unwrap(), value);
        assert_eq!(
            std::fs::metadata(f.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            archive
                .db
                .query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn missing_archive_never_silently_becomes_an_empty_one() {
        let f = Fixture::new();
        assert!(Archive::read(&f.path()).is_err());
        assert!(!f.path().exists());
        let archive = Archive::open(&f.path()).unwrap();
        assert!(archive.get("receipt", &"0".repeat(64)).is_err());
    }

    #[test]
    fn corruption_wrong_kind_and_future_format_fail_closed() {
        let f = Fixture::new();
        let archive = Archive::open(&f.path()).unwrap();
        let key = archive.put("receipt", &json!({"ok":true})).unwrap();
        assert!(archive.get("issue", &key).is_err());
        archive
            .db
            .execute("UPDATE objects SET content=x'000102' WHERE key=?1", [&key])
            .unwrap();
        assert!(archive.get("receipt", &key).is_err());
        drop(archive);
        let raw = rusqlite::Connection::open(f.path()).unwrap();
        raw.pragma_update(None, "user_version", 999).unwrap();
        drop(raw);
        assert!(Archive::read(&f.path()).is_err());
        assert!(Archive::open(&f.path()).is_err());
    }

    #[test]
    fn refuses_unrelated_databases_and_filesystem_aliases() {
        let f = Fixture::new();
        let archive = Archive::open(&f.path()).unwrap();
        let inode = std::fs::metadata(f.path()).unwrap().ino();
        let alias = f.0.join("alias.db");
        symlink(f.path(), &alias).unwrap();
        assert!(Archive::open(&alias).is_err());
        assert!(Archive::read(&alias).is_err());
        std::fs::remove_file(alias).unwrap();
        std::fs::hard_link(f.path(), f.0.join("hard.db")).unwrap();
        assert!(Archive::open(&f.path()).is_err());
        std::fs::remove_file(f.0.join("hard.db")).unwrap();
        assert_eq!(std::fs::metadata(f.path()).unwrap().ino(), inode);
        drop(archive);
        let foreign = f.0.join("foreign.db");
        let raw = rusqlite::Connection::open(&foreign).unwrap();
        raw.execute_batch(
            "CREATE TABLE private(value TEXT); INSERT INTO private VALUES('preserve');",
        )
        .unwrap();
        drop(raw);
        assert!(Archive::open(&foreign).is_err());
        let raw = rusqlite::Connection::open(&foreign).unwrap();
        assert_eq!(
            raw.query_row("SELECT value FROM private", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "preserve"
        );
    }

    #[test]
    fn receipt_grace_boundary_keeps_exact_replay_and_hot_identity() {
        let f = Fixture::new();
        let hot = f.0.join("issues.db");
        let db = Store::open(&hot).unwrap().into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',1); INSERT INTO agents VALUES('a','{}',0);").unwrap();
        let original =
            json!({"ok":true,"issue":{"number":12,"body":"large receipt\n".repeat(10_000)}});
        db.execute("INSERT INTO requests(project_id,actor,request_id,payload,response,created_at) VALUES('p','a','once','{}',?1,1000)", [original.to_string()]).unwrap();
        assert_eq!(archive_receipts(&db, GRACE_MS + 999).unwrap(), 0);
        assert_eq!(archive_receipts(&db, GRACE_MS + 1000).unwrap(), 1);
        assert_eq!(archive_receipts(&db, GRACE_MS + 1001).unwrap(), 0);
        let (response, key): (String, Option<String>) = db
            .query_row(
                "SELECT response,archive_key FROM requests WHERE request_id='once'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(response.is_empty());
        assert!(key.is_some());
        assert_eq!(
            receipt_response(&db, &response, key.as_deref()).unwrap(),
            original
        );
        assert_eq!(
            db.query_row(
                "SELECT payload FROM requests WHERE request_id='once'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "{}"
        );
        drop(db);
        let db = Store::open(&hot).unwrap().into_database();
        assert_eq!(
            receipt_response(&db, &response, key.as_deref()).unwrap(),
            original
        );
        std::fs::rename(f.path(), f.0.join("missing.archive.db")).unwrap();
        assert!(receipt_response(&db, &response, key.as_deref()).is_err());
    }

    #[test]
    fn failed_receipt_publication_preserves_hot_content() {
        let f = Fixture::new();
        let db = Store::open(&f.0.join("issues.db")).unwrap().into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',1); INSERT INTO agents VALUES('a','{}',0); INSERT INTO requests(project_id,actor,request_id,payload,response,created_at) VALUES('p','a','once','{}','{\"ok\":true}',1);").unwrap();
        std::fs::write(f.path(), b"not a database").unwrap();
        assert!(archive_receipts(&db, GRACE_MS + 2).is_err());
        assert_eq!(
            db.query_row("SELECT response FROM requests", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "{\"ok\":true}"
        );
        assert!(
            db.query_row("SELECT archive_key IS NULL FROM requests", [], |r| r
                .get::<_, bool>(0))
                .unwrap()
        );
    }

    #[test]
    fn archived_mutation_replays_without_repeating_effects_and_rejects_changed_payload() {
        let f = Fixture::new();
        let hot = f.0.join("issues.db");
        let mut store = Store::open(&hot).unwrap();
        let mut request: crate::issues::Request = serde_json::from_value(json!({
            "version":1,"project":{"id":"named:Archive","name":"Archive"},
            "actor":{"id":"human:boss","kind":"human","session_id":null,"machine":"test","host":"test","pid":null,"process_start":null,"cwd":f.0,"source":"test"},
            "request_id":"create-once","operation":{"action":"create","title":"Keep exactly once","body":"Body preserved","labels":[],"draft":true}
        })).unwrap();
        let created = store.execute(&request).unwrap();
        let db = Store::open(&hot).unwrap().into_database();
        db.execute("UPDATE requests SET created_at=1", []).unwrap();
        assert_eq!(archive_receipts(&db, GRACE_MS + 2).unwrap(), 1);
        assert_eq!(store.execute(&request).unwrap(), created);
        assert_eq!(
            db.query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        request.operation =
            serde_json::from_value(json!({"action":"request_status","id":"create-once"})).unwrap();
        request.request_id = None;
        let status = store.execute(&request).unwrap();
        assert_eq!(status["request"]["state"], "recorded");
        assert_eq!(status["request"]["response"], created);
        request.request_id = Some("create-once".into());
        request.operation = serde_json::from_value(
            json!({"action":"create","title":"Different","body":"","labels":[],"draft":true}),
        )
        .unwrap();
        assert_eq!(store.execute(&request).unwrap_err().code, "conflict");
    }

    #[test]
    fn cold_copy_survives_failed_hot_commit_and_retry_reuses_it() {
        let f = Fixture::new();
        let db = Store::open(&f.0.join("issues.db")).unwrap().into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',1); INSERT INTO agents VALUES('a','{}',0); INSERT INTO requests(project_id,actor,request_id,payload,response,created_at) VALUES('p','a','once','{}','{\"ok\":true}',1);
            CREATE TRIGGER simulate_failed_commit BEFORE UPDATE ON requests BEGIN SELECT RAISE(ABORT,'hot commit failed'); END;").unwrap();
        assert!(archive_receipts(&db, GRACE_MS + 2).is_err());
        assert_eq!(
            db.query_row("SELECT response FROM requests", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "{\"ok\":true}"
        );
        let archive = Archive::read(&f.path()).unwrap();
        assert_eq!(
            archive
                .db
                .query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        db.execute_batch("DROP TRIGGER simulate_failed_commit")
            .unwrap();
        assert_eq!(archive_receipts(&db, GRACE_MS + 2).unwrap(), 1);
        assert_eq!(
            archive
                .db
                .query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn cleanup_preserves_old_read_snapshots_and_does_not_wait_for_writer_when_idle() {
        let f = Fixture::new();
        let hot = f.0.join("issues.db");
        let db = Store::open(&hot).unwrap().into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',1); INSERT INTO agents VALUES('a','{}',0); INSERT INTO requests(project_id,actor,request_id,payload,response,created_at) VALUES('p','a','once','{}','{\"ok\":true}',1);").unwrap();
        let reader = Store::open(&hot).unwrap().into_database();
        let snapshot = reader.read_transaction().unwrap();
        assert_eq!(
            snapshot
                .query_row("SELECT response FROM requests", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "{\"ok\":true}"
        );
        assert_eq!(archive_receipts(&db, GRACE_MS + 1).unwrap(), 1);
        assert_eq!(
            snapshot
                .query_row("SELECT response FROM requests", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "{\"ok\":true}"
        );
        snapshot.commit().unwrap();
        let writer = crate::database::Transaction::new_unchecked(
            &reader,
            rusqlite::TransactionBehavior::Immediate,
        )
        .unwrap();
        assert_eq!(archive_receipts(&db, GRACE_MS + 2).unwrap(), 0);
        writer.rollback().unwrap();
    }

    #[test]
    fn finished_runs_archive_payloads_and_logs_without_changing_retry_state() {
        let f = Fixture::new();
        let db = Store::open(&f.0.join("issues.db")).unwrap().into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',2); INSERT INTO agents VALUES('a','{}',0);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('p',1,'Task','','open','a',0,0,1,'[]');
            INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,retry_count,retry_allowed,expanded_prompt) VALUES
            ('old','p',1,'{\"issue\":{\"title\":\"Task\",\"number\":1,\"body\":\"Full context\"},\"resume_session\":\"session\",\"config\":{\"cwd\":\"/work\",\"prompt\":\"Original instructions\"}}','a','failed',1,'start','local',1,100,100,3,0,'Expanded prompt');
            INSERT INTO worker_events(id,run_id,created_at,text) VALUES(10,'old',10,'First'),(20,'old',20,'Second'),(30,'old',100,'Third');").unwrap();
        db.execute("UPDATE worker_runs SET job=json_set(job,'$.config.provider','pi','$.session_ref',json('{\"provider\":\"pi\",\"id\":\"saved-session\"}'))",[]).unwrap();
        let job: String = db
            .query_row("SELECT job FROM worker_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(archive_runs(&db, GRACE_MS + 99).unwrap(), 0);
        assert_eq!(archive_runs(&db, GRACE_MS + 100).unwrap(), 1);
        assert_eq!(archive_runs(&db, GRACE_MS + 101).unwrap(), 0);
        assert_eq!(
            worker_payload(&db, "old", "p").unwrap(),
            ("Expanded prompt".into(), job)
        );
        let compact: Value = serde_json::from_str(
            &db.query_row("SELECT job FROM worker_runs", [], |r| r.get::<_, String>(0))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(compact["issue"]["title"], "Task");
        assert_eq!(compact["config"]["cwd"], "/work");
        assert_eq!(compact["config"]["provider"], "pi");
        assert_eq!(
            compact["session_ref"],
            json!({"provider":"pi","id":"saved-session"})
        );
        assert_eq!(compact["resume_session"], "session");
        assert!(compact["issue"]["body"].is_null());
        assert_eq!(
            db.query_row("SELECT retry_count FROM worker_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM worker_events", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let events = worker_event_tails(&db, &["old"]).unwrap();
        assert_eq!(
            events["old"]
                .iter()
                .map(|(_, v)| v["text"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["Third", "Second", "First"]
        );
        // SQLite rowids must not be reused after moving the last run's logs.
        db.execute(
            "INSERT INTO worker_events(run_id,created_at,text) VALUES('old',101,'Late')",
            [],
        )
        .unwrap();
        assert!(db.last_insert_rowid() > 30);
        let events = worker_event_tails(&db, &["old"]).unwrap();
        assert_eq!(
            events["old"]
                .iter()
                .map(|(_, v)| v["text"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["Late", "Third", "Second", "First"]
        );
        // A later prompt revision can move back through the hot store without
        // dropping log entries that belonged to the previous archive copy.
        db.execute("UPDATE worker_runs SET archive_key=NULL,expanded_prompt='Revised',job=?1,updated_at=102", [worker_payload(&db, "old", "p").unwrap().1]).unwrap();
        assert_eq!(archive_runs(&db, GRACE_MS + 102).unwrap(), 1);
        assert_eq!(worker_payload(&db, "old", "p").unwrap().0, "Revised");
        let events = worker_event_tails(&db, &["old"]).unwrap();
        assert_eq!(
            events["old"]
                .iter()
                .map(|(_, v)| v["text"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["Late", "Third", "Second", "First"]
        );
    }

    #[test]
    fn active_runs_recent_logs_and_recent_changes_stay_hot() {
        let f = Fixture::new();
        let db = Store::open(&f.0.join("issues.db")).unwrap().into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',2); INSERT INTO agents VALUES('a','{}',0);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('p',1,'Task','','open','a',0,0,1,'[]');
            INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at) VALUES
            ('running','p',1,'{}','a','running',1,'start','local',1,1,NULL),
            ('updated','p',1,'{}','a','completed',1,'start','local',1,101,1),
            ('logged','p',1,'{}','a','completed',1,'start','local',1,1,1);
            INSERT INTO worker_events(run_id,created_at,text) VALUES('logged',101,'Recent observation');").unwrap();
        assert_eq!(archive_runs(&db, GRACE_MS + 100).unwrap(), 0);
        assert!(!f.path().exists());
        assert_eq!(archive_runs(&db, GRACE_MS + 101).unwrap(), 1);
        assert_eq!(archive_runs(&db, GRACE_MS + 101).unwrap(), 1);
        assert_eq!(archive_runs(&db, GRACE_MS + 101).unwrap(), 0);
        assert!(
            db.query_row(
                "SELECT archive_key IS NULL FROM worker_runs WHERE id='running'",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
    }
}
