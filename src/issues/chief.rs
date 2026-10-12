//! Read-only compatibility for saved conversations from the retired scheduler.
use crate::issues::{Result, worker};
use rusqlite::TransactionBehavior;
use serde_json::Value;

pub(in crate::issues) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS project_chiefs(
 project_id TEXT NOT NULL REFERENCES projects(id),machine TEXT NOT NULL,cwd TEXT NOT NULL,
 session_id TEXT,next_at INTEGER NOT NULL DEFAULT 0,owner_pid INTEGER,owner_start TEXT,
 pid INTEGER,process_start TEXT,state TEXT NOT NULL DEFAULT 'idle',summary TEXT NOT NULL DEFAULT '',
 worker_id TEXT REFERENCES issue_workers(id),started_at INTEGER,finished_at INTEGER,
 last_event TEXT NOT NULL DEFAULT '',queued INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(project_id,machine));";
pub(in crate::issues) const ACTIVITY_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS project_chiefs_worker ON project_chiefs(worker_id);";
const INTERVAL_MS: i64 = 60 * 60 * 1000;

pub(in crate::issues) fn migrate(db: &crate::database::Connection) -> Result<()> {
    let columns = db
        .prepare("SELECT name FROM pragma_table_info('project_chiefs')")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let additions = [
        ("worker_id", "TEXT REFERENCES issue_workers(id)"),
        ("started_at", "INTEGER"),
        ("finished_at", "INTEGER"),
        ("last_event", "TEXT NOT NULL DEFAULT ''"),
        ("retry_count", "INTEGER NOT NULL DEFAULT 0"),
        ("queued", "INTEGER NOT NULL DEFAULT 0"),
    ];
    let complete = additions
        .iter()
        .all(|(name, _)| columns.iter().any(|c| c == name));
    // Older installed workers may resume without updating the new ownership fields.
    // Healthy opens remain read-only; reconcile only an unambiguous live owner.
    let matching_owner = "SELECT MIN(w.id) FROM issue_workers w WHERE w.owner_pid=project_chiefs.owner_pid AND w.owner_start=project_chiefs.owner_start AND w.machine=project_chiefs.machine HAVING COUNT(*)=1";
    let needs_reconcile = complete && db.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM project_chiefs WHERE state='running' AND ({matching_owner}) IS NOT NULL AND (worker_id IS NOT ({matching_owner}) OR started_at IS NULL))"),
        [], |r| r.get::<_, bool>(0),
    )?;
    if complete && !needs_reconcile {
        crate::chief_ownership::migrate(db)?;
        return Ok(());
    }
    let tx = crate::database::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    let adding_retry = !columns.iter().any(|c| c == "retry_count");
    for (name, definition) in additions {
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('project_chiefs') WHERE name=?1)",
            [name],
            |r| r.get::<_, bool>(0),
        )? {
            tx.execute_batch(&format!(
                "ALTER TABLE project_chiefs ADD COLUMN {name} {definition};"
            ))?;
        }
    }
    if adding_retry {
        tx.execute("UPDATE project_chiefs SET retry_count=1,next_at=min(next_at,?1) WHERE state='blocked' AND owner_pid IS NULL",[worker::now()+30_000])?;
    }
    tx.execute(&format!("UPDATE project_chiefs SET worker_id=({matching_owner}),started_at=COALESCE(started_at,next_at-?1) WHERE state='running' AND ({matching_owner}) IS NOT NULL AND (worker_id IS NOT ({matching_owner}) OR started_at IS NULL)"), [INTERVAL_MS])?;
    crate::chief_ownership::migrate(&tx)?;
    tx.commit()?;
    Ok(())
}

const STATUS_QUERY: &str = "SELECT c.project_id,p.name,c.machine,c.state,c.pid,c.session_id,c.started_at,c.finished_at,c.next_at,c.summary,c.last_event,c.worker_id,COALESCE(s.chief_enabled,0),c.queued FROM json_each(?1) selected CROSS JOIN project_chiefs c ON c.worker_id=selected.value CROSS JOIN projects p ON p.id=c.project_id LEFT JOIN project_settings s ON s.project_id=c.project_id WHERE p.hidden_at IS NULL ORDER BY c.state='running' DESC,c.started_at DESC,c.project_id,c.machine";

pub(in crate::issues) fn status(
    db: &crate::database::Connection,
    worker: Option<&str>,
) -> Result<Vec<Value>> {
    let Some(worker) = worker else {
        return Ok(Vec::new());
    };
    Ok(status_for(db, &[worker])?
        .remove(worker)
        .unwrap_or_default())
}

pub(in crate::issues) fn status_for(
    db: &crate::database::Connection,
    workers: &[&str],
) -> Result<std::collections::HashMap<String, Vec<Value>>> {
    let mut by_worker = std::collections::HashMap::<String, Vec<Value>>::new();
    if workers.is_empty() {
        return Ok(by_worker);
    }
    let chiefs = db.query_collect(STATUS_QUERY, [serde_json::to_string(workers)?], |r| -> rusqlite::Result<_> {
        let project: String = r.get(0)?;
        let machine: String = r.get(2)?;
        let state: String = r.get(3)?;
        let running = state == "running";
        let next: i64 = r.get(8)?;
        let finished: Option<i64> = r.get(7)?;
        Ok(serde_json::json!({
            "id":format!("chief:{machine}:{project}"),"kind":"chief",
            "project_id":project,"project_name":r.get::<_,String>(1)?,
            "machine":machine,"worker_id":r.get::<_,Option<String>>(11)?,
            "enabled":r.get::<_,bool>(12)?,"title":"Organizing project","state":state,"pid":r.get::<_,Option<u32>>(4)?,
            "session_id":r.get::<_,Option<String>>(5)?,"started_at":r.get::<_,Option<i64>>(6)?,
            "finished_at":if running { None } else { Some(finished.unwrap_or(next-INTERVAL_MS)) },
            "queued":r.get::<_,bool>(13)? || running && next<=worker::now(),
            "next_at":next,"summary":r.get::<_,String>(9)?,"last_event":r.get::<_,String>(10)?
        }))
    })?;
    let models = super::actor_labels::models(db, chiefs.iter())?;
    for mut chief in chiefs {
        chief["model"] = models
            .get(&format!(
                "codex:{}",
                chief["session_id"].as_str().unwrap_or_default()
            ))
            .cloned()
            .unwrap_or(Value::Null);
        by_worker
            .entry(chief["worker_id"].as_str().unwrap().into())
            .or_default()
            .push(chief);
    }
    Ok(by_worker)
}

impl crate::issues::Store {
    pub(crate) fn stop_retired_chiefs(&self) -> Result<()> {
        crate::chief_ownership::stop_unassigned(&self.db)
    }
}
