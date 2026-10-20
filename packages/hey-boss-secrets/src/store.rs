use crate::{
    Enrollment, Error, KIND, MAX_SECRET, Result, Secret,
    backend::{self, Backend},
    crypto::{self, Identity},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    record: String,
    name: String,
    owner: Enrollment,
    version: u64,
    updated_ms: u64,
    members: Vec<Enrollment>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedPolicy {
    policy: Policy,
    signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    access: SignedPolicy,
    writer: Enrollment,
    modified_ms: u64,
    logical: u64,
    kind: String,
    deleted: bool,
    ciphertext: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    body: Body,
    signature: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    protocol: u32,
    kind: String,
    revision: Revision,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub owner: String,
    pub owner_label: String,
    pub writer: String,
    pub writer_label: String,
    pub revision: String,
    pub access_version: u64,
    pub recipients: Vec<String>,
    pub updated_ms: u64,
    pub deleted: bool,
    pub status: String,
}
#[derive(Serialize)]
pub struct Inventory {
    pub machine: Option<Enrollment>,
    pub backend: String,
    pub records: Vec<Record>,
    pub machines: Vec<Enrollment>,
}

pub struct Store {
    root: PathBuf,
    journal: PathBuf,
    db: Connection,
    backend: Box<dyn Backend>,
}
fn storage<T>(r: std::result::Result<T, impl std::fmt::Display>) -> Result<T> {
    r.map_err(|_| Error::StorageFailure)
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
        && name
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}
impl Revision {
    fn id(&self) -> Result<String> {
        Ok(hey_boss_sync::digest(&crypto::encode(self)?))
    }
    fn validate(&self) -> Result<()> {
        let b = &self.body;
        let p = &b.access.policy;
        p.owner.validate()?;
        b.writer.validate()?;
        if !valid_name(&p.name)
            || p.version == 0
            || p.members.is_empty()
            || p.members.len() > 64
            || p.record
                != hey_boss_sync::digest(format!("{}:{}", p.owner.machine.id, p.name).as_bytes())
            || !matches!(b.kind.as_str(), "value" | "bundle" | "blob" | "cookies")
            || b.ciphertext.len() > 2 * MAX_SECRET
            || b.deleted != b.ciphertext.is_empty()
            || b.modified_ms < p.updated_ms
        {
            return Err(Error::InvalidRecord);
        }
        let mut ids = std::collections::BTreeSet::new();
        for m in &p.members {
            m.validate()?;
            if !ids.insert(&m.machine.id) {
                return Err(Error::InvalidRecord);
            }
        }
        if !p.members.contains(&p.owner) || !p.members.contains(&b.writer) {
            return Err(Error::InvalidRecord);
        }
        crypto::verify(&p.owner.machine.signing_key, p, &b.access.signature)?;
        crypto::verify(&b.writer.machine.signing_key, b, &self.signature)
    }
    /// Access changes dominate data written under an obsolete access policy.
    /// Within that policy the hybrid clock and stable identity resolve ties.
    fn order(&self) -> Result<(u64, u64, String, u64, u64, String, String)> {
        let p = &self.body.access.policy;
        Ok((
            p.version,
            p.updated_ms,
            hey_boss_sync::digest(&crypto::encode(&self.body.access)?),
            self.body.modified_ms,
            self.body.logical,
            self.body.writer.machine.id.clone(),
            self.id()?,
        ))
    }
}
impl Store {
    pub fn open(root: &Path, journal: &Path, backend: Box<dyn Backend>) -> Result<Self> {
        backend::private_dir(root)?;
        let path = root.join("index.db");
        backend::private_file(&path)?;
        let db = storage(Connection::open(path))?;
        storage(db.busy_timeout(std::time::Duration::from_secs(10)))?;
        storage(db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS revisions(id TEXT PRIMARY KEY,record TEXT NOT NULL,name TEXT NOT NULL,payload TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS revision_record ON revisions(record);
            CREATE INDEX IF NOT EXISTS revision_name ON revisions(name);
            CREATE TABLE IF NOT EXISTS trusted(id TEXT PRIMARY KEY,enrollment TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS cursor(id INTEGER PRIMARY KEY CHECK(id=1),position INTEGER NOT NULL);
            INSERT OR IGNORE INTO cursor VALUES(1,0);"))?;
        Ok(Self {
            root: root.into(),
            journal: journal.into(),
            db,
            backend,
        })
    }
    pub fn journal_path(&self) -> &Path {
        &self.journal
    }
    fn lock(&self) -> Result<std::fs::File> {
        let f = backend::private_file(&self.root.join("write.lock"))?;
        storage(f.lock())?;
        Ok(f)
    }
    pub fn enrollment(&self) -> Result<Enrollment> {
        let bytes =
            std::fs::read(self.root.join("machine.json")).map_err(|_| Error::NotInitialized)?;
        let e: Enrollment = serde_json::from_slice(&bytes).map_err(|_| Error::InvalidRecord)?;
        e.validate()?;
        Ok(e)
    }
    pub fn initialize(&mut self, label: &str) -> Result<Enrollment> {
        let _lock = self.lock()?;
        if self
            .root
            .join("machine.json")
            .try_exists()
            .map_err(|_| Error::StorageFailure)?
        {
            let e = self.enrollment()?;
            self.identity()?;
            return Ok(e);
        }
        // A failed public-file publication must not replace an existing identity.
        let identity = match self.backend.get("identity")? {
            Some(bytes) => {
                serde_json::from_slice(bytes.expose()).map_err(|_| Error::InvalidRecord)?
            }
            None => {
                let id = Identity::generate();
                let bytes = Secret::new(crypto::encode(&id)?);
                self.backend.put("identity", bytes.expose())?;
                id
            }
        };
        let enrollment = identity.enrollment(label)?;
        let path = self.root.join("machine.json");
        backend::write_private(&path, &crypto::encode(&enrollment)?)?;
        self.trust(&enrollment)?;
        Ok(enrollment)
    }
    fn identity(&self) -> Result<Identity> {
        let e = self.enrollment()?;
        let raw = self.backend.get("identity")?.ok_or(Error::NotInitialized)?;
        let id: Identity =
            serde_json::from_slice(raw.expose()).map_err(|_| Error::InvalidRecord)?;
        if id.enrollment(&e.machine.label)? != e {
            return Err(Error::InvalidRecord);
        }
        Ok(id)
    }
    pub fn trust(&mut self, enrollment: &Enrollment) -> Result<()> {
        enrollment.validate()?;
        let old: Option<String> = storage(
            self.db
                .query_row(
                    "SELECT enrollment FROM trusted WHERE id=?1",
                    [&enrollment.machine.id],
                    |r| r.get(0),
                )
                .optional(),
        )?;
        let encoded =
            String::from_utf8(crypto::encode(enrollment)?).map_err(|_| Error::InvalidRecord)?;
        if old.as_ref().is_some_and(|o| o != &encoded) {
            return Err(Error::Conflict);
        }
        storage(self.db.execute(
            "INSERT OR IGNORE INTO trusted VALUES(?1,?2)",
            params![enrollment.machine.id, encoded],
        ))?;
        Ok(())
    }
    fn trusted(&self, e: &Enrollment) -> Result<bool> {
        let encoded = String::from_utf8(crypto::encode(e)?).map_err(|_| Error::InvalidRecord)?;
        storage(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM trusted WHERE id=?1 AND enrollment=?2)",
            params![e.machine.id, encoded],
            |r| r.get(0),
        ))
    }
    pub fn synchronize(&mut self) -> Result<()> {
        let journal = storage(hey_boss_sync::Engine::open(&self.journal))?;
        loop {
            let after: i64 = storage(
                self.db
                    .query_row("SELECT position FROM cursor", [], |r| r.get(0)),
            )?;
            let entries = storage(journal.pending(after))?;
            if entries.is_empty() {
                break;
            }
            let tx = storage(self.db.transaction())?;
            for (position, _, payload) in &entries {
                if payload.contains(KIND)
                    && let Ok(w) = serde_json::from_str::<Wire>(payload)
                    && w.protocol == 2
                    && w.kind == KIND
                    && w.revision.validate().is_ok()
                {
                    let rev = &w.revision;
                    let p = &rev.body.access.policy;
                    let previous: Option<String> = storage(
                        tx.query_row(
                            "SELECT payload FROM revisions WHERE record=?1",
                            [&p.record],
                            |r| r.get(0),
                        )
                        .optional(),
                    )?;
                    let previous: Option<Revision> = previous
                        .map(|s| serde_json::from_str(&s).map_err(|_| Error::InvalidRecord))
                        .transpose()?;
                    if previous
                        .as_ref()
                        .map(|r| r.order())
                        .transpose()?
                        .is_some_and(|o| rev.order().is_ok_and(|n| n <= o))
                    {
                        storage(
                            tx.execute("UPDATE cursor SET position=max(position,?1)", [position]),
                        )?;
                        continue;
                    }
                    // History remains immutable in the shared journal. The
                    // local index retains only the winning ciphertext.
                    storage(tx.execute("DELETE FROM revisions WHERE record=?1", [&p.record]))?;
                    storage(tx.execute(
                        "INSERT OR IGNORE INTO revisions VALUES(?1,?2,?3,?4)",
                        params![
                            rev.id()?,
                            p.record,
                            p.name,
                            serde_json::to_string(rev).map_err(|_| Error::InvalidRecord)?
                        ],
                    ))?;
                }
                storage(tx.execute("UPDATE cursor SET position=max(position,?1)", [position]))?;
            }
            storage(tx.commit())?;
        }
        Ok(())
    }
    fn latest(&self, name: &str) -> Result<Option<Revision>> {
        let mut statement = storage(
            self.db
                .prepare("SELECT payload FROM revisions WHERE name=?1 OR record=?1"),
        )?;
        let rows = storage(statement.query_map([name], |r| r.get::<_, String>(0)))?;
        let mut records = BTreeMap::<String, Revision>::new();
        for row in rows {
            let rev: Revision =
                serde_json::from_str(&storage(row)?).map_err(|_| Error::InvalidRecord)?;
            let key = rev.body.access.policy.record.clone();
            if records
                .get(&key)
                .map(|r| r.order())
                .transpose()?
                .is_none_or(|old| rev.order().is_ok_and(|new| new > old))
            {
                records.insert(key, rev);
            }
        }
        if records.len() > 1 {
            let mut trusted = Vec::new();
            for r in records.values() {
                if self.trusted(&r.body.access.policy.owner)? {
                    trusted.push(r.clone());
                }
            }
            return match trusted.len() {
                0 => Err(Error::UntrustedWriter),
                1 => Ok(trusted.pop()),
                _ => Err(Error::Conflict),
            };
        }
        Ok(records.into_values().next())
    }
    fn authorize(&self, rev: &Revision) -> Result<()> {
        if !self.trusted(&rev.body.access.policy.owner)? {
            return Err(Error::UntrustedWriter);
        }
        if rev.body.deleted {
            return Err(Error::NotFound);
        }
        let me = self.enrollment()?;
        if !rev.body.access.policy.members.contains(&me) {
            return Err(Error::NotGranted);
        }
        Ok(())
    }
    fn clear(&self, rev: &Revision) -> Result<Secret> {
        self.authorize(rev)?;
        let key = format!("records/{}", rev.id()?);
        if let Some(bytes) = self.backend.get(&key)? {
            return Ok(bytes);
        }
        let bytes = self.identity()?.decrypt(&rev.body.ciphertext)?;
        self.backend.put(&key, bytes.expose())?;
        Ok(bytes)
    }
    pub fn get(&mut self, name: &str) -> Result<Secret> {
        self.read(name).map(|(_, secret)| secret)
    }
    pub fn read(&mut self, name: &str) -> Result<(Record, Secret)> {
        self.synchronize()?;
        let rev = self.latest(name)?.ok_or(Error::NotFound)?;
        let secret = self.clear(&rev)?;
        Ok((self.describe(&rev)?, secret))
    }
    pub fn put(
        &mut self,
        name: &str,
        bytes: &[u8],
        kind: &str,
        expected: Option<&str>,
    ) -> Result<Record> {
        if !valid_name(name) || !matches!(kind, "value" | "bundle" | "blob" | "cookies") {
            return Err(Error::InvalidInput);
        }
        if bytes.len() > MAX_SECRET {
            return Err(Error::TooLarge);
        }
        let _lock = self.lock()?;
        self.synchronize()?;
        let me = self.enrollment()?;
        let id = self.identity()?;
        let old = self.latest(name)?;
        if let Some(expected) = expected {
            if old.as_ref().map(|r| r.id()).transpose()?.as_deref() != Some(expected) {
                return Err(Error::StaleRevision);
            }
        }
        let access = if let Some(r) = &old {
            self.authorize(r)?;
            r.body.access.clone()
        } else {
            let policy = Policy {
                record: hey_boss_sync::digest(format!("{}:{name}", me.machine.id).as_bytes()),
                name: name.into(),
                owner: me.clone(),
                version: 1,
                updated_ms: now(),
                members: vec![me.clone()],
            };
            let signature = id.sign(&policy)?;
            SignedPolicy { policy, signature }
        };
        self.publish(access, old.as_ref(), kind, Some(bytes))
    }
    fn publish(
        &mut self,
        access: SignedPolicy,
        old: Option<&Revision>,
        kind: &str,
        bytes: Option<&[u8]>,
    ) -> Result<Record> {
        let me = self.enrollment()?;
        let id = self.identity()?;
        let modified_ms = now()
            .max(access.policy.updated_ms)
            .max(old.map_or(0, |r| r.body.modified_ms));
        let logical = match old {
            Some(r) if modified_ms == r.body.modified_ms => {
                r.body.logical.checked_add(1).ok_or(Error::InvalidRecord)?
            }
            _ => 0,
        };
        let ciphertext = bytes
            .map(|b| crypto::encrypt(b, &access.policy.members))
            .transpose()?
            .unwrap_or_default();
        let body = Body {
            access,
            writer: me,
            modified_ms,
            logical,
            kind: kind.into(),
            deleted: bytes.is_none(),
            ciphertext,
        };
        let signature = id.sign(&body)?;
        let revision = Revision { body, signature };
        revision.validate()?;
        let wire = Wire {
            protocol: 2,
            kind: KIND.into(),
            revision: revision.clone(),
        };
        let mut engine = storage(hey_boss_sync::Engine::open(&self.journal))?;
        storage(engine.put(&serde_json::to_string(&wire).map_err(|_| Error::InvalidRecord)?))?;
        self.synchronize()?;
        self.describe(&revision)
    }
    pub fn grant(&mut self, name: &str, machine: &str, allow: bool) -> Result<Record> {
        let _lock = self.lock()?;
        self.synchronize()?;
        let old = self.latest(name)?.ok_or(Error::NotFound)?;
        let mut policy = old.body.access.policy.clone();
        let me = self.enrollment()?;
        if policy.owner != me {
            return Err(Error::NotWriter);
        }
        if machine == me.machine.id && !allow {
            return Err(Error::InvalidInput);
        }
        let has = policy.members.iter().any(|m| m.machine.id == machine);
        if has == allow {
            return self.describe(&old);
        }
        let bytes = self.clear(&old)?;
        if allow {
            let data: String = storage(self.db.query_row(
                "SELECT enrollment FROM trusted WHERE id=?1",
                [machine],
                |r| r.get(0),
            ))?;
            policy
                .members
                .push(serde_json::from_str(&data).map_err(|_| Error::InvalidRecord)?);
        } else {
            policy.members.retain(|m| m.machine.id != machine)
        }
        policy
            .members
            .sort_by(|a, b| a.machine.id.cmp(&b.machine.id));
        policy.version = policy.version.checked_add(1).ok_or(Error::InvalidRecord)?;
        policy.updated_ms = now().max(policy.updated_ms);
        let signature = self.identity()?.sign(&policy)?;
        self.publish(
            SignedPolicy { policy, signature },
            Some(&old),
            &old.body.kind,
            Some(bytes.expose()),
        )
    }
    pub fn delete(&mut self, name: &str) -> Result<Record> {
        let _lock = self.lock()?;
        self.synchronize()?;
        let old = self.latest(name)?.ok_or(Error::NotFound)?;
        self.authorize(&old)?;
        self.publish(old.body.access.clone(), Some(&old), &old.body.kind, None)
    }
    fn describe(&self, rev: &Revision) -> Result<Record> {
        let p = &rev.body.access.policy;
        let b = &rev.body;
        let status = match self.authorize(rev) {
            Ok(()) => "available".into(),
            Err(Error::NotFound) => "deleted".into(),
            Err(e) => e.to_string(),
        };
        Ok(Record {
            id: p.record.clone(),
            name: p.name.clone(),
            kind: b.kind.clone(),
            owner: p.owner.machine.id.clone(),
            owner_label: p.owner.machine.label.clone(),
            writer: b.writer.machine.id.clone(),
            writer_label: b.writer.machine.label.clone(),
            revision: rev.id()?,
            access_version: p.version,
            recipients: p.members.iter().map(|m| m.machine.id.clone()).collect(),
            updated_ms: b.modified_ms,
            deleted: b.deleted,
            status,
        })
    }
    pub fn inventory(&mut self) -> Result<Inventory> {
        self.synchronize()?;
        let mut statement = storage(
            self.db
                .prepare("SELECT DISTINCT record FROM revisions ORDER BY name,record"),
        )?;
        let ids = storage(statement.query_map([], |r| r.get::<_, String>(0)))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| Error::StorageFailure)?;
        let records = ids
            .iter()
            .map(|id| self.describe(&self.latest(id)?.ok_or(Error::NotFound)?))
            .collect::<Result<Vec<_>>>()?;
        let mut statement = storage(
            self.db
                .prepare("SELECT enrollment FROM trusted ORDER BY id"),
        )?;
        let machines = storage(statement.query_map([], |r| r.get::<_, String>(0)))?
            .map(|r| serde_json::from_str(&storage(r)?).map_err(|_| Error::InvalidRecord))
            .collect::<Result<Vec<_>>>()?;
        Ok(Inventory {
            machine: self.enrollment().ok(),
            backend: self.backend.name().into(),
            records,
            machines,
        })
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::*;
    fn revision(owner: &Identity, writer: &Identity, timestamp: u64) -> Revision {
        let owner_card = owner.enrollment("owner").unwrap();
        let writer_card = writer.enrollment("writer").unwrap();
        let policy = Policy {
            record: hey_boss_sync::digest(format!("{}:key", owner_card.machine.id).as_bytes()),
            name: "key".into(),
            owner: owner_card.clone(),
            version: 1,
            updated_ms: 1,
            members: vec![owner_card, writer_card.clone()],
        };
        let access = SignedPolicy {
            signature: owner.sign(&policy).unwrap(),
            policy,
        };
        let body = Body {
            access,
            writer: writer_card,
            modified_ms: timestamp,
            logical: 0,
            kind: "value".into(),
            deleted: false,
            ciphertext: "synthetic-ciphertext-for-order-test".into(),
        };
        Revision {
            signature: writer.sign(&body).unwrap(),
            body,
        }
    }
    #[test]
    fn equal_clocks_have_stable_ties_and_observed_clock_advances() {
        let owner = Identity::generate();
        let writer = Identity::generate();
        let a = revision(&owner, &writer, 100);
        let mut b = a.clone();
        b.body.ciphertext = "other-synthetic-ciphertext".into();
        b.signature = writer.sign(&b.body).unwrap();
        a.validate().unwrap();
        b.validate().unwrap();
        assert_ne!(a.order().unwrap(), b.order().unwrap());
        let left = [a.clone(), b.clone()]
            .into_iter()
            .max_by_key(|r| r.order().unwrap())
            .unwrap()
            .id()
            .unwrap();
        let right = [b, a.clone()]
            .into_iter()
            .max_by_key(|r| r.order().unwrap())
            .unwrap()
            .id()
            .unwrap();
        assert_eq!(left, right);
        let mut after = a.clone();
        after.body.logical = 1;
        after.signature = writer.sign(&after.body).unwrap();
        assert!(after.order().unwrap() > a.order().unwrap());
    }
    #[test]
    fn valid_signature_cannot_substitute_an_ungranted_writer() {
        let owner = Identity::generate();
        let writer = Identity::generate();
        let outsider = Identity::generate();
        let mut rev = revision(&owner, &writer, 100);
        rev.body.writer = outsider.enrollment("outsider").unwrap();
        rev.signature = outsider.sign(&rev.body).unwrap();
        assert_eq!(rev.validate().unwrap_err(), Error::InvalidRecord);
    }
}
