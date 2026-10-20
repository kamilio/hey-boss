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
    if let Some(key) = desired["archive_key"].as_str() {
        if db.query_row(
            "SELECT EXISTS(SELECT 1 FROM distributed_archive_supersessions WHERE key=?1)",
            [key],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(replica::invalid(
                "A superseded archive cannot replace the verified current history",
            ));
        }
    }
    if let Some(reason) = db.query_row("SELECT reason FROM distributed_archive_failures WHERE project_id=?1 AND number=?2 LIMIT 1", params![project,number], |r|r.get::<_,String>(0)).optional()? {
        return Err(replica::invalid(&format!("Retained archive needs repair: {reason}")));
    }
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
    let catalog = transfer::Catalog::new(db)?;
    if catalog.contains(&manifest.key, &manifest.project, manifest.number)? {
        remember_pages(db, manifest)?;
        return Ok(());
    }
    if catalog.has_copy(&manifest.key, &manifest.project, manifest.number)? {
        transfer::map_local_history(db, &manifest.key, &manifest.project, manifest.number)?;
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

// A bad historical identity must not stop delivery of unrelated task facts.
// Keep the immutable manifest, block only its issue, and retry outside the hot
// writer. SQL/transport failures still propagate rather than becoming conflicts.
pub(super) fn receive_or_retain(
    db: &Connection,
    engine: &hey_boss_sync::Engine,
    manifest: &Manifest,
) -> Result<()> {
    let now = crate::issues::worker::now();
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM distributed_archive_supersessions WHERE key=?1)",
        [&manifest.key],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let attempted: Option<i64> = db
        .query_row(
            "SELECT attempted_at FROM distributed_archive_failures WHERE key=?1",
            [&manifest.key],
            |r| r.get(0),
        )
        .optional()?;
    if attempted.is_some_and(|at| at > now - 30_000 && at <= now) {
        return Ok(());
    }
    let entity = json!({"key":manifest.key,"project_id":manifest.project,"number":manifest.number})
        .to_string();
    match receive(db, engine, manifest) {
        Ok(()) => {
            if attempted.is_none() {
                return Ok(());
            }
            let tx = db.unchecked_transaction()?;
            let removed = tx.execute(
                "DELETE FROM distributed_archive_failures WHERE key=?1",
                [&manifest.key],
            )?;
            tx.execute(
                "DELETE FROM distributed_rejections WHERE table_name='archive' AND entity=?1",
                [entity],
            )?;
            if removed > 0 {
                tx.execute(
                    "INSERT OR IGNORE INTO distributed_dirty VALUES('issues',?1)",
                    [json!({"project_id":manifest.project,"number":manifest.number}).to_string()],
                )?;
            }
            tx.commit()?;
            Ok(())
        }
        Err(error)
            if error
                .downcast_ref::<crate::issues::Error>()
                .is_some_and(|e| {
                    matches!(e.code.as_str(), "archive_conflict" | "archive_unavailable")
                }) =>
        {
            let tx = db.unchecked_transaction()?;
            tx.execute("INSERT INTO distributed_archive_failures VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(key) DO UPDATE SET reason=excluded.reason,attempted_at=excluded.attempted_at",params![manifest.key,manifest.project,manifest.number,serde_json::to_string(manifest)?,error.to_string(),now])?;
            tx.execute("INSERT INTO distributed_rejections VALUES('archive',?1,?2) ON CONFLICT(table_name,entity) DO UPDATE SET reason=excluded.reason",params![entity,error.to_string()])?;
            tx.commit()?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub(super) fn retry_failed(db: &Connection, engine: &hey_boss_sync::Engine) -> Result<()> {
    let now = crate::issues::worker::now();
    let manifest: Option<String> = db.query_row("SELECT manifest FROM distributed_archive_failures WHERE attempted_at<=?1 OR attempted_at>?2 ORDER BY attempted_at,key LIMIT 1",params![now-30_000,now],|r|r.get(0)).optional()?;
    if let Some(manifest) = manifest {
        let manifest: Manifest = serde_json::from_str(&manifest)?;
        let issue = json!({"project_id":manifest.project,"number":manifest.number});
        let desired = projection::row(db, "issues", &issue)?;
        let replacement = desired.as_ref().and_then(|r| r["archive_key"].as_str());
        if let Some(replacement) = replacement.filter(|k| *k != manifest.key) {
            if transfer::Catalog::new(db)?.contains(
                replacement,
                &manifest.project,
                manifest.number,
            )? && matches!(
                crate::issues::archive::covered_by(
                    db,
                    &manifest.key,
                    replacement,
                    &manifest.project,
                    manifest.number
                ),
                Ok(true)
            ) {
                let tx = db.unchecked_transaction()?;
                tx.execute("INSERT OR REPLACE INTO distributed_archive_supersessions VALUES(?1,?2,?3,?4,?5)",params![manifest.key,replacement,manifest.project,manifest.number,now])?;
                tx.execute(
                    "DELETE FROM distributed_archive_failures WHERE key=?1",
                    [&manifest.key],
                )?;
                tx.execute("DELETE FROM distributed_rejections WHERE table_name='archive' AND entity=?1",[json!({"key":manifest.key,"project_id":manifest.project,"number":manifest.number}).to_string()])?;
                tx.execute(
                    "UPDATE distributed_archive_history SET complete=1 WHERE key=?1",
                    [&manifest.key],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO distributed_dirty VALUES('issues',?1)",
                    [issue.to_string()],
                )?;
                remember_pages(&tx, &manifest)?;
                tx.commit()?;
                return Ok(());
            }
        }
        receive_or_retain(db, engine, &manifest)?;
    }
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
    if header["kind"] == hey_boss_secrets::KIND {
        // The secret projector authenticates these independently. Never project
        // secret metadata or ciphertext into issue rows.
        return Ok(None);
    }
    if header["kind"] == "archive_page" {
        if !header["page"].is_object() {
            return Err(replica::invalid("Unsupported archive protocol"));
        }
        return Ok(None);
    }
    Ok(Some(serde_json::from_value(header)?))
}
