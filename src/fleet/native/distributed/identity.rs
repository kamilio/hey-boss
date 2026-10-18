use super::*;
use rusqlite::{OptionalExtension, params};
use serde_json::json;

/// Version-one shadow facts gave forwarded legacy history a new receiver ID.
/// Rebuild only derived state; the application and immutable journal stay intact.
pub(super) fn upgrade_shadow(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('distributed_meta') WHERE name='format')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        let format: u32 =
            db.query_row("SELECT format FROM distributed_meta WHERE id=1", [], |r| {
                r.get(0)
            })?;
        return if format == PROTOCOL {
            Ok(())
        } else {
            Err(replica::invalid("Unsupported distributed store format"))
        };
    }
    let tx = db.unchecked_transaction()?;
    if tx.query_row(
        "SELECT active=1 FROM distributed_meta WHERE id=1",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(replica::invalid(
            "Cannot rebuild an active version-one distributed store",
        ));
    }
    tx.execute_batch(
        "ALTER TABLE distributed_meta ADD COLUMN format INTEGER NOT NULL DEFAULT 2;
        DROP VIEW IF EXISTS distributed_job_runs;
        DROP TABLE IF EXISTS distributed_issue_graph;
        DROP TABLE distributed_fields;
        DROP TABLE distributed_dirty;
        DROP TABLE distributed_aliases;
        DROP TABLE distributed_rejections;
        DROP TABLE distributed_archives;
        DROP TABLE distributed_archive_history;
        DROP TABLE distributed_history_edits;
        DROP TABLE distributed_attempts;
        DROP TABLE distributed_job_attempts;
        DELETE FROM distributed_baseline;
        DELETE FROM distributed_outbox WHERE table_name='distributed_members';
        UPDATE distributed_meta SET exported=0,projected=0,
            legacy_cutoff=CASE WHEN controller=origin THEN 0
                ELSE coalesce((SELECT max(seq) FROM fleet_outbox),0) END;",
    )?;
    tx.execute_batch(SCHEMA)?;
    let syncing: i64 = tx.query_row("SELECT syncing FROM fleet_meta WHERE id=1", [], |r| {
        r.get(0)
    })?;
    tx.execute("UPDATE fleet_meta SET syncing=1 WHERE id=1", [])?;
    tx.execute("UPDATE distributed_members SET ready=0", [])?;
    tx.execute("UPDATE fleet_meta SET syncing=?1 WHERE id=1", [syncing])?;
    tx.commit()?;
    Ok(())
}

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
    controller.ok_or_else(|| {
        replica::invalid("Replica must identify its legacy controller before exporting history")
    })?;
    let mapped: Option<(String, i64)> = db.query_row(
        "SELECT origin,origin_id FROM fleet_row_ids WHERE table_name=?1 AND local_id=?2 ORDER BY rowid LIMIT 1",
        params![table,id], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    let entity = match mapped {
        Some((origin, id)) => format!("{origin}:{id}"),
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
    if let Some(id) = db
        .query_row(
            "SELECT local_id FROM distributed_aliases WHERE table_name=?1 AND entity=?2",
            params![table, entity],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(Some(id));
    }
    if !append(table) {
        return Err(replica::invalid("Unknown history table"));
    }
    let (origin, id) = entity
        .rsplit_once(':')
        .ok_or_else(|| replica::invalid("Invalid history identity"))?;
    let id: i64 = id
        .parse()
        .map_err(|_| replica::invalid("Invalid history identity"))?;
    let mut local: Option<i64> = db
        .query_row(
            "SELECT local_id FROM fleet_row_ids WHERE origin=?1 AND table_name=?2 AND origin_id=?3",
            params![origin, table, id],
            |r| r.get(0),
        )
        .optional()?;
    if local.is_none()
        && db.query_row(
            "SELECT origin=?1 FROM distributed_meta WHERE id=1",
            [origin],
            |r| r.get::<_, bool>(0),
        )?
    {
        local = db
            .query_row(&format!("SELECT id FROM {table} WHERE id=?1"), [id], |r| {
                r.get(0)
            })
            .optional()?;
    }
    if let Some(local) = local {
        if canonical(db, table, local)? != entity {
            return Err(replica::invalid(
                "Legacy history has conflicting creation identities",
            ));
        }
    }
    Ok(local)
}
