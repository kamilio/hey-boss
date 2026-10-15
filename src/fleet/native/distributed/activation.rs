use super::super::context::Context;
use super::*;
use rusqlite::{OptionalExtension, params};
use serde_json::json;

pub(in crate::fleet::native) fn enroll(ctx: &Context, node: &str, slot: u16) -> Result<()> {
    if node.len() != 32 || !node.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(replica::invalid("Invalid machine identity"));
    }
    let db = ctx.db()?;
    install(&db, &ctx.node)?;
    let tx = db.unchecked_transaction()?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT slot FROM distributed_members WHERE node=?1",
            [node],
            |r| r.get(0),
        )
        .optional()?;
    if existing.is_some_and(|n| n != i64::from(slot)) {
        return Err(replica::invalid(
            "An enrolled machine cannot change its number namespace",
        ));
    }
    tx.execute(
        "INSERT OR IGNORE INTO distributed_members(node,slot) VALUES(?1,?2)",
        params![node, slot],
    )?;
    if !tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM distributed_members WHERE node=?1 AND slot=?2)",
        params![node, slot],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(replica::invalid(
            "Number namespace is already assigned to another machine",
        ));
    }
    tx.commit()?;
    Ok(())
}

pub(in crate::fleet::native) fn activate(ctx: &Context) -> Result<()> {
    let Some(_lock) = ctx.lock("distributed-sync.lock", true)? else {
        return Err(replica::invalid("Sync is busy"));
    };
    let db = ctx.db()?;
    install(&db, &ctx.node)?;
    if db.query_row(
        "SELECT active=1 FROM distributed_meta WHERE id=1",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let mut engine = hey_boss_sync::Engine::open(&ctx.path.with_extension("sync.db"))?;
    while export(&db, &mut engine)? > 0 {}
    while import(&db, &engine)? > 0 {}
    let tx = db.unchecked_transaction()?;
    if tx.query_row("SELECT EXISTS(SELECT 1 FROM fleet_outbox WHERE seq<=(SELECT legacy_cutoff FROM distributed_meta WHERE id=1))", [], |r|r.get::<_,bool>(0))? {
        return Err(replica::invalid("Legacy edits from before shadow capture must be acknowledged by the old relay before cutover"));
    }
    let slot: i64 = tx
        .query_row(
            "SELECT slot FROM distributed_members WHERE node=?1",
            [&ctx.node],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| replica::invalid("Enroll this machine's number namespace before cutover"))?;
    let ready:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM distributed_members WHERE node=(SELECT controller FROM distributed_meta WHERE id=1) AND ready=1) AND NOT EXISTS(SELECT 1 FROM distributed_baseline WHERE complete=0) AND (SELECT exported>=coalesce((SELECT max(clock) FROM distributed_outbox),0) AND projected=?1 FROM distributed_meta WHERE id=1)",[engine.head()?],|r|r.get(0))?;
    if !ready {
        return Err(replica::invalid(
            "Initial capture and SSH transfer must finish before cutover",
        ));
    }
    let base = slot << 32;
    tx.execute(
        "UPDATE distributed_meta SET active=1,slot=?1 WHERE id=1",
        [slot],
    )?;
    tx.execute("UPDATE fleet_meta SET role='peer',syncing=1 WHERE id=1", [])?;
    tx.execute("DELETE FROM fleet_allocations", [])?;
    tx.execute("UPDATE projects SET next_number=max(?1+1,coalesce((SELECT max(number)+1 FROM issues WHERE project_id=projects.id AND number>?1 AND number<?2),?1+1),CASE WHEN next_number>?1 AND next_number<?2 THEN next_number ELSE ?1+1 END)",params![base,base+(1i64<<32)])?;
    tx.execute_batch("CREATE TRIGGER IF NOT EXISTS distributed_project_numbers AFTER INSERT ON projects WHEN (SELECT active FROM distributed_meta WHERE id=1)=1 BEGIN UPDATE projects SET next_number=((SELECT slot FROM distributed_meta WHERE id=1)<<32)+1 WHERE id=NEW.id; END;")?;
    tx.execute_batch("CREATE TRIGGER IF NOT EXISTS distributed_number_guard BEFORE INSERT ON issues
        WHEN (SELECT active FROM distributed_meta WHERE id=1)=1 AND (SELECT syncing FROM fleet_meta WHERE id=1)=0
        AND (NEW.number<=((SELECT slot FROM distributed_meta WHERE id=1)<<32) OR NEW.number>=(((SELECT slot FROM distributed_meta WHERE id=1)+1)<<32))
        BEGIN SELECT RAISE(ABORT,'Machine task number namespace exhausted or invalid'); END;")?;
    tx.execute("UPDATE fleet_meta SET syncing=0 WHERE id=1", [])?;
    // Old capture is fenced even if a still-running worker uses an older binary.
    for (table, _) in replica::TABLES {
        for action in ["INSERT", "UPDATE", "DELETE"] {
            tx.execute_batch(&format!(
                "DROP TRIGGER IF EXISTS fleet_capture_{table}_{action}"
            ))?;
        }
    }
    // Validate and materialize the complete shadow state before exposing peer
    // execution. A missing dependency rolls back the whole cutover.
    while materialize::apply(&tx)? > 0 {}
    let pending: i64 = tx.query_row("SELECT count(*) FROM distributed_dirty", [], |r| r.get(0))?;
    if pending > 0 {
        let errors = replica::rows(
            &tx,
            "SELECT table_name,entity,reason FROM distributed_rejections ORDER BY table_name,entity LIMIT 5",
            &[],
        )?;
        return Err(replica::invalid(&format!(
            "Cutover has {pending} unresolved projections: {}",
            serde_json::to_string(&errors)?
        )));
    }
    tx.commit()?;
    Ok(())
}

pub(in crate::fleet::native) fn status(ctx: &Context) -> Result<Value> {
    let db = ctx.db()?;
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_meta')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(json!({"mode":"legacy"}));
    }
    let mut status=replica::rows(&db,"SELECT origin,active,slot,exported,projected,(SELECT count(*) FROM distributed_outbox WHERE clock>m.exported) AS pending_local,(SELECT count(*) FROM distributed_dirty) AS pending_projection,(SELECT count(*) FROM distributed_rejections) AS conflicts,(SELECT count(*) FROM distributed_baseline WHERE complete=0) AS baseline_remaining FROM distributed_meta m WHERE id=1",&[])?.remove(0);
    status["mode"] = json!(if status["active"] == 1 {
        "distributed"
    } else {
        "shadow"
    });
    status["members"] = json!(replica::rows(
        &db,
        "SELECT * FROM distributed_members ORDER BY slot",
        &[]
    )?);
    status["legacy_pending"] = json!(db.query_row("SELECT count(*) FROM fleet_outbox WHERE seq<=(SELECT legacy_cutoff FROM distributed_meta WHERE id=1)", [], |r|r.get::<_,i64>(0))?);
    status["projection_errors"] = json!(replica::rows(
        &db,
        "SELECT table_name,entity,reason FROM distributed_rejections ORDER BY table_name,entity LIMIT 20",
        &[]
    )?);
    Ok(status)
}
