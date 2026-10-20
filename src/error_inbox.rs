//! Local diagnostic inbox. Raw telemetry never enters fleet replication.
use crate::issues::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub source: String,
    pub event_id: String,
    /// Unix milliseconds; upstream occurrence time, not collection time.
    pub timestamp: i64,
    pub message: String,
    #[serde(default)]
    pub stack: Option<String>,
    #[serde(default)]
    pub release: Option<String>,
    #[serde(default)]
    pub request_ref: Option<String>,
    /// Stable operation/code supplied by an adapter; message fallback otherwise.
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default = "one")]
    pub count: i64,
}
fn one() -> i64 {
    1
}
pub struct Inbox {
    db: Connection,
    log: Option<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(id: &str, message: &str, time: i64) -> Event {
        Event {
            source: "worker".into(),
            event_id: id.into(),
            timestamp: time,
            message: message.into(),
            stack: None,
            release: None,
            request_ref: None,
            fingerprint: None,
            count: 1,
        }
    }
    #[test]
    fn journal_rotates_and_preserves_valid_json_lines() {
        let root = std::env::temp_dir().join(format!("hey-boss-errors-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("events.jsonl");
        for i in 0..30 {
            append_journal(
                &path,
                &[json!({"event_id":i,"message":"diagnostic evidence"})],
                100,
            )
            .unwrap();
        }
        assert!(!root.join("events.jsonl.4").exists());
        for name in [
            "events.jsonl",
            "events.jsonl.1",
            "events.jsonl.2",
            "events.jsonl.3",
        ] {
            let text = std::fs::read_to_string(root.join(name)).unwrap();
            assert!(text.len() <= 100);
            for line in text.lines() {
                serde_json::from_str::<Value>(line).unwrap();
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn opt_in_replay_and_rolling_frequency() {
        let mut inbox = Inbox::memory();
        let now = 1_800_000_000_000;
        assert!(
            inbox
                .ingest("p", &[event("1", "failed", now)], now)
                .is_err()
        );
        inbox.enable("p", true).unwrap();
        inbox
            .ingest(
                "p",
                &[
                    event("1", "failed", now),
                    event("2", "failed", now),
                    event("3", "old", now - 86_400_001),
                ],
                now,
            )
            .unwrap();
        assert_eq!(
            inbox
                .ingest("p", &[event("1", "failed", now)], now)
                .unwrap()["accepted"],
            0
        );
        let groups = inbox.list("p", now).unwrap();
        assert_eq!(groups["groups"][0]["count_24h"], 2);
        assert_eq!(groups["groups"][1]["count_24h"], 0);
        assert_eq!(inbox.list("other", now).unwrap()["groups"], json!([]));
    }
    #[test]
    fn batch_validation_is_atomic_and_stale_replays_are_rejected() {
        let mut inbox = Inbox::memory();
        let now = 1_800_000_000_000;
        inbox.enable("p", true).unwrap();
        let mut invalid = event("2", "bad", now);
        invalid.count = -1;
        assert!(
            inbox
                .ingest("p", &[event("1", "ok", now), invalid], now)
                .is_err()
        );
        assert_eq!(inbox.list("p", now).unwrap()["groups"], json!([]));
        assert!(
            inbox
                .ingest("p", &[event("old", "old", now - 31 * 86_400_000)], now)
                .is_err()
        );
    }
    #[test]
    fn grouping_preserves_original_evidence() {
        let mut inbox = Inbox::memory();
        let now = 1_800_000_000_000;
        inbox.enable("p", true).unwrap();
        inbox
            .ingest(
                "p",
                &[
                    event("a", "GET /chat/123456 failed", now),
                    event("b", "GET /chat/654321 failed", now),
                    event("c", "other", now),
                ],
                now,
            )
            .unwrap();
        let groups = inbox.list("p", now).unwrap();
        assert_eq!(groups["groups"].as_array().unwrap().len(), 2);
        let a = groups["groups"][0]["id"].as_str().unwrap();
        assert!(inbox.detail("other", a, now).is_err());
        assert_eq!(
            inbox.detail("p", a, now).unwrap()["samples"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}

const DAY: i64 = 86_400_000;
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn normalize(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or(message)
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if (s.len() >= 6 && s.chars().all(|c| c.is_ascii_digit()))
                || (s.len() >= 24 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
            {
                "<id>"
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
impl Inbox {
    pub fn open(issue_path: &Path) -> Result<Self> {
        let path = issue_path.with_extension("errors.db");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Connection::open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        let mut inbox = Self::initialize(db)?;
        inbox.log = Some(issue_path.with_extension("errors.jsonl"));
        Ok(inbox)
    }
    #[cfg(test)]
    fn memory() -> Self {
        Self::initialize(Connection::open_in_memory().unwrap()).unwrap()
    }
    fn initialize(db: Connection) -> Result<Self> {
        db.busy_timeout(std::time::Duration::from_secs(10))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
CREATE TABLE IF NOT EXISTS error_settings(project TEXT PRIMARY KEY,enabled INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS error_groups(project TEXT NOT NULL,id TEXT NOT NULL,title TEXT NOT NULL,total INTEGER NOT NULL DEFAULT 0,issue INTEGER,snapshot TEXT,PRIMARY KEY(project,id));
CREATE TABLE IF NOT EXISTS error_events(project TEXT NOT NULL,source TEXT NOT NULL,event_id TEXT NOT NULL,group_id TEXT NOT NULL,time INTEGER NOT NULL,count INTEGER NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(project,source,event_id));
CREATE INDEX IF NOT EXISTS error_event_frequency ON error_events(project,time,group_id);
CREATE INDEX IF NOT EXISTS error_event_samples ON error_events(project,group_id,time DESC);")?;
        db.execute_batch(
            "CREATE INDEX IF NOT EXISTS error_event_retention ON error_events(time);",
        )?;
        db.execute("DELETE FROM error_events WHERE time<?1", [now() - 30 * DAY])?;
        Ok(Self { db, log: None })
    }
    pub fn enable(&self, project: &str, enabled: bool) -> Result<Value> {
        crate::issues::identifier(project, "project", 512)?;
        self.db.execute("INSERT INTO error_settings VALUES(?1,?2) ON CONFLICT(project) DO UPDATE SET enabled=excluded.enabled",params![project,enabled])?;
        Ok(json!({"ok":true,"enabled":enabled}))
    }
    fn enabled(&self, project: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT COALESCE((SELECT enabled FROM error_settings WHERE project=?1),0)",
            [project],
            |r| r.get(0),
        )?)
    }
    pub fn ingest(&mut self, project: &str, events: &[Event], now: i64) -> Result<Value> {
        crate::issues::identifier(project, "project", 512)?;
        if !self.enabled(project)? {
            return Err(Error::new(
                "collection_disabled",
                "Error collection is disabled for this project",
            ));
        }
        if events.len() > 500 {
            return Err(Error::invalid("Maximum 500 events per batch"));
        }
        for e in events {
            for (v, max) in [(&e.source, 256), (&e.event_id, 512), (&e.message, 8192)] {
                if v.trim().is_empty() || v.len() > max {
                    return Err(Error::invalid("Missing or oversized event field"));
                }
            }
            for (v, max) in [
                (&e.stack, 16384),
                (&e.release, 256),
                (&e.request_ref, 512),
                (&e.fingerprint, 1024),
            ] {
                if v.as_ref().is_some_and(|s| s.len() > max) {
                    return Err(Error::invalid("Oversized diagnostic field"));
                }
            }
            if e.timestamp < now - 30 * DAY
                || e.timestamp > now + 300_000
                || !(1..=1_000_000).contains(&e.count)
            {
                return Err(Error::invalid(
                    "Invalid timestamp or count (30-day retention, five-minute clock tolerance)",
                ));
            }
        }
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM error_events WHERE time<?1", [now - 30 * DAY])?;
        let mut accepted = 0;
        let mut journal = Vec::new();
        for e in events {
            let group = hash(&format!(
                "{}\n{}",
                e.source,
                e.fingerprint
                    .clone()
                    .unwrap_or_else(|| normalize(&e.message))
            ));
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO error_events VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    project,
                    e.source,
                    e.event_id,
                    group,
                    e.timestamp,
                    e.count,
                    serde_json::to_string(e)?
                ],
            )?;
            if inserted > 0 {
                tx.execute("INSERT INTO error_groups(project,id,title,total) VALUES(?1,?2,?3,?4) ON CONFLICT(project,id) DO UPDATE SET total=total+excluded.total",params![project,group,e.message.lines().next().unwrap_or(&e.message),e.count])?;
                accepted += 1;
                journal.push(json!({"project":project,"event":e}));
            }
        }
        let size: i64 = tx.query_row("SELECT COUNT(*) FROM error_events", [], |r| r.get(0))?;
        if size > 1_000_000 {
            return Err(Error::new("inbox_full", "Inbox retention limit reached"));
        }
        let groups: i64 = tx.query_row("SELECT COUNT(*) FROM error_groups", [], |r| r.get(0))?;
        if groups > 10_000 {
            return Err(Error::new("inbox_full", "Inbox group limit reached"));
        }
        // The SQLite writer lock serializes rotation across collectors. A crash
        // can leave duplicate journal lines; event_id remains the replay key.
        if let Some(path) = &self.log {
            append_journal(path, &journal, 8 * 1024 * 1024)?;
        }
        tx.commit()?;
        Ok(json!({"ok":true,"accepted":accepted,"duplicates":events.len()-accepted}))
    }
    pub fn list(&self, project: &str, now: i64) -> Result<Value> {
        let mut stmt=self.db.prepare("SELECT g.id,g.title,g.total,g.issue,COALESCE(SUM(CASE WHEN e.time>=?2 THEN e.count ELSE 0 END),0),MAX(e.time),COALESCE(SUM(CASE WHEN e.time>=?3 AND e.time<?2 THEN e.count ELSE 0 END),0) FROM error_groups g LEFT JOIN error_events e ON e.project=g.project AND e.group_id=g.id WHERE g.project=?1 GROUP BY g.id ORDER BY 5 DESC,6 DESC,g.id LIMIT 200")?;
        let groups=stmt.query_map(params![project,now-DAY,now-2*DAY],|r|Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"total":r.get::<_,i64>(2)?,"issue":r.get::<_,Option<i64>>(3)?,"count_24h":r.get::<_,i64>(4)?,"last_seen":r.get::<_,Option<i64>>(5)?,"previous_24h":r.get::<_,i64>(6)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(
            json!({"ok":true,"enabled":self.enabled(project)?,"groups":groups,"limit":200,"retention_days":30}),
        )
    }
    pub fn detail(&self, project: &str, id: &str, now: i64) -> Result<Value> {
        Self::read_detail(&self.db, project, id, now)
    }
    fn read_detail(db: &Connection, project: &str, id: &str, now: i64) -> Result<Value> {
        let mut group=db.query_row("SELECT id,title,total,issue,snapshot FROM error_groups WHERE project=?1 AND id=?2",params![project,id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"total":r.get::<_,i64>(2)?,"issue":r.get::<_,Option<i64>>(3)?,"snapshot":r.get::<_,Option<String>>(4)?}))).optional()?.ok_or_else(||Error::new("not_found","Error group not found"))?;
        let mut stmt=db.prepare("SELECT payload FROM error_events WHERE project=?1 AND group_id=?2 ORDER BY time DESC,source,event_id LIMIT 10")?;
        let samples = stmt
            .query_map(params![project, id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .map(|s| serde_json::from_str::<Value>(&s))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        group["samples"] = json!(samples);
        group["observed_at"] = json!(now);
        group["ok"] = json!(true);
        Ok(group)
    }
    /// Freeze before creating a task: a lost acknowledgement retries identical evidence.
    pub fn snapshot(&mut self, project: &str, id: &str) -> Result<Value> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let detail = Self::read_detail(&tx, project, id, now())?;
        let text = serde_json::to_string_pretty(&detail)?;
        tx.execute(
            "UPDATE error_groups SET snapshot=COALESCE(snapshot,?3) WHERE project=?1 AND id=?2",
            params![project, id, text],
        )?;
        let saved: String = tx.query_row(
            "SELECT snapshot FROM error_groups WHERE project=?1 AND id=?2",
            params![project, id],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(serde_json::from_str(&saved)?)
    }
    pub fn link(&self, project: &str, id: &str, number: i64) -> Result<Value> {
        self.db.execute("UPDATE error_groups SET issue=?3 WHERE project=?1 AND id=?2 AND (issue IS NULL OR issue=?3)",params![project,id,number])?;
        self.detail(project, id, now())
    }
}

fn append_journal(path: &Path, rows: &[Value], limit: u64) -> Result<()> {
    use std::io::Write;
    for row in rows {
        let line = serde_json::to_vec(row)?;
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) + line.len() as u64 + 1 > limit {
            for index in (1..=3).rev() {
                let from = if index == 1 {
                    path.to_path_buf()
                } else {
                    std::path::PathBuf::from(format!("{}.{}", path.display(), index - 1))
                };
                let to = std::path::PathBuf::from(format!("{}.{}", path.display(), index));
                match std::fs::rename(from, to) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(&line)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
    }
    Ok(())
}
