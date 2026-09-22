//! Transactional issue replication using the CLI's bundled SQLite.
use super::Result;
use rusqlite::{
    Connection, OptionalExtension, params_from_iter,
    types::{Value as SqlValue, ValueRef},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const TABLES: &[(&str, &[&str])] = &[
    ("projects", &["id"]),
    ("agents", &["id"]),
    ("issues", &["project_id", "number"]),
    ("issue_status_updates", &["id"]),
    (
        "issue_agent_launches",
        &["project_id", "issue_number", "run_id"],
    ),
    ("issue_subtasks", &["project_id", "child_number"]),
    ("comments", &["id"]),
    ("events", &["id"]),
    ("project_settings", &["project_id"]),
    ("global_settings", &["id"]),
    (
        "issue_pull_requests",
        &["project_id", "issue_number", "url"],
    ),
];
pub(super) const READY: &str = "i.draft=0 AND EXISTS(SELECT 1 FROM issue_pickup_ready ready WHERE ready.project_id=i.project_id AND ready.number=i.number)";
const ALLOCATED: &str = "(EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL) OR (i.draft=0 AND EXISTS(SELECT 1 FROM issue_pickup_ready ready WHERE ready.project_id=i.project_id AND ready.number=i.number)))";
pub(super) fn invalid(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message).into()
}
pub(super) fn args(values: &[Value]) -> Vec<SqlValue> {
    values
        .iter()
        .map(|v| match v {
            Value::Null => SqlValue::Null,
            Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
            Value::Number(n) => n
                .as_i64()
                .map(SqlValue::Integer)
                .unwrap_or_else(|| SqlValue::Real(n.as_f64().unwrap())),
            Value::String(s) => SqlValue::Text(s.clone()),
            _ => SqlValue::Text(v.to_string()),
        })
        .collect()
}
pub(super) fn execute(db: &Connection, sql: &str, values: &[Value]) -> Result<usize> {
    Ok(db.execute(sql, params_from_iter(args(values)))?)
}
pub(super) fn rows(db: &Connection, sql: &str, values: &[Value]) -> Result<Vec<Value>> {
    let mut stmt = db.prepare(sql)?;
    let columns = stmt
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let mut cursor = stmt.query(params_from_iter(args(values)))?;
    let mut result = vec![];
    while let Some(row) = cursor.next()? {
        let mut value = serde_json::Map::new();
        for (index, name) in columns.iter().enumerate() {
            let item = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Real(n) => json!(n),
                ValueRef::Text(s) => json!(std::str::from_utf8(s)?),
                ValueRef::Blob(_) => return Err(invalid("Unexpected blob in fleet row")),
            };
            value.insert(name.clone(), item);
        }
        result.push(Value::Object(value));
    }
    Ok(result)
}
pub(super) fn state_get(db: &Connection, key: &str, default: Value) -> Result<Value> {
    let saved: Option<String> = db
        .query_row("SELECT value FROM fleet_state WHERE key=?", [key], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(match saved {
        Some(s) => serde_json::from_str(&s)?,
        None => default,
    })
}
pub(super) fn state_set(db: &Connection, key: &str, value: &Value) -> Result<()> {
    db.execute(
        "INSERT INTO fleet_state VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [key, &value.to_string()],
    )?;
    Ok(())
}
fn keys(table: &str) -> Result<&'static [&'static str]> {
    TABLES
        .iter()
        .find(|(t, _)| *t == table)
        .map(|(_, k)| *k)
        .ok_or_else(|| invalid("Unknown replicated table"))
}
fn append(table: &str) -> bool {
    matches!(table, "comments" | "events")
}
fn key_where(table: &str, row: &Value) -> Result<(String, Vec<Value>)> {
    let k = keys(table)?;
    Ok((
        k.iter()
            .map(|k| format!("\"{k}\"=?"))
            .collect::<Vec<_>>()
            .join(" AND "),
        k.iter().map(|k| row[*k].clone()).collect(),
    ))
}
pub(super) fn current_row(db: &Connection, table: &str, row: &Value) -> Result<Value> {
    let (clause, values) = key_where(table, row)?;
    Ok(rows(
        db,
        &format!("SELECT * FROM {table} WHERE {clause}"),
        &values,
    )?
    .into_iter()
    .next()
    .unwrap_or(Value::Null))
}
pub(super) fn put_row(db: &Connection, table: &str, row: &Value) -> Result<()> {
    let k = keys(table)?;
    let mut row = row.clone();
    if table == "issues" {
        let m = row
            .as_object_mut()
            .ok_or_else(|| invalid("Invalid issue row"))?;
        let manual = i64::from(m["state"] == "blocked");
        m.entry("manual_blocked").or_insert(json!(manual));
        m.entry("blockers").or_insert(json!("[]"));
        m.entry("draft").or_insert(json!(0));
        m.entry("plan").or_insert(Value::Null);
        if m.get("origin").is_none_or(Value::is_null) {
            let existing: Option<Option<String>> = db
                .query_row(
                    "SELECT origin FROM issues WHERE project_id=?1 AND number=?2",
                    rusqlite::params![m["project_id"].as_str(), m["number"].as_i64()],
                    |r| r.get(0),
                )
                .optional()?;
            m.insert("origin".into(), json!(existing.flatten()));
        }
    }
    if table == "project_settings" {
        let m = row
            .as_object_mut()
            .ok_or_else(|| invalid("Invalid settings row"))?;
        m.entry("drafts_enabled").or_insert(json!(1));
        m.entry("plan_template")
            .or_insert(json!("plans/{timestamp}-{number}.md"));
        m.entry("worktree_enabled").or_insert(json!(0));
        m.entry("prompt_overrides").or_insert(json!("{}"));
        m.entry("chief_enabled").or_insert(json!(0));
        m.entry("chief_prompt").or_insert(Value::Null);
    }
    if table == "issue_pull_requests" {
        if row.get("purpose").is_none() {
            // Older peers cannot classify links; omitted metadata must not
            // reset a purpose already known by this replica.
            let existing = current_row(db, table, &row)?;
            row.as_object_mut()
                .ok_or_else(|| invalid("Invalid PR row"))?
                .insert(
                    "purpose".into(),
                    existing
                        .get("purpose")
                        .cloned()
                        .unwrap_or(json!("unspecified")),
                );
        }
    }
    let columns = rows(db, &format!("PRAGMA table_info({table})"), &[])?
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let object = row
        .as_object()
        .ok_or_else(|| invalid("Invalid fleet row"))?;
    if columns.iter().any(|c| !object.contains_key(c)) || object.len() != columns.len() {
        return Err(invalid(&format!("Schema mismatch for {table}")));
    }
    let names = columns
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let updates = columns
        .iter()
        .filter(|c| !k.contains(&c.as_str()))
        .map(|c| format!("\"{c}\"=excluded.\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let placeholders = vec!["?"; columns.len()].join(",");
    execute(
        db,
        &format!(
            "INSERT INTO {table}({names}) VALUES({placeholders}) ON CONFLICT({}) DO UPDATE SET {updates}",
            k.join(",")
        ),
        &columns.iter().map(|c| row[c].clone()).collect::<Vec<_>>(),
    )?;
    Ok(())
}
pub(super) fn ensure_metadata(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS fleet_ranges(node TEXT NOT NULL,project_id TEXT NOT NULL,first_number INTEGER NOT NULL,last_number INTEGER NOT NULL,PRIMARY KEY(node,project_id)); CREATE TABLE IF NOT EXISTS fleet_number_reservations(node TEXT NOT NULL,project_id TEXT NOT NULL,first_number INTEGER NOT NULL,last_number INTEGER NOT NULL,PRIMARY KEY(node,project_id,first_number)); CREATE TABLE IF NOT EXISTS fleet_signals(id TEXT PRIMARY KEY,host TEXT NOT NULL,worker TEXT NOT NULL,signal TEXT NOT NULL,state TEXT NOT NULL,result TEXT,created_at REAL NOT NULL); CREATE TABLE IF NOT EXISTS fleet_state(key TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
    Ok(())
}
pub(super) fn install_capture(db: &Connection, role: &str, node: &str) -> Result<()> {
    let tx = if db.is_autocommit() {
        Some(db.unchecked_transaction()?)
    } else {
        None
    };
    db.execute(
        "UPDATE fleet_meta SET role=?,node=? WHERE id=1",
        [role, node],
    )?;
    if rows(db, "PRAGMA index_list(fleet_row_ids)", &[])?
        .iter()
        .any(|r| r["origin"] == "u")
    {
        db.execute_batch("ALTER TABLE fleet_row_ids RENAME TO fleet_row_ids_legacy; CREATE TABLE fleet_row_ids(origin TEXT NOT NULL,table_name TEXT NOT NULL,origin_id INTEGER NOT NULL,local_id INTEGER NOT NULL,PRIMARY KEY(origin,table_name,origin_id)); INSERT INTO fleet_row_ids SELECT * FROM fleet_row_ids_legacy; DROP TABLE fleet_row_ids_legacy;")?;
    }
    db.execute_batch(
        "CREATE INDEX IF NOT EXISTS fleet_row_local ON fleet_row_ids(table_name,local_id);",
    )?;
    if role == "controller" {
        for table in ["comments", "events"] {
            db.execute(
                &format!("INSERT OR IGNORE INTO fleet_row_ids SELECT ?,?,id,id FROM {table}"),
                [node, table],
            )?;
        }
    }
    for (table, _) in TABLES {
        let columns = rows(db, &format!("PRAGMA table_info({table})"), &[])?
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        for (operation, before, after) in [
            ("INSERT", None, Some("NEW")),
            ("UPDATE", Some("OLD"), Some("NEW")),
            ("DELETE", Some("OLD"), None),
        ] {
            let row_json = |prefix: Option<&str>| {
                prefix
                    .map(|p| {
                        format!(
                            "json_object({})",
                            columns
                                .iter()
                                .map(|c| format!("'{c}',{p}.\"{c}\""))
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    })
                    .unwrap_or("NULL".into())
            };
            let different = if operation == "UPDATE" {
                format!(
                    " AND NOT ({})",
                    columns
                        .iter()
                        .map(|c| format!("OLD.\"{c}\" IS NEW.\"{c}\""))
                        .collect::<Vec<_>>()
                        .join(" AND ")
                )
            } else {
                String::new()
            };
            db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS fleet_capture_{table}_{operation} AFTER {operation} ON {table} WHEN (SELECT syncing FROM fleet_meta WHERE id=1)=0{different} BEGIN INSERT INTO fleet_outbox(table_name,before_json,after_json,created_at) VALUES('{table}',{},{},CAST(strftime('%s','now') AS INTEGER)*1000); END;",row_json(before),row_json(after)))?;
        }
    }
    ensure_metadata(db)?;
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(())
}
pub(super) fn journal(db: &Connection, after: i64) -> Result<Vec<Value>> {
    let bootstrap = state_get(db, "bootstrap_last_seq", json!(0))?
        .as_i64()
        .unwrap_or(0);
    let mut result = vec![];
    let mut size = 0;
    for mut row in rows(
        db,
        "SELECT * FROM fleet_outbox WHERE seq>? ORDER BY seq LIMIT 300",
        &[json!(after)],
    )? {
        if row["seq"].as_i64().unwrap() <= bootstrap {
            row["bootstrap"] = json!(true);
        }
        size += row.to_string().len();
        if !result.is_empty() && size > crate::issues::WIRE_LIMIT / 3 {
            break;
        }
        result.push(row);
    }
    Ok(result)
}
fn append_row(
    db: &Connection,
    origin: &str,
    table: &str,
    row: &Value,
    bootstrap: bool,
) -> Result<i64> {
    if !append(table) {
        return Err(invalid("Unknown history table"));
    }
    let origin_id = row["id"]
        .as_i64()
        .ok_or_else(|| invalid("Invalid history ID"))?;
    let mapped: Option<i64> = db
        .query_row(
            "SELECT local_id FROM fleet_row_ids WHERE origin=? AND table_name=? AND origin_id=?",
            rusqlite::params![origin, table, origin_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = mapped {
        return Ok(id);
    }
    let own: String = db.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    let local_id = if own == origin {
        db.query_row(
            &format!("SELECT id FROM {table} WHERE id=?"),
            [origin_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| invalid("Missing original append"))?
    } else {
        let mut values = row
            .as_object()
            .ok_or_else(|| invalid("Invalid history row"))?
            .clone();
        values.remove("id");
        if table == "events" {
            let mut data: Value = serde_json::from_str(
                values["data"]
                    .as_str()
                    .ok_or_else(|| invalid("Invalid event data"))?,
            )?;
            if data.is_object() && data.get("comment_id").is_some() {
                let comment_origin = data["comment_origin"].as_str().unwrap_or(origin);
                let comment_id = data
                    .get("comment_origin_id")
                    .unwrap_or(&data["comment_id"])
                    .as_i64()
                    .ok_or_else(|| invalid("Invalid comment ID"))?;
                let mut comment:Option<i64>=db.query_row("SELECT local_id FROM fleet_row_ids WHERE origin=? AND table_name='comments' AND origin_id=?",rusqlite::params![comment_origin,comment_id],|r|r.get(0)).optional()?;
                if comment.is_none() && comment_origin == own {
                    comment = rows(
                        db,
                        "SELECT id FROM comments WHERE id=? AND project_id=? AND issue_number=?",
                        &[
                            json!(comment_id),
                            values["project_id"].clone(),
                            values["issue_number"].clone(),
                        ],
                    )?
                    .first()
                    .and_then(|r| r["id"].as_i64());
                }
                if comment.is_none()
                    && matches!(
                        values["action"].as_str(),
                        Some("comment_resolved" | "comment_unresolved")
                    )
                {
                    return Err(invalid("Resolved comment is not available on this replica"));
                }
                if let Some(id) = comment {
                    data["comment_id"] = json!(id);
                    values.insert("data".into(), json!(data.to_string()));
                }
            }
        }
        let columns = values.keys().cloned().collect::<Vec<_>>();
        let parameters = columns
            .iter()
            .map(|c| values[c].clone())
            .collect::<Vec<_>>();
        let existing = if bootstrap {
            rows(
                db,
                &format!(
                    "SELECT id FROM {table} WHERE {}",
                    columns
                        .iter()
                        .map(|k| format!("{k} IS ?"))
                        .collect::<Vec<_>>()
                        .join(" AND ")
                ),
                &parameters,
            )?
            .first()
            .and_then(|r| r["id"].as_i64())
        } else {
            None
        };
        match existing {
            Some(id) => id,
            None => {
                execute(
                    db,
                    &format!(
                        "INSERT INTO {table}({}) VALUES({})",
                        columns.join(","),
                        vec!["?"; columns.len()].join(",")
                    ),
                    &parameters,
                )?;
                db.last_insert_rowid()
            }
        }
    };
    db.execute(
        "INSERT OR IGNORE INTO fleet_row_ids VALUES(?,?,?,?)",
        rusqlite::params![origin, table, origin_id, local_id],
    )?;
    Ok(local_id)
}
fn conflict(db: &Connection, node: &str, change: &Value, reason: &str) -> Result<Value> {
    let id = format!("{node}:{}", change["seq"]);
    execute(
        db,
        "INSERT OR IGNORE INTO fleet_conflicts(id,node,seq,table_name,data,reason,created_at) VALUES(?,?,?,?,?,?,?)",
        &[
            json!(id),
            json!(node),
            change["seq"].clone(),
            change["table_name"].clone(),
            json!(change.to_string()),
            json!(reason),
            json!(crate::issues::worker::now()),
        ],
    )?;
    Ok(json!({"state":"conflict","id":id,"reason":reason}))
}
fn row_json(change: &Value, field: &str) -> Result<Value> {
    Ok(match change[field].as_str() {
        Some(s) => serde_json::from_str(s)?,
        None => Value::Null,
    })
}
fn apply_change(db: &Connection, node: &str, change: &Value) -> Result<Value> {
    let table = change["table_name"]
        .as_str()
        .ok_or_else(|| invalid("Invalid replicated table"))?;
    keys(table)?;
    let before = row_json(change, "before_json")?;
    let after = row_json(change, "after_json")?;
    let key = if after.is_null() { &before } else { &after };
    let old = current_row(db, table, key)?;
    let mut result = json!({"state":"applied"});
    if table == "issue_subtasks" && !after.is_null() {
        let collision = rows(
            db,
            "SELECT 1 FROM fleet_conflicts WHERE node=? AND table_name='issues' AND seq<? AND json_extract(data,'$.before_json') IS NULL AND json_extract(json_extract(data,'$.after_json'),'$.project_id')=? AND json_extract(json_extract(data,'$.after_json'),'$.number') IN (?,?)",
            &[
                json!(node),
                change["seq"].clone(),
                after["project_id"].clone(),
                after["parent_number"].clone(),
                after["child_number"].clone(),
            ],
        )?;
        if !collision.is_empty() {
            return Err(invalid(
                "Subtask endpoint belongs to a rejected offline issue creation; retained for review",
            ));
        }
    }
    if table == "issue_status_updates" {
        if !before.is_null() || after.is_null() {
            return Err(invalid("Status history cannot be rewritten"));
        }
        if !old.is_null() && old != after {
            return Err(invalid("Status update identity already exists"));
        }
        if old.is_null() && change["bootstrap"] != true {
            let eligible = rows(
                db,
                "SELECT 1 FROM issues WHERE project_id=? AND number=? AND state='open' AND draft=0 AND deleted_at IS NULL",
                &[after["project_id"].clone(), after["issue_number"].clone()],
            )?;
            if eligible.is_empty() {
                return Err(invalid(
                    "Issue closed, drafted or deleted while offline; status update retained for review",
                ));
            }
        }
        put_row(db, table, &after)?;
    } else if append(table) {
        if !before.is_null() || after.is_null() {
            return Err(invalid("Append-only history cannot be rewritten"));
        }
        if table == "events"
            && matches!(
                after["action"].as_str(),
                Some("subtask_added" | "subtask_removed" | "parent_added" | "parent_removed")
            )
        {
            let data: Value = serde_json::from_str(
                after["data"]
                    .as_str()
                    .ok_or_else(|| invalid("Invalid event data"))?,
            )?;
            let latest = rows(
                db,
                "SELECT * FROM fleet_subtask_receipts WHERE node=? AND project_id=? AND child_number=? AND seq<? ORDER BY seq DESC LIMIT 1",
                &[
                    json!(node),
                    after["project_id"].clone(),
                    data["child"].clone(),
                    change["seq"].clone(),
                ],
            )?;
            let adding = matches!(
                after["action"].as_str(),
                Some("subtask_added" | "parent_added")
            );
            let relation = rows(
                db,
                "SELECT parent_number FROM issue_subtasks WHERE project_id=? AND child_number=?",
                &[after["project_id"].clone(), data["child"].clone()],
            )?;
            if latest.first().is_some_and(|r| {
                r["state"] != "applied"
                    || r["parent_number"] != data["parent"]
                    || r["kind"] != if adding { "add" } else { "remove" }
            }) {
                return Err(invalid(
                    "Subtask relationship was rejected; attempted history retained for review",
                ));
            }
            if adding
                != relation
                    .first()
                    .is_some_and(|r| r["parent_number"] == data["parent"])
            {
                return Err(invalid(
                    "Subtask history does not match the canonical relationship",
                ));
            }
        }
        if change["bootstrap"]==true && !rows(db,"SELECT 1 FROM fleet_conflicts WHERE node=? AND table_name='issues' AND json_extract(data,'$.after_json') IS NOT NULL AND json_extract(json_extract(data,'$.after_json'),'$.project_id')=? AND json_extract(json_extract(data,'$.after_json'),'$.number')=?",&[json!(node),after["project_id"].clone(),after["issue_number"].clone()])?.is_empty() {return Err(invalid("Legacy history belongs to an issue-number collision; retained for review"));}
        let local = append_row(db, node, table, &after, change["bootstrap"] == true)?;
        let origin = rows(
            db,
            "SELECT origin,origin_id FROM fleet_row_ids WHERE table_name=? AND local_id=? ORDER BY rowid LIMIT 1",
            &[json!(table), json!(local)],
        )?;
        result["canonical_append"] = origin[0].clone();
    } else if table == "projects" {
        if old.is_null() {
            return Err(invalid(
                "New offline projects require registration before disconnection",
            ));
        }
        execute(
            db,
            "UPDATE projects SET activity_at=max(activity_at,?) WHERE id=?",
            &[after["activity_at"].clone(), after["id"].clone()],
        )?;
    } else if table == "agents" {
        if !after.is_null() {
            put_row(db, table, &after)?;
        }
    } else if table == "issues" {
        let owner = if after.is_null() {
            vec![]
        } else {
            rows(
                db,
                "SELECT node FROM fleet_allocations WHERE project_id=? AND issue_number=?",
                &[after["project_id"].clone(), after["number"].clone()],
            )?
        };
        if before.is_null() {
            let allocated=!rows(db,"SELECT 1 FROM fleet_number_reservations WHERE node=? AND project_id=? AND ? BETWEEN first_number AND last_number",&[json!(node),after["project_id"].clone(),after["number"].clone()])?.is_empty();
            let legacy=change["bootstrap"]==true && rows(db,"SELECT 1 FROM fleet_number_reservations WHERE node<>? AND project_id=? AND ? BETWEEN first_number AND last_number",&[json!(node),after["project_id"].clone(),after["number"].clone()])?.is_empty();
            if old != after {
                if !old.is_null() || !(allocated || legacy) {
                    return Err(invalid("Offline issue number is not exclusively allocated"));
                }
                put_row(db, table, &after)?;
                execute(
                    db,
                    "UPDATE projects SET next_number=max(next_number,?) WHERE id=?",
                    &[
                        json!(after["number"].as_i64().unwrap() + 1),
                        after["project_id"].clone(),
                    ],
                )?;
                execute(
                    db,
                    "INSERT OR IGNORE INTO fleet_allocations VALUES(?,?,?)",
                    &[
                        after["project_id"].clone(),
                        after["number"].clone(),
                        json!(node),
                    ],
                )?;
            }
        } else {
            if old.is_null() {
                return Err(invalid("Issue no longer exists"));
            }
            if after.is_null() {
                return Err(invalid("Physical issue deletion is not supported"));
            }
            let changed = after
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, v)| {
                    before[*k] != **v
                        && !matches!(
                            k.as_str(),
                            "version" | "updated_at" | "sort_order" | "origin"
                        )
                })
                .collect::<Vec<_>>();
            if !changed.is_empty() && !owner.first().is_some_and(|r| r["node"] == node) {
                return Err(invalid(
                    "Issue allocation was revoked or belongs to another machine",
                ));
            }
            if after["state"] == "closed"
                && before["state"] != "closed"
                && ["title", "body", "labels"]
                    .iter()
                    .any(|k| old[*k] != before[*k])
            {
                return Err(invalid(
                    "Issue requirements changed before offline completion",
                ));
            }
            if changed
                .iter()
                .any(|(k, v)| old[*k] != before[*k] && old[*k] != **v)
            {
                return Err(invalid(
                    "Concurrent edits changed the same issue field or ownership",
                ));
            }
            let mut merged = old.clone();
            for (k, v) in changed {
                merged[k] = v.clone();
            }
            if before["blockers"] != after["blockers"] {
                let links: Vec<i64> = serde_json::from_str(
                    merged["blockers"]
                        .as_str()
                        .ok_or_else(|| invalid("Invalid blocker links"))?,
                )?;
                crate::issues::blockers::validate_links(
                    db,
                    merged["project_id"].as_str().unwrap(),
                    merged["number"].as_i64().unwrap(),
                    &links,
                )
                .map_err(|e| invalid(&e.message))?;
            }
            merged["version"] = json!(old["version"].as_i64().unwrap() + 1);
            merged["updated_at"] = json!(
                old["updated_at"]
                    .as_i64()
                    .unwrap()
                    .max(after["updated_at"].as_i64().unwrap())
            );
            put_row(db, table, &merged)?;
        }
    } else {
        if !before.is_null() && old != before && old != after {
            return Err(invalid("Concurrent configuration or PR change"));
        }
        if before.is_null() && !old.is_null() && old != after {
            return Err(invalid("Conflicting inserted row"));
        }
        if after.is_null() {
            let (clause, values) = key_where(table, &before)?;
            execute(db, &format!("DELETE FROM {table} WHERE {clause}"), &values)?;
        } else {
            put_row(db, table, &after)?;
        }
    }
    Ok(result)
}
fn is_conflict(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|e|e.kind()==std::io::ErrorKind::InvalidInput) || error.downcast_ref::<rusqlite::Error>().is_some_and(|e|matches!(e,rusqlite::Error::SqliteFailure(code,_) if code.code==rusqlite::ErrorCode::ConstraintViolation))
}
pub(super) fn accept_changes(db: &Connection, node: &str, changes: &[Value]) -> Result<Vec<Value>> {
    let tx = if db.is_autocommit() {
        Some(db.unchecked_transaction()?)
    } else {
        None
    };
    let mut results = vec![];
    for change in changes {
        let previous = rows(
            db,
            "SELECT result FROM fleet_receipts WHERE node=? AND seq=?",
            &[json!(node), change["seq"].clone()],
        )?;
        let mut receipt = if let Some(r) = previous.first() {
            serde_json::from_str(r["result"].as_str().unwrap())?
        } else {
            db.execute_batch("SAVEPOINT incoming")?;
            let result = match apply_change(db, node, change) {
                Ok(r) => {
                    db.execute_batch("RELEASE incoming")?;
                    r
                }
                Err(e) => {
                    db.execute_batch("ROLLBACK TO incoming; RELEASE incoming")?;
                    if !is_conflict(e.as_ref()) {
                        return Err(e);
                    }
                    conflict(db, node, change, &e.to_string())?
                }
            };
            execute(
                db,
                "INSERT INTO fleet_receipts VALUES(?,?,?)",
                &[
                    json!(node),
                    change["seq"].clone(),
                    json!(result.to_string()),
                ],
            )?;
            result
        };
        if change["table_name"] == "issue_subtasks" {
            let after = row_json(change, "after_json")?;
            let row = if after.is_null() {
                row_json(change, "before_json")?
            } else {
                after.clone()
            };
            execute(
                db,
                "INSERT OR IGNORE INTO fleet_subtask_receipts VALUES(?,?,?,?,?,?,?)",
                &[
                    json!(node),
                    change["seq"].clone(),
                    row["project_id"].clone(),
                    row["child_number"].clone(),
                    row["parent_number"].clone(),
                    json!(if after.is_null() { "remove" } else { "add" }),
                    receipt["state"].clone(),
                ],
            )?;
            receipt["canonical_subtask"] = json!({"project_id":row["project_id"],"child_number":row["child_number"],"row":current_row(db,"issue_subtasks",&row)?});
        }
        receipt["seq"] = change["seq"].clone();
        results.push(receipt);
    }
    if changes
        .iter()
        .any(|c| matches!(c["table_name"].as_str(), Some("issues" | "issue_subtasks")))
    {
        crate::issues::blockers::reconcile_all(db)?;
    }
    // Project the final graph, including earlier receipts in the same batch.
    for (change, receipt) in changes.iter().zip(&mut results) {
        if change["table_name"] == "issue_subtasks" {
            let after = row_json(change, "after_json")?;
            let row = if after.is_null() {
                row_json(change, "before_json")?
            } else {
                after
            };
            receipt["canonical_subtask"]["row"] = current_row(db, "issue_subtasks", &row)?;
        }
    }
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(results)
}
fn canonical_append(
    own: &str,
    table: &str,
    row: &Value,
    identities: &BTreeMap<(String, i64), (String, i64)>,
) -> Result<Value> {
    let mut row = row.clone();
    if let Some((origin, id)) = identities.get(&(table.into(), row["id"].as_i64().unwrap())) {
        row["id"] = json!(id);
        if table == "events" {
            let mut data: Value = serde_json::from_str(row["data"].as_str().unwrap())?;
            if let Some(comment_id) = data["comment_id"].as_i64()
                && let Some((co, ci)) = identities.get(&("comments".into(), comment_id))
            {
                let resolution = matches!(
                    row["action"].as_str(),
                    Some("comment_resolved" | "comment_unresolved")
                );
                if resolution || co == origin {
                    data["comment_id"] = json!(ci);
                    if resolution {
                        data["comment_origin"] = json!(co);
                        data["comment_origin_id"] = json!(ci);
                    }
                    row["data"] = json!(data.to_string());
                }
            }
        }
        Ok(json!({"origin":origin,"row":row}))
    } else {
        Ok(json!({"origin":own,"row":row}))
    }
}
fn identities(db: &Connection) -> Result<BTreeMap<(String, i64), (String, i64)>> {
    let mut ids = BTreeMap::new();
    for r in rows(
        db,
        "SELECT table_name,local_id,origin,origin_id FROM fleet_row_ids ORDER BY rowid",
        &[],
    )? {
        ids.entry((
            r["table_name"].as_str().unwrap().into(),
            r["local_id"].as_i64().unwrap(),
        ))
        .or_insert((
            r["origin"].as_str().unwrap().into(),
            r["origin_id"].as_i64().unwrap(),
        ));
    }
    Ok(ids)
}
fn allocation_payload(db: &Connection, node: &str, mut payload: Value) -> Result<Value> {
    payload["allocations"] = json!(rows(db, "SELECT * FROM fleet_allocations", &[])?);
    payload["ranges"] = json!(rows(
        db,
        "SELECT project_id,first_number,last_number FROM fleet_ranges WHERE node=?",
        &[json!(node)]
    )?);
    Ok(payload)
}
pub(super) fn snapshot(db: &Connection, node: &str) -> Result<Value> {
    let tx = if db.is_autocommit() {
        Some(db.unchecked_transaction()?)
    } else {
        None
    };
    let own: String = db.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    let ids = identities(db)?;
    let mut tables = serde_json::Map::new();
    for (table, _) in TABLES {
        let mut data = rows(db, &format!("SELECT * FROM {table}"), &[])?;
        if append(table) {
            data = data
                .iter()
                .map(|r| canonical_append(&own, table, r, &ids))
                .collect::<Result<_>>()?;
        }
        tables.insert((*table).into(), json!(data));
    }
    // Canonical name choices belong to the supervisor. Keep this additive
    // snapshot metadata out of companion write journals and older protocols.
    for table in ["project_name_keys", "project_name_collisions"] {
        tables.insert(
            table.into(),
            json!(rows(db, &format!("SELECT * FROM {table}"), &[])?),
        );
    }
    let cursor: i64 = db.query_row("SELECT coalesce(max(seq),0) FROM fleet_outbox", [], |r| {
        r.get(0)
    })?;
    let result = allocation_payload(db, node, json!({"tables":tables,"cursor":cursor}))?;
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(result)
}
pub(super) fn incremental(db: &Connection, node: &str, cursor: i64) -> Result<Value> {
    let own: String = db.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    let ids = identities(db)?;
    let mut changes = journal(db, cursor)?;
    for c in &mut changes {
        let table = c["table_name"].as_str().unwrap();
        if append(table) && c["after_json"].is_string() {
            c["append"] = canonical_append(&own, table, &row_json(c, "after_json")?, &ids)?;
        }
    }
    allocation_payload(
        db,
        node,
        json!({"cursor":changes.last().map(|c|c["seq"].clone()).unwrap_or(json!(cursor)),"changes":changes}),
    )
}

fn pending_key(table: &str, row: &Value) -> Result<String> {
    Ok(format!(
        "{table}:{}",
        json!(
            keys(table)?
                .iter()
                .map(|k| row[*k].clone())
                .collect::<Vec<_>>()
        )
    ))
}
fn apply_row(
    db: &Connection,
    pending: &BTreeSet<String>,
    table: &str,
    row: &Value,
    origin: &str,
) -> Result<()> {
    if table == "issue_subtasks" || pending.contains(&pending_key(table, row)?) {
        return Ok(());
    }
    if append(table) {
        append_row(db, origin, table, row, false)?;
    } else {
        let mut row = row.clone();
        if table == "projects" {
            let old = current_row(db, table, &row)?;
            if !old.is_null() {
                row["next_number"] = old["next_number"].clone();
            }
        }
        put_row(db, table, &row)?;
    }
    Ok(())
}
fn graph_key(row: &Value) -> Result<(String, i64)> {
    Ok((
        row["project_id"]
            .as_str()
            .ok_or_else(|| invalid("Invalid subtask project"))?
            .into(),
        row["child_number"]
            .as_i64()
            .ok_or_else(|| invalid("Invalid child number"))?,
    ))
}
fn apply_graph(
    db: &Connection,
    pending: &BTreeSet<String>,
    payload: &Value,
    acknowledged: BTreeMap<(String, i64), Value>,
) -> Result<()> {
    let mut desired = BTreeMap::new();
    for r in rows(db, "SELECT * FROM fleet_deferred_subtasks", &[])? {
        desired.insert(
            graph_key(&r)?,
            match r["row_json"].as_str() {
                Some(s) => serde_json::from_str(s)?,
                None => Value::Null,
            },
        );
    }
    if let Some(snapshot) = payload["tables"]["issue_subtasks"].as_array() {
        for row in desired.values_mut() {
            *row = Value::Null;
        }
        for r in rows(
            db,
            "SELECT project_id,child_number FROM issue_subtasks",
            &[],
        )? {
            desired.insert(graph_key(&r)?, Value::Null);
        }
        for r in snapshot {
            desired.insert(graph_key(r)?, r.clone());
        }
    }
    for change in payload["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["table_name"] == "issue_subtasks")
    {
        let after = row_json(change, "after_json")?;
        let row = if after.is_null() {
            row_json(change, "before_json")?
        } else {
            after.clone()
        };
        desired.insert(graph_key(&row)?, after);
    }
    desired.extend(acknowledged);
    for (project, child) in desired.keys() {
        let key = json!({"project_id":project,"child_number":child});
        if !pending.contains(&pending_key("issue_subtasks", &key)?) {
            execute(
                db,
                "DELETE FROM issue_subtasks WHERE project_id=? AND child_number=?",
                &[json!(project), json!(child)],
            )?;
        }
    }
    for ((project, child), row) in desired {
        let key = json!({"project_id":project,"child_number":child});
        let defer = |row: &Value| {
            execute(
                db,
                "INSERT INTO fleet_deferred_subtasks VALUES(?,?,?) ON CONFLICT(project_id,child_number) DO UPDATE SET row_json=excluded.row_json",
                &[
                    json!(project),
                    json!(child),
                    if row.is_null() {
                        Value::Null
                    } else {
                        json!(row.to_string())
                    },
                ],
            )
        };
        if pending.contains(&pending_key("issue_subtasks", &key)?) {
            defer(&row)?;
            continue;
        }
        db.execute_batch("SAVEPOINT subtask_pull")?;
        let result = if row.is_null() {
            Ok(())
        } else {
            put_row(db, "issue_subtasks", &row)
        };
        match result {
            Ok(()) => {
                db.execute_batch("RELEASE subtask_pull")?;
                execute(
                    db,
                    "DELETE FROM fleet_deferred_subtasks WHERE project_id=? AND child_number=?",
                    &[json!(project), json!(child)],
                )?;
            }
            Err(e) => {
                db.execute_batch("ROLLBACK TO subtask_pull; RELEASE subtask_pull")?;
                if !is_conflict(e.as_ref()) {
                    return Err(e);
                }
                defer(&row)?;
            }
        }
    }
    Ok(())
}
pub(super) fn apply_pull(
    db: &Connection,
    node: &str,
    payload: &Value,
    receipts: &[Value],
) -> Result<()> {
    let tx = if db.is_autocommit() {
        Some(db.unchecked_transaction()?)
    } else {
        None
    };
    db.execute("UPDATE fleet_meta SET syncing=1 WHERE id=1", [])?;
    let mut acknowledged = BTreeMap::new();
    for receipt in receipts {
        if receipt.get("canonical_subtask").is_some() {
            let row = &receipt["canonical_subtask"];
            acknowledged.insert(graph_key(row)?, row["row"].clone());
        }
        let change = rows(
            db,
            "SELECT * FROM fleet_outbox WHERE seq=?",
            &[receipt["seq"].clone()],
        )?
        .into_iter()
        .next();
        if let Some(change) = change {
            if let Some(origin) = receipt.get("canonical_append") {
                let local = row_json(&change, "after_json")?;
                execute(
                    db,
                    "INSERT OR IGNORE INTO fleet_row_ids VALUES(?,?,?,?)",
                    &[
                        origin["origin"].clone(),
                        change["table_name"].clone(),
                        origin["origin_id"].clone(),
                        local["id"].clone(),
                    ],
                )?;
            }
            if receipt["state"] == "conflict" {
                conflict(
                    db,
                    node,
                    &change,
                    receipt["reason"]
                        .as_str()
                        .unwrap_or("Synchronization conflict"),
                )?;
                if change["table_name"] == "events" && change["after_json"].is_string() {
                    let event = row_json(&change, "after_json")?;
                    if matches!(
                        event["action"].as_str(),
                        Some(
                            "subtask_added" | "subtask_removed" | "parent_added" | "parent_removed"
                        )
                    ) {
                        let mut data: Value =
                            serde_json::from_str(event["data"].as_str().unwrap())?;
                        data["sync_conflict"] = receipt["reason"].clone();
                        data["attempted_action"] = event["action"].clone();
                        execute(
                            db,
                            "UPDATE events SET action='subtask_change_conflict',data=? WHERE id=?",
                            &[json!(data.to_string()), event["id"].clone()],
                        )?;
                    }
                }
            }
        }
        execute(
            db,
            "DELETE FROM fleet_outbox WHERE seq=?",
            &[receipt["seq"].clone()],
        )?;
    }
    let mut pending = BTreeSet::new();
    for c in rows(
        db,
        "SELECT table_name,before_json,after_json FROM fleet_outbox",
        &[],
    )? {
        let after = row_json(&c, "after_json")?;
        let row = if after.is_null() {
            row_json(&c, "before_json")?
        } else {
            after
        };
        pending.insert(pending_key(c["table_name"].as_str().unwrap(), &row)?);
    }
    // Apply endpoints before edges and history, regardless of JSON object order.
    for (table, _) in TABLES {
        for row in payload["tables"][*table].as_array().into_iter().flatten() {
            if append(table) {
                apply_row(
                    db,
                    &pending,
                    table,
                    &row["row"],
                    row["origin"]
                        .as_str()
                        .ok_or_else(|| invalid("Missing history origin"))?,
                )?;
            } else {
                apply_row(db, &pending, table, row, "")?;
            }
        }
    }
    for row in payload["tables"]["project_name_keys"]
        .as_array()
        .into_iter()
        .flatten()
    {
        execute(
            db,
            "INSERT INTO project_name_keys(name,project_id) VALUES(?,?) ON CONFLICT(name) DO UPDATE SET project_id=excluded.project_id",
            &[row["name"].clone(), row["project_id"].clone()],
        )?;
    }
    for row in payload["tables"]["project_name_collisions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        execute(
            db,
            "INSERT INTO project_name_collisions(rejected_id,name,project_id,legacy) VALUES(?,?,?,?) ON CONFLICT(rejected_id) DO UPDATE SET project_id=excluded.project_id,legacy=excluded.legacy",
            &[
                row["rejected_id"].clone(),
                row["name"].clone(),
                row["project_id"].clone(),
                row["legacy"].clone(),
            ],
        )?;
    }
    for change in payload["changes"].as_array().into_iter().flatten() {
        let table = change["table_name"]
            .as_str()
            .ok_or_else(|| invalid("Invalid replicated table"))?;
        keys(table)?;
        if append(table) {
            if let Some(item) = change.get("append") {
                apply_row(
                    db,
                    &pending,
                    table,
                    &item["row"],
                    item["origin"]
                        .as_str()
                        .ok_or_else(|| invalid("Missing history origin"))?,
                )?;
            }
        } else {
            let after = row_json(change, "after_json")?;
            if !after.is_null() {
                apply_row(db, &pending, table, &after, "")?;
            } else {
                let row = row_json(change, "before_json")?;
                if table != "issue_subtasks" && !pending.contains(&pending_key(table, &row)?) {
                    let (clause, args) = key_where(table, &row)?;
                    execute(db, &format!("DELETE FROM {table} WHERE {clause}"), &args)?;
                }
            }
        }
    }
    apply_graph(db, &pending, payload, acknowledged)?;
    if payload["tables"]["issues"].is_array()
        || payload["changes"].as_array().is_some_and(|changes| {
            changes
                .iter()
                .any(|c| matches!(c["table_name"].as_str(), Some("issues" | "issue_subtasks")))
        })
    {
        crate::issues::blockers::reconcile_all(db)?;
    }
    db.execute("DELETE FROM fleet_allocations", [])?;
    for row in payload["allocations"]
        .as_array()
        .ok_or_else(|| invalid("Missing fleet allocations"))?
    {
        execute(
            db,
            "INSERT INTO fleet_allocations VALUES(?,?,?)",
            &[
                row["project_id"].clone(),
                row["issue_number"].clone(),
                row["node"].clone(),
            ],
        )?;
    }
    for r in payload["ranges"]
        .as_array()
        .ok_or_else(|| invalid("Missing fleet number ranges"))?
    {
        let previous = rows(
            db,
            "SELECT first_number,last_number FROM fleet_number_ranges WHERE project_id=?",
            &[r["project_id"].clone()],
        )?;
        execute(
            db,
            "INSERT INTO fleet_number_ranges VALUES(?,?,?) ON CONFLICT(project_id) DO UPDATE SET first_number=excluded.first_number,last_number=excluded.last_number",
            &[
                r["project_id"].clone(),
                r["first_number"].clone(),
                r["last_number"].clone(),
            ],
        )?;
        if !previous.first().is_some_and(|p| {
            p["first_number"] == r["first_number"] && p["last_number"] == r["last_number"]
        }) {
            let used = rows(
                db,
                "SELECT coalesce(max(number),?-1)+1 next FROM issues WHERE project_id=? AND number BETWEEN ? AND ?",
                &[
                    r["first_number"].clone(),
                    r["project_id"].clone(),
                    r["first_number"].clone(),
                    r["last_number"].clone(),
                ],
            )?;
            execute(
                db,
                "UPDATE projects SET next_number=? WHERE id=?",
                &[used[0]["next"].clone(), r["project_id"].clone()],
            )?;
        } else {
            execute(
                db,
                "UPDATE projects SET next_number=? WHERE id=? AND next_number NOT BETWEEN ? AND ?",
                &[
                    r["first_number"].clone(),
                    r["project_id"].clone(),
                    r["first_number"].clone(),
                    json!(r["last_number"].as_i64().unwrap() + 1),
                ],
            )?;
        }
    }
    state_set(db, "cursor", &payload["cursor"])?;
    state_set(
        db,
        "last_sync",
        &json!(crate::issues::worker::now() as f64 / 1000.0),
    )?;
    db.execute("UPDATE fleet_meta SET syncing=0 WHERE id=1", [])?;
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(())
}
fn tags(config: &Value) -> BTreeSet<String> {
    config["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
fn matches(labels: &Value, wanted: &BTreeSet<String>) -> Result<bool> {
    let labels: Vec<String> = serde_json::from_str(
        labels
            .as_str()
            .ok_or_else(|| invalid("Invalid issue labels"))?,
    )?;
    Ok(wanted.iter().all(|t| labels.contains(t)))
}
pub(super) fn allocate(db: &Connection, node: &str, workers: &[Value]) -> Result<()> {
    let tx = if db.is_autocommit() {
        Some(db.unchecked_transaction()?)
    } else {
        None
    };
    let mut pools: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for worker in workers {
        let config = &worker["config"];
        if worker["intent"]
            .as_str()
            .unwrap_or(if config["enabled"] == false {
                "pause"
            } else {
                "running"
            })
            != "running"
        {
            continue;
        }
        for project in config["projects"].as_array().into_iter().flatten() {
            let project = project
                .as_str()
                .ok_or_else(|| invalid("Invalid worker project"))?;
            pools.entry(project.into()).or_default().push(config);
        }
    }
    for (project, configs) in pools {
        let projects = rows(
            db,
            "SELECT * FROM projects WHERE id=? AND hidden_at IS NULL",
            &[json!(project)],
        )?;
        let Some(row) = projects.first() else {
            continue;
        };
        let ranges = rows(
            db,
            "SELECT * FROM fleet_ranges WHERE node=? AND project_id=?",
            &[json!(node), json!(project)],
        )?;
        let replenish = if let Some(range) = ranges.first() {
            let used = rows(
                db,
                "SELECT coalesce(max(number),0) used FROM issues WHERE project_id=? AND number BETWEEN ? AND ?",
                &[
                    json!(project),
                    range["first_number"].clone(),
                    range["last_number"].clone(),
                ],
            )?;
            used[0]["used"].as_i64().unwrap() > range["last_number"].as_i64().unwrap() - 20
        } else {
            true
        };
        if replenish {
            let first = row["next_number"].as_i64().unwrap();
            execute(
                db,
                "UPDATE projects SET next_number=next_number+100 WHERE id=?",
                &[json!(project)],
            )?;
            let values = [json!(node), json!(project), json!(first), json!(first + 99)];
            execute(
                db,
                "INSERT INTO fleet_ranges VALUES(?,?,?,?) ON CONFLICT(node,project_id) DO UPDATE SET first_number=excluded.first_number,last_number=excluded.last_number",
                &values,
            )?;
            execute(
                db,
                "INSERT INTO fleet_number_reservations VALUES(?,?,?,?)",
                &values,
            )?;
        }
        let mut supplied = rows(
            db,
            &format!(
                "SELECT i.labels FROM fleet_allocations a JOIN issues i ON i.project_id=a.project_id AND i.number=a.issue_number WHERE a.node=? AND a.project_id=? AND i.state='open' AND i.deleted_at IS NULL AND {ALLOCATED}"
            ),
            &[json!(node), json!(project)],
        )?;
        let candidates = rows(
            db,
            &format!(
                "SELECT i.number,i.labels FROM issues i WHERE project_id=? AND state='open' AND deleted_at IS NULL AND assignee IS NULL AND {READY} AND NOT EXISTS(SELECT 1 FROM fleet_allocations a WHERE a.project_id=i.project_id AND a.issue_number=i.number) AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL) ORDER BY sort_order,number"
            ),
            &[json!(project)],
        )?;
        let mut used = BTreeSet::new();
        let mut filters = BTreeMap::<BTreeSet<String>, i64>::new();
        for c in &configs {
            *filters.entry(tags(c)).or_default() += c["concurrency"].as_i64().unwrap_or(1);
        }
        for (filter, capacity) in &filters {
            let mut needed = (capacity * 2
                - supplied
                    .iter()
                    .filter(|r| matches(&r["labels"], filter).unwrap_or(false))
                    .count() as i64)
                .max(0);
            for c in &candidates {
                let number = c["number"].as_i64().unwrap();
                if needed > 0 && !used.contains(&number) && matches(&c["labels"], filter)? {
                    execute(
                        db,
                        "INSERT INTO fleet_allocations VALUES(?,?,?)",
                        &[json!(project), json!(number), json!(node)],
                    )?;
                    used.insert(number);
                    supplied.push(c.clone());
                    needed -= 1;
                }
            }
        }
        let mut needed = (filters.values().sum::<i64>() * 2
            - supplied
                .iter()
                .filter(|r| {
                    filters
                        .keys()
                        .any(|f| matches(&r["labels"], f).unwrap_or(false))
                })
                .count() as i64)
            .max(0);
        for c in &candidates {
            let number = c["number"].as_i64().unwrap();
            if needed > 0
                && !used.contains(&number)
                && filters
                    .keys()
                    .any(|f| matches(&c["labels"], f).unwrap_or(false))
            {
                execute(
                    db,
                    "INSERT INTO fleet_allocations VALUES(?,?,?)",
                    &[json!(project), json!(number), json!(node)],
                )?;
                used.insert(number);
                needed -= 1;
            }
        }
    }
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::issues::{Actor, Operation, Project, Request, Store};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn blocker_migration_updates_existing_capture_triggers_before_normalizing() {
        for partial in [false, true] {
            let main = Fixture::new();
            main.db.execute_batch("ALTER TABLE issues DROP COLUMN blockers; ALTER TABLE issues DROP COLUMN manual_blocked;").unwrap();
            main.capture();
            if partial {
                main.db.execute_batch("ALTER TABLE issues ADD COLUMN manual_blocked INTEGER NOT NULL DEFAULT 0; ALTER TABLE issues ADD COLUMN blockers TEXT NOT NULL DEFAULT '[]';").unwrap();
            }
            drop(Store::open(&main.path).unwrap());
            main.db
                .execute("UPDATE issues SET blockers='[2]' WHERE number=1", [])
                .unwrap();
            let change: String = main.db.query_row("SELECT after_json FROM fleet_outbox WHERE table_name='issues' ORDER BY seq DESC LIMIT 1", [], |r| r.get(0)).unwrap();
            let row: Value = serde_json::from_str(&change).unwrap();
            assert_eq!(row["blockers"], "[2]");
            assert_eq!(row["manual_blocked"], 0);
            assert_eq!(row["title"], "Original");
            main.db
                .execute("UPDATE issues SET manual_blocked=1 WHERE number=1", [])
                .unwrap();
            let change: String = main.db.query_row("SELECT after_json FROM fleet_outbox WHERE table_name='issues' ORDER BY seq DESC LIMIT 1", [], |r| r.get(0)).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&change).unwrap()["manual_blocked"],
                1
            );
        }
    }

    #[test]
    fn snapshots_preserve_legacy_project_rows_and_the_supervisors_unique_name_choice() {
        let main = Fixture::new();
        let agent = Fixture::new();
        main.capture();
        install_capture(&agent.db, "agent", "agent").unwrap();
        main.db.execute_batch("UPDATE fleet_meta SET syncing=1;
            INSERT INTO projects(id,name,next_number) VALUES('github.com/other/Native fleet','Native fleet',1);
            UPDATE fleet_meta SET syncing=0;").unwrap();
        agent.db.execute_batch("UPDATE fleet_meta SET syncing=1;
            INSERT INTO projects(id,name,next_number) VALUES('github.com/other/Native fleet','Native fleet',1);
            UPDATE project_name_keys SET project_id='github.com/other/Native fleet' WHERE name='Native fleet';
            UPDATE fleet_meta SET syncing=0;").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            agent
                .db
                .query_row(
                    "SELECT project_id FROM project_name_keys WHERE name='Native fleet'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "named:Native fleet"
        );
        assert_eq!(
            agent
                .db
                .query_row(
                    "SELECT count(*) FROM projects WHERE name='Native fleet'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        assert!(agent.db.query_row("SELECT legacy FROM project_name_collisions WHERE rejected_id='github.com/other/Native fleet'", [], |r| r.get::<_,bool>(0)).unwrap());
    }

    struct Fixture {
        path: PathBuf,
        db: Connection,
    }
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hb-native-fleet-{}.db",
                crate::issues::worker::random_id().unwrap()
            ));
            let mut store = Store::open(&path).unwrap();
            store
                .execute(&Request {
                    version: 1,
                    project: Project {
                        id: "named:Native fleet".into(),
                        name: "Native fleet".into(),
                    },
                    project_override: None,
                    actor: Some(Actor {
                        id: "human:fixture".into(),
                        kind: "human".into(),
                        session_id: None,
                        machine: "main".into(),
                        host: "fixture".into(),
                        pid: None,
                        process_start: None,
                        cwd: std::env::temp_dir(),
                        source: "test".into(),
                        invocation: None,
                        creation_run: None,
                    }),
                    operation: Operation::Create {
                        title: "Original".into(),
                        body: "Requirements".into(),
                        labels: vec![],
                        at_top: false,
                        draft: false,
                    },
                    request_id: None,
                })
                .unwrap();
            drop(store);
            let db = Connection::open(&path).unwrap();
            db.pragma_update(None, "foreign_keys", true).unwrap();
            Self { path, db }
        }
        fn capture(&self) {
            install_capture(&self.db, "controller", "main").unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn issue_transfer_replicates_append_only_comments_and_resolutions() {
        let main = Fixture::new();
        main.capture();
        main.db
            .execute(
                "INSERT INTO issue_agent_launches VALUES('launch-1','named:Native fleet',1,1)",
                [],
            )
            .unwrap();
        let actor = Actor {
            id: "human:boss".into(),
            kind: "human".into(),
            session_id: None,
            machine: "main".into(),
            host: "test".into(),
            pid: None,
            process_start: None,
            cwd: std::env::temp_dir(),
            source: "test".into(),
            invocation: None,
            creation_run: None,
        };
        let mut store = Store::open(&main.path).unwrap();
        let mut call = |project: &str, operation: Value| {
            store
                .execute(&Request {
                    version: 1,
                    project: Project {
                        id: "named:Native fleet".into(),
                        name: "Native fleet".into(),
                    },
                    project_override: Some(project.into()),
                    actor: Some(actor.clone()),
                    operation: serde_json::from_value(operation).unwrap(),
                    request_id: None,
                })
                .unwrap()
        };
        call(
            "Destination",
            json!({"action":"create","title":"Existing","body":"","labels":[]}),
        );
        let comment = call(
            "named:Native fleet",
            json!({"action":"comment","number":1,"body":"Keep this comment"}),
        );
        call(
            "named:Native fleet",
            json!({"action":"resolve_comment","number":1,"comment_id":comment["comment_id"],"resolved":true}),
        );
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        let before = snapshot(&main.db, "agent").unwrap();
        apply_pull(&agent.db, "agent", &before, &[]).unwrap();
        let issue = call("named:Native fleet", json!({"action":"view","number":1}));
        call(
            "named:Native fleet",
            json!({"action":"transfer","number":1,"destination":"Destination","if_version":issue["issue"]["version"]}),
        );
        let delta = incremental(&main.db, "agent", before["cursor"].as_i64().unwrap()).unwrap();
        apply_pull(&agent.db, "agent", &delta, &[]).unwrap();
        let mut replica = Store::open(&agent.path).unwrap();
        let result = replica
            .execute(&Request {
                version: 1,
                project: Project {
                    id: "named:Destination".into(),
                    name: "Destination".into(),
                },
                project_override: None,
                actor: Some(actor),
                operation: Operation::View { number: 2 },
                request_id: None,
            })
            .unwrap();
        assert_eq!(result["issue"]["agent_launch_count"], 1);
        assert_eq!(result["comments"][0]["body"], "Keep this comment");
        assert_eq!(result["comments"][0]["resolved"], true);
        let foreign_keys: i64 = agent
            .db
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(foreign_keys, 0);
    }

    #[test]
    fn offline_launch_replay_is_deduplicated_across_receipts_and_snapshots() {
        let main = Fixture::new();
        main.capture();
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        agent.db.execute("INSERT INTO issue_agent_launches VALUES('offline-launch','named:Native fleet',1,123)", []).unwrap();
        let changes = journal(&agent.db, 0).unwrap();
        let first = accept_changes(&main.db, "agent", &changes).unwrap();
        assert!(first.iter().all(|r| r["state"] != "conflict"), "{first:?}");
        let repeated = accept_changes(&main.db, "agent", &changes).unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &repeated,
        )
        .unwrap();
        for db in [&main.db, &agent.db] {
            assert_eq!(
                db.query_row("SELECT count(*) FROM issue_agent_launches", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }

    #[test]
    fn creation_origin_survives_replication_and_legacy_snapshots() {
        let main = Fixture::new();
        main.capture();
        let origin =
            json!({"session_id":"creator","host":"source-device","invocation":{"offset":123}})
                .to_string();
        main.db
            .execute("UPDATE issues SET origin=?1", [&origin])
            .unwrap();
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            agent
                .db
                .query_row("SELECT origin FROM issues", [], |r| r.get::<_, String>(0))
                .unwrap(),
            origin
        );
        let mut legacy = current_row(
            &agent.db,
            "issues",
            &json!({"project_id":"named:Native fleet","number":1}),
        )
        .unwrap();
        legacy.as_object_mut().unwrap().remove("origin");
        put_row(&agent.db, "issues", &legacy).unwrap();
        assert_eq!(
            agent
                .db
                .query_row("SELECT origin FROM issues", [], |r| r.get::<_, String>(0))
                .unwrap(),
            origin
        );
    }

    #[test]
    fn offline_status_keeps_stable_history_and_current_update_on_every_replica() {
        let main = Fixture::new();
        main.db
            .execute("UPDATE issues SET assignee='human:fixture'", [])
            .unwrap();
        main.db
            .execute(
                "INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'agent')",
                [],
            )
            .unwrap();
        main.capture();
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        for (id, level, at) in [("status-one", "green", 100), ("status-two", "orange", 101)] {
            agent.db.execute("INSERT INTO issue_status_updates VALUES(?1,'named:Native fleet',1,'human:fixture',?2,'Checking the layout.',?3)", rusqlite::params![id,level,at]).unwrap();
        }
        let changes = journal(&agent.db, 0).unwrap();
        let receipts = accept_changes(&main.db, "agent", &changes).unwrap();
        assert!(
            receipts.iter().all(|r| r["state"] != "conflict"),
            "{receipts:?}"
        );
        accept_changes(&main.db, "agent", &changes).unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &receipts,
        )
        .unwrap();
        for db in [&main.db, &agent.db] {
            assert_eq!(
                db.query_row("SELECT count(*) FROM issue_status_updates", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                2
            );
            assert_eq!(
                db.query_row(
                    "SELECT id FROM issue_status_updates ORDER BY created_at DESC,id DESC LIMIT 1",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "status-two"
            );
        }
        let third = Fixture::new();
        install_capture(&third.db, "agent", "third").unwrap();
        apply_pull(
            &third.db,
            "third",
            &snapshot(&main.db, "third").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            third
                .db
                .query_row("SELECT count(*) FROM issue_status_updates", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        let change = json!({"seq":999,"table_name":"issue_status_updates","before_json":rows(&agent.db,"SELECT * FROM issue_status_updates WHERE id='status-one'",&[]).unwrap()[0].to_string(),"after_json":null});
        assert_eq!(
            accept_changes(&main.db, "agent", &[change]).unwrap()[0]["state"],
            "conflict"
        );
        main.db
            .execute("UPDATE issues SET assignee=NULL", [])
            .unwrap();
        main.db
            .execute("UPDATE fleet_allocations SET node='another-machine'", [])
            .unwrap();
        agent.db.execute("INSERT INTO issue_status_updates VALUES('status-unassigned','named:Native fleet',1,'human:fixture','red','An update without ownership.',102)",[]).unwrap();
        let replay = accept_changes(&main.db, "agent", &journal(&agent.db, 0).unwrap()).unwrap();
        assert!(
            replay.iter().all(|r| r["state"] != "conflict"),
            "{replay:?}"
        );
        assert_eq!(
            main.db
                .query_row("SELECT count(*) FROM issue_status_updates", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &replay,
        )
        .unwrap();
        assert!(
            rows(&agent.db, "SELECT assignee FROM issues", &[]).unwrap()[0]["assignee"].is_null()
        );
        assert_eq!(
            rows(&main.db, "SELECT node FROM fleet_allocations", &[]).unwrap()[0]["node"],
            "another-machine"
        );
        for (id, change) in [
            ("status-closed", "state='closed'"),
            ("status-draft", "state='open',draft=1"),
            ("status-deleted", "draft=0,deleted_at=1"),
        ] {
            main.db
                .execute(&format!("UPDATE issues SET {change}"), [])
                .unwrap();
            agent.db.execute("INSERT INTO issue_status_updates VALUES(?1,'named:Native fleet',1,'human:fixture','red','Not eligible.',103)",[id]).unwrap();
            let replay =
                accept_changes(&main.db, "agent", &journal(&agent.db, 0).unwrap()).unwrap();
            assert!(replay.iter().any(|r| r["state"] == "conflict"));
            assert_eq!(
                rows(
                    &main.db,
                    "SELECT count(*) AS count FROM issue_status_updates",
                    &[]
                )
                .unwrap()[0]["count"],
                3
            );
        }
    }

    #[test]
    fn upgraded_pr_capture_preserves_classification_and_history_across_later_syncs() {
        for partial in [false, true] {
            let main = Fixture::new();
            main.capture();
            main.db
                .execute(
                    "INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'agent')",
                    [],
                )
                .unwrap();
            main.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at) VALUES('named:Native fleet',1,'https://github.com/example/repo/pull/1','human:fixture',123)", []).unwrap();
            let agent = Fixture::new();
            agent
                .db
                .execute_batch("ALTER TABLE issue_pull_requests DROP COLUMN purpose")
                .unwrap();
            install_capture(&agent.db, "agent", "agent").unwrap();
            if partial {
                agent.db.execute_batch("ALTER TABLE issue_pull_requests ADD COLUMN purpose TEXT NOT NULL DEFAULT 'unspecified'").unwrap();
            }
            let mut store = Store::open(&agent.path).unwrap();
            apply_pull(
                &agent.db,
                "agent",
                &snapshot(&main.db, "agent").unwrap(),
                &[],
            )
            .unwrap();
            let mut request: Request = serde_json::from_value(json!({
                "version":1,"project":{"id":"named:Native fleet","name":"Native fleet"},
                "project_override":null,"request_id":null,
                "actor":{"id":"human:fixture","kind":"human","session_id":null,"machine":"agent","host":"fixture","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},
                "operation":{"action":"classify_pull_request","number":1,"url":"https://github.com/example/repo/pull/1","purpose":"fix"}
            })).unwrap();
            store.execute(&request).unwrap();
            let changes = journal(&agent.db, 0).unwrap();
            assert!(
                changes
                    .iter()
                    .any(|c| c["table_name"] == "issue_pull_requests"),
                "An upgraded trigger must capture a purpose-only update"
            );
            let receipts = accept_changes(&main.db, "agent", &changes).unwrap();
            assert!(receipts.iter().all(|r| r["state"] == "applied"));
            apply_pull(
                &agent.db,
                "agent",
                &snapshot(&main.db, "agent").unwrap(),
                &receipts,
            )
            .unwrap();
            agent
                .db
                .execute("UPDATE issues SET title='Unrelated edit'", [])
                .unwrap();
            let receipts =
                accept_changes(&main.db, "agent", &journal(&agent.db, 0).unwrap()).unwrap();
            apply_pull(
                &agent.db,
                "agent",
                &snapshot(&main.db, "agent").unwrap(),
                &receipts,
            )
            .unwrap();
            for db in [&main.db, &agent.db] {
                assert_eq!(
                    rows(db, "SELECT purpose FROM issue_pull_requests", &[]).unwrap()[0]["purpose"],
                    "fix"
                );
                assert_eq!(rows(db, "SELECT json_extract(data,'$.purpose') AS purpose FROM events WHERE action='pr_classified'", &[]).unwrap()[0]["purpose"], "fix");
            }
            request.operation =
                serde_json::from_value(json!({"action":"pull_requests","number":1})).unwrap();
            assert_eq!(
                store.execute(&request).unwrap()["pull_requests"][0]["purpose"],
                "fix"
            );
            request.operation =
                serde_json::from_value(json!({"action":"view","number":1})).unwrap();
            assert_eq!(
                store.execute(&request).unwrap()["issue"]["pull_requests"][0]["purpose"],
                "fix"
            );
        }
    }

    #[test]
    fn legacy_pr_snapshot_does_not_erase_a_known_purpose() {
        let f = Fixture::new();
        f.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose) VALUES('named:Native fleet',1,'https://github.com/example/repo/pull/1','human:fixture',123,'supporting-evidence')", []).unwrap();
        put_row(&f.db, "issue_pull_requests", &json!({"project_id":"named:Native fleet","issue_number":1,"url":"https://github.com/example/repo/pull/1","added_by":"human:fixture","created_at":123})).unwrap();
        assert_eq!(
            rows(&f.db, "SELECT purpose FROM issue_pull_requests", &[]).unwrap()[0]["purpose"],
            "supporting-evidence"
        );
    }

    #[test]
    fn pr_purpose_survives_snapshot_and_offline_classification_replay() {
        let main = Fixture::new();
        main.capture();
        main.db
            .execute(
                "INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'agent')",
                [],
            )
            .unwrap();
        main.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose) VALUES('named:Native fleet',1,'https://github.com/example/repo/pull/1','human:fixture',123,'fix')", []).unwrap();
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            rows(&agent.db, "SELECT purpose FROM issue_pull_requests", &[]).unwrap()[0]["purpose"],
            "fix"
        );
        agent
            .db
            .execute(
                "UPDATE issue_pull_requests SET purpose='supporting-evidence'",
                [],
            )
            .unwrap();
        let changes = journal(&agent.db, 0).unwrap();
        let receipts = accept_changes(&main.db, "agent", &changes).unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &receipts,
        )
        .unwrap();
        for db in [&main.db, &agent.db] {
            let pr = &rows(db, "SELECT * FROM issue_pull_requests", &[]).unwrap()[0];
            assert_eq!(pr["purpose"], "supporting-evidence");
            assert_eq!(pr["added_by"], "human:fixture");
            assert_eq!(pr["created_at"], 123);
        }
    }

    #[test]
    fn legacy_project_settings_replay_uses_migration_defaults() {
        let main = Fixture::new();
        main.capture();
        main.db.execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,boss_name) VALUES('named:Native fleet','Instructions',0,1,'Boss')", []).unwrap();
        let expected = rows(&main.db, "SELECT * FROM project_settings", &[]).unwrap()[0].clone();
        let mut legacy = expected.clone();
        for column in [
            "drafts_enabled",
            "plan_template",
            "worktree_enabled",
            "prompt_overrides",
            "chief_enabled",
            "chief_prompt",
        ] {
            legacy.as_object_mut().unwrap().remove(column);
        }
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        let pull = json!({"changes":[{"seq":100,"table_name":"project_settings","before_json":null,"after_json":legacy.to_string()}],"cursor":100,"allocations":[],"ranges":[]});
        apply_pull(&agent.db, "agent", &pull, &[]).unwrap();
        assert_eq!(
            rows(&agent.db, "SELECT * FROM project_settings", &[]).unwrap()[0],
            expected
        );
        assert_eq!(state_get(&agent.db, "cursor", json!(0)).unwrap(), 100);
    }

    #[test]
    fn project_settings_snapshot_preserves_explicit_additive_fields() {
        let main = Fixture::new();
        main.capture();
        main.db.execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,boss_name,drafts_enabled,plan_template,worktree_enabled,prompt_overrides,chief_enabled,chief_prompt) VALUES('named:Native fleet','Shared instructions',1,7,'Boss',0,'custom/{number}.md',1,'{\"main\":\"Ship {{number}}\"}',1,'Review the release')", []).unwrap();
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            rows(&agent.db, "SELECT * FROM project_settings", &[]).unwrap(),
            rows(&main.db, "SELECT * FROM project_settings", &[]).unwrap()
        );
    }

    #[test]
    fn legacy_project_settings_reject_missing_required_and_unknown_fields() {
        let f = Fixture::new();
        let legacy = json!({"project_id":"named:Native fleet","prompt":"Instructions","prs_enabled":0,"version":1,"boss_name":"Boss"});
        put_row(&f.db, "project_settings", &legacy).unwrap();
        let original = rows(&f.db, "SELECT * FROM project_settings", &[]).unwrap();
        for required in [
            "project_id",
            "prompt",
            "prs_enabled",
            "version",
            "boss_name",
        ] {
            let mut incomplete = legacy.clone();
            incomplete.as_object_mut().unwrap().remove(required);
            assert!(
                put_row(&f.db, "project_settings", &incomplete)
                    .unwrap_err()
                    .to_string()
                    .contains("Schema mismatch for project_settings"),
                "{required}"
            );
        }
        let mut unknown = legacy;
        unknown["future_setting"] = json!(true);
        assert!(
            put_row(&f.db, "project_settings", &unknown)
                .unwrap_err()
                .to_string()
                .contains("Schema mismatch for project_settings")
        );
        assert_eq!(
            rows(&f.db, "SELECT * FROM project_settings", &[]).unwrap(),
            original
        );
    }

    #[test]
    fn legacy_replicated_pr_defaults_to_unspecified() {
        let f = Fixture::new();
        let pr = json!({"project_id":"named:Native fleet","issue_number":1,"url":"https://github.com/example/repo/pull/1","added_by":"human:fixture","created_at":123});
        put_row(&f.db, "issue_pull_requests", &pr).unwrap();
        assert_eq!(
            rows(&f.db, "SELECT purpose FROM issue_pull_requests", &[]).unwrap()[0]["purpose"],
            "unspecified"
        );
    }

    #[test]
    fn snapshot_preserves_protocol_and_journals_only_real_changes() {
        let f = Fixture::new();
        f.capture();
        f.db.execute("UPDATE issues SET title=title", []).unwrap();
        let unchanged: i64 =
            f.db.query_row("SELECT count(*) FROM fleet_outbox", [], |r| r.get(0))
                .unwrap();
        assert_eq!(unchanged, 0);
        f.db.execute("UPDATE issues SET title='Edited'", [])
            .unwrap();
        let payload = snapshot(&f.db, "agent").unwrap();
        assert_eq!(payload["tables"]["issues"][0]["title"], "Edited");
        assert_eq!(payload["cursor"], 1);
        assert!(payload["allocations"].is_array());
        assert!(payload["ranges"].is_array());
        assert_eq!(
            f.db.query_row("SELECT role FROM fleet_meta", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "controller"
        );
    }

    #[test]
    fn replay_merges_independent_fields_and_returns_durable_receipts() {
        let f = Fixture::new();
        f.capture();
        f.db.execute(
            "INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'agent')",
            [],
        )
        .unwrap();
        let before = snapshot(&f.db, "agent").unwrap()["tables"]["issues"][0].clone();
        let mut after = before.clone();
        after["body"] = json!("Offline body");
        let change = json!({"seq":1,"table_name":"issues","before_json":before.to_string(),"after_json":after.to_string(),"created_at":0});
        f.db.execute("UPDATE issues SET title='Online title'", [])
            .unwrap();
        let receipts = accept_changes(&f.db, "agent", std::slice::from_ref(&change)).unwrap();
        assert_eq!(receipts[0]["state"], "applied");
        assert_eq!(accept_changes(&f.db, "agent", &[change]).unwrap(), receipts);
        let row = &snapshot(&f.db, "agent").unwrap()["tables"]["issues"][0];
        assert_eq!(row["title"], "Online title");
        assert_eq!(row["body"], "Offline body");
    }

    #[test]
    fn offline_completion_cannot_overwrite_changed_requirements() {
        let f = Fixture::new();
        f.capture();
        f.db.execute(
            "INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'agent')",
            [],
        )
        .unwrap();
        let before = snapshot(&f.db, "agent").unwrap()["tables"]["issues"][0].clone();
        let mut after = before.clone();
        after["state"] = json!("closed");
        f.db.execute("UPDATE issues SET body='New requirements'", [])
            .unwrap();
        let receipts=accept_changes(&f.db,"agent",&[json!({"seq":1,"table_name":"issues","before_json":before.to_string(),"after_json":after.to_string(),"created_at":0})]).unwrap();
        assert_eq!(receipts[0]["state"], "conflict");
        assert_eq!(
            snapshot(&f.db, "agent").unwrap()["tables"]["issues"][0]["state"],
            "open"
        );
        assert_eq!(
            f.db.query_row("SELECT count(*) FROM fleet_conflicts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn pull_retains_pending_edits_then_converges_after_acknowledgment() {
        let main = Fixture::new();
        main.capture();
        main.db
            .execute(
                "INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'agent')",
                [],
            )
            .unwrap();
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        agent
            .db
            .execute("UPDATE issues SET body='Offline edit'", [])
            .unwrap();
        main.db
            .execute("UPDATE issues SET title='Online edit'", [])
            .unwrap();
        let payload = snapshot(&main.db, "agent").unwrap();
        apply_pull(&agent.db, "agent", &payload, &[]).unwrap();
        assert_eq!(
            snapshot(&agent.db, "agent").unwrap()["tables"]["issues"][0]["body"],
            "Offline edit"
        );
        let changes = journal(&agent.db, 0).unwrap();
        let receipts = accept_changes(&main.db, "agent", &changes).unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &receipts,
        )
        .unwrap();
        let row = &snapshot(&agent.db, "agent").unwrap()["tables"]["issues"][0];
        assert_eq!(row["title"], "Online edit");
        assert_eq!(row["body"], "Offline edit");
        assert!(journal(&agent.db, 0).unwrap().is_empty());
        assert_eq!(
            agent
                .db
                .query_row("SELECT syncing FROM fleet_meta", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn allocations_preserve_ownership_and_exclude_drafts() {
        let f = Fixture::new();
        f.capture();
        let workers = vec![
            json!({"id":"worker","config":{"projects":["named:Native fleet"],"concurrency":1,"tags":[],"enabled":true},"intent":"running"}),
        ];
        f.db.execute("UPDATE issues SET draft=1", []).unwrap();
        allocate(&f.db, "agent", &workers).unwrap();
        assert!(
            snapshot(&f.db, "agent").unwrap()["allocations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        f.db.execute("UPDATE issues SET draft=0", []).unwrap();
        allocate(&f.db, "agent", &workers).unwrap();
        allocate(&f.db, "other", &workers).unwrap();
        let payload = snapshot(&f.db, "agent").unwrap();
        assert_eq!(payload["allocations"].as_array().unwrap().len(), 1);
        assert_eq!(payload["allocations"][0]["node"], "agent");
        assert_eq!(
            payload["ranges"][0]["last_number"].as_i64().unwrap()
                - payload["ranges"][0]["first_number"].as_i64().unwrap(),
            99
        );
    }
    #[test]
    fn rejected_pull_rolls_back_receipts_and_preserves_journal() {
        let f = Fixture::new();
        f.capture();
        f.db.execute("UPDATE issues SET body='Pending edit'", [])
            .unwrap();
        let before = journal(&f.db, 0).unwrap();
        assert!(
            apply_pull(
                &f.db,
                "main",
                &json!({"tables":{},"cursor":1}),
                &[json!({"seq":1,"state":"applied"})]
            )
            .is_err()
        );
        assert_eq!(journal(&f.db, 0).unwrap(), before);
        assert_eq!(
            f.db.query_row("SELECT syncing FROM fleet_meta", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn blocker_links_and_automatic_lifecycle_converge_across_snapshots() {
        let main = Fixture::new();
        main.capture();
        main.db.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order,blockers) VALUES('named:Native fleet',2,'Dependent','','open','human:fixture',0,0,1,'[]',2,'[1]');").unwrap();
        crate::issues::blockers::reconcile_all(&main.db).unwrap();
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            rows(
                &agent.db,
                "SELECT state,blockers FROM issues WHERE number=2",
                &[]
            )
            .unwrap()[0],
            json!({"state":"blocked","blockers":"[1]"})
        );
        main.db
            .execute("UPDATE issues SET state='closed' WHERE number=1", [])
            .unwrap();
        crate::issues::blockers::reconcile_all(&main.db).unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            rows(&agent.db, "SELECT state FROM issues WHERE number=2", &[]).unwrap()[0]["state"],
            "open"
        );
        main.db
            .execute("UPDATE issues SET state='open' WHERE number=1", [])
            .unwrap();
        crate::issues::blockers::reconcile_all(&main.db).unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            rows(&agent.db, "SELECT state FROM issues WHERE number=2", &[]).unwrap()[0]["state"],
            "blocked"
        );
    }
}
