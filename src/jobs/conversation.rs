//! Read-only identity for a scheduled execution, independent of worker history.
use crate::{database::Connection, issues::Result};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};

pub(crate) fn saved(db: &Connection, reference: &str) -> Result<Option<Value>> {
    let Some(id) = reference.strip_prefix("job:") else {
        return Ok(None);
    };
    super::validate_id(id)?;
    let exists = |table: &str| {
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |r| r.get::<_, bool>(0),
        )
    };
    let mut source = None;
    if exists("scheduled_job_runs")? {
        source = db.query_row("SELECT r.snapshot,r.session_id,r.machine,r.state,r.started_at,r.finished_at,r.task_number,p.name FROM scheduled_job_runs r JOIN projects p ON p.id=r.project_id WHERE r.id=?1 AND p.hidden_at IS NULL",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<i64>>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,Option<i64>>(6)?,r.get::<_,String>(7)?))).optional()?;
    }
    // A companion keeps the dispatched snapshot and its own saved session even
    // after the authority acknowledges completion or the schedule is deleted.
    if source.is_none() && exists("local_job_executions")? {
        source = db.query_row("SELECT json_extract(e.dispatch,'$.run.snapshot'),json_extract(e.report,'$.session_id'),json_extract(e.dispatch,'$.node'),json_extract(e.report,'$.state'),json_extract(e.report,'$.started_at'),json_extract(e.report,'$.finished_at'),json_extract(e.dispatch,'$.run.task_number'),p.name FROM local_job_executions e JOIN projects p ON p.id=json_extract(e.dispatch,'$.run.snapshot.project_id') WHERE e.run_id=?1 AND p.hidden_at IS NULL",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<i64>>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,Option<i64>>(6)?,r.get::<_,String>(7)?))).optional()?;
    }
    source.map(|(snapshot,session,machine,state,started,finished,number,project_name)| -> Result<Value> {
        let snapshot: super::Snapshot = serde_json::from_str(&snapshot)?;
        Ok(json!({"id":reference,"kind":"job","standalone":true,"project_id":snapshot.project_id,"project_name":project_name,"number":number,"title":snapshot.definition.name,"session_id":session,"machine":machine,"state":state,"started_at":started,"finished_at":finished,"actor_id":session.as_ref().map(|s|format!("{}:{s}",snapshot.definition.harness)),"model":snapshot.definition.model}))
    }).transpose()
}
