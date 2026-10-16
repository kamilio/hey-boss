use super::*;
use crate::issues::archive::transfer;
use rusqlite::{OptionalExtension, params};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    key: String,
    project: String,
    number: i64,
    pages: Vec<String>,
}

/// Export at most four MiB per cycle, outside the application writer. History
/// can be arbitrarily long without making task edits wait for its transfer.
pub(super) fn prepare(db: &Connection, engine: &mut hey_boss_sync::Engine) -> Result<()> {
    let mut remaining = 8;
    while remaining > 0 {
        let used = prepare_archive(db, engine, remaining)?;
        if used == 0 {
            break;
        }
        remaining -= used;
    }
    Ok(())
}

fn prepare_archive(
    db: &Connection,
    engine: &mut hey_boss_sync::Engine,
    budget: usize,
) -> Result<usize> {
    let candidates = replica::rows(
        db,
        "SELECT archive_key AS key,project_id,number FROM issues WHERE archive_key IS NOT NULL AND NOT EXISTS(SELECT 1 FROM distributed_archives a WHERE a.key=issues.archive_key AND a.complete=1) LIMIT 1",
        &[],
    )?;
    let candidate = if let Some(row) = candidates.into_iter().next() {
        Some(row)
    } else {
        replica::rows(db,"SELECT json_extract(j.value,'$.archive_key') AS key,json_extract(j.value,'$.project_id') AS project_id,json_extract(j.value,'$.number') AS number FROM distributed_outbox o,json_each(json_array(json(o.before_json),json(o.after_json))) j WHERE o.clock>(SELECT exported FROM distributed_meta WHERE id=1) AND o.table_name='issues' AND json_extract(j.value,'$.archive_key') IS NOT NULL AND NOT EXISTS(SELECT 1 FROM distributed_archives a WHERE a.key=json_extract(j.value,'$.archive_key') AND a.complete=1) LIMIT 1",&[])?.into_iter().next()
    };
    let Some(candidate) = candidate else {
        return Ok(0);
    };
    let key = candidate["key"].as_str().unwrap();
    let project = candidate["project_id"].as_str().unwrap();
    let number = candidate["number"].as_i64().unwrap();
    db.execute(
        "INSERT OR IGNORE INTO distributed_archives VALUES(?1,'[]','null',0)",
        [key],
    )?;
    let (pages, cursor): (String, String) = db.query_row(
        "SELECT pages,cursor FROM distributed_archives WHERE key=?1",
        [key],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut pages: Vec<String> = serde_json::from_str(&pages)?;
    let mut cursor: Value = serde_json::from_str(&cursor)?;
    for page_index in 0..budget {
        let mut page = transfer::export_page(db, key, project, number, &cursor)?;
        // Hot rows and relayed archives must retain the same legacy creation
        // identity even when their machine-local numeric IDs differ.
        for chunk in page["chunks"].as_array_mut().unwrap() {
            let kind = chunk["kind"].as_str().unwrap();
            if !matches!(kind, "comments" | "events") {
                continue;
            }
            let origin = chunk["identity"]["origin"].as_str().unwrap();
            let id = chunk["identity"]["id"].as_i64().unwrap();
            let local: i64 = db.query_row(
                "SELECT CASE WHEN ?1=(SELECT node FROM fleet_meta WHERE id=1) THEN ?3 ELSE (SELECT local_id FROM fleet_row_ids WHERE origin=?1 AND table_name=?2 AND origin_id=?3) END",
                params![origin, kind, id], |r| r.get(0))?;
            let canonical = identity::canonical(db, kind, local)?;
            let (origin, id) = canonical
                .rsplit_once(':')
                .ok_or_else(|| replica::invalid("Invalid archive history identity"))?;
            chunk["identity"] = json!({"origin":origin,"id":id.parse::<i64>()?});
        }
        let id = engine
            .put(&json!({"protocol":PROTOCOL,"kind":"archive_page","page":page}).to_string())?;
        pages.push(id);
        cursor = page["next"].clone();
        db.execute(
            "UPDATE distributed_archives SET pages=?2,cursor=?3,complete=?4 WHERE key=?1",
            params![
                key,
                serde_json::to_string(&pages)?,
                cursor.to_string(),
                page["done"] == true
            ],
        )?;
        if page["done"] == true {
            return Ok(page_index + 1);
        }
    }
    Ok(budget)
}

pub(super) fn capture(db: &Connection, fact: &mut Revision) -> Result<bool> {
    if fact.table != "issues" {
        return Ok(true);
    }
    for row in [&mut fact.before, &mut fact.after].into_iter().flatten() {
        if let Some(key) = row["archive_key"].as_str() {
            row["body"] = json!(crate::issues::archive::issue_body(
                db,
                row["project_id"].as_str().unwrap(),
                row["number"].as_i64().unwrap(),
                key
            )?);
        }
    }
    let Some(row) = fact
        .after
        .as_ref()
        .filter(|r| r["archive_key"].is_string())
        .or_else(|| {
            fact.before
                .as_ref()
                .filter(|r| r["archive_key"].is_string())
        })
    else {
        return Ok(true);
    };
    let Some(key) = row["archive_key"].as_str() else {
        return Ok(true);
    };
    let pages: Option<String> = db
        .query_row(
            "SELECT pages FROM distributed_archives WHERE key=?1 AND complete=1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    let Some(pages) = pages else {
        return Ok(false);
    };
    fact.archive = Some(Manifest {
        key: key.into(),
        project: row["project_id"].as_str().unwrap().into(),
        number: row["number"].as_i64().unwrap(),
        pages: serde_json::from_str(&pages)?,
    });
    Ok(true)
}

pub(super) fn remember(db: &Connection, manifest: &Manifest) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO distributed_archive_history(key,project_id,number) VALUES(?1,?2,?3)",
        params![manifest.key, manifest.project, manifest.number],
    )?;
    Ok(())
}

/// Hydrate one bounded history page while the issue still has its cold header.
/// Header publication and the final page commit together. All cursor movement
/// is in the caller's savepoint, so failures and restarts safely retry.
pub(super) fn project(db: &Connection, desired: &mut Value) -> Result<bool> {
    let project = desired["project_id"].as_str().unwrap();
    let number = desired["number"].as_i64().unwrap();
    let current: Option<Option<String>> = db
        .query_row(
            "SELECT archive_key FROM issues WHERE project_id=?1 AND number=?2",
            params![project, number],
            |r| r.get(0),
        )
        .optional()?;
    let active: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL)",params![project,number],|r|r.get(0))?;
    for key in [
        current.as_ref().and_then(|k| k.as_deref()),
        desired["archive_key"].as_str(),
    ]
    .into_iter()
    .flatten()
    {
        db.execute("INSERT OR IGNORE INTO distributed_archive_history(key,project_id,number) VALUES(?1,?2,?3)",params![key,project,number])?;
    }
    let history_changed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM distributed_history_edits WHERE project_id=?1 AND number=?2)",
        params![project, number],
        |r| r.get(0),
    )?;
    let warm =
        desired["archive_key"].is_null() || current == Some(None) || active || history_changed;
    if !warm {
        return Ok(true);
    }
    let pending = replica::rows(db,
        "SELECT key,cursor_kind,cursor_id FROM distributed_archive_history WHERE project_id=?1 AND number=?2 AND complete=0 ORDER BY key LIMIT 1",
        &[json!(project),json!(number)])?.into_iter().next();
    if let Some(pending) = pending {
        let key = pending["key"].as_str().unwrap();
        if current.is_none() {
            // Missing parents can retry through ordinary projection ordering.
            // A cold staging header keeps the history reachable across a crash;
            // draft prevents pickup before the requested content is published.
            let mut staging = desired.clone();
            staging["archive_key"] = json!(key);
            staging["draft"] = json!(1);
            materialize::write(
                db,
                "issues",
                &json!({"project_id":project,"number":number}),
                Some(staging),
            )?;
        }
        let page = crate::issues::archive::projection_history_page(
            db,
            key,
            project,
            number,
            pending["cursor_kind"].as_str().unwrap(),
            pending["cursor_id"].as_str().unwrap(),
        )?;
        for (table, cursor, row) in &page {
            restore_row(db, table, row, false)?;
            db.execute(
                "UPDATE distributed_archive_history SET cursor_kind=?2,cursor_id=?3 WHERE key=?1",
                params![key, table, cursor],
            )?;
        }
        if !page.is_empty() {
            return Ok(false);
        }
        db.execute(
            "UPDATE distributed_archive_history SET complete=1 WHERE key=?1",
            [key],
        )?;
        return Ok(false);
    }
    // Storage temperature is local. Once warm, a remote archive cannot cool an
    // active task or hide locally retained history behind another cold copy.
    db.execute(
        "DELETE FROM distributed_history_edits WHERE project_id=?1 AND number=?2",
        params![project, number],
    )?;
    desired["archive_key"] = Value::Null;
    desired["archived_comments"] = json!(0);
    Ok(true)
}

pub(super) fn restore_row(
    db: &Connection,
    table: &str,
    row: &Value,
    preserve_hot: bool,
) -> Result<()> {
    let exists = preserve_hot
        && db.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
            crate::database::params_from_iter(replica::args(&[row["id"].clone()])),
            |r| r.get::<_, bool>(0),
        )?;
    let mut fact = Revision {
        protocol: PROTOCOL,
        origin: "archive".into(),
        clock: 0,
        table: table.into(),
        before: None,
        after: Some(row.clone()),
        resource: None,
        archive: None,
    };
    identity::normalize(db, &mut fact)?;
    let row = fact.after.unwrap();
    projection::seed_history(db, table, &row, "archive")?;
    if exists {
        // Ordinary local restore may race unexported edits. Keep existing hot
        // rows until the projector catches up with their captured revisions.
        return Ok(());
    }
    let key = projection::key(db, table, &row)?;
    materialize::write(db, table, &key, projection::row(db, table, &key)?)?;
    db.execute(
        "DELETE FROM distributed_dirty WHERE table_name=?1 AND entity=?2",
        params![table, key.to_string()],
    )?;
    Ok(())
}

pub(super) fn receive(
    db: &Connection,
    engine: &hey_boss_sync::Engine,
    manifest: &Manifest,
) -> Result<()> {
    if transfer::Catalog::new(db)?.contains(&manifest.key, &manifest.project, manifest.number)? {
        remember_pages(db, manifest)?;
        return Ok(());
    }
    let mut download =
        transfer::Download::new(db, &manifest.key, &manifest.project, manifest.number)?;
    let mut complete = false;
    for id in &manifest.pages {
        let payload = engine
            .payload(id)?
            .ok_or_else(|| replica::invalid("Archived input is still pending"))?;
        let value: Value = serde_json::from_str(&payload)?;
        if value["protocol"] != PROTOCOL || value["kind"] != "archive_page" {
            return Err(replica::invalid("Invalid archive page reference"));
        }
        complete = download.receive(&value["page"])?;
    }
    if !complete {
        return Err(replica::invalid("Incomplete archived input"));
    }
    drop(download);
    transfer::map_local_history(db, &manifest.key, &manifest.project, manifest.number)?;
    remember_pages(db, manifest)?;
    Ok(())
}

fn remember_pages(db: &Connection, manifest: &Manifest) -> Result<()> {
    db.execute("INSERT INTO distributed_archives VALUES(?1,?2,'null',1) ON CONFLICT(key) DO UPDATE SET pages=excluded.pages,cursor='null',complete=1",
        params![manifest.key,serde_json::to_string(&manifest.pages)?])?;
    Ok(())
}

pub(super) fn revision(payload: &str) -> Result<Option<Revision>> {
    let header: Value = serde_json::from_str(payload)?;
    if header["protocol"] == 1 {
        // Retained for recovery, but superseded by the version-two baseline.
        return Ok(None);
    }
    if header["protocol"] != PROTOCOL {
        return Err(replica::invalid("Unsupported distributed protocol"));
    }
    if header["kind"] == "archive_page" {
        if !header["page"].is_object() {
            return Err(replica::invalid("Unsupported archive protocol"));
        }
        return Ok(None);
    }
    Ok(Some(serde_json::from_value(header)?))
}
