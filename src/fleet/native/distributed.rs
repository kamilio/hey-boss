//! Local, transactional capture. SSH transports immutable facts; projections
//! never import process ownership or turn a received attempt into a local run.
mod activation;
mod archives;
mod capture;
pub(super) use activation::{activate, enroll, status};
mod graph;
pub(super) mod health;
mod identity;
mod local;
pub(super) use local::request as local_request;
mod materialize;
mod projection;
pub(super) mod reconcile;
mod resources;
mod transport;
pub(super) use transport::{serve, start};
#[cfg(test)]
mod tests;

use super::{Result, replica};
use crate::database::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const PROTOCOL: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    pub protocol: u32,
    pub origin: String,
    pub clock: i64,
    pub table: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<resources::Resource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<archives::Manifest>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS distributed_meta(
 id INTEGER PRIMARY KEY CHECK(id=1),origin TEXT NOT NULL,clock INTEGER NOT NULL DEFAULT 0,
 exported INTEGER NOT NULL DEFAULT 0,projected INTEGER NOT NULL DEFAULT 0,
 active INTEGER NOT NULL DEFAULT 0,slot INTEGER,controller TEXT,legacy_cutoff INTEGER NOT NULL DEFAULT 0,
 format INTEGER NOT NULL DEFAULT 2);
CREATE TABLE IF NOT EXISTS distributed_outbox(
 clock INTEGER PRIMARY KEY,table_name TEXT NOT NULL,before_json TEXT,after_json TEXT);
CREATE TABLE IF NOT EXISTS distributed_baseline(
 table_name TEXT PRIMARY KEY,after_rowid INTEGER NOT NULL DEFAULT 0,complete INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS distributed_fields(
 table_name TEXT NOT NULL,entity TEXT NOT NULL,field TEXT NOT NULL,
 clock INTEGER NOT NULL,origin TEXT NOT NULL,value TEXT NOT NULL,
 PRIMARY KEY(table_name,entity,field)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS distributed_dependency_graph ON distributed_fields(json_extract(entity,'$.project_id'),entity,json_extract(value,'$.blockers')) WHERE table_name='issues' AND field='@lifecycle';
CREATE TABLE IF NOT EXISTS distributed_dirty(
 table_name TEXT NOT NULL,entity TEXT NOT NULL,PRIMARY KEY(table_name,entity)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS distributed_aliases(
 table_name TEXT NOT NULL,entity TEXT NOT NULL,local_id INTEGER NOT NULL,
 PRIMARY KEY(table_name,entity),UNIQUE(table_name,local_id));
CREATE INDEX IF NOT EXISTS distributed_legacy_identity ON fleet_row_ids(origin,table_name,local_id,origin_id);
CREATE TABLE IF NOT EXISTS distributed_rejections(
 table_name TEXT NOT NULL,entity TEXT NOT NULL,reason TEXT NOT NULL,PRIMARY KEY(table_name,entity));
CREATE TABLE IF NOT EXISTS distributed_archives(key TEXT PRIMARY KEY,pages TEXT NOT NULL,cursor TEXT NOT NULL,complete INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS distributed_archive_history(key TEXT PRIMARY KEY,project_id TEXT NOT NULL,number INTEGER NOT NULL,cursor_kind TEXT NOT NULL DEFAULT '',cursor_id TEXT NOT NULL DEFAULT '',complete INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS distributed_archive_history_pending ON distributed_archive_history(project_id,number,key) WHERE complete=0;
CREATE TABLE IF NOT EXISTS distributed_archive_failures(key TEXT PRIMARY KEY,project_id TEXT NOT NULL,number INTEGER NOT NULL,manifest TEXT NOT NULL,reason TEXT NOT NULL,attempted_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS distributed_archive_failure_scope ON distributed_archive_failures(project_id,number);
CREATE TABLE IF NOT EXISTS distributed_archive_supersessions(key TEXT PRIMARY KEY,replacement TEXT NOT NULL,project_id TEXT NOT NULL,number INTEGER NOT NULL,verified_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS distributed_history_edits(project_id TEXT NOT NULL,number INTEGER NOT NULL,PRIMARY KEY(project_id,number)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS distributed_members(node TEXT PRIMARY KEY,slot INTEGER NOT NULL UNIQUE,ready INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS distributed_attempts(
 id TEXT PRIMARY KEY,machine TEXT NOT NULL,project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,
 state TEXT NOT NULL,started_at INTEGER NOT NULL,finished_at INTEGER,summary TEXT NOT NULL,payload TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS distributed_issue_attempts ON distributed_attempts(project_id,issue_number,started_at,id);
CREATE INDEX IF NOT EXISTS distributed_working_attempts ON distributed_attempts(project_id,issue_number,started_at,id) WHERE state='running' AND finished_at IS NULL;
CREATE TABLE IF NOT EXISTS distributed_job_attempts(sequence INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,project_id TEXT NOT NULL,job_id TEXT NOT NULL,created_at INTEGER NOT NULL,scheduled_at INTEGER NOT NULL,state TEXT NOT NULL,payload TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS distributed_job_history ON distributed_job_attempts(job_id,sequence DESC);
CREATE INDEX IF NOT EXISTS distributed_job_calendar ON distributed_job_attempts(project_id,scheduled_at,id);
CREATE VIEW IF NOT EXISTS distributed_job_runs AS SELECT sequence,id,project_id,job_id,created_at,scheduled_at,state,
 json_extract(payload,'$.snapshot') AS snapshot,json_extract(payload,'$.trigger') AS trigger,
 json_extract(payload,'$.request_key') AS request_key,json_extract(payload,'$.reason') AS reason,
 json_extract(payload,'$.task_number') AS task_number,json_extract(payload,'$.machine') AS machine,
 json_extract(payload,'$.session_id') AS session_id,json_extract(payload,'$.started_at') AS started_at,
 json_extract(payload,'$.finished_at') AS finished_at,json_extract(payload,'$.worker_id') AS worker_id,
 json_extract(payload,'$.parent_run_id') AS parent_run_id,json_extract(payload,'$.target_machine') AS target_machine
 FROM distributed_job_attempts;
";

pub(crate) fn restore_history_row(
    db: &Connection,
    table: &str,
    row: &Value,
) -> crate::issues::Result<bool> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_meta')",
        [],
        |r| r.get::<_, bool>(0),
    )? || !db.query_row(
        "SELECT active=1 FROM distributed_meta WHERE id=1",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(false);
    }
    archives::restore_row(db, table, row, true)
        .map_err(|e| crate::issues::Error::new("archive_sync", e.to_string()))?;
    Ok(true)
}

/// Installing capture does not change scheduling, authorities, or running work.
pub(super) fn install(db: &Connection, origin: &str) -> Result<()> {
    if db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_job_runs') AND NOT EXISTS(SELECT 1 FROM pragma_table_info('distributed_job_runs') WHERE name='parent_run_id')", [], |r| r.get::<_,bool>(0))? {
        let tx=db.unchecked_transaction()?;
        tx.execute_batch("DROP VIEW distributed_job_runs")?;
        tx.execute_batch(&SCHEMA[SCHEMA.find("CREATE VIEW").unwrap()..])?;
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            tx.execute_batch(&format!("DROP TRIGGER IF EXISTS distributed_capture_scheduled_job_runs_{operation}"))?;
        }
        capture::install(&tx)?;
        tx.commit()?;
    }
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_meta')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        let saved: String =
            db.query_row("SELECT origin FROM distributed_meta WHERE id=1", [], |r| {
                r.get(0)
            })?;
        if saved != origin {
            return Err(replica::invalid(
                "Replica identity changed; enroll restored data as a new replica",
            ));
        }
        identity::upgrade_shadow(db)?;
        if !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_archive_failures') AND EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_archive_supersessions')",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            let tx = db.unchecked_transaction()?;
            tx.execute_batch(SCHEMA)?;
            tx.commit()?;
        }
        // Additive protocol data needs capture on already-enrolled replicas too.
        if !db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_capture_worker_allocations_INSERT')", [], |r|r.get::<_,bool>(0))? {
            let tx=db.unchecked_transaction()?;
            capture::install(&tx)?;
            tx.commit()?;
        }
        graph::install(db)?;
        activation::protect_role(db)?;
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    tx.execute_batch(SCHEMA)?;
    tx.execute("INSERT OR IGNORE INTO distributed_meta(id,origin,controller) SELECT 1,?1,CASE WHEN role='agent' THEN NULL ELSE ?1 END FROM fleet_meta WHERE id=1", [origin])?;
    tx.execute("UPDATE distributed_meta SET legacy_cutoff=coalesce((SELECT max(seq) FROM fleet_outbox),0) WHERE id=1 AND controller IS NULL", [])?;
    let saved: String =
        tx.query_row("SELECT origin FROM distributed_meta WHERE id=1", [], |r| {
            r.get(0)
        })?;
    if saved != origin {
        return Err(replica::invalid(
            "Replica identity changed; enroll restored data as a new replica",
        ));
    }
    capture::install(&tx)?;
    resources::initialize(&tx)?;
    activation::protect_role(&tx)?;
    tx.commit()?;
    graph::install(db)?;
    Ok(())
}

/// The application writer stays held until the immutable record and checkpoint
/// are durable. A crash between the two commits repeats the same content hash.
pub(super) fn export(db: &Connection, engine: &mut hey_boss_sync::Engine) -> Result<usize> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM distributed_outbox WHERE clock>(SELECT exported FROM distributed_meta WHERE id=1))",
        [], |r| r.get::<_, bool>(0),
    )? {
        return Ok(0);
    }
    let tx = db.unchecked_transaction()?;
    let origin: String =
        tx.query_row("SELECT origin FROM distributed_meta WHERE id=1", [], |r| {
            r.get(0)
        })?;
    let rows = replica::rows(
        &tx,
        "SELECT * FROM distributed_outbox WHERE clock>(SELECT exported FROM distributed_meta WHERE id=1) ORDER BY clock LIMIT 128",
        &[],
    )?;
    let mut bytes = 0;
    let mut count = 0;
    for row in rows {
        let mut fact = Revision {
            protocol: PROTOCOL,
            origin: origin.clone(),
            clock: row["clock"].as_i64().unwrap(),
            table: row["table_name"].as_str().unwrap().into(),
            before: row["before_json"]
                .as_str()
                .map(serde_json::from_str)
                .transpose()?,
            after: row["after_json"]
                .as_str()
                .map(serde_json::from_str)
                .transpose()?,
            resource: None,
            archive: None,
        };
        if !archives::capture(&tx, &mut fact)? {
            break;
        }
        fact.resource = resources::capture(&tx, &fact)?;
        identity::normalize(&tx, &mut fact)?;
        let payload = serde_json::to_string(&fact)?;
        if count > 0 && bytes + payload.len() > 2 * 1024 * 1024 {
            break;
        }
        bytes += payload.len();
        engine.put(&payload)?;
        tx.execute(
            "UPDATE distributed_meta SET exported=?1 WHERE id=1",
            [fact.clock],
        )?;
        count += 1;
    }
    tx.commit()?;
    Ok(count)
}

/// Backfill one bounded page without holding the writer over a full snapshot.
/// Clock zero cannot overwrite an edit captured after enrollment.
pub(super) fn baseline(
    db: &Connection,
    engine: &mut hey_boss_sync::Engine,
    controller: bool,
) -> Result<bool> {
    use serde_json::json;
    let tables: Vec<_> = capture::TABLES
        .iter()
        .filter(|table| {
            controller
                || matches!(
                    **table,
                    "worker_runs" | "scheduled_job_runs" | "worker_allocations"
                )
        })
        .copied()
        .collect();
    let complete: usize = db.query_row(
        "SELECT count(*) FROM distributed_baseline WHERE complete=1 AND table_name IN (SELECT value FROM json_each(?1))",
        [serde_json::to_string(&tables)?], |r| r.get(0),
    )?;
    if complete == tables.len() {
        return Ok(true);
    }
    let tx = db.unchecked_transaction()?;
    let origin: String =
        tx.query_row("SELECT origin FROM distributed_meta WHERE id=1", [], |r| {
            r.get(0)
        })?;
    for table in &tables {
        tx.execute(
            "INSERT OR IGNORE INTO distributed_baseline(table_name) VALUES(?1)",
            [table],
        )?;
        let (after, complete): (i64, bool) = tx.query_row(
            "SELECT after_rowid,complete FROM distributed_baseline WHERE table_name=?1",
            [table],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if complete {
            continue;
        }
        let columns = capture::columns(&tx, table)?
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(",");
        let ownership = if *table == "worker_allocations" {
            " AND machine=(SELECT origin FROM distributed_meta WHERE id=1)"
        } else {
            ""
        };
        let rows = replica::rows(
            &tx,
            &format!(
                "SELECT rowid AS __cursor,{columns} FROM {table} WHERE rowid>?1{ownership} ORDER BY rowid LIMIT 64"
            ),
            &[json!(after)],
        )?;
        let mut cursor = after;
        let mut bytes = 0;
        let mut count = 0;
        for mut row in rows.iter().cloned() {
            let next_cursor = row
                .as_object_mut()
                .unwrap()
                .remove("__cursor")
                .unwrap()
                .as_i64()
                .unwrap();
            let mut fact = Revision {
                protocol: PROTOCOL,
                origin: origin.clone(),
                clock: 0,
                table: (*table).into(),
                before: None,
                after: Some(row),
                resource: None,
                archive: None,
            };
            if !archives::capture(&tx, &mut fact)? {
                tx.execute(
                    "UPDATE distributed_baseline SET after_rowid=?2 WHERE table_name=?1",
                    rusqlite::params![table, cursor],
                )?;
                tx.commit()?;
                return Ok(false);
            }
            fact.resource = resources::capture(&tx, &fact)?;
            identity::normalize(&tx, &mut fact)?;
            fact.origin = format!(
                "{}:baseline:{}",
                origin,
                &hey_boss_sync::digest(fact.after.as_ref().unwrap().to_string().as_bytes())[..16]
            );
            let payload = serde_json::to_string(&fact)?;
            if count > 0 && bytes + payload.len() > 2 * 1024 * 1024 {
                break;
            }
            engine.put(&payload)?;
            bytes += payload.len();
            count += 1;
            cursor = next_cursor;
        }
        tx.execute(
            "UPDATE distributed_baseline SET after_rowid=?2,complete=?3 WHERE table_name=?1",
            rusqlite::params![table, cursor, rows.len() < 64 && count == rows.len()],
        )?;
        tx.commit()?;
        return Ok(false);
    }
    tx.commit()?;
    Ok(true)
}

pub(super) fn import(db: &Connection, engine: &hey_boss_sync::Engine) -> Result<usize> {
    archives::retry_failed(db, engine)?;
    let after: i64 = db.query_row(
        "SELECT projected FROM distributed_meta WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    let facts = engine.pending(after)?;
    if facts.is_empty()
        && !db.query_row(
            "SELECT active=1 AND EXISTS(SELECT 1 FROM distributed_dirty) FROM distributed_meta WHERE id=1",
            [], |r| r.get::<_, bool>(0),
        )?
    {
        return Ok(0);
    }
    for (_, _, payload) in &facts {
        if let Some(fact) = archives::revision(payload)?
            && let Some(archive) = &fact.archive
        {
            archives::receive_or_retain(db, engine, archive)?;
        }
    }
    let tx = db.unchecked_transaction()?;
    for (position, _, payload) in &facts {
        if let Some(fact) = archives::revision(payload)? {
            if let Some(archive) = &fact.archive {
                archives::remember(&tx, archive)?;
            }
            if let Some(resource) = &fact.resource {
                resources::receive(&tx, resource)?;
            }
            projection::reduce(&tx, &fact)?;
            if fact.table == "distributed_members" {
                let key = projection::key(
                    &tx,
                    &fact.table,
                    fact.after.as_ref().or(fact.before.as_ref()).unwrap(),
                )?;
                tx.execute("UPDATE fleet_meta SET syncing=1 WHERE id=1", [])?;
                materialize::write(
                    &tx,
                    &fact.table,
                    &key,
                    projection::row(&tx, &fact.table, &key)?,
                )?;
                tx.execute("UPDATE fleet_meta SET syncing=0 WHERE id=1", [])?;
            }
        }
        tx.execute(
            "UPDATE distributed_meta SET projected=?1 WHERE id=1",
            [position],
        )?;
    }
    // Do not publish an earlier remote page over local changes whose exported
    // facts are still waiting later in this journal's delivery order.
    let ready:bool=tx.query_row("SELECT active=1 AND exported>=coalesce((SELECT max(clock) FROM distributed_outbox),0) AND projected=?1 FROM distributed_meta WHERE id=1",[engine.head()?],|r|r.get(0))?;
    if ready {
        materialize::apply(&tx)?;
    }
    tx.commit()?;
    Ok(facts.len())
}
