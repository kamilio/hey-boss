//! Completed runs retain scheduling and provenance metadata in the hot store.
use super::*;
use crate::database::{Connection as HotConnection, Transaction};
use serde_json::json;
use std::collections::HashMap;

pub(crate) fn worker_payload(
    db: &HotConnection,
    run: &str,
    project: &str,
) -> Result<(String, String)> {
    let saved: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT expanded_prompt,job,archive_key FROM worker_runs WHERE id=?1 AND project_id=?2",
            params![run, project],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (prompt, job, key) =
        saved.ok_or_else(|| Error::new("not_found", "Worker run was not found"))?;
    if let Some(key) = key {
        let value = Archive::read(&archive_path(db)?)?.get("worker-run", &key)?;
        let prompt = value["prompt"]
            .as_str()
            .ok_or_else(|| unavailable("Invalid archived worker prompt"))?;
        let job = value["job"]
            .as_str()
            .ok_or_else(|| unavailable("Invalid archived worker job"))?;
        return Ok((prompt.into(), job.into()));
    }
    Ok((prompt, job))
}

pub(crate) type EventTails = HashMap<String, Vec<(i64, Value)>>;

/// Both worker dashboards share this bounded lookup. Cold tails merge with any
/// late progress received after completion; stable row IDs prevent duplicates.
pub(crate) fn worker_event_tails(db: &HotConnection, runs: &[&str]) -> Result<EventTails> {
    let mut events = EventTails::new();
    if runs.is_empty() {
        return Ok(events);
    }
    let selected = serde_json::to_string(runs)?;
    let keys = db.query_collect("SELECT id,events_archive_key FROM worker_runs WHERE id IN (SELECT value FROM json_each(?1)) AND events_archive_key IS NOT NULL", [&selected], |r| -> rusqlite::Result<_> { Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)) })?;
    if !keys.is_empty() {
        let archive = Archive::read(&archive_path(db)?)?;
        for (run, key) in keys {
            let value = archive.get("worker-events", &key)?;
            let saved = value
                .as_array()
                .ok_or_else(|| unavailable("Invalid archived worker events"))?;
            let target = events.entry(run).or_default();
            for item in saved.iter().rev().take(12) {
                let id = item["id"]
                    .as_i64()
                    .ok_or_else(|| unavailable("Invalid archived event ID"))?;
                target.push((id, json!({"at":item["at"],"text":item["text"]})));
            }
        }
    }
    for (run,id,value) in db.query_collect(
        "SELECT e.run_id,e.id,e.created_at,e.text FROM json_each(?1) selected CROSS JOIN worker_events e WHERE e.id IN (SELECT id FROM worker_events WHERE run_id=selected.value ORDER BY id DESC LIMIT 12)",
        [&selected], |r| -> rusqlite::Result<_> { Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,json!({"at":r.get::<_,i64>(2)?,"text":r.get::<_,String>(3)?}))) }
    )? {
        let target = events.entry(run).or_default();
        target.retain(|(old,_)| *old != id);
        target.push((id,value));
    }
    for tail in events.values_mut() {
        tail.sort_unstable_by_key(|(id, _)| std::cmp::Reverse(*id));
        tail.truncate(12);
    }
    Ok(events)
}

pub(crate) fn archive_runs(db: &HotConnection, now: i64) -> Result<usize> {
    if !db.is_autocommit() {
        return Err(unavailable(
            "Archive maintenance cannot run inside a hot transaction",
        ));
    }
    let cutoff = now.saturating_sub(GRACE_MS);
    let snapshot = db.read_transaction()?;
    let saved: Option<(String,String,String,i64,i64,Option<String>)> = snapshot.query_row(
        "SELECT id,job,expanded_prompt,updated_at,finished_at,events_archive_key FROM worker_runs WHERE archive_key IS NULL AND finished_at<=?1 AND updated_at<=?1 AND NOT EXISTS(SELECT 1 FROM worker_events e WHERE e.run_id=worker_runs.id AND e.created_at>?1) ORDER BY finished_at,id LIMIT 1", [cutoff],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))
    ).optional()?;
    let Some((id, job, prompt, updated, finished, previous_events)) = saved else {
        return Ok(0);
    };
    let mut events = snapshot.query_collect("SELECT id,created_at,text FROM worker_events WHERE run_id=?1 ORDER BY id", [&id], |r| -> rusqlite::Result<_> { Ok(json!({"id":r.get::<_,i64>(0)?,"at":r.get::<_,i64>(1)?,"text":r.get::<_,String>(2)?})) })?;
    snapshot.commit()?;
    let last_id = events.last().and_then(|event| event["id"].as_i64());
    let archive = Archive::open(&archive_path(db)?)?;
    if let Some(previous) = &previous_events {
        let old = archive.get("worker-events", previous)?;
        let mut merged = std::collections::BTreeMap::new();
        for event in old
            .as_array()
            .ok_or_else(|| unavailable("Invalid archived worker events"))?
            .iter()
            .chain(events.iter())
        {
            let id = event["id"]
                .as_i64()
                .ok_or_else(|| unavailable("Invalid archived worker event ID"))?;
            merged.insert(id, event.clone());
        }
        events = merged.into_values().collect();
    }
    let job_key = archive.put("worker-run", &json!({"job":job,"prompt":prompt}))?;
    let events_key = archive.put("worker-events", &json!(events))?;
    let original: Value = serde_json::from_str(&job)?;
    let compact = json!({"issue":{"number":original["issue"]["number"],"title":original["issue"]["title"]},"resume_session":original["resume_session"],"config":{"cwd":original["config"]["cwd"]}});
    let tx = Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    let changed = tx.execute("UPDATE worker_runs SET job=?6,expanded_prompt='',archive_key=?7,events_archive_key=?8 WHERE id=?1 AND job=?2 AND expanded_prompt=?3 AND updated_at=?4 AND finished_at=?5 AND archive_key IS NULL AND (SELECT max(id) FROM worker_events WHERE run_id=?1) IS ?9 AND events_archive_key IS ?10", params![id,job,prompt,updated,finished,compact.to_string(),job_key,events_key,last_id,previous_events])?;
    if changed > 0 {
        // worker_events predates AUTOINCREMENT. Retain its last identity so
        // SQLite cannot reuse an archived rowid after moving the global tail.
        tx.execute(
            "DELETE FROM worker_events WHERE run_id=?1 AND id<?2",
            params![id, last_id],
        )?;
    }
    tx.commit()?;
    Ok(changed)
}
