//! Old revisions are disposable only when the retained state supersedes every
//! effect. Archives, resources, independent subscribers and deferred facts have
//! their own owners and are not retired by the issue-field projector.
use super::*;

pub(super) fn collect(db: &Connection, engine: &mut hey_boss_sync::Engine) -> Result<bool> {
    let snapshot = db.read_transaction()?;
    let applied = snapshot.query_row(
        "SELECT projected FROM distributed_meta WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    let scanned = engine.compact(applied, |position, _, payload| {
        if snapshot.query_row(
            "SELECT EXISTS(SELECT 1 FROM distributed_deferred WHERE position=?1)",
            [position],
            |r| r.get::<_, bool>(0),
        )? {
            return Ok(false);
        }
        let header: Value = serde_json::from_str(payload)?;
        if header["protocol"] != PROTOCOL || header.get("kind").is_some() {
            return Ok(false);
        }
        let fact: Revision = serde_json::from_value(header)?;
        projection::superseded(&snapshot, &fact)
    })?;
    snapshot.commit()?;
    Ok(scanned > 0)
}
