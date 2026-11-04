use super::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS published_publications(id TEXT PRIMARY KEY,manifest TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS published_page_chunks(publication TEXT NOT NULL,hash TEXT NOT NULL,size INTEGER NOT NULL,PRIMARY KEY(publication,hash));
CREATE TABLE IF NOT EXISTS published_owners(id TEXT PRIMARY KEY,name TEXT NOT NULL,deployment TEXT NOT NULL,clock INTEGER NOT NULL,origin TEXT NOT NULL,generation TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS published_owner_name ON published_owners(name);
CREATE INDEX IF NOT EXISTS published_owner_generation ON published_owners(generation);
CREATE TABLE IF NOT EXISTS published_deletions(name TEXT PRIMARY KEY,clock INTEGER NOT NULL,origin TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS published_records(id TEXT PRIMARY KEY,publication TEXT NOT NULL,hash TEXT NOT NULL,deployment TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS published_garbage(hash TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS published_storage(id INTEGER PRIMARY KEY CHECK(id=1),clock INTEGER NOT NULL DEFAULT 0,indexed INTEGER NOT NULL DEFAULT 0,dirty INTEGER NOT NULL DEFAULT 1);
INSERT OR IGNORE INTO published_storage(id) VALUES(1);";

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='published_storage')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        let tx = db.unchecked_transaction()?;
        tx.execute_batch(super::SCHEMA)?;
        tx.execute_batch(SCHEMA)?;
        if !tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('published_pages') WHERE name='generation')",[],|r|r.get::<_,bool>(0))? {
            tx.execute_batch("ALTER TABLE published_pages ADD COLUMN generation TEXT NOT NULL DEFAULT ''")?;
        }
        tx.commit()?;
    }
    Ok(())
}

pub(crate) fn lock(database: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(database.with_extension("pages.lock"))?;
    file.lock()?;
    Ok(file)
}

fn distributed(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_meta')",
        [],
        |r| r.get(0),
    )?)
}

pub(super) fn stamp(db: &Connection) -> Result<(i64, String)> {
    if distributed(db)? {
        Ok(db.query_row(
            "SELECT clock,origin FROM distributed_meta WHERE id=1",
            [],
            |r| Ok::<_, rusqlite::Error>((r.get(0)?, r.get(1)?)),
        )?)
    } else {
        db.execute("UPDATE published_storage SET clock=clock+1 WHERE id=1", [])?;
        Ok((
            db.query_row("SELECT clock FROM published_storage WHERE id=1", [], |r| {
                r.get(0)
            })?,
            crate::issues::identity::host(),
        ))
    }
}

pub(crate) fn generation(row: &Value) -> String {
    row["generation"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            hey_boss_sync::digest(
                json!([
                    row["name"],
                    row["deployment"],
                    row["updated_at"],
                    row["machine"]
                ])
                .to_string()
                .as_bytes(),
            )
        })
}
fn publication(row: &Value) -> String {
    hey_boss_sync::digest(
        json!([
            row["name"],
            row["deployment"],
            row["updated_at"],
            row["machine"],
            generation(row)
        ])
        .to_string()
        .as_bytes(),
    )
}

pub(crate) fn remember(db: &Connection, row: &Value, clock: i64, origin: &str) -> Result<()> {
    if row["deleted"] == 1 {
        db.execute("INSERT INTO published_deletions VALUES(?1,?2,?3) ON CONFLICT(name) DO UPDATE SET clock=excluded.clock,origin=excluded.origin WHERE (excluded.clock,excluded.origin)>(clock,origin)",params![row["name"].as_str(),clock,origin])?;
        if distributed(db)? {
            db.execute(
                "INSERT OR IGNORE INTO distributed_dirty VALUES('published_pages',?1)",
                [json!({"name":row["name"]}).to_string()],
            )?;
        }
    } else {
        db.execute("INSERT INTO published_owners VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET clock=excluded.clock,origin=excluded.origin WHERE (excluded.clock,excluded.origin)>(clock,origin)",params![publication(row),row["name"].as_str(),row["deployment"].as_str(),clock,origin,generation(row)])?;
    }
    db.execute(
        "UPDATE published_storage SET clock=max(clock,?1),dirty=1 WHERE id=1",
        [clock],
    )?;
    Ok(())
}

pub(crate) fn retired(db: &Connection, id: &str) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM published_owners o JOIN published_owners birth ON birth.generation=o.generation JOIN published_deletions d ON d.name=birth.name WHERE (o.id=?1 OR o.generation=?1) AND (birth.clock,birth.origin)<=(d.clock,d.origin))",[id],|r|r.get(0))?)
}

pub(super) fn capture_content(db: &Connection, row: &Value) -> Result<()> {
    let id = publication(row);
    let generation = generation(row);
    let text: String = db.query_row(
        "SELECT manifest FROM published_deployments WHERE id=?1",
        [row["deployment"].as_str()],
        |r| r.get(0),
    )?;
    db.execute(
        "INSERT OR IGNORE INTO published_publications VALUES(?1,?2)",
        params![id, text],
    )?;
    let manifest: Manifest = serde_json::from_str(&text)?;
    let hashes: BTreeSet<_> = manifest.values().flat_map(|a| a.chunks.iter()).collect();
    for hash in hashes {
        db.execute("INSERT OR IGNORE INTO published_page_chunks SELECT ?1,hash,size FROM published_chunks WHERE hash=?2",params![generation,hash])?;
    }
    Ok(())
}

/// Give existing live publications their own resource facts before legacy data
/// is reclaimed. Identical owners on different machines have the same ID.
pub(crate) fn migrate_content(db: &Connection) -> Result<usize> {
    let rows = db.query_collect("SELECT name,deployment,updated_at,machine,generation FROM published_pages WHERE deleted=0",[],|r|Ok::<_,rusqlite::Error>(json!({"name":r.get::<_,String>(0)?,"deployment":r.get::<_,String>(1)?,"updated_at":r.get::<_,i64>(2)?,"machine":r.get::<_,String>(3)?,"generation":r.get::<_,String>(4)?,"deleted":0})))?;
    for row in rows {
        let id = publication(&row);
        if db.query_row(
            "SELECT EXISTS(SELECT 1 FROM published_publications WHERE id=?1)",
            [&id],
            |r| r.get::<_, bool>(0),
        )? {
            continue;
        }
        let Some(manifest) = super::manifest(db, row["deployment"].as_str().unwrap())? else {
            continue;
        };
        if !super::complete(db, &manifest)? {
            continue;
        }
        let (clock, origin) = if distributed(db)? {
            db.query_row("SELECT clock,origin FROM distributed_fields WHERE table_name='published_pages' AND entity=?1 AND field='@row'",[json!({"name":row["name"]}).to_string()],|r|Ok::<_,rusqlite::Error>((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).optional()?.unwrap_or((0,String::new()))
        } else {
            (0, String::new())
        };
        let tx = db.unchecked_transaction()?;
        tx.execute("UPDATE published_storage SET clock=clock WHERE id=1", [])?;
        remember(&tx, &row, clock, &origin)?;
        capture_content(&tx, &row)?;
        tx.commit()?;
        return Ok(1);
    }
    Ok(0)
}

pub(super) fn stored_size(db: &Connection, name: &str, manifest: &Manifest) -> Result<u64> {
    let database = Path::new(db.path().unwrap());
    let mut hashes: BTreeSet<_> = manifest
        .values()
        .flat_map(|a| a.chunks.iter().cloned())
        .collect();
    let deployments = db.query_collect("SELECT DISTINCT deployment FROM published_owners o WHERE name=?1 AND NOT EXISTS(SELECT 1 FROM published_owners birth JOIN published_deletions d ON d.name=birth.name WHERE birth.generation=o.generation AND (birth.clock,birth.origin)<=(d.clock,d.origin))",[name],|r|r.get::<_,String>(0))?;
    for id in deployments {
        if let Some(manifest) = super::manifest(db, &id)? {
            hashes.extend(manifest.values().flat_map(|a| a.chunks.iter().cloned()));
        }
    }
    let mut bytes = 0;
    for hash in hashes {
        for suffix in [".gz", ""] {
            match fs::symlink_metadata(root(database).join(format!("{hash}{suffix}"))) {
                Ok(metadata) if metadata.is_file() => bytes += metadata.len(),
                Ok(_) => return Err(Error::invalid("Invalid published content file")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(bytes)
}

pub(crate) fn index_record(db: &Connection, id: &str, fact: &Value) -> Result<()> {
    let row = &fact["after"];
    let table = fact["table"].as_str().unwrap_or("");
    if table == "published_pages" && row.is_object() {
        if fact["before"].is_object() && fact["before"]["deleted"] == 0 {
            // Legacy pages can have been created before the journal baseline.
            remember(db, &fact["before"], 0, "")?;
        }
        remember(
            db,
            row,
            fact["clock"].as_i64().unwrap_or(0),
            fact["origin"].as_str().unwrap_or(""),
        )?;
    }
    let (publication, hash, deployment) = match table {
        "published_page_chunks" => (
            row["publication"].as_str().unwrap_or(""),
            row["hash"].as_str().unwrap_or(""),
            String::new(),
        ),
        "published_publications" => (
            row["id"].as_str().unwrap_or(""),
            "",
            hey_boss_sync::digest(row["manifest"].as_str().unwrap_or("").as_bytes()),
        ),
        "published_chunks" => ("", row["hash"].as_str().unwrap_or(""), String::new()),
        "published_deployments" => ("", "", row["id"].as_str().unwrap_or("").to_owned()),
        _ => return Ok(()),
    };
    db.execute("UPDATE published_storage SET dirty=1 WHERE id=1", [])?;
    db.execute(
        "INSERT OR IGNORE INTO published_records VALUES(?1,?2,?3,?4)",
        params![id, publication, hash, deployment],
    )?;
    Ok(())
}

pub(crate) fn index_history(db: &Connection, engine: &hey_boss_sync::Engine) -> Result<bool> {
    let after: i64 = db.query_row(
        "SELECT indexed FROM published_storage WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    let (through, rows) = engine
        .pending_header(after, "\"table\":\"published_")
        .map_err(|e| Error::invalid(e.to_string()))?;
    if through == after {
        return Ok(false);
    }
    let tx = db.unchecked_transaction()?;
    for (_, id, payload) in rows {
        index_record(&tx, &id, &serde_json::from_str(&payload)?)?;
    }
    tx.execute(
        "UPDATE published_storage SET indexed=?1 WHERE id=1",
        [through],
    )?;
    tx.commit()?;
    Ok(true)
}

/// Collect only owners covered by a durable deletion. Unknown/in-flight owners
/// are retained, and each new publication supplies fresh resource facts even
/// when its content hashes were shared with an older, deleted publication.
pub(crate) fn collect(
    db: &Connection,
    mut engine: Option<&mut hey_boss_sync::Engine>,
) -> Result<usize> {
    if !db.query_row("SELECT dirty=1 AND EXISTS(SELECT 1 FROM published_deletions) FROM published_storage WHERE id=1",[],|r|r.get::<_,bool>(0))? { return Ok(0); }
    let distributed = distributed(db)?;
    if distributed {
        if engine.is_none() || db.query_row("SELECT exported<coalesce((SELECT max(clock) FROM distributed_outbox),0) OR EXISTS(SELECT 1 FROM distributed_dirty WHERE table_name LIKE 'published_%') FROM distributed_meta WHERE id=1",[],|r|r.get::<_,bool>(0))? { return Ok(0); }
        let head = engine
            .as_ref()
            .unwrap()
            .head()
            .map_err(|e| Error::invalid(e.to_string()))?;
        if db.query_row("SELECT projected<?1 OR (SELECT indexed FROM published_storage WHERE id=1)<?1 FROM distributed_meta WHERE id=1",[head],|r|r.get::<_,bool>(0))? { return Ok(0); }
    }
    let database = Path::new(db.path().unwrap());
    let _lock = lock(database)?;
    let tx = db.unchecked_transaction()?;
    // Take the writer before computing reachability; publishers use this same
    // file lock, and projectors cannot change ownership during collection.
    tx.execute("UPDATE published_storage SET clock=clock WHERE id=1", [])?;
    let dead: BTreeSet<String> = tx.query_collect("SELECT id FROM published_owners WHERE generation IN (SELECT o.generation FROM published_owners o JOIN published_deletions d ON d.name=o.name WHERE (o.clock,o.origin)<=(d.clock,d.origin))",[],|r|r.get(0))?.into_iter().collect();
    if dead.is_empty() {
        tx.execute("UPDATE published_storage SET dirty=0 WHERE id=1", [])?;
        tx.commit()?;
        return Ok(0);
    }
    let dead_json = serde_json::to_string(&dead)?;
    let generations: BTreeSet<String> = tx
        .query_collect(
            "SELECT generation FROM published_owners WHERE id IN (SELECT value FROM json_each(?1))",
            [&dead_json],
            |r| r.get(0),
        )?
        .into_iter()
        .collect();
    let scopes: BTreeSet<_> = dead.union(&generations).cloned().collect();
    let scopes_json = serde_json::to_string(&scopes)?;
    // A last-complete fallback from a deleted lifetime must not pin its files
    // when a different lifetime of the same name is still arriving.
    tx.execute("DELETE FROM published_live AS l WHERE EXISTS(SELECT 1 FROM published_owners o WHERE o.name=l.name AND o.deployment=l.deployment AND o.id IN (SELECT value FROM json_each(?1))) AND NOT EXISTS(SELECT 1 FROM published_owners o WHERE o.name=l.name AND o.deployment=l.deployment AND o.id NOT IN (SELECT value FROM json_each(?1)))",[&dead_json])?;
    let candidates: BTreeSet<String> = tx
        .query_collect(
            "SELECT deployment FROM published_owners WHERE id IN (SELECT value FROM json_each(?1))",
            [&dead_json],
            |r| r.get(0),
        )?
        .into_iter()
        .collect();
    let mut keep_deployments: BTreeSet<String> = tx.query_collect("SELECT deployment FROM published_owners WHERE id NOT IN (SELECT value FROM json_each(?1)) UNION SELECT deployment FROM published_pages WHERE deleted=0 UNION SELECT l.deployment FROM published_live l JOIN published_pages p ON p.name=l.name WHERE p.deleted=0",[&dead_json],|r|r.get(0))?.into_iter().collect();
    // A manifest can arrive before its page metadata on a reordered relay.
    for (id, text) in
        tx.query_collect("SELECT id,manifest FROM published_publications", [], |r| {
            Ok::<_, rusqlite::Error>((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
    {
        if !dead.contains(&id) {
            keep_deployments.insert(hey_boss_sync::digest(text.as_bytes()));
        }
    }
    let removed: BTreeSet<_> = candidates.difference(&keep_deployments).cloned().collect();
    let mut keep_hashes = BTreeSet::<String>::new();
    let mut remove_hashes: BTreeSet<String> = tx
        .query_collect("SELECT hash FROM published_garbage", [], |r| r.get(0))?
        .into_iter()
        .collect();
    for (id, text) in
        tx.query_collect("SELECT id,manifest FROM published_deployments", [], |r| {
            Ok::<_, rusqlite::Error>((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
    {
        let manifest: Manifest = serde_json::from_str(&text)?;
        let hashes = manifest.values().flat_map(|a| a.chunks.iter().cloned());
        if removed.contains(&id) {
            remove_hashes.extend(hashes);
        } else {
            keep_hashes.extend(hashes);
        }
    }
    for (owner, hash) in tx.query_collect(
        "SELECT publication,hash FROM published_page_chunks",
        [],
        |r| Ok::<_, rusqlite::Error>((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )? {
        if scopes.contains(&owner) {
            remove_hashes.insert(hash);
        } else {
            keep_hashes.insert(hash);
        }
    }
    remove_hashes.extend(tx.query_collect("SELECT hash FROM published_records WHERE publication IN (SELECT value FROM json_each(?1)) AND hash<>''",[&scopes_json],|r|r.get::<_,String>(0))?);
    remove_hashes.retain(|hash| !keep_hashes.contains(hash));
    let ids = tx
        .query_collect(
            "SELECT id,publication,hash,deployment FROM published_records",
            [],
            |r| {
                Ok::<_, rusqlite::Error>((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )?
        .into_iter()
        .filter(|(_, owner, hash, deployment)| {
            scopes.contains(owner)
                || (owner.is_empty()
                    && (remove_hashes.contains(hash) || removed.contains(deployment)))
        })
        .map(|(id, _, _, _)| id)
        .collect::<Vec<_>>();
    // Journal pruning commits first. A crash repeats collection from unchanged
    // ownership, and an old payload can never replace its retained tombstone.
    if let Some(engine) = engine.as_mut() {
        if ids.len() > 128 {
            engine
                .prune(&ids[..128])
                .map_err(|e| Error::invalid(e.to_string()))?;
            for id in &ids[..128] {
                tx.execute("DELETE FROM published_records WHERE id=?1", [id])?;
            }
            tx.commit()?;
            return Ok(128);
        }
        engine
            .prune(&ids)
            .map_err(|e| Error::invalid(e.to_string()))?;
    }
    if distributed {
        tx.execute("UPDATE fleet_meta SET syncing=2 WHERE id=1", [])?;
    }
    let mut unlinked = 0;
    for hash in &remove_hashes {
        tx.execute("INSERT OR IGNORE INTO published_garbage VALUES(?1)", [hash])?;
        for suffix in [".gz", ""] {
            match fs::remove_file(root(database).join(format!("{hash}{suffix}"))) {
                Ok(()) => unlinked += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        tx.execute("DELETE FROM published_chunks WHERE hash=?1", [hash])?;
    }
    if root(database).exists() {
        File::open(root(database))?.sync_all()?;
    }
    for id in &removed {
        tx.execute("DELETE FROM published_deployments WHERE id=?1", [id])?;
    }
    tx.execute(
        "DELETE FROM published_publications WHERE id IN (SELECT value FROM json_each(?1))",
        [&dead_json],
    )?;
    tx.execute(
        "DELETE FROM published_page_chunks WHERE publication IN (SELECT value FROM json_each(?1))",
        [&scopes_json],
    )?;
    for id in &ids {
        tx.execute("DELETE FROM published_records WHERE id=?1", [id])?;
    }
    if distributed {
        // Drop manifest copies in the projection. Keep tiny identity/clock
        // metadata and the named deletion facts needed for offline convergence.
        tx.execute("DELETE FROM distributed_fields WHERE (table_name='published_publications' AND json_extract(entity,'$.id') IN (SELECT value FROM json_each(?1))) OR (table_name='published_page_chunks' AND json_extract(entity,'$.publication') IN (SELECT value FROM json_each(?1)))",[&scopes_json])?;
        tx.execute("DELETE FROM distributed_fields WHERE table_name='published_deployments' AND json_extract(entity,'$.id') IN (SELECT value FROM json_each(?1))",[serde_json::to_string(&removed)?])?;
        tx.execute("UPDATE fleet_meta SET syncing=0 WHERE id=1", [])?;
    }
    tx.execute("UPDATE published_storage SET dirty=0 WHERE id=1", [])?;
    tx.commit()?;
    MANIFESTS
        .lock()
        .unwrap()
        .retain(|(path, id, _)| path != database.to_str().unwrap() || !removed.contains(id));
    Ok(ids.len() + unlinked)
}
