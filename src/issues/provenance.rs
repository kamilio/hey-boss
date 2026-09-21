//! Immutable creation context. Never infer a caller from a project's newest run.
use super::{Actor, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

pub(super) fn migrate(db: &Connection) -> Result<()> {
    let mut missing = Vec::new();
    for table in ["issues", "artifacts"] {
        if !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name='origin')",
            [table],
            |r| r.get::<_, bool>(0),
        )? {
            missing.push(table);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let tx = rusqlite::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    for table in missing {
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name='origin')",
            [table],
            |r| r.get::<_, bool>(0),
        )? {
            tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN origin TEXT CHECK(origin IS NULL OR json_valid(origin));"))?;
            if table == "issues" {
                // Retain capture during migration and update existing triggers
                // atomically; IF NOT EXISTS would keep the old column list.
                let triggers = tx.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND name LIKE 'fleet_capture_issues_%'")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                for (name, sql) in triggers {
                    let sql = sql
                        .replace(
                            "json_object('project_id',NEW.",
                            "json_object('origin',NEW.origin,'project_id',NEW.",
                        )
                        .replace(
                            "json_object('project_id',OLD.",
                            "json_object('origin',OLD.origin,'project_id',OLD.",
                        );
                    tx.execute_batch(&format!("DROP TRIGGER {name}; {sql}"))?;
                }
            }
            tx.execute_batch(&format!("CREATE INDEX IF NOT EXISTS {table}_origin_session ON {table}(json_extract(origin,'$.session_id'),json_extract(origin,'$.host')); CREATE INDEX IF NOT EXISTS {table}_origin_run ON {table}(json_extract(origin,'$.run.id'),json_extract(origin,'$.host'));"))?;
        }
    }
    tx.execute_batch("CREATE INDEX IF NOT EXISTS worker_origin_session ON worker_runs(session_id,started_at DESC);")?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_capture_triggers_migrate_atomically_without_inventing_origins() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE issues(project_id TEXT,number INTEGER); CREATE TABLE artifacts(project_id TEXT,id TEXT); CREATE TABLE worker_runs(session_id TEXT,started_at INTEGER); CREATE TABLE fleet_outbox(after_json TEXT); CREATE TRIGGER fleet_capture_issues_INSERT AFTER INSERT ON issues BEGIN INSERT INTO fleet_outbox VALUES(json_object('project_id',NEW.\"project_id\",'number',NEW.\"number\")); END; INSERT INTO issues VALUES('Old',1);").unwrap();
        migrate(&db).unwrap();
        migrate(&db).unwrap();
        assert!(
            db.query_row("SELECT origin FROM issues WHERE number=1", [], |r| r
                .get::<_, Option<String>>(0))
                .unwrap()
                .is_none()
        );
        db.execute(
            "INSERT INTO issues VALUES('New',2,?1)",
            [json!({"session_id":"exact-session"}).to_string()],
        )
        .unwrap();
        let origin:String=db.query_row("SELECT json_extract(after_json,'$.origin') FROM fleet_outbox ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0)).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&origin).unwrap()["session_id"],
            "exact-session"
        );
    }
}

/// Authorize a historical reference against persisted, visible resources.
pub(crate) fn referenced(db: &Connection, host: &str, run: &str) -> Result<Option<Value>> {
    let (field, key) = run
        .strip_prefix("session:")
        .map(|s| ("session_id", s))
        .unwrap_or(("run.id", run));
    for table in ["issues", "artifacts"] {
        let origin: Option<String> = db.query_row(&format!("SELECT origin FROM {table} r JOIN projects p ON p.id=r.project_id WHERE json_extract(origin,'$.{field}')=?1 AND json_extract(origin,'$.host')=?2 AND p.hidden_at IS NULL LIMIT 1"),params![key,host],|r|r.get(0)).optional()?;
        if let Some(origin) = origin {
            return Ok(Some(serde_json::from_str(&origin)?));
        }
    }
    Ok(None)
}

pub(crate) fn saved_run(db: &Connection, run: &str) -> Result<Option<Value>> {
    let saved = db.query_row("SELECT r.id,r.project_id,p.name,r.issue_number,CASE WHEN json_valid(r.job) THEN json_extract(r.job,'$.issue.title') END,r.session_id,r.state,r.started_at,r.finished_at,r.actor_id FROM worker_runs r JOIN projects p ON p.id=r.project_id WHERE r.id=?1 AND p.hidden_at IS NULL",[run],|r|Ok(json!({"id":r.get::<_,String>(0)?,"project_id":r.get::<_,String>(1)?,"project_name":r.get::<_,String>(2)?,"number":r.get::<_,i64>(3)?,"title":r.get::<_,Option<String>>(4)?,"session_id":r.get::<_,Option<String>>(5)?,"state":r.get::<_,String>(6)?,"started_at":r.get::<_,i64>(7)?,"finished_at":r.get::<_,Option<i64>>(8)?,"actor_id":r.get::<_,String>(9)?}))).optional()?;
    if saved.is_some() {
        return Ok(saved);
    }
    let Some(session) = run.strip_prefix("session:") else {
        return Ok(None);
    };
    for table in ["issues", "artifacts"] {
        let saved: Option<Value> = db.query_row(&format!("SELECT r.project_id,p.name,r.title,r.origin FROM {table} r JOIN projects p ON p.id=r.project_id WHERE json_extract(origin,'$.session_id')=?1 AND p.hidden_at IS NULL LIMIT 1"),[session],|r|{
            let raw:String=r.get(3)?;
            let origin:Value=serde_json::from_str(&raw).map_err(|e|rusqlite::Error::FromSqlConversionFailure(3,rusqlite::types::Type::Text,Box::new(e)))?;
            Ok(json!({"id":run,"project_id":r.get::<_,String>(0)?,"project_name":r.get::<_,String>(1)?,"title":r.get::<_,String>(2)?,"session_id":session,"state":"completed","started_at":origin["created_at"],"finished_at":origin["created_at"],"actor_id":origin["actor_id"],"standalone":true}))
        }).optional()?;
        if saved.is_some() {
            return Ok(saved);
        }
    }
    Ok(None)
}

pub(crate) fn enrich_conversation(db: &Connection, run: &str, result: &mut Value) -> Result<()> {
    let (field, key) = run
        .strip_prefix("session:")
        .map(|s| ("session_id", s))
        .unwrap_or(("run.id", run));
    let mut resources = Vec::new();
    let mut more = false;
    for (table, kind, id) in [
        ("issues", "issue", "number"),
        ("artifacts", "artifact", "id"),
    ] {
        let mut stmt=db.prepare(&format!("SELECT r.project_id,r.{id},r.title FROM {table} r JOIN projects p ON p.id=r.project_id WHERE json_extract(origin,'$.{field}')=?1 AND p.hidden_at IS NULL ORDER BY r.created_at,r.{id} LIMIT 51"))?;
        let mut rows=stmt.query_map([key],|r|Ok(json!({"project_id":r.get::<_,String>(0)?,"kind":kind,"id":if kind=="issue" {r.get::<_,i64>(1)?.to_string()} else {r.get::<_,String>(1)?},"title":r.get::<_,String>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        more |= rows.len() > 50;
        rows.truncate(50);
        resources.extend(rows);
    }
    result["created_resources"] = json!(resources);
    result["more_created_resources"] = json!(more);
    Ok(())
}

pub(crate) fn source_run(
    db: &Connection,
    actor: &Actor,
    now: i64,
) -> Result<Option<super::CreationRun>> {
    Ok(if let Some(session) = &actor.session_id {
        db.query_row("SELECT r.id,r.project_id,r.issue_number,json_extract(r.job,'$.issue.title'),r.started_at FROM worker_runs r WHERE r.session_id=?1 AND r.machine=?2 AND r.started_at<=?3 AND (r.finished_at IS NULL OR r.finished_at>=?3) ORDER BY r.started_at DESC,r.id DESC LIMIT 1",params![session,actor.machine,now],|r|Ok(super::CreationRun{id:r.get(0)?,project_id:r.get(1)?,number:r.get(2)?,title:r.get(3)?,started_at:r.get(4)?})).optional()?
    } else {
        None
    })
}

pub(super) fn capture(db: &Connection, actor: &Actor, now: i64) -> Result<String> {
    let run = if actor.creation_run.is_some() {
        actor.creation_run.clone()
    } else {
        source_run(db, actor, now)?
    };
    Ok(json!({"actor_id":actor.id,"kind":actor.kind,"session_id":actor.session_id,"machine":actor.machine,"host":actor.host,"cwd":actor.cwd,"source":actor.source,"created_at":now,"invocation":actor.invocation,"run":run}).to_string())
}
