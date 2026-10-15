use super::*;
use rusqlite::{OptionalExtension, params};
use serde_json::json;

pub(super) fn append(table: &str) -> bool {
    matches!(table, "comments" | "events" | "artifact_comments")
}

pub(super) fn canonical(db: &Connection, table: &str, id: i64) -> Result<String> {
    if let Some(entity) = db
        .query_row(
            "SELECT entity FROM distributed_aliases WHERE table_name=?1 AND local_id=?2",
            params![table, id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(entity);
    }
    let (origin, controller): (String, Option<String>) = db.query_row(
        "SELECT origin,controller FROM distributed_meta WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let controller = controller.ok_or_else(|| {
        replica::invalid("Replica must identify its legacy controller before exporting history")
    })?;
    let mapped:Option<i64>=db.query_row("SELECT origin_id FROM fleet_row_ids WHERE origin=?1 AND table_name=?2 AND local_id=?3 ORDER BY origin_id LIMIT 1",params![controller,table,id],|r|r.get(0)).optional()?;
    let entity = match mapped {
        Some(id) => format!("{controller}:{id}"),
        None => format!("{origin}:{id}"),
    };
    db.execute(
        "INSERT INTO distributed_aliases VALUES(?1,?2,?3)",
        params![table, entity, id],
    )?;
    Ok(entity)
}

pub(super) fn normalize(db: &Connection, fact: &mut Revision) -> Result<()> {
    for row in [&mut fact.before, &mut fact.after].into_iter().flatten() {
        if append(&fact.table) {
            let id = row["id"]
                .as_i64()
                .ok_or_else(|| replica::invalid("Invalid local history ID"))?;
            row["id"] = json!(canonical(db, &fact.table, id)?);
        }
        if fact.table == "artifact_comments"
            && let Some(parent) = row["parent"].as_i64()
        {
            row["parent"] = json!(canonical(db, "artifact_comments", parent)?);
        }
        if fact.table == "events" {
            let mut data: Value = serde_json::from_str(
                row["data"]
                    .as_str()
                    .ok_or_else(|| replica::invalid("Invalid event data"))?,
            )?;
            if let Some(id) = data["comment_id"].as_i64() {
                data["comment_id"] = json!(canonical(db, "comments", id)?);
                row["data"] = json!(data.to_string());
            }
        }
    }
    Ok(())
}

pub(super) fn local(db: &Connection, table: &str, entity: &str) -> Result<Option<i64>> {
    Ok(db
        .query_row(
            "SELECT local_id FROM distributed_aliases WHERE table_name=?1 AND entity=?2",
            params![table, entity],
            |r| r.get(0),
        )
        .optional()?)
}
