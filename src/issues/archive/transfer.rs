//! Immutable issue copies travel independently of hot replication transactions.
//! Chunks cap memory and frame size even when one history row is unusually large.
use super::*;
use crate::database::Connection as HotConnection;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::json;

const PAGE_BYTES: usize = 512 * 1024;
const PAGE_RECORDS: usize = 64;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Cursor {
    kind: String,
    id: String,
    offset: usize,
}

fn validate_key(key: &str) -> Result<()> {
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(unavailable("Invalid issue archive key"));
    }
    Ok(())
}

pub(crate) struct Catalog {
    archive: Option<Archive>,
}
impl Catalog {
    pub(crate) fn new(db: &HotConnection) -> Result<Self> {
        let path = archive_path(db)?;
        Ok(Self {
            archive: if path.try_exists()? {
                Some(Archive::read(&path)?)
            } else {
                None
            },
        })
    }
    pub(crate) fn refresh(&mut self, db: &HotConnection) -> Result<()> {
        if self.archive.is_none() {
            *self = Self::new(db)?;
        }
        Ok(())
    }
    pub(crate) fn contains(&self, key: &str, project: &str, number: i64) -> Result<bool> {
        validate_key(key)?;
        let Some(archive) = &self.archive else {
            return Ok(false);
        };
        Ok(archive.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_copies WHERE key=?1 AND project_id=?2 AND number=?3) AND NOT EXISTS(SELECT 1 FROM issue_origins WHERE archive_key=?1 AND local_id IS NULL)",params![key,project,number],|r|r.get(0))?)
    }
}

fn valid_kind(kind: &str) -> bool {
    matches!(
        kind,
        "issues" | "agents" | "comments" | "events" | "issue_status_updates"
    )
}

pub(crate) fn export_page(
    db: &HotConnection,
    key: &str,
    project: &str,
    number: i64,
    cursor: &Value,
) -> Result<Value> {
    validate_key(key)?;
    let archive = Archive::read(&archive_path(db)?)?;
    if !archive.db.query_row(
        "SELECT EXISTS(SELECT 1 FROM issue_copies WHERE key=?1 AND project_id=?2 AND number=?3)",
        params![key, project, number],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(unavailable("Requested issue archive is missing"));
    }
    let mut position: Cursor = if cursor.is_null() {
        Cursor {
            kind: "issues".into(),
            id: String::new(),
            offset: 0,
        }
    } else {
        serde_json::from_value(cursor.clone()).map_err(|_| unavailable("Invalid archive cursor"))?
    };
    if (!valid_kind(&position.kind) && !position.kind.is_empty())
        || position.id.len() > 1024
        || position.offset > MAX_OBJECT_BYTES
    {
        return Err(unavailable("Invalid archive cursor"));
    }
    let mut chunks = Vec::new();
    let mut bytes = 0;
    let mut done = false;
    while bytes < PAGE_BYTES && chunks.len() < PAGE_RECORDS {
        // offset=0 after a completed history row means seek to the next identity.
        // Root chunks use a distinct kind and can never collide with that seek.
        let item: Option<(String, String, String, usize)> = if position.kind == "issues" {
            archive.db.query_row("SELECT 'issues','',record_hash,length(CAST(record AS BLOB)) FROM issue_copies WHERE key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?
        } else if position.offset > 0 {
            archive.db.query_row("SELECT kind,text_id,record_hash,length(CAST(record AS BLOB)) FROM issue_history WHERE archive_key=?1 AND kind=?2 AND text_id=?3",params![key,position.kind,position.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?
        } else {
            archive.db.query_row("SELECT kind,text_id,record_hash,length(CAST(record AS BLOB)) FROM issue_history WHERE archive_key=?1 AND (kind,text_id)>(?2,?3) ORDER BY kind,text_id LIMIT 1",params![key,position.kind,position.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?
        };
        let Some((kind, id, hash, total)) = item else {
            if position.offset != 0 {
                return Err(unavailable("Archive cursor points to missing content"));
            }
            done = true;
            break;
        };
        let offset = position.offset;
        if total > MAX_OBJECT_BYTES || offset >= total {
            return Err(unavailable(
                "Archive record exceeds size limit or cursor bounds",
            ));
        }
        let take = (PAGE_BYTES - bytes).min(total - offset);
        let data: Vec<u8> = if kind == "issues" {
            archive.db.query_row(
                "SELECT substr(CAST(record AS BLOB),?2,?3) FROM issue_copies WHERE key=?1",
                params![key, offset + 1, take],
                |r| r.get(0),
            )?
        } else {
            archive.db.query_row("SELECT substr(CAST(record AS BLOB),?4,?5) FROM issue_history WHERE archive_key=?1 AND kind=?2 AND text_id=?3",params![key,kind,id,offset+1,take],|r|r.get(0))?
        };
        if data.len() != take {
            return Err(unavailable("Archive record changed during transfer"));
        }
        let identity = if matches!(kind.as_str(), "comments" | "events") {
            let source_id: i64 = id
                .parse()
                .map_err(|_| unavailable("Invalid archived history identity"))?;
            let saved:Option<(String,i64)> = archive.db.query_row("SELECT origin,origin_id FROM issue_origins WHERE archive_key=?1 AND kind=?2 AND source_id=?3",params![key,kind,source_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (origin,origin_id)=match saved {
                Some(identity)=>identity,
                None=> db.query_row("SELECT origin,origin_id FROM fleet_row_ids WHERE table_name=?1 AND local_id=?2 ORDER BY rowid LIMIT 1",params![kind,source_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).optional()?.unwrap_or((db.query_row("SELECT node FROM fleet_meta WHERE id=1",[],|r|r.get(0))?,source_id)),
            };
            json!({"origin":origin,"id":origin_id})
        } else {
            Value::Null
        };
        chunks.push(json!({"kind":kind,"id":id,"hash":hash,"offset":offset,"total":total,"data":STANDARD.encode(data),"identity":identity}));
        bytes += take;
        position = if offset + take < total {
            Cursor {
                kind,
                id,
                offset: offset + take,
            }
        } else if kind == "issues" {
            Cursor {
                kind: String::new(),
                id: String::new(),
                offset: 0,
            }
        } else {
            Cursor {
                kind,
                id,
                offset: 0,
            }
        };
    }
    Ok(
        json!({"ok":true,"key":key,"project":project,"number":number,"cursor":cursor,"next":if done { Value::Null } else {serde_json::to_value(position)?},"done":done,"chunks":chunks}),
    )
}

struct Pending {
    kind: String,
    id: String,
    hash: String,
    total: usize,
    identity: Value,
    bytes: Vec<u8>,
}

pub(crate) struct Download {
    archive: Archive,
    key: String,
    staging: String,
    project: String,
    number: i64,
    cursor: Value,
    pending: Option<Pending>,
    previous: Option<String>,
    done: bool,
}
impl Download {
    pub(crate) fn new(db: &HotConnection, key: &str, project: &str, number: i64) -> Result<Self> {
        validate_key(key)?;
        if !db.is_autocommit() {
            return Err(unavailable(
                "Archive transfer must precede the hot transaction",
            ));
        }
        let archive = Archive::open(&archive_path(db)?)?;
        let staging = format!("download-{}", crate::issues::worker::random_id()?);
        archive.db.execute(
            "INSERT INTO archive_downloads VALUES(?1,?2)",
            params![staging, crate::issues::worker::now()],
        )?;
        Ok(Self {
            archive,
            key: key.into(),
            staging,
            project: project.into(),
            number,
            cursor: Value::Null,
            pending: None,
            previous: None,
            done: false,
        })
    }

    pub(crate) fn receive(&mut self, page: &Value) -> Result<bool> {
        let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(page)?));
        if self.previous.as_ref() == Some(&fingerprint) {
            return Ok(self.done);
        }
        if self.done
            || page["key"] != self.key
            || page["project"] != self.project
            || page["number"] != self.number
            || page["cursor"] != self.cursor
            || page["ok"] != true
        {
            return Err(unavailable("Mismatched or out-of-order archive page"));
        }
        let chunks = page["chunks"]
            .as_array()
            .ok_or_else(|| unavailable("Missing archive chunks"))?;
        if chunks.len() > PAGE_RECORDS {
            return Err(unavailable("Archive page exceeds record limit"));
        }
        if self.archive.db.execute(
            "UPDATE archive_downloads SET updated_at=?2 WHERE staging=?1",
            params![self.staging, crate::issues::worker::now()],
        )? != 1
        {
            return Err(unavailable("Archive transfer expired; fetch a fresh copy"));
        }
        let mut page_bytes = 0;
        for chunk in chunks {
            let kind = chunk["kind"]
                .as_str()
                .filter(|kind| valid_kind(kind))
                .ok_or_else(|| unavailable("Invalid archive record kind"))?;
            let id = chunk["id"]
                .as_str()
                .filter(|id| id.len() <= 1024)
                .ok_or_else(|| unavailable("Invalid archive record ID"))?;
            let hash = chunk["hash"]
                .as_str()
                .ok_or_else(|| unavailable("Missing archive record checksum"))?;
            validate_key(hash)?;
            let total = chunk["total"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= MAX_OBJECT_BYTES as u64)
                .ok_or_else(|| unavailable("Invalid archive record size"))?
                as usize;
            let offset = chunk["offset"]
                .as_u64()
                .filter(|n| *n < total as u64)
                .ok_or_else(|| unavailable("Invalid archive record offset"))?
                as usize;
            let encoded = chunk["data"]
                .as_str()
                .filter(|data| data.len() <= PAGE_BYTES * 2)
                .ok_or_else(|| unavailable("Invalid archive chunk size"))?;
            let bytes = STANDARD
                .decode(encoded)
                .map_err(|_| unavailable("Invalid archive chunk encoding"))?;
            page_bytes += bytes.len();
            if bytes.is_empty() || page_bytes > PAGE_BYTES || offset + bytes.len() > total {
                return Err(unavailable("Archive chunk exceeds bounds"));
            }
            if self.pending.is_none() {
                if offset != 0 {
                    return Err(unavailable("Archive transfer skipped a record prefix"));
                }
                self.pending = Some(Pending {
                    kind: kind.into(),
                    id: id.into(),
                    hash: hash.into(),
                    total,
                    identity: chunk["identity"].clone(),
                    bytes: Vec::new(),
                });
            }
            let pending = self.pending.as_mut().unwrap();
            if pending.kind != kind
                || pending.id != id
                || pending.hash != hash
                || pending.total != total
                || pending.identity != chunk["identity"]
                || pending.bytes.len() != offset
            {
                return Err(unavailable("Archive chunks overlap or changed identity"));
            }
            pending.bytes.extend(bytes);
            if pending.bytes.len() == total {
                let pending = self.pending.take().unwrap();
                if format!("{:x}", Sha256::digest(&pending.bytes)) != pending.hash {
                    return Err(unavailable(
                        "Transferred archive record checksum does not match",
                    ));
                }
                self.save(pending)?;
            }
        }
        if page["done"] == true {
            if self.pending.is_some() || !page["next"].is_null() {
                return Err(unavailable("Archive transfer ended before its last record"));
            }
            history::verify_copy_as(
                &self.archive,
                &self.staging,
                &self.key,
                &self.project,
                self.number,
            )?;
            let tx = self.archive.db.unchecked_transaction()?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM issue_copies WHERE key=?1)",
                [&self.key],
                |r| r.get(0),
            )?;
            if exists {
                history::verify_copy(&self.archive, &self.key, &self.project, self.number)?;
            } else {
                for table in ["issue_history", "issue_origins"] {
                    tx.execute(
                        &format!("UPDATE {table} SET archive_key=?2 WHERE archive_key=?1"),
                        params![self.staging, self.key],
                    )?;
                }
                tx.execute("UPDATE issue_copies SET key=?2,comments=(SELECT count(*) FROM issue_history WHERE archive_key=?2 AND kind='comments') WHERE key=?1",params![self.staging,self.key])?;
            }
            tx.execute(
                "DELETE FROM archive_downloads WHERE staging=?1",
                [&self.staging],
            )?;
            tx.commit()?;
            self.done = true;
        } else if page["done"] != false || page["next"].is_null() || chunks.is_empty() {
            return Err(unavailable("Archive transfer made no progress"));
        }
        self.cursor = page["next"].clone();
        self.previous = Some(fingerprint);
        Ok(self.done)
    }

    fn save(&self, pending: Pending) -> Result<()> {
        let record: Value = serde_json::from_slice(&pending.bytes)?;
        let tx = self.archive.db.unchecked_transaction()?;
        if pending.kind == "issues" {
            if record["project_id"] != self.project
                || record["number"] != self.number
                || !pending.id.is_empty()
            {
                return Err(unavailable("Archive root belongs to another issue"));
            }
            tx.execute("INSERT INTO issue_copies(key,project_id,number,record,comments,record_hash) VALUES(?1,?2,?3,?4,0,?5)",params![self.staging,self.project,self.number,String::from_utf8(pending.bytes).map_err(|_|unavailable("Invalid archive text"))?,pending.hash])?;
        } else {
            let id = record["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| record["id"].to_string());
            if id != pending.id
                || (pending.kind != "agents"
                    && (record["project_id"] != self.project
                        || record["issue_number"] != self.number))
            {
                return Err(unavailable("Archive history belongs to another identity"));
            }
            history::save_row(
                &tx,
                &self.staging,
                &pending.kind,
                &record,
                &mut Sha256::new(),
            )?;
            if matches!(pending.kind.as_str(), "comments" | "events") {
                let origin = pending.identity["origin"]
                    .as_str()
                    .filter(|s| s.len() <= 8192)
                    .ok_or_else(|| unavailable("Invalid archive origin"))?;
                let origin_id = pending.identity["id"]
                    .as_i64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| unavailable("Invalid archive origin ID"))?;
                let source_id = record["id"]
                    .as_i64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| unavailable("Invalid archive source ID"))?;
                tx.execute("INSERT INTO issue_origins(archive_key,kind,source_id,origin,origin_id) VALUES(?1,?2,?3,?4,?5)",params![self.staging,pending.kind,source_id,origin,origin_id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
impl Drop for Download {
    fn drop(&mut self) {
        if let Ok(tx) = self.archive.db.unchecked_transaction() {
            let cleanup = (|| -> rusqlite::Result<()> {
                for table in ["issue_history", "issue_origins"] {
                    tx.execute(
                        &format!("DELETE FROM {table} WHERE archive_key=?1"),
                        [&self.staging],
                    )?;
                }
                tx.execute("DELETE FROM issue_copies WHERE key=?1", [&self.staging])?;
                tx.execute(
                    "DELETE FROM archive_downloads WHERE staging=?1",
                    [&self.staging],
                )?;
                Ok(())
            })();
            if cleanup.is_ok() {
                let _ = tx.commit();
            }
        }
    }
}

/// A killed downloader can leave partial copies, which are never visible to
/// readers. Reclaim only stale staging rows; published generations remain valid
/// for old snapshots, peers, and backups. Each cold transaction is bounded too.
pub(super) fn cleanup_downloads(db: &HotConnection, now: i64) -> Result<usize> {
    let path = archive_path(db)?;
    if !path.try_exists()? {
        return Ok(0);
    }
    let archive = Archive::open(&path)?;
    let tx = archive.db.unchecked_transaction()?;
    let staging: Option<String> = tx.query_row("SELECT staging FROM archive_downloads WHERE updated_at<=?1 ORDER BY updated_at LIMIT 1", [now.saturating_sub(GRACE_MS)], |r|r.get(0)).optional()?;
    let Some(staging) = staging else {
        return Ok(0);
    };
    let mut removed = tx.execute("DELETE FROM issue_history WHERE archive_key=?1 AND (kind,text_id) IN (SELECT kind,text_id FROM issue_history WHERE archive_key=?1 LIMIT 16)", [&staging])?;
    removed += tx.execute("DELETE FROM issue_origins WHERE archive_key=?1 AND (kind,source_id) IN (SELECT kind,source_id FROM issue_origins WHERE archive_key=?1 LIMIT 16)", [&staging])?;
    if !tx.query_row("SELECT EXISTS(SELECT 1 FROM issue_history WHERE archive_key=?1) OR EXISTS(SELECT 1 FROM issue_origins WHERE archive_key=?1)", [&staging], |r|r.get::<_,bool>(0))? {
        removed += tx.execute("DELETE FROM issue_copies WHERE key=?1", [&staging])?;
        removed += tx.execute("DELETE FROM archive_downloads WHERE staging=?1", [&staging])?;
    }
    tx.commit()?;
    Ok(removed)
}

/// Reserve local append identities before publishing an imported manifest.
/// Hot reservations commit first; repeating after a crash reuses those identities.
/// Cold writes and transfer work never happen inside the hot writer lease.
pub(crate) fn map_local_history(
    db: &HotConnection,
    key: &str,
    project: &str,
    number: i64,
) -> Result<()> {
    if !db.is_autocommit() {
        return Err(unavailable(
            "Archive identity mapping must precede the hot transaction",
        ));
    }
    let archive = Archive::open(&archive_path(db)?)?;
    history::verify_copy(&archive, key, project, number)?;
    loop {
        let origins=archive.db.prepare("SELECT kind,source_id,origin,origin_id FROM issue_origins WHERE archive_key=?1 AND local_id IS NULL ORDER BY kind,source_id LIMIT 16")?.query_map([key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if origins.is_empty() {
            break;
        }
        let mut mapped = Vec::new();
        let tx = crate::database::Transaction::new_unchecked(
            db,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let own: String =
            tx.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
        for (kind, source_id, origin, origin_id) in origins {
            if !matches!(kind.as_str(), "comments" | "events") {
                return Err(unavailable("Invalid archived append kind"));
            }
            let known:Option<i64>=tx.query_row("SELECT local_id FROM fleet_row_ids WHERE origin=?1 AND table_name=?2 AND origin_id=?3",params![origin,kind,origin_id],|r|r.get(0)).optional()?;
            let id = if let Some(known) = known {
                known
            } else if origin == own {
                origin_id
            } else {
                tx.execute("INSERT INTO sqlite_sequence(name,seq) SELECT ?1,0 WHERE NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name=?1)",[&kind])?;
                tx.query_row(&format!("UPDATE sqlite_sequence SET seq=max(seq,coalesce((SELECT max(id) FROM {kind}),0))+1 WHERE name=?1 RETURNING seq"),[&kind],|r|r.get(0))?
            };
            let existing: Option<(String, i64)> = tx
                .query_row(
                    &format!("SELECT project_id,issue_number FROM {kind} WHERE id=?1"),
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if existing.is_some_and(|scope| scope != (project.to_owned(), number)) {
                return Err(Error::new(
                    "archive_conflict",
                    "An imported history identity belongs to another issue",
                ));
            }
            tx.execute("INSERT OR IGNORE INTO fleet_row_ids(origin,table_name,origin_id,local_id) VALUES(?1,?2,?3,?4)",params![origin,kind,origin_id,id])?;
            // Own-origin rows may have been removed on a previous replica.
            // Keep SQLite's next allocation above every restored identity.
            tx.execute("INSERT INTO sqlite_sequence(name,seq) SELECT ?1,?2 WHERE NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name=?1)",params![kind,id])?;
            tx.execute(
                "UPDATE sqlite_sequence SET seq=max(seq,?2) WHERE name=?1",
                params![kind, id],
            )?;
            mapped.push((kind, source_id, id));
        }
        tx.commit()?;
        let tx = archive.db.unchecked_transaction()?;
        for (kind, source_id, id) in mapped {
            tx.execute("UPDATE issue_origins SET local_id=?4 WHERE archive_key=?1 AND kind=?2 AND source_id=?3",params![key,kind,source_id,id])?;
        }
        tx.commit()?;
    }
    Ok(())
}
