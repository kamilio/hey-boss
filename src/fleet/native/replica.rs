//! Transactional issue replication using the CLI's bundled SQLite.
use super::Result;
use crate::database::{Connection, params_from_iter};
use rusqlite::{
    OptionalExtension,
    types::{Value as SqlValue, ValueRef},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const TABLES: &[(&str, &[&str])] = &[
    ("projects", &["id"]),
    ("agents", &["id"]),
    ("issues", &["project_id", "number"]),
    ("issue_status_updates", &["id"]),
    ("github_fetch_status", &["url"]),
    ("issue_github_watches", &["project_id", "issue_number"]),
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
    db.query_collect(sql, params_from_iter(args(values)), |row| {
        let mut value = serde_json::Map::new();
        for index in 0..row.column_count() {
            let item = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Real(n) => json!(n),
                ValueRef::Text(s) => json!(std::str::from_utf8(s)?),
                ValueRef::Blob(_) => return Err(invalid("Unexpected blob in fleet row")),
            };
            value.insert(row.column_name(index)?.to_owned(), item);
        }
        Ok(Value::Object(value))
    })
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
    let encoded = value.to_string();
    // A repeated status save needs only a WAL read, even while another client
    // owns the writer. Changed values retain the atomic upsert below.
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM fleet_state WHERE key=?1 AND value=?2)",
        [key, &encoded],
        |row| row.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    db.execute(
        "INSERT INTO fleet_state VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [key, &encoded],
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
    RowWriter::new(db).put(table, row)
}

// Batch callers keep this writer inside their transaction. The next batch
// reloads schema metadata, including any migrations committed in between.
struct RowWriter<'a> {
    db: &'a Connection,
    plans: BTreeMap<String, RowPlan>,
    changed_projects: BTreeSet<String>,
    archives: Option<crate::issues::archive::transfer::Catalog>,
}
impl<'a> RowWriter<'a> {
    fn new(db: &'a Connection) -> Self {
        Self {
            db,
            plans: BTreeMap::new(),
            changed_projects: BTreeSet::new(),
            archives: None,
        }
    }
    fn put(&mut self, table: &str, row: &Value) -> Result<()> {
        keys(table)?;
        let db = self.db;
        let mut row = row.clone();
        if table == "issues" {
            super::archive::ensure_copy(db, &row, &mut self.archives)?;
            let m = row
                .as_object_mut()
                .ok_or_else(|| invalid("Invalid issue row"))?;
            super::archive::incoming_issue(m);
            let manual = i64::from(m["state"] == "blocked");
            m.entry("manual_blocked").or_insert(json!(manual));
            m.entry("blockers").or_insert(json!("[]"));
            for column in ["assignment_target", "github_ack_event", "attempt_hold"] {
                if !m.contains_key(column) {
                    let target: Option<Option<String>> = db
                        .query_row(
                            &format!(
                                "SELECT {column} FROM issues WHERE project_id=?1 AND number=?2"
                            ),
                            rusqlite::params![m["project_id"].as_str(), m["number"].as_i64()],
                            |r| r.get(0),
                        )
                        .optional()?;
                    m.insert(column.into(), json!(target.flatten()));
                }
            }
            // Older peers cannot express these fields. Preserve local values when
            // merging their rows; only an explicit modern value may change them.
            // Full modern rows take no extra database read.
            if !m.contains_key("draft") || !m.contains_key("plan") {
                let existing: Option<(i64, Option<String>)> = db
                    .query_row(
                        "SELECT draft,plan FROM issues WHERE project_id=?1 AND number=?2",
                        rusqlite::params![m["project_id"].as_str(), m["number"].as_i64()],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                let (draft, plan) = existing.unwrap_or((0, None));
                m.entry("draft").or_insert(json!(draft));
                m.entry("plan").or_insert(json!(plan));
            }
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
            // Retained only for older peers; sibling scheduling is no longer supported.
            m.insert("subtask_scheduling".into(), json!("explicit"));
        }
        let defaults: &[(&str, Value)] = match table {
            "project_settings" => &[
                ("drafts_enabled", json!(1)),
                ("plan_template", json!("plans/{timestamp}-{number}.md")),
                ("worktree_enabled", json!(0)),
                ("prompt_overrides", json!("{}")),
                ("chief_enabled", json!(0)),
                ("chief_prompt", Value::Null),
            ],
            "issue_pull_requests" => &[
                ("purpose", json!("unspecified")),
                ("status", json!("unknown")),
                ("checked_at", Value::Null),
                ("error", Value::Null),
                ("merged_at", Value::Null),
                ("pr_title", Value::Null),
                ("author_id", Value::Null),
            ],
            "global_settings" => &[
                ("auto_close_merged_prs", json!(1)),
                ("quiet_hours", Value::Null),
                ("github_user_id", Value::Null),
            ],
            _ => &[],
        };
        // Only old capture triggers omit additive fields. Read their existing
        // values once, preserving explicit nulls and avoiding reads for modern rows.
        if defaults.iter().any(|(column, _)| row.get(column).is_none()) {
            let existing = current_row(db, table, &row)?;
            let m = row
                .as_object_mut()
                .ok_or_else(|| invalid("Invalid settings or PR row"))?;
            for (column, default) in defaults {
                m.entry(*column)
                    .or_insert_with(|| existing.get(column).unwrap_or(default).clone());
            }
        }
        if !self.plans.contains_key(table) {
            self.plans
                .insert(table.to_owned(), RowPlan::load(db, table)?);
        }
        let plan = &self.plans[table];
        let object = row
            .as_object()
            .ok_or_else(|| invalid("Invalid fleet row"))?;
        if plan.columns.iter().any(|c| !object.contains_key(c))
            || object.len() != plan.columns.len()
        {
            return Err(invalid(&format!("Schema mismatch for {table}")));
        }
        let changed = execute(
            db,
            &plan.sql,
            &plan
                .columns
                .iter()
                .map(|c| row[c].clone())
                .collect::<Vec<_>>(),
        )?;
        self.changed(table, &row, changed);
        Ok(())
    }
    fn delete(&mut self, table: &str, row: &Value) -> Result<()> {
        let (clause, values) = key_where(table, row)?;
        let changed = execute(
            self.db,
            &format!("DELETE FROM {table} WHERE {clause}"),
            &values,
        )?;
        self.changed(table, row, changed);
        Ok(())
    }
    fn changed(&mut self, table: &str, row: &Value, count: usize) {
        if count > 0
            && matches!(table, "issues" | "issue_subtasks")
            && let Some(project) = row["project_id"].as_str()
        {
            self.changed_projects.insert(project.to_owned());
        }
    }
    fn reconcile(&self) -> Result<()> {
        let now = crate::issues::worker::now();
        for project in &self.changed_projects {
            crate::issues::blockers::reconcile(self.db, project, None, now)?;
        }
        Ok(())
    }
}
struct RowPlan {
    columns: Vec<String>,
    sql: String,
}
impl RowPlan {
    fn load(db: &Connection, table: &str) -> Result<Self> {
        let k = keys(table)?;
        let columns = rows(db, &format!("PRAGMA table_info({table})"), &[])?
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        let names = columns
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(",");
        let updates = columns
            .iter()
            .filter(|c| !k.contains(&c.as_str()))
            .map(|c| {
                if table=="issues" && matches!(c.as_str(),"archive_cleanup"|"archive_restoring") {
                    format!("\"{c}\"=CASE WHEN issues.archive_key IS excluded.archive_key THEN issues.\"{c}\" ELSE excluded.\"{c}\" END")
                } else if table=="issues" && c=="archive_touched_at" {
                    "archive_touched_at=max(issues.archive_touched_at,excluded.archive_touched_at)".into()
                } else { format!("\"{c}\"=excluded.\"{c}\"") }
            })
            .collect::<Vec<_>>()
            .join(",");
        let placeholders = vec!["?"; columns.len()].join(",");
        let sql = format!(
            "INSERT INTO {table}({names}) VALUES({placeholders}) ON CONFLICT({}) DO UPDATE SET {updates}",
            k.join(",")
        );
        Ok(Self { columns, sql })
    }
}
pub(super) fn ensure_metadata(db: &Connection) -> Result<()> {
    // Existing installations need only a schema read. Even a no-op DDL batch
    // would acquire the service writer and delay unrelated fleet requests.
    if db.query_row("SELECT count(*)=4 FROM sqlite_master WHERE type='table' AND name IN ('fleet_ranges','fleet_number_reservations','fleet_signals','fleet_state')", [], |row| row.get::<_,bool>(0))? {
        return Ok(());
    }
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
    db.execute_batch(crate::issues::FLEET_INDEXES)?;
    db.execute_batch("CREATE INDEX IF NOT EXISTS fleet_outbox_issue_archive ON fleet_outbox(json_extract(coalesce(after_json,before_json),'$.project_id'),coalesce(json_extract(coalesce(after_json,before_json),'$.number'),json_extract(coalesce(after_json,before_json),'$.issue_number'))) WHERE table_name IN ('issues','comments','events','issue_status_updates');")?;
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
            .filter(|column| {
                *table != "issues"
                    || (!super::archive::local_column(column)
                        && (role != "agent"
                            || !matches!(column.as_str(), "archive_key" | "archived_comments")))
            })
            .collect::<Vec<_>>();
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
        let mut repaired = false;
        for (operation, before, after) in [
            ("INSERT", None, Some("NEW")),
            ("UPDATE", Some("OLD"), Some("NEW")),
            ("DELETE", Some("OLD"), None),
        ] {
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
            let name = format!("fleet_capture_{table}_{operation}");
            let sql = format!(
                "CREATE TRIGGER {name} AFTER {operation} ON {table} WHEN (SELECT syncing FROM fleet_meta WHERE id=1)=0{different} BEGIN INSERT INTO fleet_outbox(table_name,before_json,after_json,created_at) VALUES('{table}',{},{},CAST(strftime('%s','now') AS INTEGER)*1000); END",
                row_json(before),
                row_json(after)
            );
            let existing: Option<String> = db
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                    [&name],
                    |r| r.get(0),
                )
                .optional()?;
            if existing.as_deref().map(|s| s.trim_end_matches(';')) != Some(sql.as_str()) {
                repaired |= existing.is_some();
                db.execute_batch(&format!("DROP TRIGGER IF EXISTS {name}; {sql};"))?;
            }
        }
        if repaired && role == "controller" && *table == "project_settings" {
            // Peers may already have acknowledged a truncated settings row.
            // Republish current values once, in the same transaction as repair.
            db.execute_batch(&format!("INSERT INTO fleet_outbox(table_name,after_json,created_at) SELECT '{table}',{},CAST(strftime('%s','now') AS INTEGER)*1000 FROM {table} AS replay;", row_json(Some("replay"))))?;
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
    // Indexed raw sizes are a lower bound on the escaped wire representation.
    // Select its possible prefix before loading bodies; the exact check below
    // retains the existing batch boundary and oversized-first-record behavior.
    for mut row in rows(
        db,
        "WITH candidates AS MATERIALIZED (
            SELECT seq,coalesce(length(CAST(before_json AS BLOB)),0)+coalesce(length(CAST(after_json AS BLOB)),0) AS bytes
            FROM fleet_outbox INDEXED BY fleet_outbox_retention WHERE seq>?1 ORDER BY seq LIMIT 300
        ), prefix AS MATERIALIZED (
            SELECT seq,sum(bytes) OVER (ORDER BY seq ROWS UNBOUNDED PRECEDING) AS bytes,
                row_number() OVER (ORDER BY seq) AS position FROM candidates
        )
        SELECT journal.* FROM prefix CROSS JOIN fleet_outbox journal ON journal.seq=prefix.seq
        WHERE prefix.bytes<=?2 OR prefix.position=1 ORDER BY journal.seq",
        &[json!(after), json!(crate::issues::WIRE_LIMIT / 3)],
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
) -> Result<Option<i64>> {
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
        let present: bool = db.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
            [id],
            |r| r.get(0),
        )?;
        let metadata = table == "events"
            && matches!(
                row["action"].as_str(),
                Some(
                    "moved_to"
                        | "pr_attached"
                        | "pr_classified"
                        | "attempt_reconciled"
                        | "commit_attached"
                        | "commit_removed"
                )
            );
        if present || (!metadata && db.query_row("SELECT EXISTS(SELECT 1 FROM issues WHERE project_id=?1 AND number=?2 AND archive_key IS NOT NULL)",rusqlite::params![row["project_id"].as_str(),row["issue_number"].as_i64()],|r|r.get::<_,bool>(0))?) {
            return Ok(Some(id));
        }
    }
    let (own, role): (String, String) =
        db.query_row("SELECT node,role FROM fleet_meta WHERE id=1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let local_id = if own == origin && mapped.is_none() {
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
        if let Some(id) = mapped {
            values.insert("id".into(), json!(id));
        }
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
                let invalidate = role != "agent"
                    && matches!(table, "comments" | "events")
                    && crate::issues::Store::replicated_handoff_changes(
                        db,
                        row["project_id"]
                            .as_str()
                            .ok_or_else(|| invalid("Invalid history project"))?,
                        row["issue_number"]
                            .as_i64()
                            .ok_or_else(|| invalid("Invalid history issue"))?,
                        row[if table == "comments" {
                            "author"
                        } else {
                            "actor"
                        }]
                        .as_str()
                        .ok_or_else(|| invalid("Invalid history actor"))?,
                        if table == "comments" {
                            "commented"
                        } else {
                            row["action"]
                                .as_str()
                                .ok_or_else(|| invalid("Invalid history action"))?
                        },
                    )?;
                if invalidate {
                    // Keep the guard delta ahead of the history in the journal.
                    // A policy-suppressed append must roll both writes back.
                    db.execute_batch("SAVEPOINT handoff_history")?;
                    execute(
                        db,
                        "UPDATE issues SET version=version+1,updated_at=max(updated_at,?) WHERE project_id=? AND number=?",
                        &[
                            json!(crate::issues::worker::now()),
                            values["project_id"].clone(),
                            values["issue_number"].clone(),
                        ],
                    )?;
                }
                let inserted = execute(
                    db,
                    &format!(
                        "INSERT INTO {table}({}) VALUES({})",
                        columns.join(","),
                        vec!["?"; columns.len()].join(",")
                    ),
                    &parameters,
                )?;
                if invalidate {
                    db.execute_batch(if inserted == 0 {
                        "ROLLBACK TO handoff_history; RELEASE handoff_history"
                    } else {
                        "RELEASE handoff_history"
                    })?;
                }
                // Policy guards may suppress a stale generated notice. Never
                // map its remote ID to the connection's previous insert.
                if inserted == 0 {
                    return Ok(None);
                }
                db.last_insert_rowid()
            }
        }
    };
    db.execute(
        "INSERT OR IGNORE INTO fleet_row_ids VALUES(?,?,?,?)",
        rusqlite::params![origin, table, origin_id, local_id],
    )?;
    Ok(Some(local_id))
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
fn strip_issue_archive_fields(row: &mut Value) {
    if let Some(row) = row.as_object_mut() {
        row.retain(|field, _| !field.starts_with("archive_") && field != "archived_comments");
    }
}
fn apply_change(writer: &mut RowWriter<'_>, node: &str, change: &Value) -> Result<Value> {
    let db = writer.db;
    let table = change["table_name"]
        .as_str()
        .ok_or_else(|| invalid("Invalid replicated table"))?;
    keys(table)?;
    if matches!(table, "issue_github_watches" | "github_fetch_status") {
        return Err(invalid("GitHub observations are written by the supervisor"));
    }
    let mut before = row_json(change, "before_json")?;
    let mut after = row_json(change, "after_json")?;
    if table == "issues" {
        if after["archive_key"].is_string() {
            return Err(invalid(
                "Issue archive manifests are written by the supervisor",
            ));
        }
        for row in [&mut before, &mut after] {
            strip_issue_archive_fields(row);
        }
    }
    let key = if after.is_null() { &before } else { &after };
    let old = current_row(db, table, key)?;
    if table == "issue_pull_requests" {
        // Observation fields belong to the supervisor. Offline purpose edits
        // neither conflict with polling nor overwrite newer GitHub evidence.
        for (column, default) in [
            ("status", json!("unknown")),
            ("checked_at", Value::Null),
            ("error", Value::Null),
            ("merged_at", Value::Null),
            ("pr_title", Value::Null),
        ] {
            let current = old.get(column).cloned().unwrap_or(default);
            for row in [&mut before, &mut after] {
                if let Some(row) = row.as_object_mut() {
                    row.insert(column.into(), current.clone());
                }
            }
        }
    }
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
        writer.put(table, &after)?;
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
        let Some(local) = append_row(db, node, table, &after, change["bootstrap"] == true)? else {
            result["suppressed"] = json!("obsolete dependency notice");
            return Ok(result);
        };
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
            let merged = crate::issues::model_recovery::merge_recovered(&old, &before, &after)
                .map_err(|error| invalid(&error.message))?;
            writer.put(table, &merged)?;
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
            // Compare the same logical fields on both sides. Archive bookkeeping
            // belongs to the supervisor and must neither cause a collision nor
            // be overwritten by an otherwise identical bootstrap row.
            let mut logical_old = old.clone();
            strip_issue_archive_fields(&mut logical_old);
            if logical_old != after {
                if !old.is_null() || !(allocated || legacy) {
                    return Err(invalid("Offline issue number is not exclusively allocated"));
                }
                writer.put(table, &after)?;
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
            let hold_change = before["attempt_hold"] != after["attempt_hold"];
            let hold: Value = serde_json::from_str(
                after["attempt_hold"]
                    .as_str()
                    .or(before["attempt_hold"].as_str())
                    .unwrap_or("null"),
            )?;
            let guarded_hold = hold_change
                && old["version"] == before["version"]
                && old["attempt_hold"] == before["attempt_hold"]
                && hold["machine"] == node
                && changed.iter().all(|(key, _)| {
                    key.as_str() == "attempt_hold"
                        || (key.as_str() == "assignee"
                            && after["attempt_hold"].is_null()
                            && after["assignee"].is_null()
                            && before["assignee"] == hold["owner"])
                });
            if hold_change && !guarded_hold {
                return Err(invalid(
                    "Surviving attempt changed or belongs to another machine; hold preserved",
                ));
            }
            if !old["attempt_hold"].is_null()
                && !guarded_hold
                && changed.iter().any(|(key, value)| {
                    matches!(key.as_str(), "state" | "deleted_at")
                        || (key.as_str() == "assignee" && !value.is_null())
                })
            {
                return Err(invalid(
                    "A surviving task attempt prevents lifecycle changes until reconciliation",
                ));
            }
            if !changed.is_empty()
                && !guarded_hold
                && !owner.first().is_some_and(|r| r["node"] == node)
            {
                return Err(invalid(
                    "Issue allocation was revoked or belongs to another machine",
                ));
            }
            if (after["state"] == "closed" || after["state"] == "ready")
                && before["state"] != after["state"]
                && ["title", "body", "labels"]
                    .iter()
                    .any(|k| old[*k] != before[*k])
            {
                return Err(invalid(
                    "Issue requirements changed before offline completion",
                ));
            }
            if after["state"] != before["state"]
                && after["state"] != old["state"]
                && old["assignee"] != before["assignee"]
            {
                return Err(invalid(
                    "Issue ownership changed before an offline state transition",
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
            // A field-identical merge may retain an exact handoff revision.
            // Foreign history invalidates it independently, even in split batches.
            let preserves_requirements = changed.is_empty();
            let mut merged = old.clone();
            for (k, v) in changed {
                merged[k] = v.clone();
            }
            // A companion can finish before its next pull delivers a newer
            // GitHub event. Keep that event queued instead of parking the task.
            if merged["assignment_target"] == "github" && merged["assignee"] == "watcher:github" {
                let pending: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM issue_github_watches WHERE project_id=?1 AND issue_number=?2 AND json_type(status,'$.event')='text' AND json_extract(status,'$.event') IS NOT ?3)",rusqlite::params![merged["project_id"].as_str(),merged["number"].as_i64(),merged["github_ack_event"].as_str()],|r|r.get(0))?;
                if pending {
                    merged["assignee"] = Value::Null;
                    if merged["state"] == "ready" {
                        merged["state"] = json!("open");
                    }
                } else {
                    execute(
                        db,
                        "DELETE FROM fleet_allocations WHERE project_id=? AND issue_number=?",
                        &[merged["project_id"].clone(), merged["number"].clone()],
                    )?;
                }
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
            let preserved_version = if preserves_requirements {
                crate::issues::Store::preserve_requirements_handoff(
                    db,
                    old["project_id"].as_str().unwrap(),
                    old["number"].as_i64().unwrap(),
                    None,
                    crate::issues::worker::now(),
                )?
            } else {
                false
            };
            merged["version"] =
                json!(old["version"].as_i64().unwrap() + i64::from(!preserved_version));
            merged["updated_at"] = json!(
                old["updated_at"]
                    .as_i64()
                    .unwrap()
                    .max(after["updated_at"].as_i64().unwrap())
            );
            writer.put(table, &merged)?;
        }
    } else {
        if !before.is_null() && old != before && old != after {
            return Err(invalid("Concurrent configuration or PR change"));
        }
        if before.is_null() && !old.is_null() && old != after {
            return Err(invalid("Conflicting inserted row"));
        }
        if after.is_null() {
            writer.delete(table, &before)?;
        } else {
            writer.put(table, &after)?;
        }
    }
    if table == "issue_subtasks" {
        let key = if after.is_null() { &before } else { &after };
        crate::issues::blockers::validate_subtask_claims(
            db,
            key["project_id"]
                .as_str()
                .ok_or_else(|| invalid("Invalid subtask project"))?,
        )
        .map_err(|error| invalid(&error.message))?;
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
    let mut writer = RowWriter::new(db);
    // Canonical journals use integer sequences. Read only this batch's receipts
    // and update the cache after each successful insert, including in-batch repeats.
    // Retain SQLite's affinity semantics for older/noncanonical sequence values.
    let mut saved = match changes
        .iter()
        .map(|change| change["seq"].as_i64())
        .collect::<Option<Vec<_>>>()
    {
        Some(sequences) => {
            let mut saved = BTreeMap::new();
            if !sequences.is_empty() {
                for row in rows(
                    db,
                    "SELECT r.seq,r.result FROM json_each(?2) requested CROSS JOIN fleet_receipts r WHERE r.node=?1 AND r.seq=requested.value",
                    &[json!(node), json!(serde_json::to_string(&sequences)?)],
                )? {
                    saved.insert(
                        row["seq"].as_i64().unwrap(),
                        row["result"].as_str().unwrap().to_owned(),
                    );
                }
            }
            Some(saved)
        }
        None => None,
    };
    let unapplied: Vec<_> = changes
        .iter()
        .filter(|change| {
            saved
                .as_ref()
                .is_none_or(|saved| !saved.contains_key(&change["seq"].as_i64().unwrap()))
        })
        .cloned()
        .collect();
    let archived = super::archive::archived_changes(db, node, &unapplied)?;
    if !archived.is_empty() {
        let Some(tx) = tx else {
            return Err(crate::issues::Error::new(
                "archive_retry",
                "Issue storage changed before fleet arbitration; retry after restoring history",
            )
            .into());
        };
        drop(tx);
        for (project, number) in archived {
            crate::issues::archive::restore_issue(
                db,
                &project,
                number,
                crate::issues::worker::now(),
            )?;
        }
        return accept_changes(db, node, changes);
    }
    let mut results = vec![];
    let mut subtask_keys = vec![];
    for change in changes {
        let previous = if let Some(saved) = &saved {
            saved.get(&change["seq"].as_i64().unwrap()).cloned()
        } else {
            rows(
                db,
                "SELECT result FROM fleet_receipts WHERE node=? AND seq=?",
                &[json!(node), change["seq"].clone()],
            )?
            .first()
            .map(|row| row["result"].as_str().unwrap().to_owned())
        };
        let mut receipt = if let Some(previous) = previous {
            serde_json::from_str(&previous)?
        } else {
            db.execute_batch("SAVEPOINT incoming")?;
            let result = match apply_change(&mut writer, node, change) {
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
            let encoded = result.to_string();
            execute(
                db,
                "INSERT INTO fleet_receipts VALUES(?,?,?)",
                &[json!(node), change["seq"].clone(), json!(encoded)],
            )?;
            if let Some(saved) = &mut saved {
                saved.insert(change["seq"].as_i64().unwrap(), encoded);
            }
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
            subtask_keys.push(json!([
                results.len(),
                row["project_id"],
                row["child_number"]
            ]));
            receipt["canonical_subtask"] = json!({"project_id":row["project_id"],"child_number":row["child_number"],"row":null});
        }
        receipt["seq"] = change["seq"].clone();
        results.push(receipt);
    }
    writer.reconcile()?;
    // Read the final graph once, including earlier receipts for the same child.
    // Joining on the original key values retains SQLite's affinity semantics.
    if !subtask_keys.is_empty() {
        for mut row in rows(
            db,
            "SELECT json_extract(requested.value,'$[0]') AS receipt_index,s.* FROM json_each(?1) requested CROSS JOIN issue_subtasks s WHERE s.project_id=json_extract(requested.value,'$[1]') AND s.child_number=json_extract(requested.value,'$[2]')",
            &[json!(serde_json::to_string(&subtask_keys)?)],
        )? {
            let index = row
                .as_object_mut()
                .unwrap()
                .remove("receipt_index")
                .unwrap()
                .as_u64()
                .unwrap() as usize;
            results[index]["canonical_subtask"]["row"] = row;
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
    mut row: Value,
    identities: &BTreeMap<(String, i64), (String, i64)>,
) -> Result<Value> {
    let local_id = row["id"].as_i64().unwrap();
    let (origin, id) = identities
        .get(&(table.into(), local_id))
        .map(|(origin, id)| (origin.as_str(), *id))
        .unwrap_or((own, local_id));
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
    let mut result = json!({"origin":origin});
    result["row"] = row;
    Ok(result)
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
    payload["chief_ownership"] = serde_json::to_value(crate::chief_ownership::read(db)?)?;
    payload["allocations"] = Value::Array(rows(db, "SELECT * FROM fleet_allocations", &[])?);
    payload["allocation_deadlines"] =
        Value::Array(rows(db, "SELECT * FROM fleet_allocation_deadlines", &[])?);
    payload["ranges"] = Value::Array(rows(
        db,
        "SELECT project_id,first_number,last_number FROM fleet_ranges WHERE node=?",
        &[json!(node)],
    )?);
    Ok(payload)
}
fn journal_cutoff(db: &Connection) -> Result<Option<i64>> {
    // Aggregate the expression index on the owner instead of transferring up
    // to 10,001 size records on every maintenance pass. No issue bodies load.
    let (count, bytes, oldest): (i64, i64, Option<i64>) = db.query_row(
        "SELECT count(*),coalesce(sum(bytes),0),min(seq) FROM (
            SELECT seq,coalesce(length(CAST(before_json AS BLOB)),0)+coalesce(length(CAST(after_json AS BLOB)),0) AS bytes
            FROM fleet_outbox INDEXED BY fleet_outbox_retention ORDER BY seq DESC LIMIT 10001
        )", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    )?;
    let cutoff = if bytes > 64 * 1024 * 1024 {
        // Oversized history needs the exact newest prefix that fits the byte
        // budget; the common row-budget case already has its boundary above.
        db.query_row(
            "SELECT max(seq) FROM (
                SELECT seq,
                    sum(coalesce(length(CAST(before_json AS BLOB)),0)+coalesce(length(CAST(after_json AS BLOB)),0)) OVER (ORDER BY seq DESC ROWS UNBOUNDED PRECEDING) AS bytes,
                    row_number() OVER (ORDER BY seq DESC) AS count
                FROM fleet_outbox INDEXED BY fleet_outbox_retention ORDER BY seq DESC LIMIT 10001
            ) WHERE bytes>67108864 OR count>10000", [], |r| r.get(0),
        )?
    } else if count > 10_000 {
        oldest
    } else {
        None
    };
    let Some(seq) = cutoff else {
        return Ok(None);
    };
    Ok(db.query_row(
        "SELECT max(seq) FROM (SELECT seq FROM fleet_outbox WHERE seq<=?1 ORDER BY seq LIMIT 1000)",
        [seq],
        |r| r.get(0),
    )?)
}

pub(super) fn prune_journal(db: &Connection) -> Result<usize> {
    // Companions need every unacknowledged mutation; only the canonical history
    // can be recovered by a snapshot. Keep up to 10,000 changes / 64 MiB,
    // deleting at most 1,000 per pass so cleanup does not stall ordinary writers.
    let role: String = db.query_row("SELECT role FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    if role != "controller" {
        return Ok(0);
    }
    // An ordinary maintenance pass must remain a WAL reader when there is no
    // excess history. Recheck under the writer lock before deleting anything.
    if journal_cutoff(db)?.is_none() {
        return Ok(0);
    }
    let tx = if db.is_autocommit() {
        Some(crate::database::Transaction::new_unchecked(
            db,
            rusqlite::TransactionBehavior::Immediate,
        )?)
    } else {
        None
    };
    let deleted = if let Some(cutoff) = journal_cutoff(db)? {
        let deleted = db.execute("DELETE FROM fleet_outbox WHERE seq<=?1", [cutoff])?;
        let floor = state_get(db, "journal_floor", json!(0))?
            .as_i64()
            .unwrap_or(0);
        state_set(db, "journal_floor", &json!(floor.max(cutoff)))?;
        deleted
    } else {
        0
    };
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(deleted)
}

fn journal_head(db: &Connection) -> Result<i64> {
    // AUTOINCREMENT's durable high watermark survives deletion of history.
    Ok(db.query_row(
        "SELECT coalesce((SELECT seq FROM sqlite_sequence WHERE name='fleet_outbox'),0)",
        [],
        |r| r.get(0),
    )?)
}

pub(super) fn snapshot(db: &Connection, node: &str) -> Result<Value> {
    let tx = if db.is_autocommit() {
        Some(db.read_transaction()?)
    } else {
        None
    };
    let own: String = db.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    let ids = identities(db)?;
    let mut tables = serde_json::Map::new();
    for (table, _) in TABLES {
        let mut data = rows(db, &super::archive::snapshot_sql(table), &[])?;
        for row in &mut data {
            super::archive::strip_local(table, row);
        }
        if append(table) {
            data = data
                .into_iter()
                .map(|r| canonical_append(&own, table, r, &ids))
                .collect::<Result<_>>()?;
        }
        tables.insert((*table).into(), Value::Array(data));
    }
    // Canonical name choices belong to the supervisor. Keep this additive
    // snapshot metadata out of companion write journals and older protocols.
    for table in ["project_name_keys", "project_name_collisions"] {
        tables.insert(
            table.into(),
            Value::Array(rows(db, &format!("SELECT * FROM {table}"), &[])?),
        );
    }
    let cursor = journal_head(db)?;
    let mut payload = json!({"cursor":cursor});
    payload["tables"] = Value::Object(tables);
    let result = allocation_payload(db, node, payload)?;
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(result)
}
pub(super) fn incremental(db: &Connection, node: &str, cursor: i64) -> Result<Value> {
    let tx = if db.is_autocommit() {
        Some(db.read_transaction()?)
    } else {
        None
    };
    let floor = state_get(db, "journal_floor", json!(0))?
        .as_i64()
        .unwrap_or(0);
    let payload = if cursor < floor || cursor > journal_head(db)? {
        snapshot(db, node)?
    } else {
        incremental_retained(db, node, cursor)?
    };
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(payload)
}

fn incremental_retained(db: &Connection, node: &str, cursor: i64) -> Result<Value> {
    let own: String = db.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    let mut changes = journal(db, cursor)?;
    let mut needed = BTreeSet::new();
    let mut appends = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        let table = change["table_name"].as_str().unwrap();
        if !append(table) || !change["after_json"].is_string() {
            continue;
        }
        let row = row_json(change, "after_json")?;
        needed.insert((
            table.to_owned(),
            row["id"]
                .as_i64()
                .ok_or_else(|| invalid("Missing append identity"))?,
        ));
        if table == "events" {
            let data: Value = serde_json::from_str(row["data"].as_str().unwrap())?;
            if let Some(comment) = data["comment_id"].as_i64() {
                needed.insert(("comments".to_owned(), comment));
            }
        }
        appends.push((index, table.to_owned(), row));
    }
    let mut ids = BTreeMap::new();
    if !needed.is_empty() {
        // Drive indexed lookups from this journal's keys, including missing local
        // identities only once. Multiple origins retain the first recorded row.
        for row in rows(
            db,
            "SELECT r.table_name,r.local_id,r.origin,r.origin_id
             FROM json_each(?1) requested CROSS JOIN fleet_row_ids r
             WHERE r.rowid=(SELECT rowid FROM fleet_row_ids
                 WHERE table_name=json_extract(requested.value,'$[0]')
                   AND local_id=json_extract(requested.value,'$[1]')
                 ORDER BY rowid LIMIT 1)",
            &[json!(serde_json::to_string(&needed)?)],
        )? {
            ids.insert(
                (
                    row["table_name"].as_str().unwrap().to_owned(),
                    row["local_id"].as_i64().unwrap(),
                ),
                (
                    row["origin"].as_str().unwrap().to_owned(),
                    row["origin_id"].as_i64().unwrap(),
                ),
            );
        }
    }
    for (index, table, row) in appends {
        changes[index]["append"] = canonical_append(&own, &table, row, &ids)?;
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
// None protects a whole pending creation/deletion. Updates protect only fields
// present in their saved deltas, retaining the original journal for arbitration.
type Pending = BTreeMap<String, Option<BTreeSet<String>>>;

fn apply_row(
    writer: &mut RowWriter<'_>,
    pending: &Pending,
    table: &str,
    row: &Value,
    origin: &str,
) -> Result<()> {
    let db = writer.db;
    if table == "issue_subtasks" {
        return Ok(());
    }
    if append(table) {
        if pending.contains_key(&pending_key(table, row)?) {
            return Ok(());
        }
        append_row(db, origin, table, row, false)?;
    } else {
        let mut row = row.clone();
        if let Some(fields) = pending.get(&pending_key(table, &row)?) {
            let Some(fields) = fields else {
                return Ok(());
            };
            let local = current_row(db, table, &row)?;
            if local.is_null() {
                return Ok(());
            }
            for field in fields {
                row[field] = local[field].clone();
            }
            // These fields form one valid lifecycle. A pending claim combined
            // with a canonical closure/deletion (or the reverse) violates the
            // issue constraints and prevents the journal from reaching arbitration.
            let lifecycle = ["state", "assignee", "deleted_at", "closed_at", "closed_by"];
            if table == "issues" && lifecycle.iter().any(|field| fields.contains(*field)) {
                for field in lifecycle {
                    row[field] = local[field].clone();
                }
            }
        }
        if table == "projects" {
            let old = current_row(db, table, &row)?;
            if !old.is_null() {
                row["next_number"] = old["next_number"].clone();
            }
        }
        writer.put(table, &row)?;
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
    writer: &mut RowWriter<'_>,
    pending: &Pending,
    payload: &Value,
    acknowledged: BTreeMap<(String, i64), Value>,
) -> Result<()> {
    let db = writer.db;
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
        if !pending.contains_key(&pending_key("issue_subtasks", &key)?) {
            writer.delete("issue_subtasks", &key)?;
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
        if pending.contains_key(&pending_key("issue_subtasks", &key)?) {
            defer(&row)?;
            continue;
        }
        db.execute_batch("SAVEPOINT subtask_pull")?;
        let result = if row.is_null() {
            Ok(())
        } else {
            writer.put("issue_subtasks", &row)
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
    let mut writer = RowWriter::new(db);
    let cursor = payload["cursor"]
        .as_i64()
        .filter(|cursor| *cursor >= 0)
        .ok_or_else(|| invalid("Invalid fleet pull cursor"))?;
    if cursor < state_get(db, "cursor", json!(0))?.as_i64().unwrap_or(0) {
        return Err(invalid(
            "Stale fleet pull: the cursor precedes the last committed synchronization; no changes or receipts were applied",
        ));
    }
    db.execute("UPDATE fleet_meta SET syncing=1 WHERE id=1", [])?;
    if let Some(assignments) = payload.get("chief_ownership") {
        crate::chief_ownership::apply(
            db,
            &serde_json::from_value::<Vec<crate::chief_ownership::Assignment>>(
                assignments.clone(),
            )?,
        )?;
    }
    let mut acknowledged = BTreeMap::new();
    let sequences = receipts
        .iter()
        .map(|receipt| receipt["seq"].as_i64())
        .collect::<Option<Vec<_>>>();
    let mut outgoing = sequences.as_ref().map(|_| BTreeMap::new());
    if let Some(outgoing) = &mut outgoing {
        // Successful acknowledgments only need deletion. Load bodies only for
        // receipts that must retain a conflict or map an append-only identity.
        let needed: BTreeSet<_> = receipts
            .iter()
            .filter(|receipt| {
                receipt["state"] == "conflict" || receipt.get("canonical_append").is_some()
            })
            .map(|receipt| receipt["seq"].as_i64().unwrap())
            .collect();
        if !needed.is_empty() {
            for row in rows(
                db,
                "SELECT o.* FROM json_each(?1) requested CROSS JOIN fleet_outbox o WHERE o.seq=requested.value",
                &[json!(serde_json::to_string(&needed)?)],
            )? {
                outgoing.insert(row["seq"].as_i64().unwrap(), row);
            }
        }
    }
    for receipt in receipts {
        if receipt.get("canonical_subtask").is_some() {
            let row = &receipt["canonical_subtask"];
            acknowledged.insert(graph_key(row)?, row["row"].clone());
        }
        let change = if let Some(outgoing) = &mut outgoing {
            // The first receipt consumes the row even when a later duplicate
            // has different conflict or identity metadata.
            outgoing.remove(&receipt["seq"].as_i64().unwrap())
        } else {
            rows(
                db,
                "SELECT * FROM fleet_outbox WHERE seq=?",
                &[receipt["seq"].clone()],
            )?
            .into_iter()
            .next()
        };
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
        if sequences.is_none() {
            // Preserve SQLite's affinity and duplicate-key behavior for old
            // peers that send non-integer sequence representations.
            execute(
                db,
                "DELETE FROM fleet_outbox WHERE seq=?",
                &[receipt["seq"].clone()],
            )?;
        }
    }
    if let Some(sequences) = sequences
        && !sequences.is_empty()
    {
        execute(
            db,
            "DELETE FROM fleet_outbox WHERE seq IN (SELECT value FROM json_each(?1))",
            &[json!(serde_json::to_string(&sequences)?)],
        )?;
    }
    let mut pending = Pending::new();
    for c in rows(
        db,
        "SELECT table_name,before_json,after_json FROM fleet_outbox",
        &[],
    )? {
        let after = row_json(&c, "after_json")?;
        let before = row_json(&c, "before_json")?;
        let row = if after.is_null() { &before } else { &after };
        let key = pending_key(c["table_name"].as_str().unwrap(), row)?;
        if before.is_null() || after.is_null() {
            pending.insert(key, None);
        } else if let Some(fields) = pending.entry(key).or_insert_with(|| Some(BTreeSet::new())) {
            for (field, value) in after
                .as_object()
                .ok_or_else(|| invalid("Invalid pending row"))?
            {
                if before[field] != *value {
                    fields.insert(field.clone());
                }
            }
        }
    }
    super::archive::validate_pull(db, payload)?;
    // Apply endpoints before edges and history, regardless of JSON object order.
    for (table, _) in TABLES {
        for row in payload["tables"][*table].as_array().into_iter().flatten() {
            if append(table) {
                apply_row(
                    &mut writer,
                    &pending,
                    table,
                    &row["row"],
                    row["origin"]
                        .as_str()
                        .ok_or_else(|| invalid("Missing history origin"))?,
                )?;
            } else {
                apply_row(&mut writer, &pending, table, row, "")?;
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
                    &mut writer,
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
                apply_row(&mut writer, &pending, table, &after, "")?;
            } else {
                let row = row_json(change, "before_json")?;
                if table != "issue_subtasks" && !pending.contains_key(&pending_key(table, &row)?) {
                    writer.delete(table, &row)?;
                }
            }
        }
    }
    apply_graph(&mut writer, &pending, payload, acknowledged)?;
    writer.reconcile()?;
    let allocations = payload["allocations"]
        .as_array()
        .ok_or_else(|| invalid("Missing fleet allocations"))?;
    let mut unchanged_allocations = false;
    if let Some(deadlines) = payload["allocation_deadlines"].as_array() {
        unchanged_allocations = true;
        for (table, incoming) in [
            ("fleet_allocations", allocations),
            ("fleet_allocation_deadlines", deadlines),
        ] {
            let mut current: Vec<_> = rows(db, &format!("SELECT * FROM {table}"), &[])?
                .iter()
                .map(Value::to_string)
                .collect();
            let mut incoming: Vec<_> = incoming.iter().map(Value::to_string).collect();
            current.sort_unstable();
            incoming.sort_unstable();
            if current != incoming {
                unchanged_allocations = false;
                break;
            }
        }
    }
    // Compare full row multisets in this pull's transaction. Missing/partial
    // legacy deadlines and malformed duplicates retain normal refresh/validation.
    if !unchanged_allocations {
        db.execute("DELETE FROM fleet_allocations", [])?;
        for row in allocations {
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
        for row in payload["allocation_deadlines"]
            .as_array()
            .into_iter()
            .flatten()
        {
            execute(
                db,
                "UPDATE fleet_allocation_deadlines SET expires_at=? WHERE project_id=? AND issue_number=?",
                &[
                    row["expires_at"].clone(),
                    row["project_id"].clone(),
                    row["issue_number"].clone(),
                ],
            )?;
        }
    }
    let ranges = payload["ranges"]
        .as_array()
        .ok_or_else(|| invalid("Missing fleet number ranges"))?;
    if !number_ranges_unchanged(db, ranges)? {
        for r in ranges {
            let previous = rows(
                db,
                "SELECT first_number,last_number FROM fleet_number_ranges WHERE project_id=?",
                &[r["project_id"].clone()],
            )?;
            // A pull may have been prepared before an on-demand reservation reply.
            // Never restore the older range or rewind its local allocation cursor.
            if previous
                .first()
                .is_some_and(|p| p["first_number"].as_i64() > r["first_number"].as_i64())
            {
                continue;
            }
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
fn number_ranges_unchanged(db: &Connection, ranges: &[Value]) -> Result<bool> {
    if ranges.is_empty() {
        return Ok(true);
    }
    // Canonical idle pulls need one indexed read. Changed or legacy batches
    // retain the ordered range/cursor repair path in the same transaction.
    if !ranges.iter().all(|r| {
        r["project_id"].is_string()
            && r["first_number"].as_i64().is_some()
            && r["last_number"].as_i64().is_some()
    }) {
        return Ok(false);
    }
    let matching: i64 = db.query_row(
        "SELECT count(*) FROM json_each(?1) requested
         CROSS JOIN fleet_number_ranges r
         LEFT JOIN projects p ON p.id=r.project_id
         WHERE r.project_id=json_extract(requested.value,'$.project_id')
           AND r.first_number=json_extract(requested.value,'$.first_number')
           AND r.last_number=json_extract(requested.value,'$.last_number')
           AND (p.id IS NULL OR p.next_number BETWEEN r.first_number AND r.last_number+1)",
        [serde_json::to_string(ranges)?],
        |r| r.get(0),
    )?;
    Ok(matching as usize == ranges.len())
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
/// The lease bridges worker startup and its manual claim; heartbeats carry the
/// absolute deadline, so retries never extend the same attempt indefinitely.
fn observed_deadlines(workers: &[Value]) -> Vec<(&str, i64, i64)> {
    let mut deadlines = BTreeMap::new();
    for run in workers
        .iter()
        .flat_map(|w| w["runs"].as_array().into_iter().flatten())
    {
        if run["finished_at"].is_null()
            && run["claimed_at"].is_null()
            && let Some(expires) = run["reservation_expires"].as_i64()
            && let (Some(project), Some(number)) =
                (run["project_id"].as_str(), run["number"].as_i64())
        {
            // Preserve the last eligible observation for duplicate run keys.
            deadlines.insert((project, number), expires);
        }
    }
    deadlines
        .into_iter()
        .map(|((project, number), expires)| (project, number, expires))
        .collect()
}

pub(super) fn allocation_deadlines_changed(
    db: &Connection,
    node: &str,
    workers: &[Value],
) -> Result<bool> {
    let deadlines = observed_deadlines(workers);
    if deadlines.is_empty() {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM json_each(?1) requested CROSS JOIN fleet_allocations a
         CROSS JOIN fleet_allocation_deadlines d
         WHERE a.project_id=json_extract(requested.value,'$[0]')
           AND a.issue_number=json_extract(requested.value,'$[1]') AND a.node=?2
           AND d.project_id=a.project_id AND d.issue_number=a.issue_number
           AND d.expires_at<>json_extract(requested.value,'$[2]'))",
        rusqlite::params![serde_json::to_string(&deadlines)?, node],
        |row| row.get(0),
    )?)
}

pub(super) fn refresh_allocation_deadlines(
    db: &Connection,
    node: &str,
    workers: &[Value],
) -> Result<()> {
    let deadlines = observed_deadlines(workers);
    if !deadlines.is_empty() {
        execute(
            db,
            "UPDATE fleet_allocation_deadlines AS d
             SET expires_at=json_extract(requested.value,'$[2]')
             FROM json_each(?1) requested CROSS JOIN fleet_allocations a
             WHERE a.project_id=json_extract(requested.value,'$[0]')
               AND a.issue_number=json_extract(requested.value,'$[1]') AND a.node=?2
               AND d.project_id=a.project_id AND d.issue_number=a.issue_number
               AND d.expires_at<>json_extract(requested.value,'$[2]')",
            &[json!(serde_json::to_string(&deadlines)?), json!(node)],
        )?;
    }
    Ok(())
}

/// Reserve independently of worker pools. Historical blocks stay reserved so
/// delayed offline creations remain valid after this machine receives a new one.
pub(super) fn reserve_numbers(
    db: &mut Connection,
    node: &str,
    project: &str,
    next: i64,
) -> Result<Value> {
    let tx = db.transaction_with_behavior(crate::database::TransactionBehavior::Immediate)?;
    let db = &*tx;
    let latest: i64 = db.query_row(
        "SELECT coalesce(max(number),0) FROM issues WHERE project_id=?1",
        [project],
        |r| r.get(0),
    )?;
    let ranges = rows(
        db,
        "SELECT project_id,first_number,last_number FROM fleet_ranges WHERE node=? AND project_id=?",
        &[json!(node), json!(project)],
    )?;
    let range = if let Some(range) = ranges.first().filter(|r| {
        next.max(r["first_number"].as_i64().unwrap()) > latest
            && next <= r["last_number"].as_i64().unwrap()
    }) {
        range.clone()
    } else {
        let cursor: i64 = db.query_row(
            "SELECT next_number FROM projects WHERE id=?1",
            [project],
            |r| r.get(0),
        )?;
        let first = cursor.max(
            latest
                .checked_add(1)
                .ok_or_else(|| invalid("Issue numbers exhausted"))?,
        );
        let after = first
            .checked_add(100)
            .ok_or_else(|| invalid("Issue numbers exhausted"))?;
        db.execute(
            "UPDATE projects SET next_number=?2 WHERE id=?1",
            rusqlite::params![project, after],
        )?;
        let values = [json!(node), json!(project), json!(first), json!(after - 1)];
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
        json!({"project_id":project,"first_number":first,"last_number":after-1})
    };
    tx.commit()?;
    Ok(range)
}

#[derive(Default)]
struct AllocationPlan {
    ranges: Vec<(String, i64)>,
    tasks: Vec<(String, i64)>,
}

fn plan_allocations(db: &Connection, node: &str, workers: &[Value]) -> Result<AllocationPlan> {
    let mut plan = AllocationPlan::default();
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
    if pools.is_empty() {
        return Ok(plan);
    }
    // Each heartbeat plans all selected projects together. Keep the indexed
    // range lookups in SQLite instead of requesting three rows per project.
    let projects = rows(
        db,
        "SELECT p.id,p.next_number,r.last_number,
         coalesce((SELECT max(number) FROM issues WHERE project_id=p.id AND number BETWEEN r.first_number AND r.last_number),0) AS used
         FROM json_each(?1) selected CROSS JOIN projects p
         LEFT JOIN fleet_ranges r ON r.node=?2 AND r.project_id=p.id
         WHERE p.id=selected.value AND p.hidden_at IS NULL ORDER BY p.id",
        &[json!(serde_json::to_string(&pools.keys().collect::<Vec<_>>())?), json!(node)],
    )?;
    if projects.is_empty() {
        return Ok(plan);
    }
    let keys: Vec<_> = projects.iter().map(|p| p["id"].as_str().unwrap()).collect();
    let mut supplied_by_project = BTreeMap::<String, Vec<Value>>::new();
    for supplied in rows(
        db,
        &format!(
            "SELECT a.project_id,i.labels FROM json_each(?1) selected
            CROSS JOIN fleet_allocations a CROSS JOIN issues i
            WHERE a.project_id=selected.value AND a.node=?2
              AND i.project_id=a.project_id AND i.number=a.issue_number
              AND i.state='open' AND i.deleted_at IS NULL AND {ALLOCATED}"
        ),
        &[json!(serde_json::to_string(&keys)?), json!(node)],
    )? {
        supplied_by_project
            .entry(supplied["project_id"].as_str().unwrap().to_owned())
            .or_default()
            .push(supplied);
    }
    let mut pending = Vec::new();
    for row in projects {
        let project = row["id"].as_str().unwrap().to_owned();
        let configs = &pools[&project];
        let replenish = row["last_number"]
            .as_i64()
            .is_none_or(|last| row["used"].as_i64().unwrap() > last - 20);
        if replenish {
            plan.ranges
                .push((project.clone(), row["next_number"].as_i64().unwrap()));
        }
        // Start from allocation keys; readiness checks must inspect this
        // machine's small supplied pool rather than every project issue.
        let supplied = supplied_by_project.remove(&project).unwrap_or_default();
        let mut filters = BTreeMap::<BTreeSet<String>, i64>::new();
        for c in configs {
            *filters.entry(tags(c)).or_default() += c["concurrency"].as_i64().unwrap_or(1);
        }
        // A filled pool needs no queue scan. Check both each filter and the
        // union: one allocation can match multiple filters, but must not count
        // as multiple slots in the machine's total capacity.
        if filters.iter().all(|(filter, capacity)| {
            supplied
                .iter()
                .filter(|r| matches(&r["labels"], filter).unwrap_or(false))
                .count() as i64
                >= capacity * 2
        }) && supplied
            .iter()
            .filter(|r| {
                filters
                    .keys()
                    .any(|f| matches(&r["labels"], f).unwrap_or(false))
            })
            .count() as i64
            >= filters.values().sum::<i64>() * 2
        {
            continue;
        }
        pending.push((project, filters, supplied));
    }
    if pending.is_empty() {
        return Ok(plan);
    }
    let keys: Vec<_> = pending.iter().map(|(project, _, _)| project).collect();
    let mut candidates_by_project = BTreeMap::<String, Vec<Value>>::new();
    for candidate in rows(
        db,
        &format!(
            "SELECT i.project_id,i.number,i.labels FROM json_each(?1) selected CROSS JOIN issues i
             WHERE i.project_id=selected.value AND i.state='open' AND i.deleted_at IS NULL AND i.assignee IS NULL
               AND (assignment_target IS NULL OR assignment_target NOT LIKE 'machine:%' OR assignment_target='machine:'||?2)
               AND {READY} AND NOT EXISTS(SELECT 1 FROM fleet_allocations a WHERE a.project_id=i.project_id AND a.issue_number=i.number)
               AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL)
             ORDER BY i.project_id,sort_order,number"
        ),
        &[json!(serde_json::to_string(&keys)?), json!(node)],
    )? {
        candidates_by_project.entry(candidate["project_id"].as_str().unwrap().to_owned()).or_default().push(candidate);
    }
    for (project, filters, mut supplied) in pending {
        let candidates = candidates_by_project.remove(&project).unwrap_or_default();
        let mut used = BTreeSet::new();
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
                    plan.tasks.push((project.clone(), number));
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
                plan.tasks.push((project.clone(), number));
                used.insert(number);
                needed -= 1;
            }
        }
    }
    Ok(plan)
}

/// A preflight only decides whether to acquire the writer. Planning is repeated
/// under that lock before applying any ranges or task reservations.
pub(super) fn allocation_pending(db: &Connection, node: &str, workers: &[Value]) -> Result<bool> {
    if allocation_deadlines_changed(db, node, workers)?
        || db.query_row(
            "SELECT EXISTS(SELECT 1 FROM fleet_allocation_deadlines d
             JOIN issues i ON i.project_id=d.project_id AND i.number=d.issue_number
             WHERE d.expires_at<=?1 AND i.assignee IS NULL)",
            [(super::context::now() * 1000.0) as i64],
            |row| row.get::<_, bool>(0),
        )?
    {
        return Ok(true);
    }
    let plan = plan_allocations(db, node, workers)?;
    Ok(!plan.ranges.is_empty() || !plan.tasks.is_empty())
}

pub(super) fn allocate(db: &Connection, node: &str, workers: &[Value]) -> Result<()> {
    let tx = if db.is_autocommit() {
        let snapshot = db.read_transaction()?;
        let pending = allocation_pending(&snapshot, node, workers)?;
        snapshot.commit()?;
        if !pending {
            return Ok(());
        }
        Some(crate::database::Transaction::new_unchecked(
            db,
            rusqlite::TransactionBehavior::Immediate,
        )?)
    } else {
        None
    };
    refresh_allocation_deadlines(db, node, workers)?;
    execute(
        db,
        "DELETE FROM fleet_allocations WHERE (project_id,issue_number) IN (SELECT d.project_id,d.issue_number FROM fleet_allocation_deadlines d JOIN issues i ON i.project_id=d.project_id AND i.number=d.issue_number WHERE d.expires_at<=? AND i.assignee IS NULL)",
        &[json!((super::context::now() * 1000.0) as i64)],
    )?;
    let plan = plan_allocations(db, node, workers)?;
    for (project, first) in plan.ranges {
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
    for (project, number) in plan.tasks {
        execute(
            db,
            "INSERT INTO fleet_allocations VALUES(?,?,?)",
            &[json!(project), json!(number), json!(node)],
        )?;
    }
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn journal_reads_only_payloads_near_its_existing_byte_budget() {
        let f = Fixture::new();
        f.capture();
        // Overflow reads from the main file bypass SQLite's cache counters.
        // Keep these payloads in WAL so the measured page work includes them.
        f.db.execute_batch("PRAGMA wal_autocheckpoint=0").unwrap();
        let payload = json!({"body":"\"🌍\\\n".repeat(32768)}).to_string();
        let add = |count| {
            f.db.execute("WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<?1) INSERT INTO fleet_outbox(table_name,after_json,created_at) SELECT 'issues',?2,0 FROM n", rusqlite::params![count, payload]).unwrap();
        };
        let read = || {
            f.db.execute_batch("PRAGMA cache_size=-64; PRAGMA shrink_memory")
                .unwrap();
            let mut pages = 0;
            let mut high = 0;
            unsafe {
                assert_eq!(
                    rusqlite::ffi::sqlite3_db_status(
                        f.db.handle(),
                        rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                        &mut pages,
                        &mut high,
                        1
                    ),
                    rusqlite::ffi::SQLITE_OK
                );
            }
            let batch = journal(&f.db, 0).unwrap();
            unsafe {
                assert_eq!(
                    rusqlite::ffi::sqlite3_db_status(
                        f.db.handle(),
                        rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                        &mut pages,
                        &mut high,
                        0
                    ),
                    rusqlite::ffi::SQLITE_OK
                );
            }
            (batch, pages)
        };
        add(24);
        let (expected, small) = read();
        assert!(!expected.is_empty());
        assert!(expected.len() < 24);
        add(72);
        let (actual, large) = read();
        assert_eq!(actual, expected);
        let cursor = actual.last().unwrap()["seq"].as_i64().unwrap();
        let next = journal(&f.db, cursor).unwrap();
        assert_eq!(next[0]["seq"], cursor + 1);
        eprintln!(
            "Journal page reads with 24/96 pending large records: {small}/{large}; returned {} records",
            actual.len()
        );
        assert!(
            large <= small + 32,
            "Journal loaded payloads beyond its byte budget: {small} -> {large}"
        );
    }

    #[test]
    fn journal_hydration_preserves_batch_boundaries_and_bootstrap_markers() {
        for payloads in [
            vec![json!({"small":true}).to_string(); 305],
            vec![
                json!({"body":"x".repeat(crate::issues::WIRE_LIMIT / 3)}).to_string(),
                json!({"body":"\"🌍\\\n".repeat(131072)}).to_string(),
                json!({"body":"\"🌍\\\n".repeat(131072)}).to_string(),
                json!({"body":"\"🌍\\\n".repeat(131072)}).to_string(),
                "{}".into(),
            ],
        ] {
            let f = Fixture::new();
            f.capture();
            for payload in &payloads {
                f.db.execute("INSERT INTO fleet_outbox(table_name,before_json,after_json,created_at) VALUES('issues','{}',?1,0)", [payload]).unwrap();
            }
            let all = rows(&f.db, "SELECT * FROM fleet_outbox ORDER BY seq", &[]).unwrap();
            let bootstrap = all[1]["seq"].as_i64().unwrap();
            state_set(&f.db, "bootstrap_last_seq", &json!(bootstrap)).unwrap();
            let mut cursor = 0;
            let mut seen = 0;
            loop {
                let mut expected = Vec::new();
                let mut size = 0;
                for mut row in all
                    .iter()
                    .filter(|row| row["seq"].as_i64().unwrap() > cursor)
                    .take(300)
                    .cloned()
                {
                    if row["seq"].as_i64().unwrap() <= bootstrap {
                        row["bootstrap"] = json!(true);
                    }
                    size += row.to_string().len();
                    if !expected.is_empty() && size > crate::issues::WIRE_LIMIT / 3 {
                        break;
                    }
                    expected.push(row);
                }
                let actual = journal(&f.db, cursor).unwrap();
                assert_eq!(actual, expected);
                if actual.is_empty() {
                    break;
                }
                if seen == 0 {
                    assert_eq!(actual.len(), if payloads.len() == 305 { 300 } else { 1 });
                }
                seen += actual.len();
                cursor = actual.last().unwrap()["seq"].as_i64().unwrap();
            }
            assert_eq!(seen, payloads.len());
        }
    }

    #[test]
    fn missing_metadata_tables_are_repaired_without_changing_saved_state() {
        let db = Connection::open_in_memory().unwrap();
        ensure_metadata(&db).unwrap();
        state_set(&db, "saved", &json!({"keep":true})).unwrap();
        db.execute_batch("DROP TABLE fleet_ranges; DROP TABLE fleet_signals;")
            .unwrap();
        ensure_metadata(&db).unwrap();
        assert_eq!(
            state_get(&db, "saved", Value::Null).unwrap(),
            json!({"keep":true})
        );
        db.execute(
            "INSERT INTO fleet_ranges VALUES('node','project',1,100)",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO fleet_signals(id,host,worker,signal,state,created_at) VALUES('signal','host','worker','stop','pending',1)",[]).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM fleet_number_reservations", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }
    #[test]
    fn unchanged_fleet_state_does_not_queue_behind_a_writer() {
        let f = Fixture::new();
        ensure_metadata(&f.db).unwrap();
        let value = json!({"build":"same","workers":[1,2,3]});
        state_set(&f.db, "desired", &value).unwrap();
        state_set(&f.db, "null", &Value::Null).unwrap();
        f.db.execute_batch("CREATE TABLE state_writes(kind TEXT);
            CREATE TRIGGER state_insert_audit AFTER INSERT ON fleet_state BEGIN INSERT INTO state_writes VALUES('insert'); END;
            CREATE TRIGGER state_update_audit AFTER UPDATE ON fleet_state BEGIN INSERT INTO state_writes VALUES('update'); END;").unwrap();
        let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
        let writer = Connection::connect(&f.path).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let reader = Connection::connect(&f.path).unwrap();
        let repeated = value.clone();
        let (done, completed) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = state_set(&reader, "desired", &repeated);
            done.send(result.is_ok()).unwrap();
            result.unwrap();
        });
        let finished_while_locked = completed
            .recv_timeout(std::time::Duration::from_secs(2))
            .ok();
        writer.execute_batch("COMMIT").unwrap();
        worker.join().unwrap();
        drop(writer);
        let (db, transport) = crate::database::tests::measured_connection(&f.path);
        for _ in 0..128 {
            state_set(&db, "desired", &value).unwrap();
            state_set(&db, "null", &Value::Null).unwrap();
        }
        drop(db);
        let (commands, steps) = transport.join().unwrap();
        owner.stop();
        let writes: i64 =
            f.db.query_row("SELECT count(*) FROM state_writes", [], |r| r.get(0))
                .unwrap();
        eprintln!(
            "Unchanged fleet state: {commands} RPCs, {steps} query steps, {writes} writes, completed while writer held={finished_while_locked:?}"
        );
        // Changed values and missing keys still save, including JSON null.
        state_set(&f.db, "new", &Value::Null).unwrap();
        state_set(&f.db, "desired", &json!({"build":"new"})).unwrap();
        assert_eq!(
            state_get(&f.db, "desired", Value::Null).unwrap(),
            json!({"build":"new"})
        );
        assert_eq!(
            f.db.query_row("SELECT value FROM fleet_state WHERE key='new'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "null"
        );
        f.db.execute_batch("BEGIN IMMEDIATE").unwrap();
        state_set(&f.db, "desired", &json!("temporary")).unwrap();
        state_set(&f.db, "desired", &json!("temporary")).unwrap();
        f.db.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            state_get(&f.db, "desired", Value::Null).unwrap(),
            json!({"build":"new"})
        );
        assert_eq!(
            finished_while_locked,
            Some(true),
            "Unchanged state waited for the writer"
        );
        assert_eq!(writes, 0, "Unchanged state caused writes");
    }

    #[test]
    fn on_demand_numbers_refresh_stale_and_exhausted_ranges_without_workers() {
        let mut f = Fixture::new();
        f.capture();
        let first = reserve_numbers(&mut f.db, "agent", "named:Native fleet", 1).unwrap();
        assert_eq!(first["first_number"], 2);
        assert_eq!(
            reserve_numbers(&mut f.db, "agent", "named:Native fleet", 3).unwrap(),
            first
        );
        f.db.execute_batch("UPDATE projects SET next_number=1100; INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) VALUES('named:Native fleet',1099,'Recent','','open','human:fixture',0,0,1,'[]',2);").unwrap();
        let fresh = reserve_numbers(&mut f.db, "agent", "named:Native fleet", 3).unwrap();
        assert_eq!(fresh["first_number"], 1100);
        let other = reserve_numbers(&mut f.db, "other", "named:Native fleet", 1).unwrap();
        assert_eq!(other["first_number"], 1200);
        let exhausted = reserve_numbers(&mut f.db, "agent", "named:Native fleet", 1200).unwrap();
        assert_eq!(exhausted["first_number"], 1300);
        assert_eq!(
            rows(
                &f.db,
                "SELECT count(*) count FROM fleet_number_reservations",
                &[]
            )
            .unwrap()[0]["count"],
            4
        );
    }

    #[test]
    fn unchanged_number_ranges_do_not_scale_owner_calls_or_rewrite_rows() {
        let mut measurements = Vec::new();
        for count in [16, 128] {
            let f = Fixture::new();
            install_capture(&f.db, "agent", "peer").unwrap();
            f.db.execute_batch(&format!(
                "WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<{count})
                 INSERT INTO projects(id,name,next_number) SELECT 'range-'||id,'Range '||id,10 FROM n;
                 INSERT INTO fleet_number_ranges SELECT id,10,109 FROM projects WHERE id LIKE 'range-%';
                 WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<8192)
                 INSERT INTO fleet_number_ranges SELECT 'unrelated-'||id,10,109 FROM n;
                 DELETE FROM fleet_outbox;
                 CREATE TABLE range_updates(project_id TEXT);
                 CREATE TRIGGER range_update_audit AFTER UPDATE ON fleet_number_ranges BEGIN INSERT INTO range_updates VALUES(NEW.project_id); END;"
            )).unwrap();
            let ranges: Vec<_> = (1..=count).map(|number| json!({"project_id":format!("range-{number}"),"first_number":10,"last_number":109})).collect();
            let mut payload =
                json!({"cursor":0,"allocations":[],"allocation_deadlines":[],"ranges":ranges});
            let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
            let (db, transport) = crate::database::tests::measured_connection(&f.path);
            apply_pull(&db, "peer", &payload, &[]).unwrap();
            drop(db);
            let (commands, steps) = transport.join().unwrap();
            owner.stop();
            let updates: i64 =
                f.db.query_row("SELECT count(*) FROM range_updates", [], |r| r.get(0))
                    .unwrap();
            eprintln!(
                "{count} unchanged ranges: {commands} RPCs, {steps} query VM steps, {updates} rewrites"
            );
            measurements.push((count, commands, steps, updates));
            // Unchanged range boundaries must still repair an out-of-range cursor.
            f.db.execute("UPDATE projects SET next_number=500 WHERE id='range-1'", [])
                .unwrap();
            apply_pull(&f.db, "peer", &payload, &[]).unwrap();
            let next: i64 =
                f.db.query_row(
                    "SELECT next_number FROM projects WHERE id='range-1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(next, 10);
            // A later range wins even if an older duplicate follows it in the pull.
            let old = payload["ranges"][0].clone();
            payload["ranges"][0] =
                json!({"project_id":"range-1","first_number":110,"last_number":209});
            payload["ranges"].as_array_mut().unwrap().push(old);
            apply_pull(&f.db, "peer", &payload, &[]).unwrap();
            let next: i64 =
                f.db.query_row(
                    "SELECT next_number FROM projects WHERE id='range-1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(next, 110);
        }
        for (count, commands, steps, updates) in measurements {
            assert!(
                commands < 25,
                "{count} unchanged ranges required {commands} RPCs"
            );
            assert_eq!(updates, 0);
            assert!(
                steps < count * 100 + 1000,
                "Unrelated ranges were scanned: {steps} query VM steps"
            );
        }
    }

    #[test]
    fn delayed_pull_cannot_rewind_an_on_demand_number_reservation() {
        let mut main = Fixture::new();
        main.capture();
        reserve_numbers(&mut main.db, "agent", "named:Native fleet", 1).unwrap();
        let old = snapshot(&main.db, "agent").unwrap();
        let peer = Fixture::new();
        install_capture(&peer.db, "agent", "agent").unwrap();
        apply_pull(&peer.db, "agent", &old, &[]).unwrap();
        peer.db.execute_batch("UPDATE fleet_number_ranges SET first_number=1100,last_number=1199; UPDATE projects SET next_number=1102;").unwrap();
        apply_pull(&peer.db, "agent", &old, &[]).unwrap();
        assert_eq!(
            rows(
                &peer.db,
                "SELECT first_number FROM fleet_number_ranges",
                &[]
            )
            .unwrap()[0]["first_number"],
            1100
        );
        assert_eq!(
            rows(&peer.db, "SELECT next_number FROM projects", &[]).unwrap()[0]["next_number"],
            1102
        );
    }
    use super::*;
    use crate::issues::{Actor, Operation, Project, Request, Store};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn github_fetch_activity_replicates_but_companions_cannot_overwrite_it() {
        let main = Fixture::new();
        main.capture();
        main.db.execute("INSERT INTO github_fetch_status(url,finished_at) VALUES('https://github.com/o/r/pull/1',100)", []).unwrap();
        let peer = Fixture::new();
        install_capture(&peer.db, "agent", "peer").unwrap();
        apply_pull(&peer.db, "peer", &snapshot(&main.db, "peer").unwrap(), &[]).unwrap();
        let key = json!({"url":"https://github.com/o/r/pull/1"});
        let before = current_row(&peer.db, "github_fetch_status", &key).unwrap();
        assert_eq!(before["finished_at"], 100);
        let mut after = before.clone();
        after["requested_at"] = json!(200);
        let change = json!({"seq":901,"table_name":"github_fetch_status","before_json":before.to_string(),"after_json":after.to_string()});
        assert_eq!(
            accept_changes(&main.db, "peer", &[change]).unwrap()[0]["state"],
            "conflict"
        );
        assert_eq!(
            current_row(&main.db, "github_fetch_status", &key).unwrap(),
            before
        );
    }

    #[test]
    fn github_watch_status_flows_from_supervisor_and_rejects_companion_edits() {
        let main = Fixture::new();
        main.capture();
        let key = json!({"project_id":"named:Native fleet","issue_number":1});
        let status = json!({"prs":{},"event":"new-failure"});
        execute(
            &main.db,
            "INSERT INTO issue_github_watches VALUES(?,?,?)",
            &[
                key["project_id"].clone(),
                json!(1),
                json!(status.to_string()),
            ],
        )
        .unwrap();
        let peer = Fixture::new();
        install_capture(&peer.db, "agent", "peer").unwrap();
        apply_pull(&peer.db, "peer", &snapshot(&main.db, "peer").unwrap(), &[]).unwrap();
        assert_eq!(
            current_row(&peer.db, "issue_github_watches", &key).unwrap()["status"],
            status.to_string()
        );
        let before = current_row(&peer.db, "issue_github_watches", &key).unwrap();
        let mut after = before.clone();
        after["status"] = json!("{\"prs\":{},\"event\":\"stale\"}");
        for (seq, before, after) in [
            (901, Value::Null, after.clone()),
            (902, before.clone(), after),
            (903, before, Value::Null),
        ] {
            let change = json!({"seq":seq,"table_name":"issue_github_watches","before_json":if before.is_null(){Value::Null}else{json!(before.to_string())},"after_json":if after.is_null(){Value::Null}else{json!(after.to_string())}});
            let receipt = accept_changes(&main.db, "peer", &[change]).unwrap();
            assert_eq!(receipt[0]["state"], "conflict");
            assert_eq!(
                current_row(&main.db, "issue_github_watches", &key).unwrap()["status"],
                status.to_string()
            );
        }
    }

    #[test]
    fn github_watcher_remote_handoff_cannot_hide_a_newer_supervisor_event() {
        for (handled, ready) in [(false, false), (false, true), (true, false), (true, true)] {
            let main = Fixture::new();
            main.capture();
            main.db.execute_batch("INSERT INTO agents(id,metadata,last_seen) VALUES('watcher:github','{}',0); UPDATE issues SET assignment_target='github',assignee='human:fixture'; INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'peer');").unwrap();
            let key = json!({"project_id":"named:Native fleet","number":1});
            let before = current_row(&main.db, "issues", &key).unwrap();
            let mut after = before.clone();
            after["assignee"] = json!("watcher:github");
            after["github_ack_event"] = json!(if handled { "second" } else { "first" });
            if ready {
                after["state"] = json!("ready");
            }
            main.db
                .execute(
                    "INSERT INTO issue_github_watches VALUES('named:Native fleet',1,?1)",
                    [json!({"prs":{},"event":"second"}).to_string()],
                )
                .unwrap();
            let change = json!({"seq":911,"table_name":"issues","before_json":before.to_string(),"after_json":after.to_string()});
            let receipts = accept_changes(&main.db, "peer", &[change]).unwrap();
            assert_eq!(receipts[0]["state"], "applied", "{receipts:?}");
            let issue = current_row(&main.db, "issues", &key).unwrap();
            assert_eq!(
                issue["state"],
                if ready && handled { "ready" } else { "open" },
                "Pending watcher work must remain eligible for pickup"
            );
            assert_eq!(
                issue["assignee"],
                if handled {
                    json!("watcher:github")
                } else {
                    Value::Null
                }
            );
            assert_eq!(
                rows(&main.db, "SELECT * FROM fleet_allocations", &[])
                    .unwrap()
                    .is_empty(),
                handled,
                "A parked watcher must release its worker allocation"
            );
        }
    }

    #[test]
    fn attempt_holds_replicate_without_allocation_and_reject_stale_or_foreign_release() {
        let main = Fixture::new();
        main.capture();
        let key = json!({"project_id":"named:Native fleet","number":1});
        let before = current_row(&main.db, "issues", &key).unwrap();
        let mut after = before.clone();
        after["attempt_hold"] = json!(json!({"machine":"peer","owner":"codex:original","attempt_id":"validation-1","pid":62791,"process_start":"saved-start","worktree":"/tmp/retained","log_path":"/tmp/retained.log"}).to_string());
        after["version"] = json!(before["version"].as_i64().unwrap() + 1);
        let change = |seq: i64, before: &Value, after: &Value| json!({"seq":seq,"table_name":"issues","before_json":before.to_string(),"after_json":after.to_string()});
        assert_eq!(
            accept_changes(&main.db, "peer", &[change(901, &before, &after)]).unwrap()[0]["state"],
            "applied"
        );
        let held = current_row(&main.db, "issues", &key).unwrap();
        assert_eq!(held["attempt_hold"], after["attempt_hold"]);
        assert!(
            rows(
                &main.db,
                "SELECT * FROM issue_pickup_ready WHERE number=1",
                &[]
            )
            .unwrap()
            .is_empty()
        );
        let mut cleared = held.clone();
        cleared["attempt_hold"] = Value::Null;
        assert_eq!(
            accept_changes(&main.db, "other", &[change(902, &held, &cleared)]).unwrap()[0]["state"],
            "conflict"
        );
        main.db
            .execute("UPDATE issues SET version=version+1 WHERE number=1", [])
            .unwrap();
        assert_eq!(
            accept_changes(&main.db, "peer", &[change(903, &held, &cleared)]).unwrap()[0]["state"],
            "conflict"
        );
        let current = current_row(&main.db, "issues", &key).unwrap();
        let mut old_peer = current.clone();
        old_peer.as_object_mut().unwrap().remove("attempt_hold");
        put_row(&main.db, "issues", &old_peer).unwrap();
        assert_eq!(
            current_row(&main.db, "issues", &key).unwrap()["attempt_hold"],
            held["attempt_hold"]
        );
        cleared = current.clone();
        cleared["attempt_hold"] = Value::Null;
        assert_eq!(
            accept_changes(&main.db, "peer", &[change(904, &current, &cleared)]).unwrap()[0]["state"],
            "applied"
        );
    }

    #[test]
    fn offline_content_edits_preserve_newer_state_and_ownership() {
        let main = Fixture::new();
        main.db
            .execute(
                "INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'agent')",
                [],
            )
            .unwrap();
        let key = json!({"project_id":"named:Native fleet","number":1});
        let mut before = current_row(&main.db, "issues", &key).unwrap();
        before["state"] = json!("blocked");
        let mut after = before.clone();
        after["title"] = json!("Offline title");
        main.db
            .execute(
                "UPDATE issues SET assignee='human:fixture' WHERE number=1",
                [],
            )
            .unwrap();
        let change = json!({"seq":1,"table_name":"issues","before_json":before.to_string(),"after_json":after.to_string(),"created_at":0});
        let receipts = accept_changes(&main.db, "agent", &[change]).unwrap();
        assert_eq!(receipts[0]["state"], "applied");
        let current = current_row(&main.db, "issues", &key).unwrap();
        assert_eq!(current["title"], "Offline title");
        assert_eq!(current["state"], "open");
        assert_eq!(current["assignee"], "human:fixture");
    }

    #[test]
    fn stale_subtask_links_cannot_release_canonical_parent_claims() {
        for ancestor in [false, true] {
            let main = Fixture::new();
            main.db.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
                VALUES('named:Native fleet',2,'Child','','open','human:fixture',0,0,1,'[]',2),
                      ('named:Native fleet',3,'Ancestor','','open','human:fixture',0,0,1,'[]',3);").unwrap();
            if ancestor {
                main.db.execute_batch("UPDATE issues SET state='closed' WHERE number=1;
                    INSERT INTO issue_subtasks(project_id,parent_number,child_number,created_at,created_by) VALUES('named:Native fleet',3,1,0,'human:fixture');").unwrap();
            }
            main.db
                .execute(
                    "INSERT INTO fleet_allocations VALUES('named:Native fleet',?1,'agent')",
                    [if ancestor { 3 } else { 1 }],
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
            agent.db.execute_batch("INSERT INTO issue_subtasks(project_id,parent_number,child_number,created_at,created_by) VALUES('named:Native fleet',1,2,0,'human:fixture');").unwrap();
            let claimed = if ancestor { 3 } else { 1 };
            agent
                .db
                .execute(
                    "UPDATE issues SET state='blocked',version=version+1 WHERE number=?1",
                    [claimed],
                )
                .unwrap();
            main.db
                .execute(
                    "UPDATE issues SET assignee='human:fixture' WHERE number=?1",
                    [claimed],
                )
                .unwrap();
            let before = snapshot(&main.db, "agent").unwrap()["tables"].clone();
            let changes = journal(&agent.db, 0).unwrap();
            assert_eq!(changes.len(), 2);
            let receipts = accept_changes(&main.db, "agent", &changes).unwrap();
            assert_eq!(receipts[0]["state"], "conflict");
            assert_eq!(receipts[1]["state"], "conflict");
            assert!(
                receipts[0]["reason"]
                    .as_str()
                    .unwrap()
                    .contains("existing claim")
            );
            assert_eq!(snapshot(&main.db, "agent").unwrap()["tables"], before);
            assert_eq!(
                accept_changes(&main.db, "agent", &changes).unwrap(),
                receipts
            );
            apply_pull(
                &agent.db,
                "agent",
                &snapshot(&main.db, "agent").unwrap(),
                &receipts,
            )
            .unwrap();
            assert!(
                rows(
                    &agent.db,
                    "SELECT * FROM issue_subtasks WHERE child_number=2",
                    &[]
                )
                .unwrap()
                .is_empty()
            );
            assert_eq!(
                current_row(
                    &agent.db,
                    "issues",
                    &json!({"project_id":"named:Native fleet","number":claimed})
                )
                .unwrap()["assignee"],
                "human:fixture"
            );
        }
    }

    fn grow_journal(f: &Fixture, count: i64) {
        let row = current_row(
            &f.db,
            "issues",
            &json!({"project_id":"named:Native fleet","number":1}),
        )
        .unwrap();
        f.db.execute("WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<?1) INSERT INTO fleet_outbox(table_name,after_json,created_at) SELECT 'issues',?2,0 FROM n", rusqlite::params![count,row.to_string()]).unwrap();
    }

    fn journal_count(f: &Fixture) -> i64 {
        f.db.query_row("SELECT count(*) FROM fleet_outbox", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn healthy_journal_probes_aggregate_history_on_the_owner() {
        let mut measurements = Vec::new();
        for count in [100, 10_000] {
            let f = Fixture::new();
            f.capture();
            grow_journal(&f, count);
            let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
            let (db, transport) = crate::database::tests::measured_connection(&f.path);
            let mut elapsed = Vec::new();
            for _ in 0..16 {
                let start = std::time::Instant::now();
                assert_eq!(prune_journal(&db).unwrap(), 0);
                elapsed.push(start.elapsed());
            }
            drop(db);
            let (commands, steps) = transport.join().unwrap();
            owner.stop();
            elapsed.sort();
            eprintln!(
                "16 healthy journal probes, {count} entries: {commands} RPCs, {steps} query VM steps, median {:?}",
                elapsed[8]
            );
            assert_eq!(journal_count(&f), count);
            measurements.push(commands);
        }
        assert!(
            measurements.iter().all(|commands| *commands <= 32),
            "Healthy journal probes fetched history rows instead of a scalar: {measurements:?}"
        );
    }

    #[test]
    fn journal_retention_bounds_utf8_bytes_and_recovers_pruned_cursors() {
        let f = Fixture::new();
        f.capture();
        let original = snapshot(&f.db, "agent").unwrap();
        let mut row = current_row(
            &f.db,
            "issues",
            &json!({"project_id":"named:Native fleet","number":1}),
        )
        .unwrap();
        row["body"] = json!("🌍".repeat(32_768));
        let encoded = row.to_string();
        f.db.execute("WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<650) INSERT INTO fleet_outbox(table_name,after_json,created_at) SELECT 'issues',?1,0 FROM n", [&encoded]).unwrap();
        let head = journal_head(&f.db).unwrap();
        let keep = (64 * 1024 * 1024 / encoded.len()) as i64;
        f.db.execute("UPDATE fleet_meta SET role='agent'", [])
            .unwrap();
        assert_eq!(
            prune_journal(&f.db).unwrap(),
            0,
            "Pending companion changes must survive the canonical byte budget"
        );
        assert_eq!(journal_count(&f), 650);
        f.db.execute("UPDATE fleet_meta SET role='controller'", [])
            .unwrap();
        assert_eq!(
            prune_journal(&f.db).unwrap() as i64,
            650 - keep,
            "Canonical history needs a byte budget even below 10,000 rows"
        );
        assert_eq!(journal_count(&f), keep);
        assert_eq!(prune_journal(&f.db).unwrap(), 0);
        let floor = state_get(&f.db, "journal_floor", Value::Null)
            .unwrap()
            .as_i64()
            .unwrap();
        assert_eq!(floor, head - keep);
        let recovery = incremental(&f.db, "agent", floor - 1).unwrap();
        assert!(recovery["tables"].is_object());
        assert_eq!(recovery["tables"], original["tables"]);
        assert_eq!(recovery["cursor"], head);
        let boundary = incremental(&f.db, "agent", floor).unwrap();
        assert_eq!(boundary["changes"][0]["seq"], floor + 1);
        assert_eq!(
            snapshot(&f.db, "agent").unwrap()["tables"],
            original["tables"]
        );
        assert_eq!(journal_head(&f.db).unwrap(), head);
    }

    #[test]
    #[ignore = "Profiles byte-budget checks on an explicitly supplied private backup"]
    fn profile_journal_byte_budget() {
        let path = PathBuf::from(
            std::env::var_os("HEY_BOSS_JOURNAL_PROFILE_DB")
                .expect("Set HEY_BOSS_JOURNAL_PROFILE_DB to a disposable backup"),
        );
        assert!(path.is_file(), "Existing private backup required");
        drop(Store::open(&path).unwrap());
        let db = Connection::open(&path).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        let original = snapshot(&db, "byte-profile").unwrap();
        let head = journal_head(&db).unwrap();
        let sql = "SELECT seq,coalesce(length(CAST(before_json AS BLOB)),0)+coalesce(length(CAST(after_json AS BLOB)),0) FROM fleet_outbox ORDER BY seq DESC LIMIT 10001";
        let indexed = sql.replace(
            "FROM fleet_outbox",
            "FROM fleet_outbox INDEXED BY fleet_outbox_retention",
        );
        let unindexed = sql.replace("FROM fleet_outbox", "FROM fleet_outbox NOT INDEXED");
        let scan = |sql: &str| {
            let start = std::time::Instant::now();
            let mut statement = db.prepare(sql).unwrap();
            let result = statement
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (result, start.elapsed())
        };
        let mut times = [vec![], vec![]];
        for trial in 0..10 {
            for index in [trial % 2, 1 - trial % 2] {
                let (actual, elapsed) = scan([&indexed, &unindexed][index]);
                assert_eq!(actual, scan([&indexed, &unindexed][1 - index]).0);
                times[index].push(elapsed);
            }
        }
        for values in &mut times {
            values.sort();
        }
        let mut passes = 0;
        let start = std::time::Instant::now();
        while prune_journal(&db).unwrap() > 0 {
            passes += 1;
        }
        let elapsed = start.elapsed();
        let retained: i64 = db.query_row("SELECT coalesce(sum(coalesce(length(CAST(before_json AS BLOB)),0)+coalesce(length(CAST(after_json AS BLOB)),0)),0) FROM fleet_outbox", [], |r| r.get(0)).unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM fleet_outbox", [], |r| r.get(0))
            .unwrap();
        assert!(retained <= 64 * 1024 * 1024 && count <= 10_000);
        assert_eq!(snapshot(&db, "byte-profile").unwrap(), original);
        assert_eq!(journal_head(&db).unwrap(), head);
        assert_eq!(
            db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(
            rows(&db, "PRAGMA foreign_key_check", &[])
                .unwrap()
                .is_empty()
        );
        eprintln!(
            "Journal byte scan: identical rows; indexed median {:?}; body scan median {:?}; {passes} bounded passes in {elapsed:?}; {count} rows / {retained} UTF-8 bytes retained; unchanged canonical snapshot/highwater; full integrity/FK ok",
            times[0][5], times[1][5]
        );
    }

    #[test]
    #[ignore = "Profiles pruning and compaction of an explicitly supplied private backup"]
    fn profile_journal_retention() {
        let path = std::path::PathBuf::from(
            std::env::var_os("HEY_BOSS_RETENTION_PROFILE_DB")
                .expect("Set HEY_BOSS_RETENTION_PROFILE_DB to a disposable backup"),
        );
        drop(Store::open(&path).unwrap());
        let db = Connection::open(&path).unwrap();
        db.pragma_update(None, "foreign_keys", true).unwrap();
        db.pragma_update(None, "synchronous", "FULL").unwrap();
        let before: i64 = db
            .query_row("SELECT count(*) FROM fleet_outbox", [], |r| r.get(0))
            .unwrap();
        let cursor = journal_head(&db).unwrap();
        let started = std::time::Instant::now();
        let mut deleted = 0;
        let mut maximum = std::time::Duration::ZERO;
        loop {
            let pass = std::time::Instant::now();
            let count = prune_journal(&db).unwrap();
            maximum = maximum.max(pass.elapsed());
            deleted += count;
            if count == 0 {
                break;
            }
        }
        let elapsed = started.elapsed();
        let free: i64 = db
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))
            .unwrap();
        let retained: i64 = db
            .query_row(
                "SELECT sum(pgsize) FROM dbstat WHERE name='fleet_outbox'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(journal_head(&db).unwrap(), cursor);
        let snapshot_before = snapshot(&db, "compaction-probe").unwrap();
        let started = std::time::Instant::now();
        db.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        assert!(
            snapshot_before == snapshot(&db, "compaction-probe").unwrap(),
            "Compaction changed the canonical snapshot"
        );
        assert_eq!(
            db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(
            rows(&db, "PRAGMA foreign_key_check", &[])
                .unwrap()
                .is_empty()
        );
        eprintln!(
            "Retention: {before} before; {deleted} deleted in {elapsed:?}; slowest 1000-row pass {maximum:?}; {free} free pages; {retained} outbox bytes retained; vacuum/check {:?}; {} compacted database bytes",
            started.elapsed(),
            std::fs::metadata(&path).unwrap().len()
        );
    }

    #[test]
    fn retained_journal_maintenance_does_not_wait_for_another_writer() {
        let f = Fixture::new();
        f.capture();
        grow_journal(&f, 10_000);
        let writer = Connection::open(&f.path).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        f.db.busy_timeout(std::time::Duration::from_millis(50))
            .unwrap();
        assert_eq!(prune_journal(&f.db).unwrap(), 0);
        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn journal_retention_is_bounded_and_stale_cursors_receive_a_current_snapshot() {
        let f = Fixture::new();
        f.capture();
        grow_journal(&f, 12_080);
        let head = snapshot(&f.db, "agent").unwrap()["cursor"]
            .as_i64()
            .unwrap();
        assert_eq!(prune_journal(&f.db).unwrap(), 1_000);
        assert_eq!(journal_count(&f), 11_080);
        assert_eq!(prune_journal(&f.db).unwrap(), 1_000);
        assert_eq!(prune_journal(&f.db).unwrap(), 80);
        assert_eq!(prune_journal(&f.db).unwrap(), 0);
        assert_eq!(journal_count(&f), 10_000);
        let floor = state_get(&f.db, "journal_floor", Value::Null)
            .unwrap()
            .as_i64()
            .unwrap();
        assert_eq!(floor, head - 10_000);
        assert_eq!(snapshot(&f.db, "agent").unwrap()["cursor"], head);
        for cursor in [0, floor - 1, head + 1] {
            let pull = incremental(&f.db, "agent", cursor).unwrap();
            assert!(
                pull["tables"].is_object(),
                "Cursor {cursor} must trigger recovery"
            );
            assert_eq!(pull["cursor"], head);
            assert_eq!(pull["tables"]["issues"][0]["title"], "Original");
        }
        let boundary = incremental(&f.db, "agent", floor).unwrap();
        assert!(boundary["changes"].is_array());
        assert_eq!(boundary["changes"][0]["seq"], floor + 1);
        assert_eq!(
            incremental(&f.db, "agent", head - 2).unwrap()["changes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            incremental(&f.db, "agent", head).unwrap()["changes"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // Deletion must never make a snapshot cursor go backward.
        f.db.execute("DELETE FROM fleet_outbox", []).unwrap();
        assert_eq!(snapshot(&f.db, "agent").unwrap()["cursor"], head);
    }

    #[test]
    fn pending_issue_lifecycle_stays_valid_until_controller_arbitration() {
        for (local, remote) in [
            (
                "assignee='human:fixture'",
                "state='closed',closed_at=2,closed_by='human:fixture'",
            ),
            (
                "state='closed',closed_at=1,closed_by='human:fixture'",
                "assignee='human:fixture'",
            ),
            ("assignee='human:fixture'", "deleted_at=2"),
            ("deleted_at=1", "assignee='human:fixture'"),
        ] {
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
            agent
                .db
                .execute(&format!("UPDATE issues SET {local}"), [])
                .unwrap();
            let key = json!({"project_id":"named:Native fleet","number":1});
            let before = current_row(&agent.db, "issues", &key).unwrap();
            let pending = journal(&agent.db, 0).unwrap();
            main.db
                .execute(
                    &format!("UPDATE issues SET {remote},title='Online title'"),
                    [],
                )
                .unwrap();
            apply_pull(
                &agent.db,
                "agent",
                &snapshot(&main.db, "agent").unwrap(),
                &[],
            )
            .unwrap();
            let merged = current_row(&agent.db, "issues", &key).unwrap();
            for field in ["state", "assignee", "deleted_at", "closed_at", "closed_by"] {
                assert_eq!(merged[field], before[field], "{local} / {remote}: {field}");
            }
            assert_eq!(merged["title"], "Online title");
            assert_eq!(journal(&agent.db, 0).unwrap(), pending);
            let receipts = accept_changes(&main.db, "agent", &pending).unwrap();
            assert!(receipts.iter().all(|r| r["state"] == "conflict"));
            apply_pull(
                &agent.db,
                "agent",
                &snapshot(&main.db, "agent").unwrap(),
                &receipts,
            )
            .unwrap();
            assert_eq!(
                current_row(&agent.db, "issues", &key).unwrap(),
                current_row(&main.db, "issues", &key).unwrap()
            );
            assert_eq!(journal_count(&agent), 0);
            assert_eq!(
                rows(&agent.db, "SELECT * FROM fleet_conflicts", &[])
                    .unwrap()
                    .len(),
                pending.len()
            );
        }
    }

    #[test]
    fn pulls_merge_canonical_fields_while_preserving_pending_local_field_changes() {
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
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        let key = json!({"project_id":"named:Native fleet","number":1});
        let row = current_row(&agent.db, "issues", &key).unwrap();
        assert_eq!(row["body"], "Offline edit");
        assert_eq!(row["title"], "Online edit");
        agent
            .db
            .execute("UPDATE issues SET body='Offline second edit'", [])
            .unwrap();
        main.db
            .execute(
                "UPDATE issues SET title='Online second edit',body='Conflicting online edit'",
                [],
            )
            .unwrap();
        let cursor = state_get(&agent.db, "cursor", Value::Null)
            .unwrap()
            .as_i64()
            .unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &incremental(&main.db, "agent", cursor).unwrap(),
            &[],
        )
        .unwrap();
        let row = current_row(&agent.db, "issues", &key).unwrap();
        assert_eq!(row["body"], "Offline second edit");
        assert_eq!(row["title"], "Online second edit");
        let pending = journal(&agent.db, 0).unwrap();
        assert_eq!(pending.len(), 2);
        let receipts = accept_changes(&main.db, "agent", &pending).unwrap();
        assert!(receipts.iter().all(|r| r["state"] == "conflict"));
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &receipts,
        )
        .unwrap();
        assert_eq!(
            current_row(&agent.db, "issues", &key).unwrap()["body"],
            "Conflicting online edit"
        );
        assert_eq!(journal_count(&agent), 0);
        assert_eq!(
            rows(&agent.db, "SELECT * FROM fleet_conflicts", &[])
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn retention_recovery_preserves_offline_edits_and_receipts_after_ack_loss() {
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
        grow_journal(&main, 10_080);
        assert!(prune_journal(&main.db).unwrap() > 0);
        let old_cursor = state_get(&agent.db, "cursor", Value::Null)
            .unwrap()
            .as_i64()
            .unwrap();
        let pull = incremental(&main.db, "agent", old_cursor).unwrap();
        assert!(pull["tables"].is_object());
        apply_pull(&agent.db, "agent", &pull, &[]).unwrap();
        let row = current_row(
            &agent.db,
            "issues",
            &json!({"project_id":"named:Native fleet","number":1}),
        )
        .unwrap();
        assert_eq!(row["body"], "Offline edit");
        assert_eq!(row["title"], "Online edit");
        let changes = journal(&agent.db, 0).unwrap();
        assert!(!changes.is_empty());
        let receipts = accept_changes(&main.db, "agent", &changes).unwrap();
        assert!(
            receipts.iter().all(|r| r["state"] == "applied"),
            "{receipts:?}"
        );
        // Lose the acknowledgment, then prune the canonical mutation from history.
        grow_journal(&main, 10_080);
        while prune_journal(&main.db).unwrap() > 0 {}
        assert_eq!(
            accept_changes(&main.db, "agent", &changes).unwrap(),
            receipts
        );
        let cursor = state_get(&agent.db, "cursor", Value::Null)
            .unwrap()
            .as_i64()
            .unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &incremental(&main.db, "agent", cursor).unwrap(),
            &receipts,
        )
        .unwrap();
        assert_eq!(journal_count(&agent), 0);
        assert_eq!(
            current_row(&agent.db, "issues", &row).unwrap()["body"],
            "Offline edit"
        );
        assert_eq!(
            current_row(&agent.db, "issues", &row).unwrap()["title"],
            "Online edit"
        );
        assert_eq!(
            agent
                .db
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(
            rows(&agent.db, "PRAGMA foreign_key_check", &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn retention_respects_existing_wal_readers_and_persists_recovery_across_reopen() {
        let f = Fixture::new();
        f.capture();
        grow_journal(&f, 10_080);
        f.db.execute_batch("BEGIN; SELECT count(*) FROM fleet_outbox")
            .unwrap();
        let writer = Connection::open(&f.path).unwrap();
        assert_eq!(prune_journal(&writer).unwrap(), 80);
        let old_view = incremental(&f.db, "agent", 0).unwrap();
        assert!(old_view["changes"].is_array());
        assert_eq!(old_view["changes"][0]["seq"], 1);
        f.db.execute_batch("COMMIT").unwrap();
        drop(writer);
        let restarted = Connection::open(&f.path).unwrap();
        let new_view = incremental(&restarted, "agent", 0).unwrap();
        assert!(new_view["tables"].is_object());
        assert_eq!(new_view["cursor"], 10_080);
        assert_eq!(
            state_get(&restarted, "journal_floor", Value::Null).unwrap(),
            80
        );
    }

    #[test]
    fn retention_never_prunes_companion_changes_and_rolls_back_with_its_floor() {
        let f = Fixture::new();
        install_capture(&f.db, "agent", "agent").unwrap();
        grow_journal(&f, 10_080);
        assert_eq!(prune_journal(&f.db).unwrap(), 0);
        assert_eq!(journal_count(&f), 10_080);
        assert_eq!(
            state_get(&f.db, "journal_floor", Value::Null).unwrap(),
            Value::Null
        );
        f.capture();
        f.db.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(prune_journal(&f.db).unwrap(), 80);
        assert_eq!(state_get(&f.db, "journal_floor", Value::Null).unwrap(), 80);
        f.db.execute_batch("ROLLBACK").unwrap();
        assert_eq!(journal_count(&f), 10_080);
        assert_eq!(
            state_get(&f.db, "journal_floor", Value::Null).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn incremental_identity_reads_are_batched_and_preserve_first_origin() {
        let mut measurements = Vec::new();
        for count in [16, 128] {
            let f = Fixture::new();
            f.capture();
            f.db.execute_batch(&format!(
                "WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<{count})
                 INSERT INTO comments(id,project_id,issue_number,author,body,created_at)
                 SELECT id,'named:Native fleet',1,'human:fixture','Comment',1 FROM n;
                 INSERT INTO fleet_row_ids SELECT 'first','comments',id+1000,id FROM comments WHERE id%2=0;
                 INSERT INTO fleet_row_ids SELECT 'later','comments',id+2000,id FROM comments WHERE id%2=0;
                 INSERT INTO events(project_id,issue_number,actor,action,created_at,data)
                 SELECT project_id,issue_number,author,'comment_resolved',2,json_object('comment_id',id) FROM comments;
                 WITH RECURSIVE n(id) AS (VALUES(100000) UNION ALL SELECT id+1 FROM n WHERE id<129999)
                 INSERT INTO fleet_row_ids SELECT 'unrelated','events',id,id FROM n;"
            )).unwrap();
            let expected = incremental(&f.db, "agent", 0).unwrap();
            let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
            let (db, transport) = crate::database::tests::measured_connection(&f.path);
            let started = std::time::Instant::now();
            let actual = incremental(&db, "agent", 0).unwrap();
            let elapsed = started.elapsed();
            drop(db);
            let (commands, steps) = transport.join().unwrap();
            owner.stop();
            eprintln!(
                "{count} comments and resolutions: {commands} RPCs, {steps} VM steps in {elapsed:?}"
            );
            assert_eq!(actual, expected);
            assert_eq!(actual["changes"].as_array().unwrap().len(), count * 2);
            for change in actual["changes"].as_array().unwrap() {
                let row = &change["append"]["row"];
                if change["table_name"] == "comments" {
                    let local = row_json(change, "after_json").unwrap()["id"]
                        .as_i64()
                        .unwrap();
                    assert_eq!(
                        change["append"]["origin"],
                        if local % 2 == 0 { "first" } else { "main" }
                    );
                    assert_eq!(row["id"], if local % 2 == 0 { local + 1000 } else { local });
                } else {
                    let local_data: Value = serde_json::from_str(
                        row_json(change, "after_json").unwrap()["data"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap();
                    let local = local_data["comment_id"].as_i64().unwrap();
                    let data: Value = serde_json::from_str(row["data"].as_str().unwrap()).unwrap();
                    assert_eq!(
                        data["comment_id"],
                        if local % 2 == 0 { local + 1000 } else { local }
                    );
                    if local % 2 == 0 {
                        assert_eq!(data["comment_origin"], "first");
                    }
                }
            }
            measurements.push((count, commands, steps));
        }
        for (count, commands, steps) in measurements {
            assert!(
                commands < 25,
                "{count} comments required {commands} owner RPCs"
            );
            assert!(
                steps < count as i64 * 200 + 2000,
                "Unrelated identities were scanned: {steps} steps"
            );
        }
    }

    #[test]
    fn incremental_sync_work_is_bounded_by_the_changed_rows() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        unsafe extern "C" fn count_steps(context: *mut std::ffi::c_void) -> std::ffi::c_int {
            let count = unsafe { &*context.cast::<AtomicUsize>() };
            count.fetch_add(100, Ordering::Relaxed);
            0
        }
        let f = Fixture::new();
        f.capture();
        f.db.execute_batch(
            "WITH RECURSIVE n(id) AS (VALUES(100000) UNION ALL SELECT id+1 FROM n WHERE id<129999)
            INSERT INTO fleet_row_ids SELECT 'unrelated','events',id,id FROM n;",
        )
        .unwrap();
        for changed in [false, true] {
            if changed {
                f.db.execute_batch("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Native fleet',1,'human:fixture','New comment',1);").unwrap();
            }
            let steps = AtomicUsize::new(0);
            unsafe {
                rusqlite::ffi::sqlite3_progress_handler(
                    f.db.handle(),
                    100,
                    Some(count_steps),
                    (&steps as *const AtomicUsize).cast_mut().cast(),
                );
            }
            let started = std::time::Instant::now();
            let result = incremental(&f.db, "agent", 0);
            unsafe {
                rusqlite::ffi::sqlite3_progress_handler(
                    f.db.handle(),
                    0,
                    None,
                    std::ptr::null_mut(),
                );
            }
            let payload = result.unwrap();
            assert_eq!(
                payload["changes"].as_array().unwrap().len(),
                usize::from(changed)
            );
            let steps = steps.load(Ordering::Relaxed);
            let elapsed = started.elapsed();
            eprintln!(
                "Incremental pull ({changed} changed): fewer than {} VM steps in {elapsed:?}",
                steps + 100
            );
            assert!(
                steps < 5000,
                "A small pull scanned unrelated identities: {steps} VM steps in {elapsed:?}"
            );
        }
    }

    #[test]
    fn new_local_resolution_canonicalizes_a_remote_comment_without_an_event_mapping() {
        let f = Fixture::new();
        f.capture();
        f.db.execute_batch("INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES(41,'named:Native fleet',1,'human:fixture','Offline comment',1);
            INSERT INTO fleet_row_ids VALUES('offline','comments',77,41);
            INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data) VALUES(51,'named:Native fleet',1,'human:fixture','comment_resolved',2,'{\"comment_id\":41}');").unwrap();
        for payload in [
            incremental(&f.db, "agent", 0).unwrap(),
            snapshot(&f.db, "agent").unwrap(),
        ] {
            let event = if payload["changes"].is_array() {
                &payload["changes"][1]["append"]
            } else {
                payload["tables"]["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["row"]["id"] == 51)
                    .unwrap()
            };
            assert_eq!(event["origin"], "main");
            assert_eq!(event["row"]["id"], 51);
            let data: Value = serde_json::from_str(event["row"]["data"].as_str().unwrap()).unwrap();
            assert_eq!(data["comment_id"], 77);
            assert_eq!(data["comment_origin"], "offline");
            assert_eq!(data["comment_origin_id"], 77);
        }
    }

    #[test]
    fn incremental_resolution_keeps_the_remote_comments_original_identity() {
        let f = Fixture::new();
        f.capture();
        f.db.execute_batch("INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES(41,'named:Native fleet',1,'human:fixture','Offline comment',1);
            INSERT INTO fleet_row_ids VALUES('offline','comments',77,41);
            INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data) VALUES(51,'named:Native fleet',1,'human:fixture','comment_resolved',2,'{\"comment_id\":41}');
            INSERT INTO fleet_row_ids VALUES('main','events',51,51);").unwrap();
        let pull = incremental(&f.db, "agent", 0).unwrap();
        let comment = &pull["changes"][0]["append"];
        assert_eq!(comment["origin"], "offline");
        assert_eq!(comment["row"]["id"], 77);
        let event = &pull["changes"][1]["append"];
        assert_eq!(event["origin"], "main");
        let data: Value = serde_json::from_str(event["row"]["data"].as_str().unwrap()).unwrap();
        assert_eq!(data["comment_id"], 77);
        assert_eq!(data["comment_origin"], "offline");
        assert_eq!(data["comment_origin_id"], 77);
    }

    #[test]
    fn blocker_migration_updates_existing_capture_triggers_before_normalizing() {
        for partial in [false, true] {
            let main = Fixture::new();
            main.db
                .execute_batch(include_str!(
                    "../../../tests/fixtures/pre_dependency_notices.sql"
                ))
                .unwrap();
            // The pre-blocker schema also predates dependency-aware readiness.
            main.db
                .execute_batch("DROP VIEW issue_pickup_ready;")
                .unwrap();
            let legacy_view = include_str!("../../issues/subtasks.sql")
                .split_once("CREATE VIEW")
                .unwrap()
                .1;
            main.db
                .execute_batch(&format!("CREATE VIEW{legacy_view}"))
                .unwrap();
            main.db.execute_batch("DROP INDEX issue_list_summary; DROP INDEX issue_dependency_sources; DROP INDEX issue_active_graph; ALTER TABLE issues DROP COLUMN blockers; ALTER TABLE issues DROP COLUMN manual_blocked;").unwrap();
            main.capture();
            if partial {
                main.db.execute_batch("ALTER TABLE issues ADD COLUMN manual_blocked INTEGER NOT NULL DEFAULT 0; ALTER TABLE issues ADD COLUMN blockers TEXT NOT NULL DEFAULT '[]';").unwrap();
            }
            drop(Store::open(&main.path).unwrap());
            assert!(main.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='issue_list_summary')",
                [],
                |row| row.get::<_, bool>(0),
            ).unwrap());
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
            Connection::open(&path).unwrap().execute("INSERT INTO projects(id,name,next_number) VALUES('named:Native fleet','Native fleet',1)", []).unwrap();
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
                        model: None,
                    }),
                    operation: Operation::Create {
                        title: "Original".into(),
                        body: "Requirements".into(),
                        labels: vec![],
                        at_top: false,
                        draft: false,
                        blockers: vec![],
                        then_titles: vec![],
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
    fn model_recovery_journals_sync_fill_only_metadata_to_ordinary_lists() {
        let main = Fixture::new();
        main.capture();
        let session = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let actor = format!("codex:{session}");
        // The last CLI command ran elsewhere; the exact session transcript is
        // on this peer. Command-host attribution is not transcript ownership.
        let metadata = json!({"id":actor,"kind":"codex","machine":"other-machine","session_id":session,"model":null,"host":"peer-host","cwd":"/saved","source":"CODEX_THREAD_ID"});
        main.db
            .execute(
                "INSERT INTO agents VALUES(?1,?2,123)",
                rusqlite::params![actor, metadata.to_string()],
            )
            .unwrap();
        main.db
            .execute("UPDATE issues SET assignee=?1", [&actor])
            .unwrap();
        let peer = Fixture::new();
        install_capture(&peer.db, "agent", "peer").unwrap();
        apply_pull(&peer.db, "peer", &snapshot(&main.db, "peer").unwrap(), &[]).unwrap();
        let root = peer.path.with_extension("sessions");
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        std::fs::write(
            root.join(format!("sessions/rollout-{session}.jsonl")),
            format!(
                "{}\n{}\n",
                json!({"type":"session_meta","payload":{"id":session}}),
                json!({"type":"turn_context","payload":{"model":"gpt-recovered"}})
            ),
        )
        .unwrap();
        let mut recovery = crate::issues::model_recovery::Recovery::default();
        while !recovery.step(&peer.db, &root).unwrap() {}
        let changes = rows(
            &peer.db,
            "SELECT * FROM fleet_outbox WHERE table_name='agents'",
            &[],
        )
        .unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            accept_changes(&main.db, "peer", &changes).unwrap()[0]["state"],
            "applied"
        );
        let canonical = current_row(&main.db, "agents", &json!({"id":actor})).unwrap();
        assert_eq!(canonical["last_seen"], 123);
        let saved: Value = serde_json::from_str(canonical["metadata"].as_str().unwrap()).unwrap();
        assert_eq!(saved["model"], "gpt-recovered");
        serde_json::from_value::<Actor>(saved)
            .expect("Recovered metadata retains the Actor schema");
        // A newer live capture must win against a delayed repair.
        main.db.execute("UPDATE agents SET metadata=json_set(metadata,'$.model','newer-model','$.cwd','/newer'),last_seen=456 WHERE id=?1",[&actor]).unwrap();
        let mut delayed = changes[0].clone();
        delayed["seq"] = json!(99999);
        accept_changes(&main.db, "peer", &[delayed]).unwrap();
        let canonical = current_row(&main.db, "agents", &json!({"id":actor})).unwrap();
        assert_eq!(canonical["last_seen"], 456);
        let saved: Value = serde_json::from_str(canonical["metadata"].as_str().unwrap()).unwrap();
        assert_eq!(saved["model"], "newer-model");
        assert_eq!(saved["cwd"], "/newer");
        let replica = Fixture::new();
        install_capture(&replica.db, "agent", "viewer").unwrap();
        apply_pull(
            &replica.db,
            "viewer",
            &snapshot(&main.db, "viewer").unwrap(),
            &[],
        )
        .unwrap();
        let mut store = Store::open(&replica.path).unwrap();
        let response = store
            .execute(&Request {
                version: 1,
                project: Project {
                    id: "named:Native fleet".into(),
                    name: "Native fleet".into(),
                },
                project_override: None,
                actor: None,
                operation: serde_json::from_value(json!({"action":"list","state":"open","mine":false,"unassigned":false,"labels":[],"limit":100,"offset":0})).unwrap(),
                request_id: None,
            })
            .unwrap();
        assert_eq!(response["actor_models"][&actor], "newer-model");
        let detail = store
            .execute(&Request {
                version: 1,
                project: Project {
                    id: "named:Native fleet".into(),
                    name: "Native fleet".into(),
                },
                project_override: None,
                actor: None,
                operation: Operation::View { number: 1 },
                request_id: None,
            })
            .unwrap();
        assert_eq!(detail["actor_models"][&actor], "newer-model");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bootstrap_identical_issue_preserves_archive_progress_and_reconnect_edits() {
        for legacy in [false, true] {
            let main = Fixture::new();
            main.capture();
            main.db.execute_batch("UPDATE issues SET archive_touched_at=123; INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'peer');").unwrap();
            let key = json!({"project_id":"named:Native fleet","number":1});
            let canonical = current_row(&main.db, "issues", &key).unwrap();
            let mut incoming = canonical.clone();
            if legacy {
                incoming.as_object_mut().unwrap().retain(|field, _| {
                    !field.starts_with("archive_") && field != "archived_comments"
                });
            } else {
                incoming["archive_touched_at"] = json!(456);
            }
            let history = rows(&main.db, "SELECT * FROM events", &[]).unwrap()[0].clone();
            let changes = vec![
                json!({"seq":1,"table_name":"issues","bootstrap":true,"after_json":incoming.to_string()}),
                json!({"seq":2,"table_name":"events","bootstrap":true,"after_json":history.to_string()}),
            ];
            let receipts = accept_changes(&main.db, "peer", &changes).unwrap();
            assert!(
                receipts.iter().all(|r| r["state"] == "applied"),
                "{receipts:?}"
            );
            assert_eq!(
                accept_changes(&main.db, "peer", &changes).unwrap(),
                receipts
            );
            assert_eq!(current_row(&main.db, "issues", &key).unwrap(), canonical);
            assert_eq!(
                rows(&main.db, "SELECT * FROM events", &[]).unwrap().len(),
                1
            );

            let mut edited = incoming.clone();
            edited["body"] = json!("Completed independently while disconnected");
            edited["version"] = json!(2);
            let edit = json!({"seq":3,"table_name":"issues","before_json":incoming.to_string(),"after_json":edited.to_string()});
            assert_eq!(
                accept_changes(&main.db, "peer", &[edit]).unwrap()[0]["state"],
                "applied"
            );
            let saved = current_row(&main.db, "issues", &key).unwrap();
            assert_eq!(saved["body"], edited["body"]);
            assert_eq!(saved["archive_touched_at"], 123);
        }
    }

    #[test]
    fn bootstrap_still_rejects_real_collisions_and_their_history() {
        for field in ["title", "body", "created_by", "origin"] {
            let main = Fixture::new();
            main.capture();
            let key = json!({"project_id":"named:Native fleet","number":1});
            let canonical = current_row(&main.db, "issues", &key).unwrap();
            let mut incoming = canonical.clone();
            incoming[field] = json!("Different legacy issue");
            let mut history = rows(&main.db, "SELECT * FROM events", &[]).unwrap()[0].clone();
            history["data"] = json!("{\"attempted\":true}");
            let changes = vec![
                json!({"seq":1,"table_name":"issues","bootstrap":true,"after_json":incoming.to_string()}),
                json!({"seq":2,"table_name":"events","bootstrap":true,"after_json":history.to_string()}),
            ];
            let receipts = accept_changes(&main.db, "peer", &changes).unwrap();
            assert_eq!(
                receipts[0]["reason"], "Offline issue number is not exclusively allocated",
                "{field}"
            );
            assert_eq!(
                receipts[1]["reason"],
                "Legacy history belongs to an issue-number collision; retained for review"
            );
            assert_eq!(current_row(&main.db, "issues", &key).unwrap(), canonical);
            assert_eq!(
                rows(&main.db, "SELECT * FROM events", &[]).unwrap().len(),
                1
            );
            assert_eq!(
                rows(&main.db, "SELECT * FROM fleet_conflicts", &[])
                    .unwrap()
                    .len(),
                2
            );
        }
    }

    #[test]
    fn bootstrap_does_not_bypass_number_reservations_or_edit_ownership() {
        let main = Fixture::new();
        main.capture();
        main.db.execute_batch("INSERT INTO fleet_number_reservations VALUES('other','named:Native fleet',2,100); INSERT INTO fleet_number_reservations VALUES('peer','named:Native fleet',101,200); INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'other');").unwrap();
        let original = rows(&main.db, "SELECT * FROM issues", &[]).unwrap()[0].clone();
        for (seq, number, bootstrap, expected) in [
            (1, 2, true, "conflict"),
            (2, 201, false, "conflict"),
            (3, 101, false, "applied"),
            (4, 201, true, "applied"),
        ] {
            let mut incoming = original.clone();
            incoming["number"] = json!(number);
            let change = json!({"seq":seq,"table_name":"issues","bootstrap":bootstrap,"after_json":incoming.to_string()});
            assert_eq!(
                accept_changes(&main.db, "peer", &[change]).unwrap()[0]["state"],
                expected
            );
        }
        let mut edited = original.clone();
        edited["body"] = json!("Unauthorized edit");
        let change = json!({"seq":5,"table_name":"issues","before_json":original.to_string(),"after_json":edited.to_string()});
        assert_eq!(
            accept_changes(&main.db, "peer", &[change]).unwrap()[0]["reason"],
            "Issue allocation was revoked or belongs to another machine"
        );
        assert_eq!(
            current_row(&main.db, "issues", &original).unwrap(),
            original
        );
    }

    #[test]
    fn repeated_dependency_replay_from_distinct_actors_stabilizes() {
        let main = Fixture::new();
        main.capture();
        main.db.execute_batch("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,subtask_scheduling) VALUES('named:Native fleet','',1,1,'explicit');
            INSERT INTO issues(project_id,number,title,body,state,created_by,assignee,created_at,updated_at,version,labels)
            VALUES('named:Native fleet',2,'Held upstream','','open','human:fixture','human:fixture',0,0,1,'[]');
            UPDATE issues SET blockers='[2]',assignee='human:fixture' WHERE number=1;").unwrap();
        let before = current_row(
            &main.db,
            "issues",
            &json!({"project_id":"named:Native fleet","number":1}),
        )
        .unwrap();
        let body = "Dependency rework: upstream tasks [2] need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies.";
        for n in 0..6 {
            let origin = format!("notice-peer-{n}");
            let actor = format!("codex:notice-{n}");
            main.db.execute("INSERT INTO agents SELECT ?1,json_set(metadata,'$.id',?1,'$.kind','codex','$.model','test-model'),0 FROM agents WHERE id='human:fixture'", [&actor]).unwrap();
            let history = [
                (
                    "comments",
                    json!({"id":400,"project_id":"named:Native fleet","issue_number":1,"author":actor,"body":body,"created_at":123+n}),
                ),
                (
                    "events",
                    json!({"id":401,"project_id":"named:Native fleet","issue_number":1,"actor":actor,"action":"commented","created_at":123+n,"data":json!({"comment_id":400,"body":body,"actor_model":"test-model"}).to_string()}),
                ),
                (
                    "events",
                    json!({"id":402,"project_id":"named:Native fleet","issue_number":1,"actor":actor,"action":"dependency_rework","created_at":123+n,"data":json!({"dependencies":[[2,n+1]],"actor_model":"test-model"}).to_string()}),
                ),
            ];
            let changes=history.iter().enumerate().map(|(seq,(table,row))|json!({"seq":seq+1,"table_name":table,"before_json":null,"after_json":row.to_string()})).collect::<Vec<_>>();
            let receipts = if n == 0 {
                changes
                    .chunks(1)
                    .flat_map(|part| {
                        let receipts = accept_changes(&main.db, &origin, part).unwrap();
                        crate::issues::blockers::reconcile(
                            &main.db,
                            "named:Native fleet",
                            Some("human:fixture"),
                            200,
                        )
                        .unwrap();
                        receipts
                    })
                    .collect::<Vec<_>>()
            } else {
                accept_changes(&main.db, &origin, &changes).unwrap()
            };
            assert!(
                receipts.iter().all(|r| r["state"] == "applied"),
                "{receipts:?}"
            );
            assert_eq!(
                accept_changes(&main.db, &origin, &changes).unwrap(),
                receipts
            );
            assert_eq!(
                rows(&main.db, "SELECT * FROM comments WHERE issue_number=1", &[])
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                rows(
                    &main.db,
                    "SELECT * FROM events WHERE issue_number=1 AND action='dependency_rework'",
                    &[]
                )
                .unwrap()
                .len(),
                1
            );
            assert_eq!(
                rows(
                    &main.db,
                    "SELECT * FROM events WHERE issue_number=1 AND action='commented'",
                    &[]
                )
                .unwrap()
                .len(),
                1
            );
            assert_eq!(current_row(&main.db, "issues", &before).unwrap(), before);
            if n > 0 {
                assert!(receipts.iter().all(|r| r.get("canonical_append").is_none()));
                assert!(
                    rows(
                        &main.db,
                        "SELECT * FROM fleet_row_ids WHERE origin=?",
                        &[json!(origin)]
                    )
                    .unwrap()
                    .is_empty()
                );
            }
        }
        // Canonical snapshot replay preserves history and does not generate a
        // fresh notice while reconciling on the receiving companion.
        let peer = Fixture::new();
        install_capture(&peer.db, "agent", "notice-receiver").unwrap();
        let pull = snapshot(&main.db, "notice-receiver").unwrap();
        for _ in 0..3 {
            apply_pull(&peer.db, "notice-receiver", &pull, &[]).unwrap();
            assert_eq!(
                rows(&peer.db, "SELECT * FROM comments WHERE issue_number=1", &[])
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                rows(
                    &peer.db,
                    "SELECT * FROM events WHERE issue_number=1 AND action='dependency_rework'",
                    &[]
                )
                .unwrap()
                .len(),
                1
            );
        }
    }

    #[test]
    fn obsolete_dependency_replay_is_acknowledged_without_a_false_comment_mapping() {
        let main = Fixture::new();
        main.capture();
        main.db.execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,subtask_scheduling) VALUES('named:Native fleet','',1,1,'explicit')", []).unwrap();
        let body = "Dependency rework: upstream tasks [99] need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies.";
        let rows = [
            (
                "comments",
                json!({"id":400,"project_id":"named:Native fleet","issue_number":1,"author":"human:fixture","body":body,"created_at":123}),
            ),
            (
                "events",
                json!({"id":401,"project_id":"named:Native fleet","issue_number":1,"actor":"human:fixture","action":"commented","created_at":123,"data":json!({"comment_id":400,"body":body}).to_string()}),
            ),
            (
                "events",
                json!({"id":402,"project_id":"named:Native fleet","issue_number":1,"actor":"human:fixture","action":"dependency_rework","created_at":123,"data":json!({"dependencies":[[99,1]]}).to_string()}),
            ),
        ];
        let changes = rows.iter().enumerate().map(|(n,(table,row))|json!({"seq":n+1,"table_name":table,"before_json":null,"after_json":row.to_string()})).collect::<Vec<_>>();
        let receipts = accept_changes(&main.db, "legacy-notice-peer", &changes).unwrap();
        assert_eq!(receipts.len(), 3);
        for receipt in &receipts {
            assert_eq!(receipt["state"], "applied");
            assert_eq!(receipt["suppressed"], "obsolete dependency notice");
            assert!(receipt.get("canonical_append").is_none());
        }
        assert_eq!(
            accept_changes(&main.db, "legacy-notice-peer", &changes).unwrap(),
            receipts
        );
        assert!(
            super::rows(
                &main.db,
                "SELECT * FROM fleet_row_ids WHERE origin='legacy-notice-peer'",
                &[]
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            super::rows(&main.db, "SELECT * FROM comments WHERE created_at=123", &[])
                .unwrap()
                .is_empty()
        );
        assert!(
            super::rows(&main.db, "SELECT * FROM events WHERE created_at=123", &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn archived_snapshots_fetch_cold_history_without_refilling_the_hot_replica() {
        use super::super::archive as archive_sync;
        use crate::issues::archive as cold;
        let main = Fixture::new();
        main.capture();
        main.db.execute_batch("UPDATE issues SET state='closed',closed_at=100,closed_by='human:fixture',updated_at=100; UPDATE events SET created_at=1;
            INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Native fleet',1,'human:fixture','Historical comment',10);").unwrap();
        main.db.execute_batch("INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data) VALUES
            (9,'named:Native fleet',1,'human:fixture','commit_attached',20,'{\"sha\":\"1234567890abcdef\",\"url\":\"https://github.com/example/repo/commit/1234567890abcdef\"}'),
            (10,'named:Native fleet',1,'human:fixture','commit_removed',30,'{\"sha\":\"1234567890abcdef\"}'),
            (11,'named:Native fleet',1,'human:fixture','commit_attached',40,'{\"sha\":\"abcdef1234567890\",\"url\":\"https://github.com/example/repo/commit/abcdef1234567890\"}');").unwrap();
        assert!(
            cold::archive_issue(&main.db, "named:Native fleet", 1, cold::GRACE_MS + 100).unwrap()
        );
        let payload = snapshot(&main.db, "agent").unwrap();
        assert!(payload["tables"]["comments"].as_array().unwrap().is_empty());
        let issue = &payload["tables"]["issues"][0];
        assert!(issue.get("archive_cleanup").is_none());
        assert!(issue.get("archive_restoring").is_none());
        assert!(issue.get("archive_touched_at").is_none());
        assert!(issue["archive_key"].is_string());
        let agent = Fixture::new();
        agent.db.execute("DELETE FROM events", []).unwrap();
        install_capture(&agent.db, "agent", "agent").unwrap();
        let mut pages = 0;
        let prepared =
            archive_sync::prepare_pull(&agent.db, &payload, |key, project, number, cursor| {
                pages += 1;
                cold::transfer::export_page(&main.db, key, project, number, cursor)
            })
            .unwrap();
        assert!(pages > 0);
        apply_pull(&agent.db, "agent", &prepared, &[]).unwrap();
        while cold::cleanup_history(&agent.db).unwrap() != 0 {}
        assert_eq!(
            agent
                .db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            cold::issue_body(
                &agent.db,
                "named:Native fleet",
                1,
                issue["archive_key"].as_str().unwrap()
            )
            .unwrap(),
            "Requirements"
        );
        let again = archive_sync::prepare_pull(&agent.db, &payload, |_, _, _, _| {
            panic!("An existing cold copy must not transfer again")
        })
        .unwrap();
        agent
            .db
            .execute(
                "UPDATE issues SET archive_restoring=1,archive_cleanup=1 WHERE number=1",
                [],
            )
            .unwrap();
        apply_pull(&agent.db, "agent", &again, &[]).unwrap();
        assert_eq!(
            agent
                .db
                .query_row(
                    "SELECT archive_restoring FROM issues WHERE number=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(cold::cleanup_history(&agent.db).unwrap(), 0);
        assert_eq!(
            agent
                .db
                .query_row("SELECT sha FROM issue_commits", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "abcdef1234567890"
        );
        cold::restore_issue(&agent.db, "named:Native fleet", 1, cold::GRACE_MS + 101).unwrap();
        assert_eq!(
            agent
                .db
                .query_row("SELECT count(*) FROM fleet_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            agent
                .db
                .query_row("SELECT sha FROM issue_commits", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "abcdef1234567890"
        );
        assert_eq!(
            agent
                .db
                .query_row("SELECT body FROM comments", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "Historical comment"
        );
        for fixture in [&main, &agent] {
            let archive =
                std::path::PathBuf::from(format!("{}.archive.db", fixture.path.display()));
            std::fs::remove_file(archive).unwrap();
        }
    }

    #[test]
    fn offline_edits_survive_archive_snapshots_and_lost_acknowledgments() {
        use super::super::archive as archive_sync;
        use crate::issues::archive as cold;
        let main = Fixture::new();
        main.capture();
        main.db.execute_batch("UPDATE issues SET state='closed',closed_at=100,closed_by='human:fixture',updated_at=100; UPDATE events SET created_at=1;
            INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Native fleet',1,'human:fixture','Original comment',10);").unwrap();
        let agent = Fixture::new();
        agent.db.execute("DELETE FROM events", []).unwrap();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        let mut store = Store::open(&agent.path).unwrap();
        let request:Request=serde_json::from_value(json!({"version":1,"project":{"id":"named:Native fleet","name":"Native fleet"},"actor":{"id":"human:fixture","kind":"human","machine":"agent","host":"fixture","cwd":std::env::temp_dir(),"source":"test"},"request_id":"offline-archive-comment","operation":{"action":"comment","number":1,"body":"Offline comment"}})).unwrap();
        store.execute(&request).unwrap();
        assert!(
            cold::archive_issue(&main.db, "named:Native fleet", 1, cold::GRACE_MS + 100).unwrap()
        );
        while cold::cleanup_history(&main.db).unwrap() != 0 {}
        let payload = snapshot(&main.db, "agent").unwrap();
        let prepared =
            archive_sync::prepare_pull(&agent.db, &payload, |key, project, number, cursor| {
                cold::transfer::export_page(&main.db, key, project, number, cursor)
            })
            .unwrap();
        assert!(prepared["tables"]["issues"][0]["archive_key"].is_null());
        apply_pull(&agent.db, "agent", &prepared, &[]).unwrap();
        assert_eq!(
            agent
                .db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            agent
                .db
                .query_row("SELECT body FROM issues", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "Requirements"
        );
        let outgoing = journal(&agent.db, 0).unwrap();
        let receipts = accept_changes(&main.db, "agent", &outgoing).unwrap();
        assert!(
            receipts.iter().all(|receipt| receipt["state"] == "applied"),
            "{receipts:?}"
        );
        assert_eq!(
            accept_changes(&main.db, "agent", &outgoing).unwrap(),
            receipts
        );
        assert_eq!(
            main.db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let payload = snapshot(&main.db, "agent").unwrap();
        let prepared = archive_sync::prepare_pull(&agent.db, &payload, |_, _, _, _| {
            panic!("Restored snapshots do not need another transfer")
        })
        .unwrap();
        apply_pull(&agent.db, "agent", &prepared, &receipts).unwrap();
        assert_eq!(
            agent
                .db
                .query_row("SELECT count(*) FROM fleet_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            agent
                .db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        for fixture in [&main, &agent] {
            std::fs::remove_file(format!("{}.archive.db", fixture.path.display())).unwrap();
        }
    }

    #[test]
    fn archive_preflight_batches_hot_metadata_reads_and_rejects_stale_snapshots_early() {
        use super::super::archive as archive_sync;
        for count in [16, 128] {
            let f = Fixture::new();
            f.capture();
            f.db.execute("WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x+1 FROM n WHERE x<?1) INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) SELECT 'named:Native fleet',x,'Task','','closed','human:fixture',1,1,1,'[]' FROM n",[count]).unwrap();
            let payload = snapshot(&f.db, "agent").unwrap();
            let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
            let (db, transport) = crate::database::tests::measured_connection(&f.path);
            archive_sync::prepare_pull(&db, &payload, |_, _, _, _| {
                panic!("No archive is referenced")
            })
            .unwrap();
            drop(db);
            let (commands, _) = transport.join().unwrap();
            assert!(
                commands <= 2,
                "{count} issues made {commands} metadata reads"
            );
            state_set(
                &f.db,
                "cursor",
                &json!(payload["cursor"].as_i64().unwrap() + 1),
            )
            .unwrap();
            assert!(
                archive_sync::prepare_pull(&f.db, &payload, |_, _, _, _| panic!(
                    "A stale pull must never transfer data"
                ))
                .is_err()
            );
            owner.stop();
        }
    }

    #[test]
    fn archived_resolution_history_survives_fleet_identity_translation() {
        use super::super::archive as archive_sync;
        use crate::issues::archive as cold;
        let main = Fixture::new();
        main.capture();
        main.db.execute_batch("UPDATE issues SET state='closed',closed_at=100,closed_by='human:fixture',updated_at=100; UPDATE events SET created_at=1;
            INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES(45,'named:Native fleet',1,'human:fixture','From offline peer',10);
            INSERT INTO fleet_row_ids VALUES('offline','comments',77,45);
            INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Native fleet',1,'human:fixture','comment_resolved',20,'{\"comment_id\":45}');").unwrap();
        let agent = Fixture::new();
        agent.db.execute("DELETE FROM events", []).unwrap();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        let before = rows(
            &agent.db,
            "SELECT * FROM events WHERE action='comment_resolved'",
            &[],
        )
        .unwrap();
        assert!(
            before[0]["data"]
                .as_str()
                .unwrap()
                .contains("comment_origin")
        );
        assert!(
            cold::archive_issue(&main.db, "named:Native fleet", 1, cold::GRACE_MS + 100).unwrap()
        );
        let payload = snapshot(&main.db, "agent").unwrap();
        let prepared =
            archive_sync::prepare_pull(&agent.db, &payload, |key, project, number, cursor| {
                cold::transfer::export_page(&main.db, key, project, number, cursor)
            })
            .unwrap();
        apply_pull(&agent.db, "agent", &prepared, &[]).unwrap();
        while cold::cleanup_history(&agent.db).unwrap() != 0 {}
        cold::restore_issue(&agent.db, "named:Native fleet", 1, cold::GRACE_MS + 101).unwrap();
        let after = rows(
            &agent.db,
            "SELECT * FROM events WHERE action='comment_resolved'",
            &[],
        )
        .unwrap();
        let before_data: Value = serde_json::from_str(before[0]["data"].as_str().unwrap()).unwrap();
        let after_data: Value = serde_json::from_str(after[0]["data"].as_str().unwrap()).unwrap();
        assert_eq!(before[0]["id"], after[0]["id"]);
        assert_eq!(before_data["comment_id"], after_data["comment_id"]);
        assert_eq!(journal_count(&agent), 0);
        for fixture in [&main, &agent] {
            std::fs::remove_file(format!("{}.archive.db", fixture.path.display())).unwrap();
        }
    }

    #[test]
    fn an_edit_after_archive_preparation_cannot_disappear_behind_the_manifest() {
        use super::super::archive as archive_sync;
        use crate::issues::archive as cold;
        let main = Fixture::new();
        main.capture();
        main.db.execute_batch("UPDATE issues SET state='closed',closed_at=100,closed_by='human:fixture',updated_at=100; UPDATE events SET created_at=1;
            INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Native fleet',1,'human:fixture','Original comment',10);").unwrap();
        let agent = Fixture::new();
        agent.db.execute("DELETE FROM events", []).unwrap();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        assert!(
            cold::archive_issue(&main.db, "named:Native fleet", 1, cold::GRACE_MS + 100).unwrap()
        );
        let payload = snapshot(&main.db, "agent").unwrap();
        let prepared =
            archive_sync::prepare_pull(&agent.db, &payload, |key, project, number, cursor| {
                cold::transfer::export_page(&main.db, key, project, number, cursor)
            })
            .unwrap();
        let mut store = Store::open(&agent.path).unwrap();
        let request:Request=serde_json::from_value(json!({"version":1,"project":{"id":"named:Native fleet","name":"Native fleet"},"actor":{"id":"human:fixture","kind":"human","machine":"agent","host":"fixture","cwd":std::env::temp_dir(),"source":"test"},"operation":{"action":"comment","number":1,"body":"Arrived during archive preparation"}})).unwrap();
        store.execute(&request).unwrap();
        let error = apply_pull(&agent.db, "agent", &prepared, &[]).unwrap_err();
        assert_eq!(
            error.downcast_ref::<crate::issues::Error>().unwrap().code,
            "archive_retry"
        );
        assert!(
            agent
                .db
                .query_row("SELECT archive_key IS NULL FROM issues", [], |r| r
                    .get::<_, bool>(0))
                .unwrap()
        );
        let retry = archive_sync::prepare_pull(&agent.db, &payload, |_, _, _, _| {
            panic!("Copy already available")
        })
        .unwrap();
        apply_pull(&agent.db, "agent", &retry, &[]).unwrap();
        assert_eq!(
            agent
                .db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        for fixture in [&main, &agent] {
            std::fs::remove_file(format!("{}.archive.db", fixture.path.display())).unwrap();
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
            model: None,
        };
        let mut store = Store::open(&main.path).unwrap();
        main.db.execute("INSERT INTO projects(id,name,next_number) VALUES('named:Destination','Destination',1)", []).unwrap();
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
        main.db
            .execute(
                "UPDATE issue_pull_requests SET status='open',checked_at=456",
                [],
            )
            .unwrap();
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
            assert_eq!(pr["status"], "open");
            assert_eq!(pr["checked_at"], 456);
            assert_eq!(pr["added_by"], "human:fixture");
            assert_eq!(pr["created_at"], 123);
        }
    }

    #[test]
    fn quiet_hours_survive_fleet_snapshots_and_legacy_rows() {
        let main = Fixture::new();
        main.capture();
        let quiet = serde_json::to_string(&crate::quiet_hours::QuietHours {
            enabled: true,
            start: "21:30".into(),
            end: "08:15".into(),
            time_zone: "America/Chicago".into(),
        })
        .unwrap();
        main.db
            .execute("UPDATE global_settings SET quiet_hours=?", [&quiet])
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
        let mut settings = rows(&agent.db, "SELECT * FROM global_settings", &[])
            .unwrap()
            .remove(0);
        assert_eq!(settings["quiet_hours"], quiet);
        settings.as_object_mut().unwrap().remove("quiet_hours");
        put_row(&agent.db, "global_settings", &settings).unwrap();
        assert_eq!(
            rows(&agent.db, "SELECT quiet_hours FROM global_settings", &[]).unwrap()[0]["quiet_hours"],
            quiet
        );
    }

    #[test]
    fn pr_status_and_merge_setting_survive_fleet_snapshots() {
        let main = Fixture::new();
        main.capture();
        main.db
            .execute(
                "INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'agent')",
                [],
            )
            .unwrap();
        main.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose,status,checked_at,merged_at,pr_title,author_id) VALUES('named:Native fleet',1,'https://github.com/example/repo/pull/1','human:fixture',123,'fix','merged',456,400,'Ship it',42)",[]).unwrap();
        main.db
            .execute(
                "UPDATE global_settings SET auto_close_merged_prs=0,github_user_id=42",
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
        let pr = &rows(&agent.db, "SELECT * FROM issue_pull_requests", &[]).unwrap()[0];
        assert_eq!(pr["status"], "merged");
        assert_eq!(pr["checked_at"], 456);
        assert_eq!(pr["merged_at"], 400);
        assert_eq!(pr["pr_title"], "Ship it");
        assert_eq!(pr["author_id"], 42);
        let mut settings = rows(&agent.db, "SELECT * FROM global_settings", &[])
            .unwrap()
            .remove(0);
        assert_eq!(settings["github_user_id"], 42);
        settings.as_object_mut().unwrap().remove("github_user_id");
        put_row(&agent.db, "global_settings", &settings).unwrap();
        assert_eq!(
            rows(&agent.db, "SELECT github_user_id FROM global_settings", &[]).unwrap()[0]["github_user_id"],
            42
        );
        assert_eq!(
            rows(
                &agent.db,
                "SELECT auto_close_merged_prs FROM global_settings",
                &[]
            )
            .unwrap()[0]["auto_close_merged_prs"],
            0
        );
        let mut legacy = pr.clone();
        for key in [
            "status",
            "checked_at",
            "error",
            "merged_at",
            "pr_title",
            "author_id",
        ] {
            legacy.as_object_mut().unwrap().remove(key);
        }
        put_row(&agent.db, "issue_pull_requests", &legacy).unwrap();
        let restored = &rows(&agent.db, "SELECT * FROM issue_pull_requests", &[]).unwrap()[0];
        assert_eq!(restored["merged_at"], 400);
        assert_eq!(restored["pr_title"], "Ship it");
        assert_eq!(restored["author_id"], 42);
        assert_eq!(
            rows(&agent.db, "SELECT status FROM issue_pull_requests", &[]).unwrap()[0]["status"],
            "merged"
        );
    }

    #[test]
    fn chief_assignment_and_revocation_travel_without_issue_changes() {
        let main = Fixture::new();
        main.capture();
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        let assignment = crate::chief_ownership::Assignment {
            project_id: "named:Native fleet".into(),
            node: "agent".into(),
            worker_id: "chosen".into(),
            generation: 1,
            revoking: false,
        };
        crate::chief_ownership::apply(&main.db, std::slice::from_ref(&assignment)).unwrap();
        let initial = snapshot(&main.db, "agent").unwrap();
        apply_pull(&agent.db, "agent", &initial, &[]).unwrap();
        assert!(
            crate::chief_ownership::allowed(&agent.db, "named:Native fleet", "chosen").unwrap()
        );
        assert!(
            !crate::chief_ownership::allowed(&agent.db, "named:Native fleet", "other").unwrap()
        );
        let revoked = crate::chief_ownership::Assignment {
            generation: 2,
            revoking: true,
            ..assignment
        };
        crate::chief_ownership::apply(&main.db, &[revoked]).unwrap();
        let next = incremental(&main.db, "agent", initial["cursor"].as_i64().unwrap()).unwrap();
        assert!(next["changes"].as_array().unwrap().is_empty());
        apply_pull(&agent.db, "agent", &next, &[]).unwrap();
        assert!(
            !crate::chief_ownership::allowed(&agent.db, "named:Native fleet", "chosen").unwrap()
        );
        apply_pull(&agent.db, "agent", &initial, &[]).unwrap();
        assert!(
            !crate::chief_ownership::allowed(&agent.db, "named:Native fleet", "chosen").unwrap()
        );
    }

    #[test]
    fn capture_upgrade_repairs_and_republishes_missed_chief_settings() {
        let main = Fixture::new();
        main.db.execute("INSERT INTO project_settings(project_id,prompt,version) VALUES('named:Native fleet','Work',1)", []).unwrap();
        main.db.execute_batch("ALTER TABLE project_settings DROP COLUMN chief_enabled; ALTER TABLE project_settings DROP COLUMN chief_prompt;").unwrap();
        main.capture();
        drop(Store::open(&main.path).unwrap());
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        main.db
            .execute(
                "UPDATE project_settings SET chief_enabled=1,chief_prompt='Organize',version=2",
                [],
            )
            .unwrap();
        let cursor = rows(
            &main.db,
            "SELECT coalesce(max(seq),0) AS seq FROM fleet_outbox",
            &[],
        )
        .unwrap()[0]["seq"]
            .as_i64()
            .unwrap();
        main.capture();
        let repaired = incremental(&main.db, "agent", cursor).unwrap();
        apply_pull(&agent.db, "agent", &repaired, &[]).unwrap();
        let settings = rows(
            &agent.db,
            "SELECT chief_enabled,chief_prompt,version FROM project_settings",
            &[],
        )
        .unwrap();
        assert_eq!(
            settings[0],
            json!({"chief_enabled":1,"chief_prompt":"Organize","version":2})
        );
        let schema: i64 = main
            .db
            .pragma_query_value(None, "schema_version", |r| r.get(0))
            .unwrap();
        let cursor = repaired["cursor"].as_i64().unwrap();
        main.capture();
        assert_eq!(
            main.db
                .pragma_query_value(None, "schema_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            schema
        );
        assert!(
            incremental(&main.db, "agent", cursor).unwrap()["changes"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        main.db
            .execute(
                "UPDATE project_settings SET chief_enabled=0,chief_prompt=NULL",
                [],
            )
            .unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &incremental(&main.db, "agent", cursor).unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            rows(
                &agent.db,
                "SELECT chief_enabled,chief_prompt FROM project_settings",
                &[]
            )
            .unwrap()[0],
            json!({"chief_enabled":0,"chief_prompt":null})
        );
    }

    #[test]
    fn scheduling_capture_upgrade_republishes_and_normalizes_legacy_replays() {
        let main = Fixture::new();
        main.db
            .execute_batch(include_str!(
                "../../../tests/fixtures/pre_dependency_notices.sql"
            ))
            .unwrap();
        main.db.execute_batch("INSERT INTO project_settings(project_id,prompt,version) VALUES('named:Native fleet','Work',1);
            DROP VIEW issue_pickup_ready;
            CREATE VIEW issue_pickup_ready AS SELECT project_id,number FROM issues WHERE state='open';
            ALTER TABLE project_settings DROP COLUMN subtask_scheduling;").unwrap();
        main.capture();
        drop(Store::open(&main.path).unwrap());
        let agent = Fixture::new();
        install_capture(&agent.db, "agent", "agent").unwrap();
        let initial = snapshot(&main.db, "agent").unwrap();
        apply_pull(&agent.db, "agent", &initial, &[]).unwrap();
        main.db
            .execute(
                "UPDATE project_settings SET subtask_scheduling='explicit',version=2",
                [],
            )
            .unwrap();
        let cursor = rows(
            &main.db,
            "SELECT coalesce(max(seq),0) AS seq FROM fleet_outbox",
            &[],
        )
        .unwrap()[0]["seq"]
            .as_i64()
            .unwrap();
        main.capture();
        let next = incremental(&main.db, "agent", cursor).unwrap();
        assert!(!next["changes"].as_array().unwrap().is_empty());
        apply_pull(&agent.db, "agent", &next, &[]).unwrap();
        assert_eq!(
            rows(
                &agent.db,
                "SELECT subtask_scheduling FROM project_settings",
                &[]
            )
            .unwrap()[0]["subtask_scheduling"],
            "explicit"
        );
        let legacy = json!({"project_id":"named:Native fleet","prompt":"Updated","prs_enabled":0,"version":3,"boss_name":"Boss"});
        put_row(&agent.db, "project_settings", &legacy).unwrap();
        assert_eq!(
            rows(
                &agent.db,
                "SELECT subtask_scheduling FROM project_settings",
                &[]
            )
            .unwrap()[0]["subtask_scheduling"],
            "explicit"
        );
        main.db
            .execute(
                "UPDATE project_settings SET subtask_scheduling='sequential',version=4",
                [],
            )
            .unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &incremental(&main.db, "agent", next["cursor"].as_i64().unwrap()).unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            rows(
                &agent.db,
                "SELECT subtask_scheduling FROM project_settings",
                &[]
            )
            .unwrap()[0]["subtask_scheduling"],
            "explicit"
        );
    }

    #[test]
    fn replication_reconciliation_does_not_scan_unrelated_projects() {
        let mut measurements = Vec::new();
        for incoming in [false, true] {
            let mut work = Vec::new();
            for other_projects in [4, 100] {
                let f = Fixture::new();
                f.db.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?1) INSERT INTO projects(id,name,next_number) SELECT 'named:unrelated-'||x,'Unrelated '||x,65 FROM n", [other_projects]).unwrap();
                f.db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<64) INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) SELECT p.id,n.x,'Unrelated','','open','human:fixture',0,0,1,'[]',n.x FROM projects p CROSS JOIN n WHERE p.id LIKE 'named:unrelated-%';
                    INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order,blockers) VALUES('named:Native fleet',2,'Dependent','','blocked','human:fixture',0,0,1,'[]',2,'[1]');
                    INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'peer');").unwrap();
                install_capture(
                    &f.db,
                    if incoming { "controller" } else { "agent" },
                    if incoming { "main" } else { "peer" },
                )
                .unwrap();
                f.db.execute("DELETE FROM fleet_outbox", []).unwrap();
                let before = current_row(
                    &f.db,
                    "issues",
                    &json!({"project_id":"named:Native fleet","number":1}),
                )
                .unwrap();
                let mut after = before.clone();
                after["state"] = json!("closed");
                after["closed_by"] = json!("human:fixture");
                after["closed_at"] = json!(123);
                let change = json!({"seq":1,"table_name":"issues","before_json":before.to_string(),"after_json":after.to_string()});
                let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
                let (db, transport) = crate::database::tests::measured_connection(&f.path);
                if incoming {
                    assert_eq!(
                        accept_changes(&db, "peer", std::slice::from_ref(&change)).unwrap()[0]["state"],
                        "applied"
                    );
                } else {
                    apply_pull(
                        &db,
                        "peer",
                        &json!({"cursor":1,"changes":[change],"allocations":[],"ranges":[]}),
                        &[],
                    )
                    .unwrap();
                }
                drop(db);
                let (commands, steps) = transport.join().unwrap();
                eprintln!(
                    "One issue update, incoming={incoming}, {other_projects} unrelated projects: {commands} RPCs, {steps} query VM steps"
                );
                work.push((commands, steps));
                if incoming {
                    let (db, transport) = crate::database::tests::measured_connection(&f.path);
                    assert_eq!(
                        accept_changes(&db, "peer", &[change]).unwrap()[0]["state"],
                        "applied"
                    );
                    drop(db);
                    let (commands, _) = transport.join().unwrap();
                    eprintln!("Receipt replay: {commands} RPCs");
                    work.push((commands, 0));
                }
                owner.stop();
                assert_eq!(rows(&f.db, "SELECT state FROM issues WHERE project_id='named:Native fleet' AND number=2", &[]).unwrap()[0]["state"], "open");
                assert_eq!(rows(&f.db, "SELECT count(*) count FROM issues WHERE project_id LIKE 'named:unrelated-%' AND (state<>'open' OR version<>1)", &[]).unwrap()[0]["count"], 0);
            }
            measurements.push((incoming, work));
        }
        for (incoming, work) in measurements {
            let stride = if incoming { 2 } else { 1 };
            assert_eq!(
                work[0], work[stride],
                "Unrelated projects added reconciliation work"
            );
            if incoming {
                assert!(
                    work[1].0 <= 3 && work[3].0 <= 3,
                    "Replay repeated reconciliation: {work:?}"
                );
            }
        }
    }

    #[test]
    fn replicated_issue_deletions_reconcile_dependents_in_each_changed_project() {
        let f = Fixture::new();
        f.db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:Second','Second',4);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order,blockers)
            SELECT id,2,'Dependent','','open','human:fixture',0,0,1,'[]',2,'[3]' FROM projects;
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
            SELECT id,3,'Completed prerequisite','','closed','human:fixture',0,0,1,'[]',3 FROM projects;").unwrap();
        let changes: Vec<_> = rows(&f.db, "SELECT * FROM issues WHERE number=3", &[]).unwrap().iter().enumerate().map(|(index,row)| json!({"seq":index+1,"table_name":"issues","before_json":row.to_string(),"after_json":null})).collect();
        install_capture(&f.db, "agent", "peer").unwrap();
        f.db.execute("DELETE FROM fleet_outbox", []).unwrap();
        apply_pull(
            &f.db,
            "peer",
            &json!({"cursor":2,"changes":changes,"allocations":[],"ranges":[]}),
            &[],
        )
        .unwrap();
        assert!(
            rows(&f.db, "SELECT * FROM issues WHERE number=3", &[])
                .unwrap()
                .is_empty()
        );
        let dependents = rows(&f.db, "SELECT state FROM issues WHERE number=2", &[]).unwrap();
        assert_eq!(dependents.len(), 2);
        assert!(
            dependents.iter().all(|row| row["state"] == "blocked"),
            "Missing prerequisites remain unresolved: {dependents:?}"
        );
    }

    #[test]
    fn graph_only_pulls_reconcile_deferred_receipted_and_removed_relationships() {
        for source in ["deferred", "receipt", "snapshot"] {
            let f = Fixture::new();
            f.db.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) VALUES('named:Native fleet',2,'Child','','open','human:fixture',0,0,1,'[]',2)").unwrap();
            let relationship = json!({"project_id":"named:Native fleet","parent_number":1,"child_number":2,"created_at":0,"created_by":"human:fixture"});
            let mut payload = json!({"cursor":1,"allocations":[],"ranges":[]});
            let mut receipts = Vec::new();
            if source == "deferred" {
                f.db.execute(
                    "INSERT INTO fleet_deferred_subtasks VALUES('named:Native fleet',2,?1)",
                    [relationship.to_string()],
                )
                .unwrap();
            } else {
                f.db.execute_batch("INSERT INTO issue_subtasks VALUES('named:Native fleet',1,2,0,'human:fixture'); UPDATE issues SET state='blocked' WHERE number=1").unwrap();
                if source == "receipt" {
                    receipts.push(json!({"seq":99,"state":"applied","canonical_subtask":{"project_id":"named:Native fleet","child_number":2,"row":null}}));
                } else {
                    payload["tables"] = json!({"issue_subtasks":[]});
                }
            }
            install_capture(&f.db, "agent", "peer").unwrap();
            f.db.execute("DELETE FROM fleet_outbox", []).unwrap();
            apply_pull(&f.db, "peer", &payload, &receipts).unwrap();
            assert_eq!(
                rows(&f.db, "SELECT state FROM issues WHERE number=1", &[]).unwrap()[0]["state"],
                if source == "deferred" {
                    "blocked"
                } else {
                    "open"
                },
                "{source}"
            );
            assert!(
                rows(&f.db, "SELECT * FROM fleet_deferred_subtasks", &[])
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn replicated_row_reads_do_not_fetch_metadata_separately() {
        let f = Fixture::new();
        let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
        let (db, transport) = crate::database::tests::measured_connection(&f.path);
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        for number in 0..128 {
            let result = rows(
                &db,
                "SELECT ?1 number,'first' label,NULL absent UNION ALL SELECT ?1+1,'second',NULL",
                &[json!(number)],
            )
            .unwrap();
            assert_eq!(
                result,
                vec![
                    json!({"number":number,"label":"first","absent":null}),
                    json!({"number":number+1,"label":"second","absent":null})
                ]
            );
        }
        db.execute_batch("COMMIT").unwrap();
        drop(db);
        let (commands, _) = transport.join().unwrap();
        owner.stop();
        eprintln!("128 transactional multirow reads: {commands} RPCs");
        assert!(commands <= 130, "{commands} RPCs");
    }

    #[test]
    fn replicated_row_reads_preserve_types_errors_and_changed_columns() {
        let f = Fixture::new();
        let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
        let remote = Connection::connect(&f.path).unwrap();
        for db in [&f.db, &remote] {
            db.execute_batch("DROP TABLE IF EXISTS row_read_test; CREATE TABLE row_read_test(number INTEGER,label TEXT,amount REAL,absent TEXT); INSERT INTO row_read_test VALUES(7,'Label',1.5,NULL)").unwrap();
            let mut expected = json!({"number":7,"label":"Label","amount":1.5,"absent":null});
            assert_eq!(
                rows(db, "SELECT * FROM row_read_test", &[]).unwrap(),
                vec![expected.clone()]
            );
            assert!(
                rows(db, "SELECT * FROM row_read_test WHERE 0", &[])
                    .unwrap()
                    .is_empty()
            );
            db.execute_batch("ALTER TABLE row_read_test ADD COLUMN added TEXT DEFAULT 'new'")
                .unwrap();
            expected["added"] = json!("new");
            assert_eq!(
                rows(db, "SELECT * FROM row_read_test", &[]).unwrap(),
                vec![expected]
            );
            assert!(rows(db, "SELECT * FROM nonexistent_table", &[]).is_err());
            let error = rows(db, "SELECT x'ff' AS unsupported", &[]).unwrap_err();
            assert_eq!(error.to_string(), "Unexpected blob in fleet row");
            assert!(is_conflict(error.as_ref()));
        }
        drop(remote);
        owner.stop();
    }

    #[test]
    fn subtask_receipt_replays_batch_the_final_graph_projection() {
        let mut work = Vec::new();
        for count in [16, 128] {
            let f = Fixture::new();
            f.db.execute("WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x+1 FROM n WHERE x<=?1) INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) SELECT 'named:Native fleet',x,'Child','','open','human:fixture',0,0,1,'[]',x FROM n", [count]).unwrap();
            f.db.execute_batch("INSERT INTO issue_subtasks SELECT project_id,1,number,0,'human:fixture' FROM issues WHERE number%2=0").unwrap();
            let changes: Vec<_> = (1..=count).map(|seq| {
                f.db.execute("INSERT INTO fleet_receipts VALUES('peer',?1,'{\"state\":\"applied\"}')", [seq]).unwrap();
                let child = if seq%3==0 { json!((seq+1).to_string()) } else { json!(seq+1) };
                let row = json!({"project_id":"named:Native fleet","child_number":child,"parent_number":1,"created_at":0,"created_by":"human:fixture"});
                json!({"seq":seq,"table_name":"issue_subtasks","before_json":row.to_string(),"after_json":null})
            }).collect();
            let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
            let (db, transport) = crate::database::tests::measured_connection(&f.path);
            let receipts = accept_changes(&db, "peer", &changes).unwrap();
            drop(db);
            let (commands, steps) = transport.join().unwrap();
            owner.stop();
            for (change, receipt) in changes.iter().zip(&receipts) {
                let key = row_json(change, "before_json").unwrap();
                assert_eq!(
                    receipt["canonical_subtask"],
                    json!({"project_id":key["project_id"],"child_number":key["child_number"],"row":current_row(&f.db,"issue_subtasks",&key).unwrap()})
                );
            }
            eprintln!("{count} replayed subtask receipts: {commands} RPCs, {steps} query VM steps");
            work.push((count, commands));
        }
        assert!(
            work.iter()
                .all(|(count, commands)| *commands <= *count as usize + 4),
            "Repeated canonical graph reads: {work:?}"
        );
    }

    #[test]
    fn subtask_receipts_all_project_the_last_relationship_in_a_batch() {
        let f = Fixture::new();
        f.db.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) VALUES('named:Native fleet',2,'Child','','open','human:fixture',0,0,1,'[]',2),('named:Native fleet',3,'Other parent','','open','human:fixture',0,0,1,'[]',3)").unwrap();
        let first = json!({"project_id":"named:Native fleet","parent_number":1,"child_number":2,"created_at":0,"created_by":"human:fixture"});
        let mut last = first.clone();
        last["parent_number"] = json!(3);
        let changes = vec![
            json!({"seq":1,"table_name":"issue_subtasks","before_json":null,"after_json":first.to_string()}),
            json!({"seq":2,"table_name":"issue_subtasks","before_json":first.to_string(),"after_json":null}),
            json!({"seq":3,"table_name":"issue_subtasks","before_json":null,"after_json":last.to_string()}),
        ];
        let receipts = accept_changes(&f.db, "peer", &changes).unwrap();
        assert!(
            receipts.iter().all(|receipt| receipt["state"] == "applied"),
            "{receipts:?}"
        );
        for receipt in &receipts {
            assert_eq!(receipt["canonical_subtask"]["row"], last);
        }
        assert_eq!(accept_changes(&f.db, "peer", &changes).unwrap(), receipts);
        f.db.execute("DELETE FROM issue_subtasks", []).unwrap();
        assert!(
            accept_changes(&f.db, "peer", &changes)
                .unwrap()
                .iter()
                .all(|receipt| receipt["canonical_subtask"]["row"].is_null())
        );
    }

    #[test]
    fn applied_receipt_batches_do_not_read_and_delete_each_journal_row() {
        let mut work = Vec::new();
        for count in [16, 128] {
            let f = Fixture::new();
            install_capture(&f.db, "agent", "peer").unwrap();
            f.db.execute("DELETE FROM fleet_outbox", []).unwrap();
            f.db.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<=?1) INSERT INTO agents SELECT 'human:ack-'||x,?2,0 FROM n", rusqlite::params![count,json!({"padding":"x".repeat(4096)}).to_string()]).unwrap();
            let pending = journal(&f.db, 0).unwrap();
            assert_eq!(pending.len(), count as usize + 1);
            let receipts: Vec<_> = pending
                .iter()
                .take(count as usize)
                .map(|row| json!({"seq":row["seq"],"state":"applied"}))
                .collect();
            let payload = json!({"cursor":1,"allocations":[],"ranges":[]});
            let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
            for replay in [false, true] {
                let (db, transport) = crate::database::tests::measured_connection(&f.path);
                apply_pull(&db, "peer", &payload, &receipts).unwrap();
                drop(db);
                let (commands, steps) = transport.join().unwrap();
                eprintln!(
                    "{count} applied receipts, replay={replay}: {commands} RPCs, {steps} query VM steps"
                );
                work.push(commands);
                assert_eq!(
                    journal(&f.db, 0).unwrap(),
                    vec![pending.last().unwrap().clone()]
                );
            }
            owner.stop();
        }
        assert!(
            work.iter().all(|commands| *commands <= 14),
            "Per-receipt database calls remain: {work:?}"
        );
    }

    #[test]
    fn pull_acknowledgments_keep_duplicate_order_append_mappings_and_atomicity() {
        let f = Fixture::new();
        install_capture(&f.db, "agent", "peer").unwrap();
        f.db.execute("DELETE FROM fleet_outbox", []).unwrap();
        f.db.execute_batch("INSERT INTO agents VALUES('human:ack-one','{}',0),('human:ack-two','{}',0);
            INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Native fleet',1,'human:fixture','Comment',123);").unwrap();
        let pending = journal(&f.db, 0).unwrap();
        assert_eq!(pending.len(), 3);
        let one = &pending[0]["seq"];
        let two = &pending[1]["seq"];
        let comment = &pending[2]["seq"];
        let receipts = vec![
            json!({"seq":one,"state":"applied"}),
            json!({"seq":one,"state":"conflict","reason":"Ignored duplicate"}),
            json!({"seq":two,"state":"conflict","reason":"Saved conflict"}),
            json!({"seq":two,"state":"conflict","reason":"Ignored duplicate"}),
            json!({"seq":comment,"state":"applied","canonical_append":{"origin":"main","origin_id":101}}),
        ];
        assert!(
            apply_pull(
                &f.db,
                "peer",
                &json!({"cursor":1,"allocations":[]}),
                &receipts
            )
            .is_err()
        );
        assert_eq!(journal(&f.db, 0).unwrap(), pending);
        assert!(
            rows(&f.db, "SELECT * FROM fleet_conflicts", &[])
                .unwrap()
                .is_empty()
        );
        assert!(
            rows(
                &f.db,
                "SELECT * FROM fleet_row_ids WHERE origin='main'",
                &[]
            )
            .unwrap()
            .is_empty()
        );
        let payload = json!({"cursor":1,"allocations":[],"ranges":[]});
        apply_pull(&f.db, "peer", &payload, &receipts).unwrap();
        assert!(journal(&f.db, 0).unwrap().is_empty());
        let conflicts = rows(&f.db, "SELECT seq,reason FROM fleet_conflicts", &[]).unwrap();
        assert_eq!(
            conflicts,
            vec![json!({"seq":two,"reason":"Saved conflict"})]
        );
        let mappings = rows(&f.db, "SELECT origin_id,local_id FROM fleet_row_ids WHERE origin='main' AND table_name='comments'", &[]).unwrap();
        assert_eq!(
            mappings,
            vec![
                json!({"origin_id":101,"local_id":row_json(&pending[2], "after_json").unwrap()["id"]})
            ]
        );
        apply_pull(&f.db, "peer", &payload, &receipts).unwrap();
        assert_eq!(
            rows(&f.db, "SELECT seq,reason FROM fleet_conflicts", &[]).unwrap(),
            conflicts
        );

        f.db.execute("INSERT INTO agents VALUES('human:ack-legacy','{}',0)", [])
            .unwrap();
        let legacy = journal(&f.db, 0).unwrap();
        let sequence = legacy[0]["seq"].as_i64().unwrap();
        apply_pull(
            &f.db,
            "peer",
            &payload,
            &[
                json!({"seq":sequence.to_string(),"state":"applied"}),
                json!({"seq":sequence,"state":"conflict","reason":"Ignored legacy duplicate"}),
            ],
        )
        .unwrap();
        assert!(journal(&f.db, 0).unwrap().is_empty());
        assert_eq!(
            rows(&f.db, "SELECT seq,reason FROM fleet_conflicts", &[]).unwrap(),
            conflicts
        );
    }

    #[test]
    fn incoming_change_batch_reuses_schema_and_keeps_receipts_replayable() {
        let f = Fixture::new();
        f.db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO fleet_receipts SELECT 'peer',10000+x,'{\"state\":\"applied\"}' FROM n").unwrap();
        let template = rows(&f.db, "SELECT * FROM agents WHERE id='human:fixture'", &[])
            .unwrap()
            .remove(0);
        let changes: Vec<_> = (1..=128).map(|number| {
            let mut row = template.clone();
            row["id"] = json!(format!("human:peer{number}"));
            json!({"seq":number,"table_name":"agents","before_json":null,"after_json":row.to_string()})
        }).collect();
        let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
        let (db, transport) = crate::database::tests::measured_connection(&f.path);
        let receipts = accept_changes(&db, "peer", &changes).unwrap();
        drop(db);
        let (commands, steps) = transport.join().unwrap();
        let (db, transport) = crate::database::tests::measured_connection(&f.path);
        let replay = accept_changes(&db, "peer", &changes).unwrap();
        drop(db);
        let (replay_commands, replay_steps) = transport.join().unwrap();
        owner.stop();
        assert!(receipts.iter().all(|receipt| receipt["state"] == "applied"));
        assert_eq!(replay, receipts);
        assert_eq!(
            rows(
                &f.db,
                "SELECT count(*) count FROM fleet_receipts WHERE node='peer'",
                &[]
            )
            .unwrap()[0]["count"],
            20128
        );
        assert_eq!(
            rows(
                &f.db,
                "SELECT count(*) count FROM agents WHERE id LIKE 'human:peer%'",
                &[]
            )
            .unwrap()[0]["count"],
            128
        );
        eprintln!("128 incoming agent changes: {commands} RPCs, {steps} VM steps");
        eprintln!("128 replayed receipts: {replay_commands} RPCs, {replay_steps} query VM steps");
        assert!(commands <= 128 * 5 + 4, "{commands} RPCs");
        assert!(replay_commands <= 3, "{replay_commands} replay RPCs");
        assert!(
            replay_steps <= 5000,
            "Receipt lookup scanned unrelated history: {replay_steps} query VM steps"
        );
    }

    #[test]
    fn receipt_batches_preserve_duplicates_conflicts_sender_scope_and_rollback() {
        let f = Fixture::new();
        let template = rows(&f.db, "SELECT * FROM agents WHERE id='human:fixture'", &[])
            .unwrap()
            .remove(0);
        let change = |seq: Value, id: &str, seen: i64| {
            let mut row = template.clone();
            row["id"] = json!(id);
            row["last_seen"] = json!(seen);
            json!({"seq":seq,"table_name":"agents","before_json":null,"after_json":row.to_string()})
        };
        f.db.execute_batch("INSERT INTO fleet_receipts VALUES('peer',2,'{\"state\":\"conflict\",\"reason\":\"Saved conflict\"}'),('other',1,'{\"state\":\"conflict\"}')").unwrap();
        let batch = vec![
            change(json!(1), "human:one", 10),
            change(json!(1), "human:one", 20),
            change(json!(2), "human:ignored", 30),
            change(json!(3), "human:two", 40),
        ];
        let receipts = accept_changes(&f.db, "peer", &batch).unwrap();
        assert_eq!(receipts[0], receipts[1]);
        assert_eq!(receipts[0]["state"], "applied");
        assert_eq!(
            receipts[2],
            json!({"state":"conflict","reason":"Saved conflict","seq":2})
        );
        assert_eq!(
            rows(
                &f.db,
                "SELECT last_seen FROM agents WHERE id='human:one'",
                &[]
            )
            .unwrap()[0]["last_seen"],
            10
        );
        assert!(
            rows(&f.db, "SELECT * FROM agents WHERE id='human:ignored'", &[])
                .unwrap()
                .is_empty()
        );
        assert_eq!(accept_changes(&f.db, "peer", &batch).unwrap(), receipts);

        let first = change(json!(4), "human:rollback", 50);
        let invalid = json!({"seq":5,"table_name":"agents","before_json":null,"after_json":"{"});
        assert!(accept_changes(&f.db, "peer", &[first.clone(), invalid]).is_err());
        assert!(
            rows(&f.db, "SELECT * FROM agents WHERE id='human:rollback'", &[])
                .unwrap()
                .is_empty()
        );
        assert!(
            rows(
                &f.db,
                "SELECT * FROM fleet_receipts WHERE node='peer' AND seq=4",
                &[]
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            accept_changes(&f.db, "peer", &[first]).unwrap()[0]["state"],
            "applied"
        );

        // SQLite's integer affinity historically accepts numeric strings and
        // treats them as the same receipt key as an integer sequence.
        let legacy = [
            change(json!("6"), "human:legacy", 60),
            change(json!(6), "human:legacy", 70),
        ];
        let receipts = accept_changes(&f.db, "peer", &legacy).unwrap();
        assert_eq!(receipts[0]["seq"], "6");
        assert_eq!(receipts[1]["seq"], 6);
        assert_eq!(
            rows(
                &f.db,
                "SELECT last_seen FROM agents WHERE id='human:legacy'",
                &[]
            )
            .unwrap()[0]["last_seen"],
            60
        );
    }

    #[test]
    fn replication_batches_read_table_schema_once() {
        let mut measurements = Vec::new();
        for incremental in [false, true] {
            for count in [16, 128] {
                let f = Fixture::new();
                install_capture(&f.db, "agent", "peer").unwrap();
                f.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose) VALUES('named:Native fleet',1,'https://github.com/o/r/pull/0','human:fixture',123,'fix')", []).unwrap();
                f.db.execute("DELETE FROM fleet_outbox", []).unwrap();
                let template = rows(&f.db, "SELECT * FROM issue_pull_requests", &[])
                    .unwrap()
                    .remove(0);
                let records: Vec<_> = (0..count)
                    .map(|number| {
                        let mut row = template.clone();
                        row["url"] = json!(format!("https://github.com/o/r/pull/{number}"));
                        row["pr_title"] = json!(format!("Change {number}"));
                        row
                    })
                    .collect();
                let mut payload = json!({"cursor":count,"allocations":[],"ranges":[]});
                if incremental {
                    payload["changes"] = json!(records.iter().enumerate().map(|(index,row)| json!({"seq":index+1,"table_name":"issue_pull_requests","before_json":null,"after_json":row.to_string()})).collect::<Vec<_>>());
                } else {
                    payload["tables"] = json!({"issue_pull_requests":records});
                }
                let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
                let (db, transport) = crate::database::tests::measured_connection(&f.path);
                apply_pull(&db, "peer", &payload, &[]).unwrap();
                drop(db);
                let (commands, steps) = transport.join().unwrap();
                owner.stop();
                eprintln!(
                    "{count} PRs, incremental={incremental}: {commands} RPCs, {steps} VM steps"
                );
                measurements.push((count, commands));
                for row in records {
                    assert_eq!(
                        current_row(&f.db, "issue_pull_requests", &row).unwrap(),
                        row
                    );
                }
                assert_eq!(state_get(&f.db, "cursor", json!(0)).unwrap(), count);
                assert!(
                    rows(&f.db, "SELECT * FROM fleet_outbox", &[])
                        .unwrap()
                        .is_empty()
                );
            }
        }
        for (count, commands) in measurements {
            assert!(commands <= count + 40, "{count} rows: {commands} RPCs");
        }
    }

    #[test]
    fn replicated_schema_changes_are_checked_again_for_each_batch() {
        let f = Fixture::new();
        install_capture(&f.db, "agent", "peer").unwrap();
        f.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at) VALUES('named:Native fleet',1,'https://github.com/o/r/pull/1','human:fixture',123)", []).unwrap();
        f.db.execute("DELETE FROM fleet_outbox", []).unwrap();
        let mut row = rows(&f.db, "SELECT * FROM issue_pull_requests", &[])
            .unwrap()
            .remove(0);
        let payload = |cursor, row: &Value| json!({"cursor":cursor,"allocations":[],"ranges":[],"tables":{"issue_pull_requests":[row]}});
        apply_pull(&f.db, "peer", &payload(1, &row), &[]).unwrap();
        f.db.execute_batch("ALTER TABLE issue_pull_requests ADD COLUMN future_field TEXT")
            .unwrap();
        assert!(
            apply_pull(&f.db, "peer", &payload(2, &row), &[])
                .unwrap_err()
                .to_string()
                .contains("Schema mismatch")
        );
        assert_eq!(state_get(&f.db, "cursor", json!(0)).unwrap(), 1);
        row["future_field"] = json!("New schema");
        apply_pull(&f.db, "peer", &payload(2, &row), &[]).unwrap();
        let mut invalid = row.clone();
        invalid["unknown_field"] = Value::Null;
        let invalid_batch = json!({"cursor":3,"allocations":[],"ranges":[],"tables":{"issue_pull_requests":[row,invalid]}});
        assert!(
            apply_pull(&f.db, "peer", &invalid_batch, &[])
                .unwrap_err()
                .to_string()
                .contains("Schema mismatch")
        );
        assert_eq!(state_get(&f.db, "cursor", json!(0)).unwrap(), 2);
        assert_eq!(
            current_row(&f.db, "issue_pull_requests", &row).unwrap(),
            row
        );
    }

    #[test]
    fn modern_replication_rows_do_not_read_compatibility_values() {
        let f = Fixture::new();
        f.db.execute_batch("INSERT INTO project_settings(project_id,prompt,version,chief_prompt) VALUES('named:Native fleet','Work',1,'Old');
            INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose,error) VALUES('named:Native fleet',1,'https://github.com/o/r/pull/1','human:fixture',123,'fix','Old');
            UPDATE global_settings SET github_user_id=42;").unwrap();
        let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
        let mut measurements = Vec::new();
        for (table, nullable) in [
            ("project_settings", "chief_prompt"),
            ("issue_pull_requests", "error"),
            ("global_settings", "github_user_id"),
        ] {
            let mut row = rows(&f.db, &format!("SELECT * FROM {table}"), &[])
                .unwrap()
                .remove(0);
            row[nullable] = Value::Null;
            let (db, transport) = crate::database::tests::measured_connection(&f.path);
            db.execute_batch("BEGIN IMMEDIATE").unwrap();
            for _ in 0..128 {
                put_row(&db, table, &row).unwrap();
            }
            db.execute_batch("COMMIT").unwrap();
            drop(db);
            let (commands, steps) = transport.join().unwrap();
            eprintln!("128 {table} writes: {commands} RPCs, {steps} VM steps");
            measurements.push((table, commands));
            assert_eq!(current_row(&f.db, table, &row).unwrap(), row);
        }
        owner.stop();
        for (table, commands) in measurements {
            // Schema query and the write per row, plus BEGIN/COMMIT.
            assert!(commands <= 128 * 2 + 2, "{table}: {commands} RPCs");
        }
    }

    #[test]
    fn legacy_pr_compatibility_uses_one_read_and_preserves_explicit_nulls() {
        let f = Fixture::new();
        f.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose,status,checked_at,error,pr_title,author_id) VALUES('named:Native fleet',1,'https://github.com/o/r/pull/1','human:fixture',123,'fix','open',456,'Old error','Title',42)", []).unwrap();
        let mut expected = rows(&f.db, "SELECT * FROM issue_pull_requests", &[])
            .unwrap()
            .remove(0);
        expected["error"] = Value::Null;
        let mut legacy = expected.clone();
        for column in [
            "purpose",
            "status",
            "checked_at",
            "merged_at",
            "pr_title",
            "author_id",
        ] {
            legacy.as_object_mut().unwrap().remove(column);
        }
        let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
        let (db, transport) = crate::database::tests::measured_connection(&f.path);
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        put_row(&db, "issue_pull_requests", &legacy).unwrap();
        db.execute_batch("COMMIT").unwrap();
        drop(db);
        let (commands, _) = transport.join().unwrap();
        owner.stop();
        assert_eq!(
            current_row(&f.db, "issue_pull_requests", &legacy).unwrap(),
            expected
        );
        assert!(commands <= 5, "Legacy PR write: {commands} RPCs");
    }

    #[test]
    fn legacy_settings_replay_preserves_fields_the_sender_does_not_know() {
        let f = Fixture::new();
        f.db.execute("INSERT INTO project_settings(project_id,prompt,version,chief_enabled,chief_prompt,worktree_enabled) VALUES('named:Native fleet','Work',1,1,'Organize',1)", []).unwrap();
        let legacy = json!({"project_id":"named:Native fleet","prompt":"Updated","prs_enabled":0,"version":2,"boss_name":"Boss"});
        put_row(&f.db, "project_settings", &legacy).unwrap();
        assert_eq!(rows(&f.db, "SELECT chief_enabled,chief_prompt,worktree_enabled,prompt,version FROM project_settings", &[]).unwrap()[0], json!({"chief_enabled":1,"chief_prompt":"Organize","worktree_enabled":1,"prompt":"Updated","version":2}));
    }

    #[test]
    fn legacy_project_settings_replay_uses_migration_defaults() {
        let main = Fixture::new();
        main.capture();
        main.db.execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,boss_name) VALUES('named:Native fleet','Instructions',0,1,'Boss')", []).unwrap();
        let mut expected =
            rows(&main.db, "SELECT * FROM project_settings", &[]).unwrap()[0].clone();
        // Older senders cannot restore implicit sequential ordering.
        expected["subtask_scheduling"] = json!("explicit");
        let mut legacy = expected.clone();
        for column in [
            "subtask_scheduling",
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
    fn legacy_issue_rows_cannot_erase_an_existing_draft_or_plan() {
        let f = Fixture::new();
        let mut legacy = current_row(
            &f.db,
            "issues",
            &json!({"project_id":"named:Native fleet","number":1}),
        )
        .unwrap();
        legacy.as_object_mut().unwrap().remove("draft");
        legacy.as_object_mut().unwrap().remove("plan");
        f.db.execute(
            "UPDATE issues SET draft=1,plan='plans/retained.md' WHERE number=1",
            [],
        )
        .unwrap();
        put_row(&f.db, "issues", &legacy).unwrap();
        let retained = current_row(&f.db, "issues", &legacy).unwrap();
        assert_eq!(
            retained["draft"], 1,
            "a sender predating drafts cannot undraft an issue"
        );
        assert_eq!(retained["plan"], "plans/retained.md");
        assert_eq!(retained["version"], legacy["version"]);
        let mut modern = retained;
        modern["draft"] = json!(0);
        modern["plan"] = Value::Null;
        put_row(&f.db, "issues", &modern).unwrap();
        let cleared = current_row(&f.db, "issues", &modern).unwrap();
        assert_eq!(cleared["draft"], 0, "an explicit undraft still applies");
        assert!(cleared["plan"].is_null());
    }

    #[test]
    fn acknowledged_draft_and_close_reopen_state_survives_repeated_replication() {
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
        for (draft, state) in [(1, "open"), (0, "open"), (0, "closed"), (0, "open")] {
            // Each transition is an acknowledged issue-row mutation, not merely
            // an event or a CLI receipt from an unaccepted offline write.
            main.db.execute("INSERT OR REPLACE INTO fleet_allocations VALUES('named:Native fleet',1,'agent')", []).unwrap();
            agent
                .db
                .execute(
                    "UPDATE issues SET draft=?1,state=?2,version=version+1",
                    rusqlite::params![draft, state],
                )
                .unwrap();
            let changes = journal(&agent.db, 0).unwrap();
            assert!(!changes.is_empty());
            let receipts = accept_changes(&main.db, "agent", &changes).unwrap();
            assert!(
                receipts.iter().all(|r| r["state"] == "applied"),
                "{receipts:?}"
            );
            assert_eq!(
                accept_changes(&main.db, "agent", &changes).unwrap(),
                receipts
            );
            for _ in 0..2 {
                apply_pull(
                    &agent.db,
                    "agent",
                    &snapshot(&main.db, "agent").unwrap(),
                    &receipts,
                )
                .unwrap();
                let rows = rows(
                    &agent.db,
                    "SELECT draft,state,version FROM issues WHERE number=1",
                    &[],
                )
                .unwrap();
                assert_eq!(rows[0]["draft"], draft);
                assert_eq!(rows[0]["state"], state);
                assert_eq!(
                    rows,
                    super::rows(
                        &main.db,
                        "SELECT draft,state,version FROM issues WHERE number=1",
                        &[]
                    )
                    .unwrap()
                );
            }
            assert!(journal(&agent.db, 0).unwrap().is_empty());
        }
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
    fn stale_snapshot_after_acknowledgment_cannot_revert_metadata() {
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
        let stale = snapshot(&main.db, "agent").unwrap();
        apply_pull(&agent.db, "agent", &stale, &[]).unwrap();
        agent
            .db
            .execute(
                "UPDATE issues SET labels='[\"accepted\"]',state='closed',version=version+1",
                [],
            )
            .unwrap();
        let changes = journal(&agent.db, 0).unwrap();
        let receipts = accept_changes(&main.db, "agent", &changes).unwrap();
        assert!(receipts.iter().all(|r| r["state"] == "applied"));
        let current = snapshot(&main.db, "agent").unwrap();
        apply_pull(&agent.db, "agent", &current, &receipts).unwrap();
        assert!(journal(&agent.db, 0).unwrap().is_empty());
        assert!(apply_pull(&agent.db, "agent", &stale, &receipts).is_err());
        assert_eq!(
            state_get(&agent.db, "cursor", Value::Null).unwrap(),
            current["cursor"]
        );
        assert_eq!(
            snapshot(&agent.db, "agent").unwrap()["tables"]["issues"],
            current["tables"]["issues"]
        );
        // A legitimate later write still wins and advances the cursor.
        main.db
            .execute(
                "UPDATE issues SET labels='[\"later\"]',state='open',version=version+1",
                [],
            )
            .unwrap();
        let later = snapshot(&main.db, "agent").unwrap();
        assert!(later["cursor"].as_i64() > current["cursor"].as_i64());
        apply_pull(&agent.db, "agent", &later, &[]).unwrap();
        assert_eq!(
            snapshot(&agent.db, "agent").unwrap()["tables"]["issues"],
            later["tables"]["issues"]
        );
    }

    #[test]
    fn concurrent_metadata_comments_and_lifecycle_converge_with_drained_journals() {
        let main = Fixture::new();
        main.capture();
        main.db
            .execute(
                "INSERT INTO fleet_allocations VALUES('named:Native fleet',1,'agent')",
                [],
            )
            .unwrap();
        let agent = Fixture::new();
        agent.db.execute("DELETE FROM events", []).unwrap();
        install_capture(&agent.db, "agent", "agent").unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        let request = |mut operation: Value, id: &str| -> Request {
            if operation["action"] == "edit" {
                operation
                    .as_object_mut()
                    .unwrap()
                    .entry("add_labels")
                    .or_insert(json!([]));
                operation
                    .as_object_mut()
                    .unwrap()
                    .entry("remove_labels")
                    .or_insert(json!([]));
            }
            if operation["action"] == "close" {
                operation["force"] = json!(false);
            }
            serde_json::from_value(json!({"version":1,"project":{"id":"named:Native fleet","name":"Native fleet"},
                "actor":{"id":"human:fixture","kind":"human","machine":"agent","host":"fixture","cwd":"/tmp","source":"test"},
                "operation":operation,"request_id":id})).unwrap()
        };
        let run = |f: &Fixture, operation: Value, id: &str| {
            Store::open(&f.path)
                .unwrap()
                .execute(&request(operation, id))
                .unwrap()
        };
        let sync = || {
            let changes = journal(&agent.db, 0).unwrap();
            let receipts = accept_changes(&main.db, "agent", &changes).unwrap();
            assert!(
                receipts.iter().all(|r| r["state"] == "applied"),
                "{receipts:?}"
            );
            assert_eq!(
                accept_changes(&main.db, "agent", &changes).unwrap(),
                receipts
            );
            let cursor = state_get(&agent.db, "cursor", json!(0)).unwrap();
            let payload = incremental(&main.db, "agent", cursor.as_i64().unwrap()).unwrap();
            assert!(payload["cursor"].as_i64() > cursor.as_i64());
            apply_pull(&agent.db, "agent", &payload, &receipts).unwrap();
            assert!(journal(&agent.db, 0).unwrap().is_empty());
            assert_eq!(
                rows(
                    &agent.db,
                    "SELECT title,body,labels,state,assignee,version FROM issues",
                    &[]
                )
                .unwrap(),
                rows(
                    &main.db,
                    "SELECT title,body,labels,state,assignee,version FROM issues",
                    &[]
                )
                .unwrap()
            );
            for table in ["comments", "events"] {
                assert_eq!(
                    rows(
                        &agent.db,
                        &format!("SELECT count(*) count FROM {table}"),
                        &[]
                    )
                    .unwrap(),
                    rows(
                        &main.db,
                        &format!("SELECT count(*) count FROM {table}"),
                        &[]
                    )
                    .unwrap()
                );
            }
        };
        let edit = json!({"action":"edit","number":1,"add_labels":["accepted"],"if_version":1});
        let accepted = run(&agent, edit.clone(), "offline-label");
        assert_eq!(run(&agent, edit, "offline-label"), accepted);
        run(
            &main,
            json!({"action":"edit","number":1,"title":"Later requirements"}),
            "online-title",
        );
        run(
            &agent,
            json!({"action":"comment","number":1,"body":"Companion comment"}),
            "offline-comment",
        );
        run(
            &main,
            json!({"action":"comment","number":1,"body":"Supervisor comment"}),
            "online-comment",
        );
        sync();
        run(
            &agent,
            json!({"action":"close","number":1,"comment":"Completed"}),
            "offline-close",
        );
        sync();
        let version =
            rows(&agent.db, "SELECT version FROM issues", &[]).unwrap()[0]["version"].clone();
        run(
            &agent,
            json!({"action":"reopen","number":1,"if_version":version}),
            "offline-reopen",
        );
        sync();
        assert_eq!(
            rows(&main.db, "SELECT state,labels FROM issues", &[]).unwrap()[0],
            json!({"state":"open","labels":"[\"accepted\"]"})
        );
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
    fn overlapping_allocation_filters_still_fill_distinct_machine_slots() {
        let f = Fixture::new();
        f.db.execute_batch("UPDATE issues SET labels='[\"a\",\"b\"]';
            WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x+1 FROM n WHERE x<4)
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
            SELECT 'named:Native fleet',x,'Queued','','open','human:fixture',0,0,1,'[\"a\",\"b\"]',x FROM n;
            UPDATE projects SET next_number=5 WHERE id='named:Native fleet';").unwrap();
        f.capture();
        for number in 1..=2 {
            f.db.execute(
                "INSERT INTO fleet_allocations VALUES('named:Native fleet',?1,'agent')",
                [number],
            )
            .unwrap();
        }
        let workers = vec![
            json!({"config":{"projects":["named:Native fleet"],"concurrency":1,"tags":["a"],"enabled":true}}),
            json!({"config":{"projects":["named:Native fleet"],"concurrency":1,"tags":["b"],"enabled":true}}),
        ];
        allocate(&f.db, "agent", &workers).unwrap();
        let supplied = rows(
            &f.db,
            "SELECT issue_number FROM fleet_allocations WHERE node='agent' ORDER BY issue_number",
            &[],
        )
        .unwrap();
        assert_eq!(
            supplied,
            vec![
                json!({"issue_number":1}),
                json!({"issue_number":2}),
                json!({"issue_number":3}),
                json!({"issue_number":4})
            ]
        );
    }

    #[test]
    fn fleet_allocates_independent_subtasks_and_syncs_progress() {
        let main = Fixture::new();
        main.db.execute("INSERT INTO project_settings(project_id,prompt,version,subtask_scheduling) VALUES('named:Native fleet',?1,1,'sequential')", [crate::issues::worker::DEFAULT_PROMPT]).unwrap();
        main.db.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
            VALUES('named:Native fleet',2,'First','','open','human:fixture',0,0,1,'[]',2),
                  ('named:Native fleet',3,'Second','','open','human:fixture',0,0,1,'[]',3);
            INSERT INTO issue_subtasks VALUES('named:Native fleet',1,2,0,'human:fixture'),('named:Native fleet',1,3,0,'human:fixture');").unwrap();
        main.capture();
        crate::issues::blockers::reconcile_all(&main.db).unwrap();
        let workers = vec![
            json!({"config":{"projects":["named:Native fleet"],"concurrency":3,"tags":[],"enabled":true}}),
        ];
        allocate(&main.db, "agent", &workers).unwrap();
        allocate(&main.db, "other", &workers).unwrap();
        let allocated = rows(
            &main.db,
            "SELECT issue_number FROM fleet_allocations ORDER BY issue_number",
            &[],
        )
        .unwrap();
        assert_eq!(
            allocated,
            vec![json!({"issue_number":2}), json!({"issue_number":3})]
        );
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
            rows(&agent.db, "SELECT state FROM issues WHERE number=3", &[]).unwrap()[0]["state"],
            "open"
        );
        main.db
            .execute("UPDATE issues SET state='closed' WHERE number=2", [])
            .unwrap();
        crate::issues::blockers::reconcile_all(&main.db).unwrap();
        allocate(&main.db, "agent", &workers).unwrap();
        apply_pull(
            &agent.db,
            "agent",
            &snapshot(&main.db, "agent").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            rows(&main.db, "SELECT a.issue_number FROM fleet_allocations a JOIN issues i ON i.project_id=a.project_id AND i.number=a.issue_number WHERE i.state='open'", &[]).unwrap(),
            vec![json!({"issue_number":3})]
        );
        assert_eq!(
            rows(&agent.db, "SELECT number FROM issue_pickup_ready", &[]).unwrap(),
            vec![json!({"number":3})]
        );
    }

    #[test]
    fn a_full_allocation_pool_does_not_scan_the_unallocated_queue() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        unsafe extern "C" fn count_steps(context: *mut std::ffi::c_void) -> std::ffi::c_int {
            unsafe { &*context.cast::<AtomicUsize>() }.fetch_add(100, Ordering::Relaxed);
            0
        }
        let f = Fixture::new();
        f.db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x+1 FROM n WHERE x<10000)
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
            SELECT 'named:Native fleet',x,'Queued','','open','human:fixture',0,0,1,'[]',x FROM n;
            UPDATE projects SET next_number=10001 WHERE id='named:Native fleet';").unwrap();
        f.capture();
        let workers = vec![
            json!({"config":{"projects":["named:Native fleet"],"concurrency":2,"enabled":true}}),
        ];
        allocate(&f.db, "agent", &workers).unwrap();
        let before = rows(
            &f.db,
            "SELECT * FROM fleet_allocations ORDER BY issue_number",
            &[],
        )
        .unwrap();
        assert_eq!(before.len(), 4);
        let steps = AtomicUsize::new(0);
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                f.db.handle(),
                100,
                Some(count_steps),
                (&steps as *const AtomicUsize).cast_mut().cast(),
            );
        }
        let result = allocate(&f.db, "agent", &workers);
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(f.db.handle(), 0, None, std::ptr::null_mut());
        }
        result.unwrap();
        assert_eq!(
            rows(
                &f.db,
                "SELECT * FROM fleet_allocations ORDER BY issue_number",
                &[]
            )
            .unwrap(),
            before
        );
        let steps = steps.load(Ordering::Relaxed);
        eprintln!(
            "Full allocation pool over 10,000 queued issues: fewer than {} VM steps",
            steps + 100
        );
        assert!(
            steps < 20_000,
            "A full pool scanned queued issues: {steps} VM steps"
        );
    }

    #[test]
    fn allocation_planning_batches_project_reads_for_full_and_empty_pools() {
        let mut measurements = Vec::new();
        for count in [16, 128] {
            let f = Fixture::new();
            f.capture();
            f.db.execute_batch(&format!("WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<{count})
                INSERT INTO projects(id,name,next_number) SELECT 'named:Batch '||id,'Batch '||id,1000 FROM n;
                INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
                SELECT id,n.number,'Queued','','open','human:fixture',0,0,1,'[]',3-n.number FROM projects CROSS JOIN (SELECT 1 AS number UNION ALL SELECT 2) n WHERE id LIKE 'named:Batch %';
                INSERT INTO fleet_ranges SELECT 'peer',id,100,199 FROM projects WHERE id LIKE 'named:Batch %';
                INSERT INTO fleet_allocations SELECT project_id,number,'peer' FROM issues WHERE project_id LIKE 'named:Batch %';")).unwrap();
            let projects: Vec<_> = (1..=count).map(|id| format!("named:Batch {id}")).collect();
            let workers =
                vec![json!({"config":{"projects":projects,"concurrency":1,"enabled":true}})];
            let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
            for full in [true, false] {
                if !full {
                    f.db.execute("DELETE FROM fleet_allocations", []).unwrap();
                }
                let (db, transport) = crate::database::tests::measured_connection(&f.path);
                let plan = plan_allocations(&db, "peer", &workers).unwrap();
                drop(db);
                let (commands, steps) = transport.join().unwrap();
                assert!(plan.ranges.is_empty());
                let expected = if full {
                    Vec::new()
                } else {
                    let mut projects = projects.clone();
                    projects.sort();
                    projects
                        .into_iter()
                        .flat_map(|project| [(project.clone(), 2), (project, 1)])
                        .collect()
                };
                assert_eq!(plan.tasks, expected);
                eprintln!(
                    "{count} project allocation plans (full={full}): {commands} RPCs, {steps} VM steps"
                );
                measurements.push((count, full, commands));
            }
            owner.stop();
        }
        for (count, full, commands) in measurements {
            assert!(
                commands <= 3,
                "{count} projects (full={full}) needed {commands} RPCs"
            );
        }
    }

    #[test]
    fn allocation_batches_keep_projects_machines_tags_and_range_boundaries_scoped() {
        let f = Fixture::new();
        f.capture();
        f.db.execute_batch("INSERT INTO projects(id,name,next_number,hidden_at) VALUES
            ('named:A','A',1000,NULL),('named:B','B',2000,NULL),('named:C','C',3000,NULL),
            ('named:Hidden','Hidden',4000,1),('named:Paused','Paused',5000,NULL);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
            SELECT p.id,n.number,'Queued','','open','human:fixture',0,0,1,CASE p.id WHEN 'named:B' THEN '[\"b\"]' ELSE '[\"a\"]' END,4-n.number
            FROM projects p CROSS JOIN (SELECT 1 AS number UNION ALL SELECT 2 UNION ALL SELECT 3) n WHERE p.id<>'named:Native fleet';
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,draft)
            VALUES('named:A',179,'Used','','open','human:fixture',0,0,1,'[]',1);
            INSERT INTO fleet_ranges VALUES('peer','named:A',100,199),('peer','named:B',100,199),('other','named:C',100,199);
            INSERT INTO fleet_allocations VALUES('named:A',1,'other'),('named:B',1,'peer');").unwrap();
        let workers = vec![
            json!({"config":{"projects":["named:A","named:Hidden"],"concurrency":1,"tags":["a"],"enabled":true}}),
            json!({"config":{"projects":["named:B"],"concurrency":1,"tags":["b"],"enabled":true}}),
            json!({"config":{"projects":["named:C","named:Missing"],"concurrency":1,"tags":["unmatched"],"enabled":true}}),
            json!({"intent":"pause","config":{"projects":["named:Paused"],"concurrency":1,"enabled":true}}),
        ];
        let plan = plan_allocations(&f.db, "peer", &workers).unwrap();
        assert_eq!(plan.ranges, vec![("named:C".into(), 3000)]);
        assert_eq!(
            plan.tasks,
            vec![
                ("named:A".into(), 3),
                ("named:A".into(), 2),
                ("named:B".into(), 3)
            ]
        );
        f.db.execute(
            "UPDATE issues SET number=180 WHERE project_id='named:A' AND number=179",
            [],
        )
        .unwrap();
        let replenished = plan_allocations(&f.db, "peer", &workers).unwrap();
        assert_eq!(
            replenished.ranges,
            vec![("named:A".into(), 1000), ("named:C".into(), 3000)]
        );
        assert_eq!(replenished.tasks, plan.tasks);
    }

    #[test]
    fn allocation_rechecks_preflight_and_idle_polls_do_not_acquire_writer() {
        use std::sync::mpsc;
        use std::time::Duration;
        let f = Fixture::new();
        f.capture();
        let workers = vec![
            json!({"config":{"projects":["named:Native fleet"],"concurrency":1,"enabled":true}}),
        ];
        let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
        let (entered, waiting) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let (db, transport) = crate::database::tests::pause_before_writer(&f.path, entered, resume);
        let queued_workers = workers.clone();
        let allocation = std::thread::spawn(move || {
            allocate(&db, "peer", &queued_workers).map_err(|e| e.to_string())
        });
        waiting
            .recv_timeout(Duration::from_secs(2))
            .expect("Allocation did not reach the writer");
        let writer = Connection::connect(&f.path).unwrap();
        writer
            .execute_batch(
                "BEGIN IMMEDIATE;
            UPDATE projects SET next_number=50 WHERE id='named:Native fleet';
            UPDATE issues SET draft=1 WHERE project_id='named:Native fleet' AND number=1;
            COMMIT;",
            )
            .unwrap();
        release.send(()).unwrap();
        allocation.join().unwrap().unwrap();
        transport.join().unwrap();
        assert_eq!(
            writer
                .query_row(
                    "SELECT next_number FROM projects WHERE id='named:Native fleet'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            150
        );
        assert_eq!(
            writer
                .query_row(
                    "SELECT first_number FROM fleet_ranges WHERE node='peer'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            50
        );
        assert_eq!(
            writer
                .query_row("SELECT count(*) FROM fleet_allocations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let reader = Connection::connect(&f.path).unwrap();
        let (send, receive) = mpsc::channel();
        let poll = std::thread::spawn(move || {
            send.send(allocate(&reader, "peer", &workers).map_err(|e| e.to_string()))
                .unwrap()
        });
        let progress = receive.recv_timeout(Duration::from_secs(1));
        writer.execute_batch("ROLLBACK").unwrap();
        poll.join().unwrap();
        drop(writer);
        owner.stop();
        progress
            .expect("Idle allocation poll waited for writer")
            .unwrap();
    }

    #[test]
    fn unclaimed_allocations_expire_and_another_machine_can_pick_up() {
        let f = Fixture::new();
        f.capture();
        let workers = vec![
            json!({"config":{"projects":["named:Native fleet"],"concurrency":1,"enabled":true}}),
        ];
        allocate(&f.db, "agent", &workers).unwrap();
        f.db.execute("UPDATE fleet_allocation_deadlines SET expires_at=0", [])
            .unwrap();
        allocate(&f.db, "other", &workers).unwrap();
        assert_eq!(
            rows(&f.db, "SELECT node FROM fleet_allocations", &[]).unwrap()[0]["node"],
            "other"
        );
    }

    #[test]
    fn allocation_heartbeat_batches_changed_deadlines_and_keeps_machine_scope() {
        let mut measurements = Vec::new();
        for count in [16, 128] {
            let f = Fixture::new();
            f.capture();
            f.db.execute_batch(&format!(
                "WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<{})
                 INSERT INTO fleet_allocations SELECT 'named:Native fleet',id,CASE WHEN id={} THEN 'other' ELSE 'peer' END FROM n;
                 UPDATE fleet_allocation_deadlines SET expires_at=100;
                 CREATE TABLE deadline_updates(number INTEGER);
                 CREATE TRIGGER record_deadline_update AFTER UPDATE ON fleet_allocation_deadlines
                 BEGIN INSERT INTO deadline_updates VALUES(NEW.issue_number); END;", count + 3, count + 1
            )).unwrap();
            let mut runs: Vec<_> = (1..=count + 4)
                .map(|number| {
                    json!({
                        "project_id":"named:Native fleet", "number":number,
                        "reservation_expires":1000 + number,
                        "claimed_at":if number == count + 2 { json!(1) } else { Value::Null },
                        "finished_at":if number == count + 3 { json!(1) } else { Value::Null }
                    })
                })
                .collect();
            runs.push(
                json!({"project_id":"named:Native fleet","number":1,"reservation_expires":2001}),
            );
            let workers = [json!({"runs":runs})];
            assert!(allocation_deadlines_changed(&f.db, "peer", &workers).unwrap());
            let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
            let (db, transport) = crate::database::tests::measured_connection(&f.path);
            db.execute_batch("BEGIN IMMEDIATE").unwrap();
            refresh_allocation_deadlines(&db, "peer", &workers).unwrap();
            db.execute_batch("COMMIT").unwrap();
            drop(db);
            let (commands, _) = transport.join().unwrap();
            owner.stop();
            assert!(!allocation_deadlines_changed(&f.db, "peer", &workers).unwrap());
            let changed: i64 =
                f.db.query_row("SELECT count(*) FROM deadline_updates", [], |r| r.get(0))
                    .unwrap();
            f.db.execute("DELETE FROM deadline_updates", []).unwrap();
            refresh_allocation_deadlines(&f.db, "peer", &workers).unwrap();
            let repeated: i64 =
                f.db.query_row("SELECT count(*) FROM deadline_updates", [], |r| r.get(0))
                    .unwrap();
            eprintln!(
                "{count} leases: {commands} owner RPCs, {changed} first writes, {repeated} repeated writes"
            );
            for row in rows(
                &f.db,
                "SELECT issue_number,expires_at FROM fleet_allocation_deadlines",
                &[],
            )
            .unwrap()
            {
                let number = row["issue_number"].as_i64().unwrap();
                assert_eq!(
                    row["expires_at"],
                    if number > count {
                        100
                    } else if number == 1 {
                        2001
                    } else {
                        1000 + number
                    }
                );
            }
            measurements.push((count, commands, changed, repeated));
        }
        for (count, commands, changed, repeated) in measurements {
            assert!(commands <= 4, "{count} leases needed {commands} RPCs");
            assert_eq!(changed, count);
            assert_eq!(repeated, 0, "An unchanged heartbeat rewrote deadlines");
        }
    }

    #[test]
    fn allocation_tracks_the_agent_claim_deadline_and_preserves_claimed_work() {
        let f = Fixture::new();
        f.capture();
        let workers = vec![
            json!({"config":{"projects":["named:Native fleet"],"concurrency":1,"enabled":true}}),
        ];
        allocate(&f.db, "agent", &workers).unwrap();
        let deadline = (super::super::context::now() * 1000.0) as i64 + 123_000;
        refresh_allocation_deadlines(&f.db, "agent", &[json!({"runs":[{"project_id":"named:Native fleet","number":1,"state":"awaiting_claim","finished_at":null,"claimed_at":null,"reservation_expires":deadline}]})]).unwrap();
        assert_eq!(
            rows(
                &f.db,
                "SELECT expires_at FROM fleet_allocation_deadlines",
                &[]
            )
            .unwrap()[0]["expires_at"],
            deadline
        );
        f.db.execute("UPDATE issues SET assignee=created_by", [])
            .unwrap();
        f.db.execute("UPDATE fleet_allocation_deadlines SET expires_at=0", [])
            .unwrap();
        allocate(&f.db, "other", &workers).unwrap();
        assert_eq!(
            rows(&f.db, "SELECT node FROM fleet_allocations", &[]).unwrap()[0]["node"],
            "agent"
        );
    }

    #[test]
    fn unchanged_allocation_pulls_preserve_rows_and_legacy_refresh() {
        let mut measurements = Vec::new();
        for count in [16, 128] {
            let f = Fixture::new();
            install_capture(&f.db, "agent", "peer").unwrap();
            let mut payload = json!({"cursor":0,"ranges":[],
                "allocations":(1..=count).map(|number| json!({"project_id":"named:Native fleet","issue_number":number,"node":"peer"})).collect::<Vec<_>>(),
                "allocation_deadlines":(1..=count).map(|number| json!({"project_id":"named:Native fleet","issue_number":number,"expires_at":1000+number})).collect::<Vec<_>>()});
            apply_pull(&f.db, "peer", &payload, &[]).unwrap();
            f.db.execute_batch("CREATE TABLE allocation_mutations(kind TEXT);
                CREATE TRIGGER allocation_insert_audit AFTER INSERT ON fleet_allocations BEGIN INSERT INTO allocation_mutations VALUES('insert'); END;
                CREATE TRIGGER allocation_delete_audit AFTER DELETE ON fleet_allocations BEGIN INSERT INTO allocation_mutations VALUES('delete'); END;
                CREATE TRIGGER allocation_deadline_audit AFTER UPDATE ON fleet_allocation_deadlines BEGIN INSERT INTO allocation_mutations VALUES('deadline'); END;").unwrap();
            // Wire order is not meaningful; compare the complete row multiset.
            payload["allocations"].as_array_mut().unwrap().reverse();
            payload["allocation_deadlines"]
                .as_array_mut()
                .unwrap()
                .reverse();
            let mut owner = crate::database::Owner::start(&f.path).unwrap().unwrap();
            let (db, transport) = crate::database::tests::measured_connection(&f.path);
            apply_pull(&db, "peer", &payload, &[]).unwrap();
            drop(db);
            let (commands, _) = transport.join().unwrap();
            owner.stop();
            let mutations: i64 =
                f.db.query_row("SELECT count(*) FROM allocation_mutations", [], |r| {
                    r.get(0)
                })
                .unwrap();
            eprintln!(
                "{count} unchanged allocations: {commands} owner RPCs, {mutations} allocation mutations"
            );
            measurements.push((count, commands, mutations));

            payload["allocation_deadlines"][0]["expires_at"] = json!(9876);
            apply_pull(&f.db, "peer", &payload, &[]).unwrap();
            assert_eq!(
                rows(
                    &f.db,
                    "SELECT expires_at FROM fleet_allocation_deadlines WHERE issue_number=?",
                    &[json!(count)]
                )
                .unwrap()[0]["expires_at"],
                9876
            );
            let before = rows(
                &f.db,
                "SELECT * FROM fleet_allocations ORDER BY issue_number",
                &[],
            )
            .unwrap();
            let mut invalid = payload.clone();
            invalid["allocations"][0] = invalid["allocations"][1].clone();
            assert!(apply_pull(&f.db, "peer", &invalid, &[]).is_err());
            assert_eq!(
                rows(
                    &f.db,
                    "SELECT * FROM fleet_allocations ORDER BY issue_number",
                    &[]
                )
                .unwrap(),
                before
            );
            payload
                .as_object_mut()
                .unwrap()
                .remove("allocation_deadlines");
            apply_pull(&f.db, "peer", &payload, &[]).unwrap();
            let minimum: i64 =
                f.db.query_row(
                    "SELECT min(expires_at) FROM fleet_allocation_deadlines",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                minimum > (super::super::context::now() * 1000.0) as i64,
                "Legacy snapshots retain their default lease refresh"
            );
        }
        for (count, commands, mutations) in measurements {
            assert!(
                commands < 25,
                "{count} unchanged allocations needed {commands} RPCs"
            );
            assert_eq!(mutations, 0, "Unchanged allocation snapshots rewrote rows");
        }
    }

    #[test]
    fn pulling_allocations_keeps_the_supervisors_deadline() {
        let main = Fixture::new();
        main.capture();
        let workers = vec![
            json!({"config":{"projects":["named:Native fleet"],"concurrency":1,"enabled":true}}),
        ];
        allocate(&main.db, "agent", &workers).unwrap();
        main.db
            .execute("UPDATE fleet_allocation_deadlines SET expires_at=1234", [])
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
            rows(
                &agent.db,
                "SELECT expires_at FROM fleet_allocation_deadlines",
                &[]
            )
            .unwrap()[0]["expires_at"],
            1234
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
    #[test]
    fn ready_dependencies_reopen_and_reblock_across_fleet_snapshots() {
        let main = Fixture::new();
        main.db.execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version) VALUES('named:Native fleet','Work',1,1)", []).unwrap();
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
            .execute("UPDATE issues SET state='ready' WHERE number=1", [])
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
