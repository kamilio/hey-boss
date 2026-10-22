use super::*;
use rusqlite::{OptionalExtension, params};
use serde_json::json;

fn where_key(key: &Value) -> Result<(String, Vec<Value>)> {
    let key = key
        .as_object()
        .ok_or_else(|| replica::invalid("Invalid entity key"))?;
    Ok((
        key.keys()
            .map(|k| format!("\"{k}\"=?"))
            .collect::<Vec<_>>()
            .join(" AND "),
        key.values().cloned().collect(),
    ))
}

fn issue_overlay(db: &Connection, row: &mut Value) -> Result<()> {
    // Keep the local claim while work is active, but allow terminal task changes
    // through. The owning worker observes cancellation through its normal checks.
    let current=replica::rows(db,"SELECT state,assignee,closed_by,closed_at,deleted_at,draft,manual_blocked,blockers,version,body,archive_key,archived_comments FROM issues WHERE project_id=?1 AND number=?2",
        &[row["project_id"].clone(),row["number"].clone()])?.into_iter().next();
    if let Some(ref current) = current {
        let active:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL)",params![row["project_id"].as_str(),row["number"].as_i64()],|r|r.get(0))?;
        if active && row["state"] == "open" && row["deleted_at"].is_null() {
            row["assignee"] = current["assignee"].clone();
        } else if row["state"] == "open" && foreign_assignment(db, row)? {
            row["assignee"] = Value::Null;
        }
        row["version"] = current["version"].clone();
        if !row["archive_key"].is_null() {
            // The reducer carries logical content; the hot row is only an
            // archive header. Replaying it must not hydrate or revise storage.
            row["body"] = if row["archive_key"] == current["archive_key"] {
                current["body"].clone()
            } else {
                json!("")
            };
        }
    } else if row["state"] == "open" && foreign_assignment(db, row)? {
        row["assignee"] = Value::Null;
    }
    if current.is_none() && !row["archive_key"].is_null() {
        row["body"] = json!("");
    }
    Ok(())
}

fn foreign_assignment(db: &Connection, row: &Value) -> Result<bool> {
    let Some(actor) = row["assignee"].as_str() else {
        return Ok(false);
    };
    if actor.starts_with("human:") {
        return Ok(false);
    }
    let finished:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 AND finished_at IS NOT NULL) AND NOT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 AND finished_at IS NULL)",params![row["project_id"].as_str(),row["number"].as_i64(),actor],|r|r.get(0))?;
    Ok(finished || !db.query_row("SELECT EXISTS(SELECT 1 FROM agents WHERE id=?1 AND json_extract(metadata,'$.machine')=(SELECT node FROM fleet_meta WHERE id=1))",[actor],|r|r.get::<_,bool>(0))?)
}

fn local_references(db: &Connection, table: &str, row: &mut Value) -> Result<()> {
    if table == "artifact_comments"
        && let Some(parent) = row["parent"].as_str()
    {
        row["parent"] = json!(
            identity::local(db, table, parent)?
                .ok_or_else(|| replica::invalid("Comment parent is pending"))?
        );
    }
    if table == "events" {
        let mut data: Value = serde_json::from_str(
            row["data"]
                .as_str()
                .ok_or_else(|| replica::invalid("Invalid event data"))?,
        )?;
        if let Some(comment) = data["comment_id"].as_str() {
            data["comment_id"] = json!(
                identity::local(db, "comments", comment)?
                    .ok_or_else(|| replica::invalid("Event comment is pending"))?
            );
            row["data"] = json!(data.to_string());
        }
    }
    Ok(())
}

pub(super) fn write(
    db: &Connection,
    table: &str,
    key: &Value,
    desired: Option<Value>,
) -> Result<()> {
    let mut key = key.clone();
    let alias = if identity::append(table) {
        Some(
            key["id"]
                .as_str()
                .ok_or_else(|| replica::invalid("Invalid history identity"))?
                .to_owned(),
        )
    } else {
        None
    };
    let local = alias
        .as_deref()
        .map(|id| identity::local(db, table, id))
        .transpose()?
        .flatten();
    if let Some(local) = local {
        key["id"] = json!(local);
    }
    let Some(mut row) = desired else {
        if alias.is_some() && local.is_none() {
            return Ok(());
        }
        // Attempts and task history are durable evidence, including deletions.
        if matches!(table, "worker_runs" | "scheduled_job_runs") {
            return Ok(());
        }
        let (predicate, values) = where_key(&key)?;
        replica::execute(
            db,
            &format!("DELETE FROM {table} WHERE {predicate}"),
            &values,
        )?;
        return Ok(());
    };
    if table == "worker_runs" {
        db.execute("INSERT INTO distributed_attempts VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(id) DO UPDATE SET state=excluded.state,finished_at=excluded.finished_at,summary=excluded.summary,payload=excluded.payload",
            params![row["id"].as_str(),row["machine"].as_str(),row["project_id"].as_str(),row["issue_number"].as_i64(),row["state"].as_str(),row["started_at"].as_i64(),row["finished_at"].as_i64(),row["summary"].as_str(),row.to_string()])?;
        return Ok(());
    }
    if table == "worker_allocations" {
        db.execute(
            "UPDATE worker_allocation_clock SET clock=max(clock,?1,?2) WHERE id=1",
            params![
                row["ticket"].as_i64(),
                row["launched"].as_i64().unwrap_or(0)
            ],
        )?;
    }
    if table == "scheduled_job_runs" {
        db.execute("INSERT INTO distributed_job_attempts(id,project_id,job_id,created_at,scheduled_at,state,payload) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET state=excluded.state,payload=excluded.payload",params![row["id"].as_str(),row["project_id"].as_str(),row["job_id"].as_str(),row["created_at"].as_i64(),row["scheduled_at"].as_i64(),row["state"].as_str(),row.to_string()])?;
        return Ok(());
    }
    if table == "scheduled_jobs" {
        let snapshot: crate::jobs::Snapshot = serde_json::from_str(
            row["snapshot"]
                .as_str()
                .ok_or_else(|| replica::invalid("Missing schedule snapshot"))?,
        )?;
        let root = std::path::Path::new(
            db.path()
                .ok_or_else(|| replica::invalid("Missing database path"))?,
        )
        .with_extension("jobs");
        crate::jobs::files::load(&root, &snapshot.instruction_digest)?;
        let previous: Option<(i64, Option<i64>)> = db
            .query_row(
                "SELECT revision,next_at FROM scheduled_jobs WHERE id=?1",
                [row["id"].as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row["next_at"] = if row["enabled"] == 0 {
            Value::Null
        } else if let Some((revision, next)) = previous
            && revision == snapshot.revision
        {
            json!(next)
        } else {
            json!(
                crate::jobs::schedule::Schedule::parse(
                    &snapshot.definition.cron,
                    &snapshot.definition.timezone
                )?
                .next(
                    row["schedule_from"]
                        .as_i64()
                        .ok_or_else(|| replica::invalid("Missing schedule start"))?
                )?
            )
        };
    }
    if table == "projects" {
        // Issue number allocation belongs to this replica, not to a row merge.
        row["next_number"] = json!(
            db.query_row(
                "SELECT next_number FROM projects WHERE id=?1",
                [row["id"].as_str()],
                |r| r.get::<_, i64>(0)
            )
            .optional()?
            .unwrap_or(1)
        );
        // Existing project names are permanent aliases in the application.
        if let Some(name) = db
            .query_row(
                "SELECT name FROM projects WHERE id=?1",
                [row["id"].as_str()],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            row["name"] = json!(name);
        }
    }
    if table == "issues" {
        issue_overlay(db, &mut row)?;
    }
    local_references(db, table, &mut row)?;
    if alias.is_some() {
        match local {
            Some(id) => {
                row["id"] = json!(id);
            }
            None => {
                row.as_object_mut().unwrap().remove("id");
            }
        }
    }
    if alias.is_none() || local.is_some() {
        let (predicate, values) = where_key(&key)?;
        if let Some(current) = replica::rows(
            db,
            &format!("SELECT * FROM {table} WHERE {predicate}"),
            &values,
        )?
        .into_iter()
        .next()
        {
            if row
                .as_object()
                .unwrap()
                .iter()
                .all(|(name, value)| current.get(name) == Some(value))
            {
                return Ok(());
            }
            if table == "issues" {
                row["version"] = json!(current["version"].as_i64().unwrap() + 1);
            }
        }
    }
    let names = row
        .as_object()
        .ok_or_else(|| replica::invalid("Invalid projected row"))?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let key_names = projection::keys(db, table)?;
    let updates = names
        .iter()
        .filter(|n| !key_names.contains(n))
        .map(|n| format!("\"{n}\"=excluded.\"{n}\""))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO {table}({}) VALUES({}) ON CONFLICT({}) DO UPDATE SET {updates}",
        names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(","),
        vec!["?"; names.len()].join(","),
        key_names.join(",")
    );
    replica::execute(
        db,
        &sql,
        &names.iter().map(|n| row[n].clone()).collect::<Vec<_>>(),
    )?;
    if let Some(alias) = alias
        && local.is_none()
    {
        db.execute(
            "INSERT INTO distributed_aliases VALUES(?1,?2,?3)",
            params![table, alias, db.last_insert_rowid()],
        )?;
    }
    Ok(())
}

pub(super) fn apply(db: &Connection) -> Result<usize> {
    let count = apply_page(db, &mut DependencyGraphs::new(), capture::TABLES)?;
    crate::issues::allocations::reconcile(db, &crate::issues::allocations::Clock::read()?)?;
    Ok(count)
}

type DependencyGraphs =
    std::collections::BTreeMap<String, std::collections::BTreeMap<i64, Vec<i64>>>;

/// Cutover holds one transaction with an immutable set of reduced facts. Reuse
/// dependency graphs across its pages; ordinary imports get a fresh cache.
pub(super) fn drain(db: &Connection) -> Result<()> {
    let mut blockers = DependencyGraphs::new();
    // Finish dependency tables before their users. An empty peer can have
    // thousands of actors/projects waiting ahead of its first task page.
    for table in capture::TABLES {
        while apply_page(db, &mut blockers, std::slice::from_ref(table))? > 0 {}
    }
    Ok(())
}

fn apply_page(db: &Connection, blockers: &mut DependencyGraphs, tables: &[&str]) -> Result<usize> {
    let mut count = 0;
    let mut projects = std::collections::BTreeSet::new();
    // Preserve immutable history without legacy notice deduplication or
    // arrival-ordered commit-link projections. Their state is reduced directly.
    db.execute("UPDATE fleet_meta SET syncing=2 WHERE id=1", [])?;
    // Dependencies precede their users. Unavailable parents stay dirty and are
    // retried from durable facts; receipt advancement never drops their data.
    for table in tables {
        let mut rebuilt = std::collections::BTreeSet::new();
        let mut pending = replica::rows(
            db,
            "SELECT entity FROM distributed_dirty d WHERE table_name=?1 AND NOT EXISTS(SELECT 1 FROM distributed_rejections r WHERE r.table_name=d.table_name AND r.entity=d.entity) ORDER BY entity LIMIT 128",
            &[json!(table)],
        )?;
        if pending.is_empty() {
            pending = replica::rows(
                db,
                "SELECT entity FROM distributed_dirty WHERE table_name=?1 ORDER BY entity LIMIT 128",
                &[json!(table)],
            )?;
        }
        for record in pending {
            let entity = record["entity"].as_str().unwrap();
            let key: Value = serde_json::from_str(entity)?;
            let mut desired = projection::row(db, table, &key)?;
            if *table == "issues"
                && let Some(row) = &mut desired
            {
                let project = row["project_id"].as_str().unwrap().to_owned();
                if !blockers.contains_key(&project) {
                    blockers.insert(project.clone(), graph::blockers(db, &project)?);
                }
                row["blockers"] = json!(serde_json::to_string(
                    blockers[&project]
                        .get(&row["number"].as_i64().unwrap())
                        .unwrap_or(&Vec::new())
                )?);
            }
            db.execute_batch("SAVEPOINT distributed_row")?;
            let mut complete = true;
            let result = if *table == "issues" {
                (|| -> Result<()> {
                    if let Some(row) = &mut desired {
                        complete = archives::project(db, row)?;
                    }
                    if complete {
                        write(db, table, &key, desired)?;
                    }
                    Ok(())
                })()
            } else if *table == "issue_subtasks" {
                let project = key["project_id"].as_str().unwrap();
                if !rebuilt.contains(project) {
                    graph::subtasks(db, project).map(|()| {
                        rebuilt.insert(project.to_owned());
                    })
                } else {
                    Ok(())
                }
            } else {
                write(db, table, &key, desired)
            };
            match result {
                Ok(()) => {
                    if matches!(*table, "issues" | "issue_subtasks" | "project_settings")
                        && let Some(project) = key["project_id"].as_str()
                    {
                        projects.insert(project.to_owned());
                    }
                    if complete {
                        db.execute(
                            "DELETE FROM distributed_dirty WHERE table_name=?1 AND entity=?2",
                            params![table, entity],
                        )?;
                    }
                    if !matches!(*table, "issue_subtasks") {
                        db.execute(
                            "DELETE FROM distributed_rejections WHERE table_name=?1 AND entity=?2",
                            params![table, entity],
                        )?;
                    }
                    db.execute_batch("RELEASE distributed_row")?;
                    count += 1;
                }
                Err(error) => {
                    db.execute_batch("ROLLBACK TO distributed_row; RELEASE distributed_row")?;
                    // Discovering a newly pending parent is progress: continue
                    // to later unseen rows instead of stopping at a failed page.
                    count += db.execute("INSERT INTO distributed_rejections VALUES(?1,?2,?3) ON CONFLICT(table_name,entity) DO UPDATE SET reason=excluded.reason WHERE reason<>excluded.reason",params![table,entity,error.to_string()])?;
                }
            }
        }
        if *table == "issues" {
            // Shadow facts can all arrive before any task is materialized.
            // Retry edges after each task page, including parents beyond the
            // first 128 rows, before treating a missing parent as a conflict.
            for project in &projects {
                db.execute("INSERT OR IGNORE INTO distributed_dirty SELECT table_name,entity FROM distributed_fields WHERE table_name='issue_subtasks' AND field='@alive' AND json_extract(entity,'$.project_id')=?1", [project])?;
            }
        }
    }
    for project in projects {
        crate::issues::blockers::reconcile_replication(db, &project)?;
    }
    db.execute("UPDATE fleet_meta SET syncing=0 WHERE id=1", [])?;
    Ok(count)
}
