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
            if value["run"] != run {
                return Err(unavailable("Archived worker events belong to another run"));
            }
            let saved = value["tail"]
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
    if cleanup_runs(db)? > 0 {
        return Ok(1);
    }
    let cutoff = now.saturating_sub(GRACE_MS);
    let snapshot = db.read_transaction()?;
    let saved: Option<(String,String,String,i64,i64,Option<String>,Option<String>)> = snapshot.query_row(
        "SELECT id,job,expanded_prompt,updated_at,finished_at,events_archive_key,archive_key FROM worker_runs WHERE archive_pending=1 AND finished_at IS NOT NULL AND finished_at<=?1 AND updated_at<=?1 AND NOT EXISTS(SELECT 1 FROM worker_events e WHERE e.run_id=worker_runs.id AND e.created_at>?1) ORDER BY finished_at,id LIMIT 1", [cutoff],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))
    ).optional()?;
    let Some((id, job, prompt, updated, finished, previous_events, previous_job)) = saved else {
        return Ok(0);
    };
    let last_id: Option<i64> = snapshot.query_row(
        "SELECT max(id) FROM worker_events WHERE run_id=?1",
        [&id],
        |r| r.get(0),
    )?;
    if previous_events.is_some() || previous_job.is_some() {
        Archive::read(&archive_path(db)?)?;
    }
    let archive = Archive::open(&archive_path(db)?)?;
    let copy = archive.db.unchecked_transaction()?;
    let mut header = match &previous_events {
        Some(key) => archive.get("worker-events", key)?,
        None => json!({"run":id,"parts":[],"tail":[],"last":null,"count":0}),
    };
    if header["run"] != id || !header["parts"].is_array() || !header["tail"].is_array() {
        return Err(unavailable("Invalid worker event archive"));
    }
    let mut cursor = header["last"].as_i64().unwrap_or(0);
    let mut part = Vec::new();
    let mut bytes = 2;
    loop {
        let events=snapshot.query_collect("SELECT id,created_at,text FROM worker_events WHERE run_id=?1 AND id>?2 ORDER BY id LIMIT 16",params![id,cursor],|r|->rusqlite::Result<_>{Ok(json!({"id":r.get::<_,i64>(0)?,"at":r.get::<_,i64>(1)?,"text":r.get::<_,String>(2)?}))})?;
        if events.is_empty() {
            break;
        }
        for event in events {
            let size = serde_json::to_vec(&event)?.len() + 1;
            if !part.is_empty() && (bytes + size > 256 * 1024 || part.len() >= 256) {
                save_part(&archive, &mut header, std::mem::take(&mut part))?;
                bytes = 2;
            }
            cursor = event["id"].as_i64().unwrap();
            bytes += size;
            part.push(event);
        }
    }
    if !part.is_empty() {
        save_part(&archive, &mut header, part)?;
    }
    let job_key = match &previous_job {
        Some(key) => key.clone(),
        None => archive.put("worker-run", &json!({"job":job,"prompt":prompt}))?,
    };
    let events_key = archive.put("worker-events", &header)?;
    copy.commit()?;
    snapshot.commit()?;
    archive.get("worker-events", &events_key)?;
    let original: Value = serde_json::from_str(&job)?;
    let compact = json!({"issue":{"number":original["issue"]["number"],"title":original["issue"]["title"]},"resume_session":original["resume_session"],"session_ref":original["session_ref"],"config":{"cwd":original["config"]["cwd"],"provider":original["config"]["provider"]}});
    let tx = Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    let changed = tx.execute("UPDATE worker_runs SET job=?6,expanded_prompt='',archive_key=?7,events_archive_key=?8,archive_event_id=?12,archive_pending=0,archive_cleanup=1 WHERE id=?1 AND job=?2 AND expanded_prompt=?3 AND updated_at=?4 AND finished_at=?5 AND archive_key IS ?11 AND (SELECT max(id) FROM worker_events WHERE run_id=?1) IS ?9 AND events_archive_key IS ?10", params![id,job,prompt,updated,finished,compact.to_string(),job_key,events_key,last_id,previous_events,previous_job,header["last"].as_i64().unwrap_or(0)])?;
    tx.commit()?;
    if changed > 0 {
        cleanup_runs(db)?;
    }
    Ok(changed)
}

fn save_part(archive: &Archive, header: &mut Value, events: Vec<Value>) -> Result<()> {
    let key = archive.put("worker-event-part", &json!(events))?;
    let first = events.first().unwrap()["id"].clone();
    let last = events.last().unwrap()["id"].clone();
    header["parts"]
        .as_array_mut()
        .ok_or_else(|| unavailable("Invalid worker archive parts"))?
        .push(json!({"key":key,"first":first,"last":last,"count":events.len()}));
    header["count"] = json!(
        header["count"]
            .as_u64()
            .ok_or_else(|| unavailable("Invalid worker archive count"))?
            + events.len() as u64
    );
    header["last"] = last;
    let tail = header["tail"]
        .as_array_mut()
        .ok_or_else(|| unavailable("Invalid worker archive tail"))?;
    tail.extend(events);
    if tail.len() > 12 {
        tail.drain(..tail.len() - 12);
    }
    Ok(())
}

fn cleanup_runs(db: &HotConnection) -> Result<usize> {
    let saved:Option<(String,String,i64)>=db.query_row("SELECT id,events_archive_key,archive_event_id FROM worker_runs WHERE archive_cleanup=1 AND finished_at IS NOT NULL LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((run, key, last)) = saved else {
        return Ok(0);
    };
    let archive = Archive::read(&archive_path(db)?)?;
    let header = archive.get("worker-events", &key)?;
    if header["run"] != run || header["last"].as_i64().unwrap_or(0) != last {
        return Err(unavailable("Worker archive boundary does not match"));
    }
    let rows=db.query_collect("SELECT id,created_at,text FROM worker_events WHERE run_id=?1 AND id<?2 ORDER BY id LIMIT 16",params![run,last],|r|->rusqlite::Result<_>{Ok(json!({"id":r.get::<_,i64>(0)?,"at":r.get::<_,i64>(1)?,"text":r.get::<_,String>(2)?}))})?;
    let mut part_key = String::new();
    let mut contents = Value::Null;
    for row in &rows {
        let id = row["id"].as_i64().unwrap();
        let part = header["parts"]
            .as_array()
            .ok_or_else(|| unavailable("Invalid worker archive parts"))?
            .iter()
            .find(|part| {
                part["first"].as_i64().is_some_and(|first| first <= id)
                    && part["last"].as_i64().is_some_and(|last| id <= last)
            })
            .ok_or_else(|| unavailable("Archived worker event is missing"))?;
        let next = part["key"]
            .as_str()
            .ok_or_else(|| unavailable("Invalid worker archive part key"))?;
        if part_key != next {
            contents = archive.get("worker-event-part", next)?;
            part_key = next.into();
        }
        if !contents
            .as_array()
            .ok_or_else(|| unavailable("Invalid worker archive part"))?
            .iter()
            .any(|saved| saved == row)
        {
            return Err(unavailable(
                "Worker event archive does not match its hot copy",
            ));
        }
    }
    let tx = Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    if !tx.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE id=?1 AND events_archive_key=?2 AND archive_event_id=?3 AND finished_at IS NOT NULL)",params![run,key,last],|r|r.get::<_,bool>(0))? { return Ok(0); }
    let actual=tx.query_collect("SELECT id,created_at,text FROM worker_events WHERE run_id=?1 AND id<?2 ORDER BY id LIMIT 16",params![run,last],|r|->rusqlite::Result<_>{Ok(json!({"id":r.get::<_,i64>(0)?,"at":r.get::<_,i64>(1)?,"text":r.get::<_,String>(2)?}))})?;
    if actual != rows {
        return Ok(0);
    }
    let ids: Vec<_> = rows.iter().map(|row| row["id"].as_i64().unwrap()).collect();
    let removed = tx.execute(
        "DELETE FROM worker_events WHERE run_id=?1 AND id IN (SELECT value FROM json_each(?2))",
        params![run, serde_json::to_string(&ids)?],
    )?;
    // The anchor preserves global rowid allocation for this pre-AUTOINCREMENT table.
    tx.execute("UPDATE worker_runs SET archive_cleanup=0 WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM worker_events WHERE run_id=?1 AND id<?2)",params![run,last])?;
    tx.commit()?;
    Ok(removed)
}
