//! Immutable facts replicated with Fly's open-source cr-sqlite fork over SSH.
//! This library never connects to Fly or any cloud service.
//!
//! A local delivery log assigns a contiguous position to each first-seen fact.
//! Peers acknowledge this log, not cr-sqlite's sparse, per-origin db_version.
//! Relays forward facts from every origin without treating the highest version
//! received as proof that all earlier changes were received.
mod extension;
pub mod session;
#[cfg(test)]
mod tests;
pub mod wire;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io, path::Path, time::Duration};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const MAX_PAYLOAD: usize = 32 * 1024 * 1024;
const PAGE_BYTES: usize = 2 * 1024 * 1024;

fn invalid(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub position: i64,
    pub id: String,
    #[serde(with = "payload_encoding")]
    pub payload: String,
    pub site: Vec<u8>,
    pub version: i64,
    pub sequence: i64,
    pub timestamp: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub replica: String,
    pub after: i64,
    pub through: i64,
    pub records: Vec<Change>,
}

pub struct Engine {
    db: Connection,
    id: String,
}

impl Engine {
    pub fn open(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(10))?;
        extension::load(&db, parent)?;
        let mut engine = Self {
            db,
            id: String::new(),
        };
        engine.db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS records(id TEXT NOT NULL PRIMARY KEY,payload TEXT NOT NULL DEFAULT '');
            CREATE TABLE IF NOT EXISTS delivery(position INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE);
            CREATE TABLE IF NOT EXISTS receipts(replica TEXT PRIMARY KEY,position INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS sent(replica TEXT PRIMARY KEY,position INTEGER NOT NULL);
            CREATE TRIGGER IF NOT EXISTS record_delivery AFTER INSERT ON records BEGIN
                INSERT OR IGNORE INTO delivery(id) VALUES(NEW.id);
            END;")?;
        engine
            .db
            .query_row("SELECT crsql_as_crr('records')", [], |_| Ok(()))?;
        engine.id = engine
            .db
            .query_row("SELECT lower(hex(crsql_site_id()))", [], |r| r.get(0))?;
        Ok(engine)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns after the fact is durably saved locally. No network dependency.
    pub fn put(&mut self, payload: &str) -> Result<String> {
        if payload.is_empty() || payload.len() > MAX_PAYLOAD {
            return Err(invalid("Invalid replication payload length"));
        }
        let id = digest(payload.as_bytes());
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO records(id,payload) VALUES(?1,?2)",
            params![id, payload],
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn payload(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT payload FROM records WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn head(&self) -> Result<i64> {
        Ok(self
            .db
            .query_row("SELECT coalesce(max(position),0) FROM delivery", [], |r| {
                r.get(0)
            })?)
    }

    pub fn received(&self, replica: &str) -> Result<i64> {
        Ok(self
            .db
            .query_row(
                "SELECT position FROM receipts WHERE replica=?1",
                [replica],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn sent(&self, replica: &str) -> Result<i64> {
        Ok(self
            .db
            .query_row(
                "SELECT position FROM sent WHERE replica=?1",
                [replica],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn acknowledge(&mut self, replica: &str, position: i64) -> Result<()> {
        if position < 0 || position > self.head()? {
            return Err(invalid("Peer acknowledged an invalid position"));
        }
        self.db.execute("INSERT INTO sent VALUES(?1,?2) ON CONFLICT(replica) DO UPDATE SET position=max(position,excluded.position)", params![replica, position])?;
        Ok(())
    }

    pub fn batch(&self, after: i64) -> Result<Batch> {
        let tx = self.db.unchecked_transaction()?;
        let head: i64 =
            tx.query_row("SELECT coalesce(max(position),0) FROM delivery", [], |r| {
                r.get(0)
            })?;
        if after < 0 || after > head {
            return Err(invalid(
                "Replication cursor is beyond retained history; restore requires a new replica identity",
            ));
        }
        let mut records = Vec::new();
        {
            let mut rows = tx.prepare("SELECT d.position,r.id,r.payload FROM delivery d JOIN records r ON r.id=d.id WHERE d.position>?1 ORDER BY d.position LIMIT 128")?;
            let mut clock = tx.prepare("SELECT site_id,db_version,seq,ts FROM crsql_changes WHERE \"table\"='records' AND pk=crsql_pack_columns(?1) AND cid='payload'")?;
            let mut bytes = 0;
            for row in rows.query_map([after], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })? {
                let (position, id, payload) = row?;
                if !records.is_empty() && bytes + payload.len() > PAGE_BYTES {
                    break;
                }
                bytes += payload.len();
                let (site, version, sequence, timestamp) =
                    clock.query_row([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
                records.push(Change {
                    position,
                    id,
                    payload,
                    site,
                    version,
                    sequence,
                    timestamp,
                });
            }
        }
        let through = records.last().map_or(after, |r| r.position);
        tx.commit()?;
        Ok(Batch {
            replica: self.id.clone(),
            after,
            through,
            records,
        })
    }

    pub fn ingest(&mut self, batch: &Batch) -> Result<()> {
        if batch.replica.len() != 32
            || !batch.replica.bytes().all(|c| c.is_ascii_hexdigit())
            || batch.after < 0
            || batch.through < batch.after
            || batch.through - batch.after != batch.records.len() as i64
            || batch.records.len() > 128
        {
            return Err(invalid("Invalid replication batch envelope"));
        }
        for (index, record) in batch.records.iter().enumerate() {
            if record.position != batch.after + index as i64 + 1
                || record.site.len() != 16
                || record.version < 1
                || record.sequence < 0
                || record.timestamp.parse::<u64>().is_err()
                || record.payload.is_empty()
                || record.payload.len() > MAX_PAYLOAD
                || record.id != digest(record.payload.as_bytes())
            {
                return Err(invalid("Invalid or corrupt replication record"));
            }
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let received = tx
            .query_row(
                "SELECT position FROM receipts WHERE replica=?1",
                [&batch.replica],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        if batch.after > received {
            return Err(invalid("Replication batch leaves an unacknowledged gap"));
        }
        {
            let mut insert = tx.prepare("INSERT INTO crsql_changes(\"table\",pk,cid,val,col_version,db_version,site_id,cl,seq,ts) VALUES('records',crsql_pack_columns(?1),'payload',?2,1,?3,?4,1,?5,?6)")?;
            for record in &batch.records {
                insert.execute(params![
                    record.id,
                    record.payload,
                    record.version,
                    record.site,
                    record.sequence,
                    record.timestamp
                ])?;
            }
        }
        tx.execute("INSERT INTO receipts VALUES(?1,?2) ON CONFLICT(replica) DO UPDATE SET position=max(position,excluded.position)", params![batch.replica,batch.through])?;
        tx.commit()?;
        Ok(())
    }

    /// Application projections have their own transactional checkpoint.
    pub fn pending(&self, after: i64) -> Result<Vec<(i64, String, String)>> {
        let mut statement = self.db.prepare("SELECT d.position,r.id,r.payload FROM delivery d JOIN records r ON r.id=d.id WHERE d.position>?1 ORDER BY d.position LIMIT 128")?;
        let mut result = Vec::new();
        let mut bytes = 0;
        for row in statement.query_map([after], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?))
        })? {
            let row = row?;
            if !result.is_empty() && bytes + row.2.len() > PAGE_BYTES {
                break;
            }
            bytes += row.2.len();
            result.push(row);
        }
        Ok(result)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.db.query_row("SELECT crsql_finalize()", [], |_| Ok(()));
    }
}

mod payload_encoding {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        text: &str,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(text.as_bytes()))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<String, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = STANDARD.decode(text).map_err(serde::de::Error::custom)?;
        String::from_utf8(bytes).map_err(serde::de::Error::custom)
    }
}
