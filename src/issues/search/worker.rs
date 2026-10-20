use super::{embed, index, rank::Field};
use crate::{database::Connection, issues::Result};
use rusqlite::{OptionalExtension, params};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

const CAPTURE: &str = "
CREATE TABLE IF NOT EXISTS issue_search_dirty(project TEXT NOT NULL,number INTEGER NOT NULL,generation INTEGER NOT NULL DEFAULT 1,dirty INTEGER NOT NULL DEFAULT 1,PRIMARY KEY(project,number));
CREATE INDEX IF NOT EXISTS search_pending ON issue_search_dirty(project,number) WHERE dirty=1;
CREATE TRIGGER IF NOT EXISTS search_issue_insert AFTER INSERT ON issues BEGIN INSERT INTO issue_search_dirty VALUES(NEW.project_id,NEW.number,1,1) ON CONFLICT(project,number) DO UPDATE SET generation=generation+1,dirty=1; END;
CREATE TRIGGER IF NOT EXISTS search_issue_update AFTER UPDATE ON issues WHEN NEW.title IS NOT OLD.title OR NEW.body IS NOT OLD.body OR NEW.archive_key IS NOT OLD.archive_key OR NEW.project_id IS NOT OLD.project_id OR NEW.number IS NOT OLD.number BEGIN INSERT INTO issue_search_dirty VALUES(OLD.project_id,OLD.number,1,1) ON CONFLICT(project,number) DO UPDATE SET generation=generation+1,dirty=1; INSERT INTO issue_search_dirty VALUES(NEW.project_id,NEW.number,1,1) ON CONFLICT(project,number) DO UPDATE SET generation=generation+1,dirty=1; END;
CREATE TRIGGER IF NOT EXISTS search_issue_delete AFTER DELETE ON issues BEGIN INSERT INTO issue_search_dirty VALUES(OLD.project_id,OLD.number,1,1) ON CONFLICT(project,number) DO UPDATE SET generation=generation+1,dirty=1; END;
CREATE TRIGGER IF NOT EXISTS search_comment_insert AFTER INSERT ON comments BEGIN INSERT INTO issue_search_dirty VALUES(NEW.project_id,NEW.issue_number,1,1) ON CONFLICT(project,number) DO UPDATE SET generation=generation+1,dirty=1; END;
CREATE TRIGGER IF NOT EXISTS search_comment_update AFTER UPDATE ON comments WHEN NEW.body IS NOT OLD.body BEGIN INSERT INTO issue_search_dirty VALUES(NEW.project_id,NEW.issue_number,1,1) ON CONFLICT(project,number) DO UPDATE SET generation=generation+1,dirty=1; END;
CREATE TRIGGER IF NOT EXISTS search_comment_delete AFTER DELETE ON comments WHEN NOT EXISTS(SELECT 1 FROM issues WHERE project_id=OLD.project_id AND number=OLD.issue_number AND archive_cleanup=1) BEGIN INSERT INTO issue_search_dirty VALUES(OLD.project_id,OLD.issue_number,1,1) ON CONFLICT(project,number) DO UPDATE SET generation=generation+1,dirty=1; END;
";

pub(crate) fn install(db: &Connection) -> Result<()> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='search_comment_delete')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        db.execute_batch(CAPTURE)?;
    }
    Ok(())
}

fn background_priority() {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

type Busy = Arc<dyn Fn() -> bool + Send + Sync>;

fn pause(stop: &AtomicBool, millis: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(millis);
    while !stop.load(Ordering::Acquire) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
    true
}

pub(crate) fn start(path: PathBuf, stop: Arc<AtomicBool>, busy: Busy) -> JoinHandle<()> {
    std::thread::spawn(move || {
        background_priority();
        if pause(&stop, 2000) {
            return;
        }
        let embedding_path = path.clone();
        let embedding_stop = stop.clone();
        let embedding_busy = busy.clone();
        let embeddings = std::thread::spawn(move || {
            embedding_loop(&embedding_path, &embedding_stop, &embedding_busy)
        });
        let mut failures = std::collections::HashMap::<(String, i64), i64>::new();
        let mut cursor = (String::new(), 0i64);
        while !stop.load(Ordering::Acquire) {
            if !super::enabled(&path) || busy() {
                if pause(&stop, 1000) {
                    break;
                }
                continue;
            }
            let result = (|| -> Result<usize> {
                let db = Connection::connect(&path)?;
                let cache = index::open(&path, true)?;
                // Populate the local queue a few rows at a time. Persisting the cursor
                // makes initial indexing resumable, including partially indexed threads.
                if !cache.query_row(
                    "SELECT EXISTS(SELECT 1 FROM meta WHERE key='backfill_done')",
                    [],
                    |r| r.get::<_, bool>(0),
                )? {
                    let saved: Option<String> = cache
                        .query_row(
                            "SELECT value FROM meta WHERE key='backfill_cursor'",
                            [],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let (project, number): (String, i64) = saved
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()?
                        .unwrap_or_default();
                    let batch = db.query_collect("SELECT project_id,number FROM issues WHERE (project_id,number)>(?1,?2) ORDER BY project_id,number LIMIT 8", params![project,number], |r| Ok::<_,rusqlite::Error>((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?;
                    for (p, n) in &batch {
                        if busy() || stop.load(Ordering::Acquire) {
                            return Ok(0);
                        }
                        db.execute("INSERT INTO issue_search_dirty VALUES(?1,?2,1,1) ON CONFLICT(project,number) DO UPDATE SET dirty=1", params![p,n])?;
                    }
                    if let Some(last) = batch.last() {
                        cache.execute(
                            "INSERT OR REPLACE INTO meta VALUES('backfill_cursor',?1)",
                            [serde_json::to_string(last)?],
                        )?;
                    } else {
                        cache.execute(
                            "INSERT OR REPLACE INTO meta VALUES('backfill_done','1')",
                            [],
                        )?;
                    }
                }
                failures.retain(|_, at| *at > crate::issues::worker::now());
                let excluded = serde_json::to_string(&failures.keys().collect::<Vec<_>>())?;
                // Round-robin pages prevent long conversations or failed archives
                // from starving newer edits. Exclude failures before the LIMIT.
                let sql = "SELECT project,number,generation FROM issue_search_dirty d WHERE dirty=1 AND (project,number)>(?1,?2) AND NOT EXISTS(SELECT 1 FROM json_each(?3) e WHERE json_extract(e.value,'$[0]')=d.project AND json_extract(e.value,'$[1]')=d.number) ORDER BY project,number LIMIT 8";
                let work = db.query_collect(sql, params![cursor.0, cursor.1, excluded], |r| {
                    Ok::<_, rusqlite::Error>((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })?;
                if work.is_empty() {
                    cursor = Default::default();
                }
                let count = work.len();
                for (project, number, generation) in work {
                    if busy() || stop.load(Ordering::Acquire) || !super::enabled(&path) {
                        break;
                    }
                    match index_step(&db, &cache, &project, number, generation) {
                        Ok(true) => {
                            db.execute("UPDATE issue_search_dirty SET dirty=0 WHERE project=?1 AND number=?2 AND generation=?3", params![project,number,generation])?;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("Search indexing deferred: {}", error.code);
                            failures.insert(
                                (project.clone(), number),
                                crate::issues::worker::now() + 60_000,
                            );
                        }
                    }
                    cursor = (project, number);
                    if pause(&stop, 20) {
                        break;
                    }
                }
                Ok(count)
            })();
            if pause(
                &stop,
                if matches!(result,Ok(n) if n>0) {
                    100
                } else {
                    1000
                },
            ) {
                break;
            }
        }
        let _ = embeddings.join();
    })
}

/// One short page per turn, with a durable comment cursor. Source snapshots end
/// before sidecar writes, network requests, or waits.
pub(super) fn index_step(
    db: &Connection,
    cache: &rusqlite::Connection,
    project: &str,
    number: i64,
    generation: i64,
) -> Result<bool> {
    let header: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT title,body,archive_key FROM issues WHERE project_id=?1 AND number=?2",
            params![project, number],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let saved: Option<(String,bool,i64)> = cache.query_row("SELECT generation,complete,after_comment FROM documents WHERE project=?1 AND number=?2", params![project,number], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let revision = generation.to_string();
    let fresh = saved.as_ref().is_none_or(|s| s.0 != revision);
    if fresh || header.is_none() {
        let tx = cache.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM passages WHERE project=?1 AND number=?2",
            params![project, number],
        )?;
        tx.execute(
            "DELETE FROM documents WHERE project=?1 AND number=?2",
            params![project, number],
        )?;
        tx.commit()?;
    }
    let Some((title, mut body, archive)) = header else {
        return Ok(true);
    };
    if !fresh && saved.as_ref().is_some_and(|s| s.1) {
        return Ok(true);
    }
    if fresh {
        if let Some(key) = &archive {
            body = crate::issues::archive::issue_body(db, project, number, key)?;
        }
        let tx = cache.unchecked_transaction()?;
        index::put(&tx, project, number, Field::Title, 0, &title)?;
        index::put(&tx, project, number, Field::Description, 0, &body)?;
        tx.execute(
            "INSERT INTO documents(project,number,generation) VALUES(?1,?2,?3)",
            params![project, number, revision],
        )?;
        tx.commit()?;
    }
    let after = if fresh { 0 } else { saved.unwrap().2 };
    let archive_db = if archive.is_some() {
        crate::issues::archive::history_connection(db, project, number)?
    } else {
        None
    };
    let comments = archive_db.as_ref().unwrap_or(db);
    let rows=comments.query_collect("SELECT id,body FROM comments WHERE project_id=?1 AND issue_number=?2 AND id>?3 ORDER BY id LIMIT 8",params![project,number,after],|r|Ok::<_,rusqlite::Error>((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?;
    let complete = rows.len() < 8;
    let mut after = after;
    let tx = cache.unchecked_transaction()?;
    for (id, text) in rows {
        index::put(&tx, project, number, Field::Comment, id, &text)?;
        after = id;
    }
    tx.execute(
        "UPDATE documents SET after_comment=?3,complete=?4 WHERE project=?1 AND number=?2",
        params![project, number, after, complete],
    )?;
    tx.commit()?;
    Ok(complete)
}

fn embedding_loop(path: &Path, stop: &AtomicBool, busy: &Busy) {
    background_priority();
    let mut backoff = 2u64;
    while !stop.load(Ordering::Acquire) {
        if !super::enabled(path) || busy() {
            if pause(stop, 1000) {
                break;
            }
            continue;
        }
        let result = (|| -> Result<usize> {
            let cache = index::open(path, true)?;
            let todo=cache.prepare("SELECT p.hash,min(p.text) FROM passages p WHERE NOT EXISTS(SELECT 1 FROM embeddings e WHERE e.hash=p.hash) GROUP BY p.hash LIMIT 64")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if todo.is_empty() {
                cache.execute(
                    "INSERT OR REPLACE INTO meta VALUES('embeddings_pending','0')",
                    [],
                )?;
                cache.execute("DELETE FROM embeddings WHERE hash IN (SELECT e.hash FROM embeddings e WHERE NOT EXISTS(SELECT 1 FROM passages p WHERE p.hash=e.hash) LIMIT 256)",[])?;
                return Ok(0);
            }
            // No SQLite transaction spans the proxy call. One batch per machine.
            let vectors = embed::request(
                &todo.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>(),
                Duration::from_secs(20),
            )?;
            let tx = cache.unchecked_transaction()?;
            for ((hash, _), vector) in todo.iter().zip(vectors) {
                tx.execute(
                    "INSERT OR IGNORE INTO embeddings VALUES(?1,?2)",
                    params![hash, index::compact(&vector)],
                )?;
            }
            tx.commit()?;
            Ok(todo.len())
        })();
        let delay = match result {
            Ok(0) => {
                backoff = 2;
                2000
            }
            Ok(_) => {
                backoff = 2;
                500
            }
            Err(_) => {
                backoff = (backoff * 2).min(120);
                backoff * 1000
            }
        };
        if pause(stop, delay) {
            break;
        }
    }
}
