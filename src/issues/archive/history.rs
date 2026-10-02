//! An issue's immutable history is copied before its hot manifest is published.
//! Cleanup and restoration use small, independently recoverable hot transactions.
use super::*;
use crate::database::{Connection as HotConnection, Transaction, params_from_iter};
use rusqlite::types::{Value as SqlValue, ValueRef};
use serde_json::json;
use std::collections::BTreeSet;

const TABLES: &[&str] = &["comments", "events", "issue_status_updates", "agents"];
const SCHEMA: &str = "
CREATE TABLE issue_copies(key TEXT PRIMARY KEY,project_id TEXT NOT NULL,number INTEGER NOT NULL,record TEXT NOT NULL,comments INTEGER NOT NULL,record_hash TEXT NOT NULL);
CREATE TABLE issue_history(archive_key TEXT NOT NULL,kind TEXT NOT NULL,id INTEGER,text_id TEXT NOT NULL,created_at INTEGER NOT NULL,author TEXT,action TEXT,record TEXT NOT NULL,record_hash TEXT NOT NULL,PRIMARY KEY(archive_key,kind,text_id)) WITHOUT ROWID;
CREATE INDEX history_number ON issue_history(archive_key,kind,id);
CREATE INDEX history_timeline ON issue_history(archive_key,kind,created_at DESC,id DESC);
CREATE INDEX history_actions ON issue_history(archive_key,kind,action,id DESC);
CREATE INDEX history_comment_resolution ON issue_history(archive_key,json_extract(json_extract(record,'$.data'),'$.comment_id'),created_at DESC,id DESC) WHERE kind='events' AND action IN ('comment_resolved','comment_unresolved');
CREATE INDEX history_comment_models ON issue_history(archive_key,json_extract(json_extract(record,'$.data'),'$.comment_id'),id DESC) WHERE kind='events' AND action='commented';
";

pub(super) fn initialize(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='issue_copies')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    if !tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='issue_copies')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        tx.execute_batch(SCHEMA)?;
    }
    tx.commit()?;
    Ok(())
}

fn record(row: &crate::database::Row<'_>) -> Result<Value> {
    let mut fields = serde_json::Map::new();
    for index in 0..row.column_count() {
        let value = match row.get_ref(index)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(value) => json!(value),
            ValueRef::Real(value) => json!(value),
            ValueRef::Text(value) => {
                json!(std::str::from_utf8(value).map_err(|e| unavailable(e.to_string()))?)
            }
            ValueRef::Blob(_) => return Err(unavailable("Unexpected binary issue history")),
        };
        fields.insert(row.column_name(index)?.into(), value);
    }
    Ok(Value::Object(fields))
}

fn digest_record(digest: &mut Sha256, kind: &str, record: &str) {
    digest.update((kind.len() as u64).to_be_bytes());
    digest.update(kind.as_bytes());
    digest.update((record.len() as u64).to_be_bytes());
    digest.update(record.as_bytes());
}

fn fingerprint(db: &HotConnection, project: &str, number: i64) -> Result<Value> {
    let mut stamp = Vec::new();
    for table in &TABLES[..3] {
        stamp.push(db.query_row(&format!("SELECT count(*),CAST(max(id) AS TEXT) FROM {table} WHERE project_id=?1 AND issue_number=?2"), params![project,number], |r| Ok(json!([r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?])))?);
    }
    stamp.push(db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL),EXISTS(SELECT 1 FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2),(SELECT role FROM fleet_meta WHERE id=1)",params![project,number],|r|Ok(json!([r.get::<_,bool>(0)?,r.get::<_,bool>(1)?,r.get::<_,String>(2)?])))?);
    Ok(json!(stamp))
}

fn issue_record(db: &HotConnection, project: &str, number: i64) -> Result<Option<Value>> {
    Ok(db
        .query_collect(
            "SELECT * FROM issues WHERE project_id=?1 AND number=?2",
            params![project, number],
            record,
        )?
        .into_iter()
        .next())
}

fn copy_rows(
    hot: &HotConnection,
    cold: &Connection,
    staging: &str,
    table: &str,
    project: &str,
    number: i64,
    actors: &mut BTreeSet<String>,
    digest: &mut Sha256,
) -> Result<()> {
    let mut cursor: Option<SqlValue> = None;
    loop {
        let (sql, args) = if let Some(cursor) = &cursor {
            (
                format!(
                    "SELECT * FROM {table} WHERE project_id=?1 AND issue_number=?2 AND id>?3 ORDER BY id LIMIT 16"
                ),
                vec![
                    SqlValue::Text(project.into()),
                    SqlValue::Integer(number),
                    cursor.clone(),
                ],
            )
        } else {
            (
                format!(
                    "SELECT * FROM {table} WHERE project_id=?1 AND issue_number=?2 ORDER BY id LIMIT 16"
                ),
                vec![SqlValue::Text(project.into()), SqlValue::Integer(number)],
            )
        };
        let rows = hot.query_collect(&sql, params_from_iter(args), record)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            if let Some(actor) = row["author"].as_str().or_else(|| row["actor"].as_str()) {
                actors.insert(actor.into());
            }
            cursor = Some(match &row["id"] {
                Value::String(value) => SqlValue::Text(value.clone()),
                Value::Number(value) => SqlValue::Integer(
                    value
                        .as_i64()
                        .ok_or_else(|| unavailable("Invalid history ID"))?,
                ),
                _ => return Err(unavailable("Missing history ID")),
            });
            save_row(cold, staging, table, &row, digest)?;
        }
    }
    Ok(())
}

fn save_row(
    db: &Connection,
    key: &str,
    table: &str,
    row: &Value,
    digest: &mut Sha256,
) -> Result<()> {
    let encoded = row.to_string();
    let text_id = row["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| row["id"].to_string());
    db.execute("INSERT INTO issue_history(archive_key,kind,id,text_id,created_at,author,action,record,record_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![key,table,row["id"].as_i64(),text_id,row["created_at"].as_i64().unwrap_or(0),row["author"].as_str().or_else(||row["actor"].as_str()),row["action"].as_str(),encoded,format!("{:x}",Sha256::digest(encoded.as_bytes()))])?;
    digest_record(digest, table, &encoded);
    Ok(())
}

fn verify_copy(archive: &Archive, key: &str, project: &str, number: i64) -> Result<Value> {
    let root: Option<String> = archive
        .db
        .query_row(
            "SELECT record FROM issue_copies WHERE key=?1 AND project_id=?2 AND number=?3",
            params![key, project, number],
            |r| r.get(0),
        )
        .optional()?;
    let root = root.ok_or_else(|| unavailable("Archived issue copy is missing"))?;
    let mut digest = Sha256::new();
    digest_record(&mut digest, "issues", &root);
    for table in TABLES {
        let mut query = archive.db.prepare("SELECT record FROM issue_history WHERE archive_key=?1 AND kind=?2 ORDER BY CASE WHEN id IS NOT NULL THEN id END,text_id")?;
        let mut rows = query.query(params![key, table])?;
        while let Some(row) = rows.next()? {
            digest_record(&mut digest, table, &row.get::<_, String>(0)?);
        }
    }
    if format!("{:x}", digest.finalize()) != key {
        return Err(unavailable("Archived issue checksum does not match"));
    }
    Ok(serde_json::from_str(&root)?)
}

pub(crate) fn archive_issue(
    db: &HotConnection,
    project: &str,
    number: i64,
    now: i64,
) -> Result<bool> {
    if !db.is_autocommit() {
        return Err(unavailable("Cannot archive inside a hot transaction"));
    }
    let snapshot = db.read_transaction()?;
    let eligible: bool = snapshot.query_row("SELECT EXISTS(SELECT 1 FROM issues i WHERE project_id=?1 AND number=?2 AND archive_key IS NULL AND (state='closed' OR deleted_at IS NOT NULL) AND max(updated_at,coalesce(closed_at,0),coalesce(deleted_at,0),archive_touched_at)<=?3 AND attempt_hold IS NULL AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL) AND NOT EXISTS(SELECT 1 FROM fleet_allocations a WHERE a.project_id=i.project_id AND a.issue_number=i.number)) AND (SELECT role!='agent' FROM fleet_meta WHERE id=1)",params![project,number,now.saturating_sub(GRACE_MS)],|r|r.get(0))?;
    if !eligible {
        return Ok(false);
    }
    // Late replicated history can arrive without changing the issue revision.
    // Its own timestamp still earns the complete grace period.
    for table in &TABLES[..3] {
        if snapshot.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE project_id=?1 AND issue_number=?2 AND created_at>?3)"),
            params![project, number, now.saturating_sub(GRACE_MS)],
            |r|r.get::<_,bool>(0),
        )? {
            return Ok(false);
        }
    }
    let original = issue_record(&snapshot, project, number)?
        .ok_or_else(|| unavailable("Issue disappeared during archive read"))?;
    let stamp = fingerprint(&snapshot, project, number)?;
    let archive = Archive::open(&archive_path(db)?)?;
    let copy = archive.db.unchecked_transaction()?;
    let staging = crate::issues::worker::random_id()?;
    let mut digest = Sha256::new();
    digest_record(&mut digest, "issues", &original.to_string());
    let mut actors = BTreeSet::new();
    for field in ["created_by", "closed_by", "assignee"] {
        if let Some(actor) = original[field].as_str() {
            actors.insert(actor.into());
        }
    }
    for table in &TABLES[..3] {
        copy_rows(
            &snapshot,
            &copy,
            &staging,
            table,
            project,
            number,
            &mut actors,
            &mut digest,
        )?;
    }
    for actor in actors {
        for row in snapshot.query_collect("SELECT * FROM agents WHERE id=?1", [actor], record)? {
            save_row(&copy, &staging, "agents", &row, &mut digest)?;
        }
    }
    let key = format!("{:x}", digest.finalize());
    let comments = stamp[0][0].as_i64().unwrap();
    let exists: bool = copy.query_row(
        "SELECT EXISTS(SELECT 1 FROM issue_copies WHERE key=?1)",
        [&key],
        |r| r.get(0),
    )?;
    if exists {
        copy.execute("DELETE FROM issue_history WHERE archive_key=?1", [&staging])?;
    } else {
        copy.execute(
            "UPDATE issue_history SET archive_key=?2 WHERE archive_key=?1",
            params![staging, key],
        )?;
        copy.execute(
            "INSERT INTO issue_copies VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                key,
                project,
                number,
                original.to_string(),
                comments,
                format!("{:x}", Sha256::digest(original.to_string().as_bytes()))
            ],
        )?;
    }
    copy.commit()?;
    snapshot.commit()?;
    verify_copy(&archive, &key, project, number)?;
    let tx = Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    if issue_record(&tx, project, number)?.as_ref() != Some(&original)
        || fingerprint(&tx, project, number)? != stamp
    {
        return Ok(false);
    }
    // Publishing the manifest is the only logical switch. Until it commits,
    // readers use hot rows; afterwards they use the complete immutable copy.
    tx.execute("UPDATE issues SET archive_key=?3,archived_comments=?4,archive_cleanup=1,body='' WHERE project_id=?1 AND number=?2",params![project,number,key,comments])?;
    tx.commit()?;
    Ok(true)
}

fn key(db: &HotConnection, project: &str, number: i64) -> Result<Option<String>> {
    Ok(db
        .query_row(
            "SELECT archive_key FROM issues WHERE project_id=?1 AND number=?2",
            params![project, number],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

pub(crate) fn mutation_targets(operation: &crate::issues::Operation) -> Vec<i64> {
    use crate::issues::Operation;
    if !operation.writes() {
        return vec![];
    }
    let mut numbers: Vec<_> = operation.number().into_iter().collect();
    match operation {
        Operation::Batch { edits } => numbers.extend(edits.iter().map(|edit| edit.number)),
        Operation::AddSubtask { child, .. } | Operation::RemoveSubtask { child, .. } => {
            numbers.push(*child)
        }
        _ => {}
    }
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

pub(crate) fn issue_body(
    db: &HotConnection,
    project: &str,
    number: i64,
    key: &str,
) -> Result<String> {
    let archive = Archive::read(&archive_path(db)?)?;
    let body: Option<String> = archive.db.query_row("SELECT json_extract(archive_record(record,record_hash),'$.body') FROM issue_copies WHERE key=?1 AND project_id=?2 AND number=?3",params![key,project,number],|r|r.get(0)).optional()?;
    body.ok_or_else(|| unavailable("Archived issue body is missing"))
}

pub(crate) fn search_bodies(
    db: &HotConnection,
    project: &str,
    state: &str,
    search: Option<&str>,
) -> Result<Vec<i64>> {
    let Some(search) = search else {
        return Ok(vec![]);
    };
    if !matches!(state, "all" | "closed" | "deleted") {
        return Ok(vec![]);
    }
    let keys = db.query_collect("SELECT archive_key FROM issues WHERE project_id=?1 AND archive_key IS NOT NULL AND ((?2='deleted' AND deleted_at IS NOT NULL) OR (?2!='deleted' AND deleted_at IS NULL))",params![project,state],|r|r.get::<_,String>(0))?;
    if keys.is_empty() {
        return Ok(vec![]);
    }
    let archive = Archive::read(&archive_path(db)?)?;
    let mut matches = Vec::new();
    for chunk in keys.chunks(256) {
        let present: i64 = archive.db.query_row("SELECT count(*) FROM issue_copies WHERE project_id=?1 AND key IN (SELECT value FROM json_each(?2))",params![project,serde_json::to_string(chunk)?],|r|r.get(0))?;
        if present != chunk.len() as i64 {
            return Err(unavailable(
                "Search requires an archived issue copy that is missing",
            ));
        }
        let mut query = archive.db.prepare("SELECT number FROM issue_copies WHERE project_id=?1 AND key IN (SELECT value FROM json_each(?2)) AND instr(lower(json_extract(archive_record(record,record_hash),'$.body')),lower(?3))>0")?;
        matches.extend(
            query
                .query_map(
                    params![project, serde_json::to_string(chunk)?, search],
                    |r| r.get::<_, i64>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(matches)
}

pub(crate) fn history_connection(
    db: &HotConnection,
    project: &str,
    number: i64,
) -> Result<Option<HotConnection>> {
    let Some(key) = key(db, project, number)? else {
        return Ok(None);
    };
    if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(unavailable("Invalid archive key"));
    }
    let archive = Archive::read(&archive_path(db)?)?;
    if !archive.db.query_row(
        "SELECT EXISTS(SELECT 1 FROM issue_copies WHERE key=?1 AND project_id=?2 AND number=?3)",
        params![key, project, number],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(unavailable("Archived issue copy is missing"));
    }
    let quoted: String = archive
        .db
        .query_row("SELECT quote(?1)", [project], |r| r.get(0))?;
    archive.db.execute_batch(&format!("CREATE TEMP VIEW comments AS SELECT id,{quoted} AS project_id,{number} AS issue_number,author,json_extract(record,'$.body') AS body,created_at FROM issue_history WHERE archive_key='{key}' AND kind='comments' AND archive_record(record,record_hash) IS NOT NULL;
        CREATE TEMP VIEW events AS SELECT id,{quoted} AS project_id,{number} AS issue_number,author AS actor,action,created_at,json_extract(record,'$.data') AS data FROM issue_history WHERE archive_key='{key}' AND kind='events' AND archive_record(record,record_hash) IS NOT NULL;
        CREATE TEMP VIEW issue_status_updates AS SELECT text_id AS id,{quoted} AS project_id,{number} AS issue_number,author,json_extract(record,'$.level') AS level,json_extract(record,'$.comment') AS comment,created_at FROM issue_history WHERE archive_key='{key}' AND kind='issue_status_updates' AND archive_record(record,record_hash) IS NOT NULL;
        CREATE TEMP TABLE agents(id TEXT PRIMARY KEY,metadata TEXT);
        INSERT INTO agents SELECT text_id,json_extract(archive_record(record,record_hash),'$.metadata') FROM issue_history WHERE archive_key='{key}' AND kind='agents';"))?;
    let actors = archive
        .db
        .prepare("SELECT id FROM agents")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for row in db.query_collect(
        "SELECT id,metadata FROM agents WHERE id IN (SELECT value FROM json_each(?1))",
        [serde_json::to_string(&actors)?],
        |r| -> rusqlite::Result<_> { Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)) },
    )? {
        archive.db.execute(
            "INSERT OR REPLACE INTO agents VALUES(?1,?2)",
            params![row.0, row.1],
        )?;
    }
    archive.db.pragma_update(None, "query_only", true)?;
    Ok(Some(HotConnection::from_archive(archive.db)))
}

pub(crate) fn cleanup_history(db: &HotConnection) -> Result<usize> {
    let target: Option<(String,i64,String)> = db.query_row("SELECT project_id,number,archive_key FROM issues WHERE archive_key IS NOT NULL AND archive_cleanup=1 AND archive_restoring=0 LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((project, number, key)) = target else {
        return Ok(0);
    };
    let archive = Archive::read(&archive_path(db)?)?;
    for (table, payload, extra, order) in [
        ("comments", "body", "", "ORDER BY id LIMIT 16"),
        (
            "events",
            "data",
            "AND action NOT IN ('moved_to','pr_attached','pr_classified','attempt_reconciled')",
            "ORDER BY id LIMIT 16",
        ),
        (
            "issue_status_updates",
            "comment",
            "",
            "ORDER BY created_at DESC,id DESC LIMIT 16 OFFSET 1",
        ),
    ] {
        let candidates = db.query_collect(&format!("SELECT CAST(id AS TEXT),length(CAST({payload} AS BLOB))+256 FROM {table} WHERE project_id=?1 AND issue_number=?2 {extra} {order}"),params![project,number],|r| -> rusqlite::Result<_> { Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)) })?;
        let mut bytes = 0;
        let mut ids = Vec::new();
        for (id, size) in candidates {
            if !ids.is_empty() && bytes + size > 1024 * 1024 {
                break;
            }
            bytes += size;
            ids.push(id);
        }
        if ids.is_empty() {
            continue;
        }
        let selected = serde_json::to_string(&ids)?;
        let hot_rows = db.query_collect(&format!("SELECT * FROM {table} WHERE id IN (SELECT value FROM json_each(?1)) AND project_id=?2 AND issue_number=?3"),params![selected,project,number],record)?;
        // Verify the exact rows being discarded, before taking the hot writer.
        // A missing copy or an unexpected history edit leaves all hot data intact.
        for row in &hot_rows {
            let id = row["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| row["id"].to_string());
            let saved: Option<String> = archive.db.query_row("SELECT record FROM issue_history WHERE archive_key=?1 AND kind=?2 AND text_id=?3",params![key,table,id],|r|r.get(0)).optional()?;
            if saved
                .as_deref()
                .map(serde_json::from_str::<Value>)
                .transpose()?
                .as_ref()
                != Some(row)
            {
                return Err(unavailable("Archive history does not match its hot copy"));
            }
        }
        let tx = Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
        let current: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM issues WHERE project_id=?1 AND number=?2 AND archive_key=?3 AND archive_restoring=0)",params![project,number,key],|r|r.get(0))?;
        if !current {
            return Ok(0);
        }
        let actual = tx.query_collect(&format!("SELECT * FROM {table} WHERE id IN (SELECT value FROM json_each(?1)) AND project_id=?2 AND issue_number=?3"),params![selected,project,number],record)?;
        if actual != hot_rows {
            return Ok(0);
        }
        let syncing: i64 = tx.query_row("SELECT syncing FROM fleet_meta WHERE id=1", [], |r| {
            r.get(0)
        })?;
        tx.execute("UPDATE fleet_meta SET syncing=1 WHERE id=1", [])?;
        let removed = tx.execute(&format!("DELETE FROM {table} WHERE project_id=?1 AND issue_number=?2 AND id IN (SELECT value FROM json_each(?3))"),params![project,number,selected])?;
        tx.execute("UPDATE fleet_meta SET syncing=?1 WHERE id=1", [syncing])?;
        tx.commit()?;
        return Ok(removed);
    }
    db.execute("UPDATE issues SET archive_cleanup=0 WHERE project_id=?1 AND number=?2 AND archive_key=?3 AND archive_restoring=0",params![project,number,key])?;
    Ok(0)
}

pub(crate) fn restore_issue(
    db: &HotConnection,
    project: &str,
    number: i64,
    now: i64,
) -> Result<()> {
    let Some(key) = key(db, project, number)? else {
        return Ok(());
    };
    if !db.is_autocommit() {
        return Err(unavailable(
            "Restore must precede the hot mutation transaction",
        ));
    }
    let archive = Archive::read(&archive_path(db)?)?;
    let root = verify_copy(&archive, &key, project, number)?;
    db.execute("UPDATE issues SET archive_restoring=1 WHERE project_id=?1 AND number=?2 AND archive_key=?3",params![project,number,key])?;
    for table in ["agents", "comments", "events", "issue_status_updates"] {
        let mut query = archive.db.prepare(
            "SELECT record FROM issue_history WHERE archive_key=?1 AND kind=?2 ORDER BY text_id",
        )?;
        let mut rows = query.query(params![key, table])?;
        let mut pending = None;
        loop {
            let mut batch = Vec::new();
            let mut bytes = 0;
            if let Some(encoded) = pending.take() {
                bytes += String::len(&encoded);
                batch.push(serde_json::from_str::<Value>(&encoded)?);
            }
            while batch.len() < 16 {
                let Some(row) = rows.next()? else {
                    break;
                };
                let encoded: String = row.get(0)?;
                if !batch.is_empty() && bytes + encoded.len() > 1024 * 1024 {
                    pending = Some(encoded);
                    break;
                }
                bytes += encoded.len();
                batch.push(serde_json::from_str::<Value>(&encoded)?);
            }
            if batch.is_empty() {
                break;
            }
            let tx = Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
            if self::key(&tx, project, number)?.as_deref() != Some(&key) {
                return Ok(());
            }
            let syncing: i64 =
                tx.query_row("SELECT syncing FROM fleet_meta WHERE id=1", [], |r| {
                    r.get(0)
                })?;
            tx.execute("UPDATE fleet_meta SET syncing=1 WHERE id=1", [])?;
            for row in batch {
                restore_row(&tx, table, &row)?;
            }
            tx.execute("UPDATE fleet_meta SET syncing=?1 WHERE id=1", [syncing])?;
            tx.commit()?;
        }
    }
    db.execute("UPDATE issues SET body=?4,archive_key=NULL,archived_comments=0,archive_restoring=0,archive_cleanup=0,archive_touched_at=?5 WHERE project_id=?1 AND number=?2 AND archive_key=?3",params![project,number,key,root["body"].as_str().ok_or_else(||unavailable("Invalid archived issue body"))?,now])?;
    Ok(())
}

fn restore_row(db: &HotConnection, table: &str, row: &Value) -> Result<()> {
    let id = match &row["id"] {
        Value::String(v) => SqlValue::Text(v.clone()),
        Value::Number(v) => SqlValue::Integer(
            v.as_i64()
                .ok_or_else(|| unavailable("Invalid archive identity"))?,
        ),
        _ => return Err(unavailable("Missing archive identity")),
    };
    let existing = db
        .query_collect(&format!("SELECT * FROM {table} WHERE id=?1"), [id], record)?
        .into_iter()
        .next();
    if let Some(existing) = existing {
        // Agent identity is shared with active work. Restore only missing
        // identities; a newer model or session description stays authoritative.
        if table == "agents" {
            return Ok(());
        }
        if row.as_object().is_none_or(|fields| {
            fields
                .iter()
                .any(|(key, value)| existing.get(key) != Some(value))
        }) {
            return Err(Error::new(
                "archive_conflict",
                "An archived history ID belongs to a different record",
            ));
        }
        return Ok(());
    }
    let row = row
        .as_object()
        .ok_or_else(|| unavailable("Invalid archived history row"))?;
    let mut columns = Vec::new();
    let mut values = Vec::new();
    for (column, value) in row {
        if !column
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(unavailable("Invalid archive column"));
        }
        columns.push(format!("\"{column}\""));
        values.push(match value {
            Value::Null => SqlValue::Null,
            Value::String(v) => SqlValue::Text(v.clone()),
            Value::Number(v) => SqlValue::Integer(
                v.as_i64()
                    .ok_or_else(|| unavailable("Invalid archive number"))?,
            ),
            _ => return Err(unavailable("Invalid archive value")),
        });
    }
    db.execute(
        &format!(
            "INSERT INTO {table}({}) VALUES({}) ON CONFLICT(id) DO NOTHING",
            columns.join(","),
            vec!["?"; values.len()].join(",")
        ),
        params_from_iter(values),
    )?;
    Ok(())
}
