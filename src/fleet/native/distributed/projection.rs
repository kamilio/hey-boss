use super::*;
use rusqlite::{OptionalExtension, params};
use serde_json::{Map, json};
use std::collections::BTreeMap;

pub(super) fn keys(db: &Connection, table: &str) -> Result<Vec<String>> {
    if !capture::TABLES.contains(&table) {
        return Err(replica::invalid("Unknown distributed table"));
    }
    if table == "scheduled_job_runs" {
        return Ok(vec!["id".into()]);
    }
    let mut columns = replica::rows(db, &format!("PRAGMA table_info({table})"), &[])?;
    columns.sort_by_key(|c| c["pk"].as_i64().unwrap_or(0));
    Ok(columns
        .into_iter()
        .filter(|c| c["pk"].as_i64().unwrap_or(0) > 0)
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect())
}

pub(super) fn key(db: &Connection, table: &str, row: &Value) -> Result<Value> {
    let mut key = Map::new();
    for name in keys(db, table)? {
        let value = row
            .get(&name)
            .filter(|v| !v.is_null() && !v.is_object() && !v.is_array())
            .ok_or_else(|| replica::invalid("Missing distributed row identity"))?;
        key.insert(name, value.clone());
    }
    if key.is_empty() {
        return Err(replica::invalid("Missing distributed primary key"));
    }
    Ok(Value::Object(key))
}

fn group<'a>(table: &str, column: &'a str) -> &'a str {
    match table {
        // Keep SQL lifecycle and scheduling constraints atomic while permitting
        // independent document and metadata edits to merge field by field.
        "issues"
            if matches!(
                column,
                "state"
                    | "assignee"
                    | "closed_by"
                    | "closed_at"
                    | "deleted_at"
                    | "draft"
                    | "manual_blocked"
                    | "blockers"
            ) =>
        {
            "@lifecycle"
        }
        // Storage-only cooling/restoration must not overwrite a concurrent
        // requirements edit just because it has a later logical clock.
        "issues" if matches!(column, "archive_key" | "archived_comments") => "@storage",
        "scheduled_jobs" | "scheduled_job_revisions" | "scheduled_job_runs" | "worker_runs" => {
            "@row"
        }
        _ => column,
    }
}

fn fields(table: &str, row: &Value) -> Result<BTreeMap<String, Value>> {
    let mut fields = BTreeMap::<String, Value>::new();
    for (name, value) in row
        .as_object()
        .ok_or_else(|| replica::invalid("Invalid distributed row"))?
    {
        let group = group(table, name);
        if group.starts_with('@') {
            fields
                .entry(group.into())
                .or_insert(json!({}))
                .as_object_mut()
                .unwrap()
                .insert(name.clone(), value.clone());
        } else {
            fields.insert(name.clone(), value.clone());
        }
    }
    Ok(fields)
}

pub(super) fn reduce(db: &Connection, fact: &Revision) -> Result<()> {
    if fact.protocol != PROTOCOL
        || fact.clock < 0
        || fact.origin.is_empty()
        || fact.origin.len() > 128
        || fact.before.is_none() && fact.after.is_none()
    {
        return Err(replica::invalid("Invalid distributed revision"));
    }
    let columns = capture::columns(db, &fact.table)?;
    for row in [&fact.before, &fact.after].into_iter().flatten() {
        let object = row
            .as_object()
            .ok_or_else(|| replica::invalid("Invalid distributed row"))?;
        if object.keys().any(|column| !columns.contains(column)) {
            return Err(replica::invalid(
                "Unsupported distributed column; upgrade both peers",
            ));
        }
    }
    let before_key = fact
        .before
        .as_ref()
        .map(|r| key(db, &fact.table, r))
        .transpose()?;
    let after_key = fact
        .after
        .as_ref()
        .map(|r| key(db, &fact.table, r))
        .transpose()?;
    if before_key.is_some() && after_key.is_some() && before_key != after_key {
        return Err(replica::invalid("Distributed primary keys are immutable"));
    }
    // Cold history may predate enrollment and have no ordinary birth fact.
    // Its before-image supplies missing clock-zero fields, never a new birth
    // at the update clock that could resurrect a concurrent deletion.
    if matches!(
        fact.table.as_str(),
        "comments" | "events" | "issue_status_updates"
    ) && let Some(before) = &fact.before
    {
        seed_history(db, &fact.table, before, "before")?;
    }
    let entity = after_key.or(before_key).unwrap().to_string();
    let before = fact
        .before
        .as_ref()
        .map(|r| fields(&fact.table, r))
        .transpose()?
        .unwrap_or_default();
    let after = fact
        .after
        .as_ref()
        .map(|r| fields(&fact.table, r))
        .transpose()?
        .unwrap_or_default();
    let mut changes = after
        .into_iter()
        .filter(|(name, value)| before.get(name) != Some(value))
        .collect::<BTreeMap<_, _>>();
    // Requirement edits do not manufacture a new completion. The snapshot
    // travels only with an actual lifecycle transition (or initial creation).
    if fact.table == "issues"
        && let Some(lifecycle) = changes.get_mut("@lifecycle")
    {
        let row = fact.after.as_ref().unwrap();
        lifecycle["@requirements"] = json!([row["title"], row["body"], row["labels"]]);
    }
    if fact.table == "issues"
        && let Some(storage) = changes.get_mut("@storage")
    {
        storage["@archived_body"] = fact.after.as_ref().unwrap()["body"].clone();
    }
    if fact.before.is_none() {
        changes.insert("@alive".into(), json!(true));
    }
    if fact.after.is_none() {
        changes.insert("@alive".into(), json!(false));
    }
    let mut changed = false;
    for (field, value) in changes {
        let encoded = value.to_string();
        let previous: Option<String> = db.query_row("SELECT value FROM distributed_fields WHERE table_name=?1 AND entity=?2 AND field=?3 AND clock=?4 AND origin=?5",
            params![fact.table,entity,field,fact.clock,fact.origin],|r|r.get(0)).optional()?;
        if previous.is_some_and(|v| v != encoded) {
            return Err(replica::invalid("A replica reused a logical revision"));
        }
        changed |= db.execute("INSERT INTO distributed_fields VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(table_name,entity,field) DO UPDATE SET clock=excluded.clock,origin=excluded.origin,value=excluded.value WHERE (excluded.clock,excluded.origin)>(clock,origin)",
            params![fact.table,entity,field,fact.clock,fact.origin,encoded])?>0;
    }
    db.execute(
        "UPDATE distributed_meta SET clock=max(clock,?1) WHERE id=1",
        [fact.clock],
    )?;
    if changed {
        if fact.clock > 0
            && matches!(
                fact.table.as_str(),
                "comments" | "events" | "issue_status_updates"
            )
        {
            let row = fact.after.as_ref().or(fact.before.as_ref()).unwrap();
            db.execute(
                "INSERT OR IGNORE INTO distributed_history_edits VALUES(?1,?2)",
                params![row["project_id"].as_str(), row["issue_number"].as_i64()],
            )?;
            db.execute(
                "INSERT OR IGNORE INTO distributed_dirty VALUES('issues',?1)",
                [
                    json!({"project_id":row["project_id"],"number":row["issue_number"]})
                        .to_string(),
                ],
            )?;
        }
        db.execute(
            "INSERT OR IGNORE INTO distributed_dirty VALUES(?1,?2)",
            params![fact.table, entity],
        )?;
        if fact.table == "issues" {
            let row = fact.after.as_ref().or(fact.before.as_ref()).unwrap();
            // A previously received edge may have been waiting for this task.
            db.execute("INSERT OR IGNORE INTO distributed_dirty SELECT table_name,entity FROM distributed_fields WHERE table_name='issue_subtasks' AND field='@alive' AND json_extract(entity,'$.project_id')=?1", [row["project_id"].as_str()])?;
        }
    }
    if changed
        && fact.table == "issues"
        && fact
            .before
            .as_ref()
            .and_then(|r| r["blockers"].as_str())
            .unwrap_or("[]")
            != fact
                .after
                .as_ref()
                .and_then(|r| r["blockers"].as_str())
                .unwrap_or("[]")
    {
        let row = fact.after.as_ref().or(fact.before.as_ref()).unwrap();
        // Cycle resolution can change other proposed edges. Independent tasks
        // have no edges to reconsider; creating one must not replay the entire
        // project, including its archived history. The changed row is already
        // dirty above, even when it just lost its last edge.
        db.execute("INSERT OR IGNORE INTO distributed_dirty SELECT table_name,entity FROM distributed_fields WHERE table_name='issues' AND field='@lifecycle' AND json_extract(entity,'$.project_id')=?1 AND json_array_length(json_extract(value,'$.blockers'))>0",[row["project_id"].as_str()])?;
    }
    if fact.table == "worker_runs"
        && let Some(row) = &fact.after
        && !row["finished_at"].is_null()
    {
        db.execute(
            "INSERT OR IGNORE INTO distributed_dirty VALUES('issues',?1)",
            [json!({"project_id":row["project_id"],"number":row["issue_number"]}).to_string()],
        )?;
    }
    Ok(())
}

pub(super) fn seed_history(db: &Connection, table: &str, row: &Value, source: &str) -> Result<()> {
    reduce(
        db,
        &Revision {
            protocol: PROTOCOL,
            // Synthetic baselines sort below enrolled machine baselines.
            origin: format!(
                "!{source}:{}",
                hey_boss_sync::digest(row.to_string().as_bytes())
            ),
            clock: 0,
            table: table.into(),
            before: None,
            after: Some(row.clone()),
            resource: None,
            archive: None,
        },
    )
}

pub(super) fn row(db: &Connection, table: &str, key: &Value) -> Result<Option<Value>> {
    let values = replica::rows(
        db,
        "SELECT field,value FROM distributed_fields WHERE table_name=?1 AND entity=?2 ORDER BY field",
        &[json!(table), json!(key.to_string())],
    )?;
    let mut result = key.clone();
    let mut alive = false;
    for field in values {
        let name = field["field"].as_str().unwrap();
        let value: Value = serde_json::from_str(field["value"].as_str().unwrap())?;
        match name {
            "@alive" => alive = value == true,
            "@row" | "@lifecycle" | "@storage" | "@content" => {
                if name == "@content" {
                    result["@archived_body"] = value["body"].clone();
                }
                result.as_object_mut().unwrap().extend(
                    value
                        .as_object()
                        .ok_or_else(|| replica::invalid("Invalid atomic field group"))?
                        .clone(),
                );
            }
            _ => {
                result[name] = value;
            }
        }
    }
    if !alive {
        return Ok(None);
    }
    if table == "issues" {
        let archived_body = result.as_object_mut().unwrap().remove("@archived_body");
        if !result["archive_key"].is_null() && archived_body != Some(result["body"].clone()) {
            // The winning body is newer/different than the chosen cold copy.
            // Hydrate its history before publishing the merged hot content.
            result["archive_key"] = Value::Null;
            result["archived_comments"] = json!(0);
        }
        let requirements = result.as_object_mut().unwrap().remove("@requirements");
        if matches!(result["state"].as_str(), Some("closed" | "ready"))
            && requirements != Some(json!([result["title"], result["body"], result["labels"]]))
        {
            result["state"] = json!("open");
            result["assignee"] = Value::Null;
            result["closed_by"] = Value::Null;
            result["closed_at"] = Value::Null;
        }
    }
    Ok(Some(result))
}
