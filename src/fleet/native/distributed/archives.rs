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
        return Ok(());
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
    for _ in 0..8 {
        let mut page = transfer::export_page(db, key, project, number, &cursor)?;
        // Legacy archives remember the creating companion, whereas the hot
        // migration uses the controller's IDs. Publish the same canonical
        // identity for both representations, including relayed archives.
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
        let id =
            engine.put(&json!({"protocol":1,"kind":"archive_page","page":page}).to_string())?;
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
            break;
        }
    }
    Ok(())
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
    let Some(row) = &fact.after else {
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

pub(super) fn receive(
    db: &Connection,
    engine: &hey_boss_sync::Engine,
    manifest: &Manifest,
) -> Result<()> {
    if transfer::Catalog::new(db)?.contains(&manifest.key, &manifest.project, manifest.number)? {
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
        if value["protocol"] != 1 || value["kind"] != "archive_page" {
            return Err(replica::invalid("Invalid archive page reference"));
        }
        complete = download.receive(&value["page"])?;
    }
    if !complete {
        return Err(replica::invalid("Incomplete archived input"));
    }
    drop(download);
    transfer::map_local_history(db, &manifest.key, &manifest.project, manifest.number)?;
    Ok(())
}

pub(super) fn revision(payload: &str) -> Result<Option<Revision>> {
    let header: Value = serde_json::from_str(payload)?;
    if header["kind"] == "archive_page" {
        if header["protocol"] != 1 || !header["page"].is_object() {
            return Err(replica::invalid("Unsupported archive protocol"));
        }
        return Ok(None);
    }
    Ok(Some(serde_json::from_value(header)?))
}
