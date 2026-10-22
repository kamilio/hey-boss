use super::*;

/// A rotation is a permutation of integer positions. Capture it once, rather
/// than capturing every crossed task (including its unchanged requirements).
pub(super) fn rotate(db: &Connection, project: &str, from: i64, to: i64) -> Result<()> {
    let installed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_order_capture')",
        [],
        |r| r.get(0),
    )?;
    let compact = installed
        && db.query_row(
            "SELECT active=1 FROM distributed_meta WHERE id=1",
            [],
            |r| r.get::<_, bool>(0),
        )?;
    if compact {
        // Initial snapshots must establish the coordinates before we rotate
        // them. No full-queue fallback while an enrollment is still copying.
        let missing: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM issues i WHERE project_id=?1 AND NOT EXISTS(SELECT 1 FROM distributed_fields f WHERE f.table_name='issues' AND f.entity=json_object('number',i.number,'project_id',i.project_id) AND f.field='sort_order') AND NOT EXISTS(SELECT 1 FROM distributed_outbox o WHERE o.table_name='issues' AND o.before_json IS NULL AND json_extract(o.after_json,'$.project_id')=i.project_id AND json_extract(o.after_json,'$.number')=i.number))", [project], |r| r.get(0))?;
        if missing {
            return Err(Error::conflict(
                "Issue ordering is still synchronizing. Retry after the initial queue copy finishes.",
            ));
        }
        db.execute("UPDATE distributed_meta SET clock=clock+1 WHERE id=1", [])?;
        db.execute("INSERT INTO distributed_outbox SELECT clock,'issue_order',NULL,json_object('project_id',?1,'from',?2,'to',?3) FROM distributed_meta WHERE id=1", params![project,from,to])?;
        db.execute("UPDATE distributed_order_capture SET active=1", [])?;
    }
    db.execute("UPDATE issues SET sort_order=CASE WHEN sort_order=?2 THEN ?3 WHEN ?3<?2 THEN sort_order+1 ELSE sort_order-1 END WHERE project_id=?1 AND sort_order BETWEEN min(?2,?3) AND max(?2,?3)", params![project,from,to])?;
    if compact {
        db.execute("UPDATE distributed_order_capture SET active=0", [])?;
    }
    Ok(())
}
