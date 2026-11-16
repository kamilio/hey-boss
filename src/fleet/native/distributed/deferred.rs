//! Keep a schema-incompatible row in the immutable journal. Only a known
//! incompatibility may advance the cursor; storage and protocol errors propagate.
use super::*;
use rusqlite::{OptionalExtension, params};

#[derive(Debug)]
pub(super) struct Incompatible(pub &'static str);
impl std::fmt::Display for Incompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Incompatible {}

// An unknown table or a move touches an unknown or multi-row scope. Fence
// its table conservatively until the original record can be interpreted.
fn scope(db: &Connection, fact: &Revision) -> Result<String> {
    if !capture::TABLES.contains(&fact.table.as_str()) {
        return Ok(String::new());
    }
    let mut key = None;
    for row in [&fact.before, &fact.after].into_iter().flatten() {
        let next = projection::key(db, &fact.table, row)?;
        if key.as_ref().is_some_and(|previous| previous != &next) {
            return Ok(String::new());
        }
        key = Some(next);
    }
    Ok(key.map(|key| key.to_string()).unwrap_or_default())
}

pub(super) fn record(
    db: &Connection,
    position: i64,
    id: &str,
    fact: &Revision,
    pending: &mut bool,
    apply: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    let mut entity = None;
    let mut reason = None;
    if *pending
        && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM distributed_deferred WHERE table_name=?1 AND position<?2)",
            params![fact.table, position],
            |r| r.get::<_, bool>(0),
        )?
    {
        let key = scope(db, fact)?;
        if db.query_row("SELECT EXISTS(SELECT 1 FROM distributed_deferred WHERE table_name=?1 AND position<?2 AND (entity IN ('',?3) OR ?3=''))",params![fact.table,position,key],|r|r.get::<_,bool>(0))? {
            reason=Some("Waiting for an earlier incompatible revision");
        }
        entity = Some(key);
    }
    if reason.is_none() {
        db.execute_batch("SAVEPOINT journal_row")?;
        match apply() {
            Ok(()) => {
                db.execute_batch("RELEASE journal_row")?;
                if *pending {
                    db.execute(
                        "DELETE FROM distributed_deferred WHERE position=?1",
                        [position],
                    )?;
                }
                return Ok(true);
            }
            Err(e) => {
                db.execute_batch("ROLLBACK TO journal_row; RELEASE journal_row")?;
                let Some(incompatible) = e.downcast_ref::<Incompatible>() else {
                    return Err(e);
                };
                reason = Some(incompatible.0);
            }
        }
    }
    let key = match entity {
        Some(k) => k,
        None => scope(db, fact)?,
    };
    db.execute("INSERT INTO distributed_deferred VALUES(?1,?2,?3,?4,?5,unixepoch()+30) ON CONFLICT(position) DO UPDATE SET reason=excluded.reason,retry_at=excluded.retry_at",params![position,id,fact.table,key,reason])?;
    *pending = true;
    Ok(false)
}

pub(super) fn due(
    db: &Connection,
    engine: &hey_boss_sync::Engine,
) -> Result<Option<(i64, String, String)>> {
    let next:Option<(i64,String)>=db.query_row("SELECT position,record FROM distributed_deferred WHERE retry_at<=unixepoch() ORDER BY retry_at,position LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((position, id)) = next else {
        return Ok(None);
    };
    let payload = engine
        .payload(&id)?
        .ok_or_else(|| replica::invalid("Missing deferred journal record"))?;
    Ok(Some((position, id, payload)))
}
