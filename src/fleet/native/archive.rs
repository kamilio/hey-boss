//! Fleet journals describe logical issue changes and immutable archive manifests.
//! Local cleanup/restoration progress never crosses the wire.
use super::{Result, replica};
use crate::{
    database::Connection,
    issues::{self, archive as cold},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn local_column(column: &str) -> bool {
    matches!(
        column,
        "archive_cleanup" | "archive_restoring" | "archive_touched_at"
    )
}

pub(super) fn snapshot_sql(table: &str) -> String {
    match table {
        "comments"=>"SELECT h.* FROM comments h WHERE NOT EXISTS(SELECT 1 FROM issues i WHERE i.project_id=h.project_id AND i.number=h.issue_number AND i.archive_key IS NOT NULL) ORDER BY h.id".into(),
        "events"=>"SELECT h.* FROM events h WHERE h.action IN ('moved_to','pr_attached','pr_classified','attempt_reconciled','commit_attached','commit_removed') OR NOT EXISTS(SELECT 1 FROM issues i WHERE i.project_id=h.project_id AND i.number=h.issue_number AND i.archive_key IS NOT NULL) ORDER BY h.id".into(),
        "issue_status_updates"=>"SELECT h.* FROM issue_status_updates h WHERE NOT EXISTS(SELECT 1 FROM issues i WHERE i.project_id=h.project_id AND i.number=h.issue_number AND i.archive_key IS NOT NULL) OR h.id=(SELECT id FROM issue_status_updates WHERE project_id=h.project_id AND issue_number=h.issue_number ORDER BY created_at DESC,id DESC LIMIT 1)".into(),
        _=>format!("SELECT * FROM {table}"),
    }
}

pub(super) fn strip_local(table: &str, row: &mut Value) {
    if table == "issues"
        && let Some(row) = row.as_object_mut()
    {
        row.retain(|field, _| !local_column(field));
    }
}

fn issue_rows(payload: &Value) -> issues::Result<Vec<Value>> {
    let mut rows = payload["tables"]["issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for change in payload["changes"].as_array().into_iter().flatten() {
        if change["table_name"] == "issues"
            && let Some(after) = change["after_json"].as_str()
        {
            rows.push(serde_json::from_str(after)?);
        }
    }
    Ok(rows)
}

pub(super) fn requires_support(payload: &Value) -> issues::Result<bool> {
    Ok(issue_rows(payload)?
        .iter()
        .any(|row| row["archive_key"].is_string()))
}

fn scope(row: &Value) -> issues::Result<(&str, i64)> {
    Ok((
        row["project_id"]
            .as_str()
            .ok_or_else(|| issues::Error::invalid("Missing archived issue project"))?,
        row["number"]
            .as_i64()
            .ok_or_else(|| issues::Error::invalid("Missing archived issue number"))?,
    ))
}

pub(super) fn prepare_pull(
    db: &Connection,
    payload: &Value,
    mut fetch: impl FnMut(&str, &str, i64, &Value) -> issues::Result<Value>,
) -> issues::Result<Value> {
    if !db.is_autocommit() {
        return Err(issues::Error::new(
            "archive_unavailable",
            "Archive transfer must precede the replication transaction",
        ));
    }
    let rows = issue_rows(payload)?;
    let mut prepared = payload.clone();
    if rows.is_empty() {
        return Ok(prepared);
    }
    let cursor = payload["cursor"]
        .as_i64()
        .filter(|n| *n >= 0)
        .ok_or_else(|| issues::Error::invalid("Invalid fleet pull cursor"))?;
    let saved: i64 = db.query_row(
        "SELECT coalesce((SELECT CAST(value AS INTEGER) FROM fleet_state WHERE key='cursor'),0)",
        [],
        |r| r.get(0),
    )?;
    if cursor < saved {
        return Err(issues::Error::invalid(
            "Stale fleet pull; no archive preparation was performed",
        ));
    }
    let mut targets = BTreeSet::new();
    let mut last = BTreeMap::new();
    let any_archived = rows.iter().any(|row| row["archive_key"].is_string());
    let mut catalog = if any_archived {
        Some(cold::transfer::Catalog::new(db)?)
    } else {
        None
    };
    for row in &rows {
        let (project, number) = scope(row)?;
        targets.insert((project.to_owned(), number));
        last.insert((project.to_owned(), number), row["archive_key"].is_string());
    }
    let mut current = BTreeSet::new();
    let mut existing = BTreeSet::new();
    let mut pending = BTreeSet::new();
    for (project,number,exists,archived,unsent) in db.query_collect("SELECT json_extract(requested.value,'$[0]'),json_extract(requested.value,'$[1]'),i.number IS NOT NULL,i.archive_key IS NOT NULL,CASE WHEN ?2 THEN EXISTS(SELECT 1 FROM fleet_outbox WHERE table_name IN ('issues','comments','events','issue_status_updates') AND json_extract(coalesce(after_json,before_json),'$.project_id')=json_extract(requested.value,'$[0]') AND coalesce(json_extract(coalesce(after_json,before_json),'$.number'),json_extract(coalesce(after_json,before_json),'$.issue_number'))=json_extract(requested.value,'$[1]')) ELSE 0 END FROM json_each(?1) requested LEFT JOIN issues i ON i.project_id=json_extract(requested.value,'$[0]') AND i.number=json_extract(requested.value,'$[1]')",rusqlite::params![serde_json::to_string(&targets)?,any_archived],|r|->rusqlite::Result<_>{Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,bool>(2)?,r.get::<_,bool>(3)?,r.get::<_,bool>(4)?))})? {
        let key=(project,number);
        if exists { existing.insert(key.clone()); }
        if archived { current.insert(key.clone()); }
        if unsent { pending.insert(key); }
    }
    let mut warm = BTreeSet::new();
    for row in &rows {
        let (project, number) = scope(row)?;
        let scope = (project.to_owned(), number);
        if let Some(key) = row["archive_key"].as_str() {
            let catalog = catalog.as_mut().unwrap();
            if !catalog.contains(key, project, number)? {
                let mut download = cold::transfer::Download::new(db, key, project, number)?;
                let mut cursor = Value::Null;
                loop {
                    let page = fetch(key, project, number, &cursor)?;
                    cursor = page["next"].clone();
                    if download.receive(&page)? {
                        break;
                    }
                }
                drop(download);
                cold::transfer::map_local_history(db, key, project, number)?;
                catalog.refresh(db)?;
            }
            if pending.contains(&scope) || last.get(&scope) == Some(&false) {
                if current.remove(&scope) {
                    cold::restore_issue(db, project, number, issues::worker::now())?;
                }
                if existing.contains(&scope) {
                    cold::materialize_history(db, key, project, number)?;
                }
                warm.insert(scope);
            }
        } else if current.remove(&scope) {
            cold::restore_issue(db, project, number, issues::worker::now())?;
        }
    }
    let rewrite = |row: &mut Value| -> issues::Result<()> {
        let (project, number) = scope(row)?;
        if warm.contains(&(project.to_owned(), number))
            && let Some(key) = row["archive_key"].as_str()
        {
            row["body"] = json!(cold::issue_body(db, project, number, key)?);
            row["archive_key"] = Value::Null;
            row["archived_comments"] = json!(0);
        }
        strip_local("issues", row);
        Ok(())
    };
    for row in prepared
        .get_mut("tables")
        .and_then(|tables| tables.get_mut("issues"))
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        rewrite(row)?;
    }
    for change in prepared
        .get_mut("changes")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if change["table_name"] == "issues"
            && let Some(after) = change["after_json"].as_str()
        {
            let mut row = serde_json::from_str(after)?;
            rewrite(&mut row)?;
            change["after_json"] = json!(row.to_string());
        }
    }
    Ok(prepared)
}

fn changed_issues(changes: &[Value]) -> Result<Vec<Value>> {
    let mut targets = Vec::new();
    for change in changes {
        let table = change["table_name"].as_str().unwrap_or("");
        if !matches!(
            table,
            "issues"
                | "comments"
                | "events"
                | "issue_status_updates"
                | "issue_subtasks"
                | "issue_pull_requests"
        ) {
            continue;
        }
        for field in ["before_json", "after_json"] {
            let Some(encoded) = change[field].as_str() else {
                continue;
            };
            let row: Value = serde_json::from_str(encoded)?;
            let Some(project) = row["project_id"].as_str() else {
                continue;
            };
            for field in ["number", "issue_number", "parent_number", "child_number"] {
                if let Some(number) = row[field]
                    .as_i64()
                    .or_else(|| row[field].as_str().and_then(|s| s.parse().ok()))
                {
                    targets.push(json!([project, number, change["seq"]]));
                }
            }
        }
    }
    Ok(targets)
}

pub(super) fn prepare_changes(db: &Connection, node: &str, changes: &[Value]) -> Result<()> {
    for (project, number) in archived_changes(db, node, changes)? {
        cold::restore_issue(db, &project, number, issues::worker::now())?;
    }
    Ok(())
}

pub(super) fn archived_changes(
    db: &Connection,
    node: &str,
    changes: &[Value],
) -> Result<Vec<(String, i64)>> {
    let targets = changed_issues(changes)?;
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    Ok(db.query_collect("SELECT DISTINCT i.project_id,i.number FROM json_each(?1) requested CROSS JOIN issues i WHERE i.project_id=json_extract(requested.value,'$[0]') AND i.number=json_extract(requested.value,'$[1]') AND i.archive_key IS NOT NULL AND NOT EXISTS(SELECT 1 FROM fleet_receipts WHERE node=?2 AND seq=json_extract(requested.value,'$[2]'))",rusqlite::params![serde_json::to_string(&targets)?,node],|r|->rusqlite::Result<_>{Ok((r.get(0)?,r.get(1)?))})?)
}

pub(super) fn incoming_issue(row: &mut serde_json::Map<String, Value>) {
    let archived = row.get("archive_key").is_some_and(Value::is_string);
    row.entry("archive_key").or_insert(Value::Null);
    row.entry("archived_comments").or_insert(json!(0));
    row.insert("archive_cleanup".into(), json!(i64::from(archived)));
    row.insert("archive_restoring".into(), json!(0));
    row.insert("archive_touched_at".into(), json!(0));
}

pub(super) fn ensure_copy(
    db: &Connection,
    row: &Value,
    catalog: &mut Option<cold::transfer::Catalog>,
) -> Result<()> {
    if let Some(key) = row["archive_key"].as_str() {
        let (project, number) = scope(row)?;
        if catalog.is_none() {
            *catalog = Some(cold::transfer::Catalog::new(db)?);
        }
        if !catalog.as_ref().unwrap().contains(key, project, number)? {
            return Err(replica::invalid(
                "A replicated issue archive must be fetched and verified before applying its manifest",
            ));
        }
    }
    Ok(())
}
