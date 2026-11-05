//! Durable operational facts shared by independent readers across the fleet.

use crate::{
    database::Connection,
    issues::{Error, Result},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_EVENT_BYTES: usize = 16 * 1024;
pub(crate) const KIND: &str = "operational_event";
pub(crate) const RETIRED: &str = "operational_event_retired";
const RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

pub(crate) mod observe;
pub(crate) mod producers;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u32,
    pub origin: String,
    pub time: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub subject: String,
    pub data: Value,
}

#[derive(Debug, Default)]
pub struct Filter {
    pub kind: Option<String>,
    pub machine: Option<String>,
    pub project: Option<String>,
    pub subject: Option<String>,
    pub since: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct Record {
    pub cursor: String,
    pub received_at: i64,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Debug, Serialize)]
pub struct Page {
    pub events: Vec<Record>,
    pub cursor: String,
    pub has_more: bool,
    pub replica: String,
    pub expired_through: Option<String>,
    pub coverage: Value,
}

pub fn open(path: &std::path::Path) -> Result<Connection> {
    Ok(crate::issues::Store::open(path)?.into_database())
}

pub(crate) fn install(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='operational_event_observations')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS operational_event_meta(id INTEGER PRIMARY KEY CHECK(id=1),replica TEXT NOT NULL,floor INTEGER NOT NULL DEFAULT 0);
        INSERT OR IGNORE INTO operational_event_meta(id,replica) VALUES(1,lower(hex(randomblob(16))));
        CREATE TABLE IF NOT EXISTS operational_events(position INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,record TEXT NOT NULL,event TEXT,type TEXT,origin TEXT,time INTEGER,project TEXT,subject TEXT,received_at INTEGER NOT NULL,pruned INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS operational_event_outbox(id TEXT PRIMARY KEY,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS operational_event_type ON operational_events(type,position) WHERE event IS NOT NULL;
        CREATE INDEX IF NOT EXISTS operational_event_origin ON operational_events(origin,position) WHERE event IS NOT NULL;
        CREATE INDEX IF NOT EXISTS operational_event_project ON operational_events(project,position) WHERE event IS NOT NULL;
        CREATE INDEX IF NOT EXISTS operational_event_subject ON operational_events(subject,position) WHERE event IS NOT NULL;
        CREATE INDEX IF NOT EXISTS operational_event_time ON operational_events(time,position) WHERE event IS NOT NULL;
        CREATE INDEX IF NOT EXISTS operational_event_prune ON operational_events(position) WHERE event IS NULL AND pruned=0;
        CREATE INDEX IF NOT EXISTS operational_event_received ON operational_events(received_at,position) WHERE event IS NOT NULL;
        CREATE TABLE IF NOT EXISTS operational_event_observations(origin TEXT NOT NULL,subject TEXT NOT NULL,sensor TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(origin,subject,sensor));")?;
    tx.commit()?;
    Ok(())
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn validate(event: &Event) -> Result<()> {
    for (name, value, limit) in [
        ("id", event.id.as_str(), 256),
        ("type", event.kind.as_str(), 128),
        ("origin", event.origin.as_str(), 256),
        ("subject", event.subject.as_str(), 1024),
    ] {
        if value.trim().is_empty() || value.len() > limit || value.chars().any(char::is_control) {
            return Err(Error::invalid(format!("Invalid event {name}")));
        }
    }
    if !event
        .kind
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        || event.version == 0
        || event.time < 0
        || !event.data.is_object()
        || event
            .project
            .as_ref()
            .is_some_and(|p| p.is_empty() || p.len() > 8192 || p.chars().any(char::is_control))
    {
        return Err(Error::invalid(
            "Invalid event type, version, time, project, or data object",
        ));
    }
    if serde_json::to_vec(event)?.len() > MAX_EVENT_BYTES {
        return Err(Error::invalid(
            "Event exceeds 16 KiB; retain detailed evidence in its original store",
        ));
    }
    Ok(())
}

fn envelope(event: &Event) -> Result<String> {
    Ok(serde_json::to_string(
        &json!({"protocol":2,"kind":KIND,"event":event}),
    )?)
}

fn insert(db: &Connection, event: &Event, record: &str) -> Result<bool> {
    if let Some((saved, retired)) = db
        .query_row(
            "SELECT record,event IS NULL FROM operational_events WHERE id=?1",
            [&event.id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
        )
        .optional()?
    {
        if saved != record {
            return Err(Error::conflict(
                "An event ID was reused with different contents",
            ));
        }
        // An old payload can arrive after its retirement. Reclaim it from the
        // CRDT journal as well, without restoring the searchable event.
        if retired {
            db.execute(
                "UPDATE operational_events SET pruned=0 WHERE id=?1 AND pruned<>0",
                [&event.id],
            )?;
        }
        return Ok(false);
    }
    db.execute("INSERT INTO operational_events(id,record,event,type,origin,time,project,subject,received_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![event.id,record,serde_json::to_string(event)?,event.kind,event.origin,event.time,event.project,event.subject,now()])?;
    Ok(true)
}

/// Joins the caller's transaction, or owns one when called independently.
/// Success acknowledges local durability; it never waits for a remote peer.
pub fn append(db: &Connection, event: &Event) -> Result<bool> {
    validate(event)?;
    if db.is_autocommit() {
        let tx = crate::database::Transaction::new_unchecked(
            db,
            crate::database::TransactionBehavior::Immediate,
        )?;
        let inserted = append(&tx, event)?;
        tx.commit()?;
        return Ok(inserted);
    }
    let payload = envelope(event)?;
    let record = hey_boss_sync::digest(payload.as_bytes());
    if !insert(db, event, &record)? {
        return Ok(false);
    }
    db.execute(
        "INSERT INTO operational_event_outbox VALUES(?1,?2)",
        params![record, payload],
    )?;
    Ok(true)
}

fn cursor(replica: &str, position: i64) -> String {
    format!("{replica}:{position}")
}

fn record(replica: &str, position: i64, received_at: i64, event: String) -> Result<Record> {
    Ok(Record {
        cursor: cursor(replica, position),
        received_at,
        event: serde_json::from_str(&event)?,
    })
}

fn filters(filter: &Filter, args: &mut Vec<rusqlite::types::Value>) -> Result<Vec<String>> {
    let mut clauses = vec!["event IS NOT NULL".to_owned()];
    for (column, value) in [
        ("type", &filter.kind),
        ("origin", &filter.machine),
        ("project", &filter.project),
        ("subject", &filter.subject),
    ] {
        if let Some(value) = value {
            if column == "type" && value.ends_with(".*") {
                let prefix = &value[..value.len() - 1];
                if !prefix
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
                {
                    return Err(Error::invalid(
                        "Event type accepts an exact type or a namespace ending in .*",
                    ));
                }
                args.push(format!("{prefix}*").into());
                clauses.push(format!("{column} GLOB ?{}", args.len()));
            } else {
                args.push(value.clone().into());
                clauses.push(format!("{column}=?{}", args.len()));
            }
        }
    }
    if let Some(since) = filter.since {
        args.push(since.into());
        clauses.push(format!("time>=?{}", args.len()));
    }
    Ok(clauses)
}

/// Built-in producers plus any extension types already observed here.
pub fn types(db: &Connection, filter: &Filter) -> Result<Vec<Value>> {
    let mut args = Vec::new();
    let clauses = filters(filter, &mut args)?;
    let rows = db.prepare(&format!("SELECT type,count(*),max(time) FROM operational_events WHERE {} GROUP BY type ORDER BY type", clauses.join(" AND ")))?
        .query_map(crate::database::params_from_iter(args), |r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut types = std::collections::BTreeMap::new();
    for (kind, description) in [
        (
            "agent.started",
            "Worker attached to an exact harness session",
        ),
        (
            "agent.finished",
            "Worker attempt ended; state and selected error code",
        ),
        (
            "agent.retry_scheduled",
            "Worker retry deadline and attempt count",
        ),
        (
            "agent.tool_failed",
            "Managed worker tool returned a failure",
        ),
        (
            "agent.input_requested",
            "Managed worker needs input or approval",
        ),
        ("job.queued", "Job occurrence or per-machine child queued"),
        ("job.started", "Job execution started"),
        ("job.finished", "Job execution or machine group ended"),
        (
            "job.skipped",
            "Job occurrence skipped because another is active",
        ),
        (
            "machine.disk_low",
            "Sampled volume below 2 GiB available or 5% free",
        ),
        (
            "machine.disk_recovered",
            "Sampled volume back above 3 GiB and 7% free",
        ),
        (
            "peer.connected",
            "Reporter observed an outgoing CRDT route connected",
        ),
        (
            "peer.disconnected",
            "Reporter observed an outgoing CRDT route disconnected",
        ),
        (
            "error.observed",
            "Selected evidence from an enabled local error collector",
        ),
    ] {
        if filter.kind.as_deref().is_none_or(|wanted| {
            wanted == kind
                || wanted
                    .strip_suffix(".*")
                    .is_some_and(|p| kind.starts_with(&format!("{p}.")))
        }) {
            types.insert(kind.to_owned(), json!({"type":kind,"version":1,"description":description,"count":0,"last_seen":null}));
        }
    }
    for (kind, count, last_seen) in rows {
        let value = types.entry(kind.clone()).or_insert_with(
            || json!({"type":kind,"description":"Extension type observed on this replica"}),
        );
        value["count"] = json!(count);
        value["last_seen"] = json!(last_seen);
    }
    Ok(types.into_values().collect())
}

/// A cursor is a position in this replica's arrival log, never an event timestamp.
pub fn read(db: &Connection, filter: &Filter, after: Option<&str>, limit: usize) -> Result<Page> {
    if !(1..=1000).contains(&limit) {
        return Err(Error::invalid("Event limit must be 1–1000"));
    }
    if db.is_autocommit() {
        let tx = db.read_transaction()?;
        let page = read(&tx, filter, after, limit)?;
        tx.commit()?;
        return Ok(page);
    }
    let (replica, floor, head): (String,i64,i64) = db.query_row("SELECT replica,floor,coalesce((SELECT max(position) FROM operational_events),0) FROM operational_event_meta WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let position = if let Some(after) = after {
        let (owner, value) = after
            .rsplit_once(':')
            .ok_or_else(|| Error::invalid("Invalid event cursor"))?;
        let value: i64 = value
            .parse()
            .map_err(|_| Error::invalid("Invalid event cursor position"))?;
        if owner != replica {
            return Err(Error::invalid(
                "Event cursor belongs to another replica; replay here and deduplicate by event ID",
            ));
        }
        if value < 0 || value > head {
            return Err(Error::invalid(
                "Event cursor is outside this replica's history",
            ));
        }
        if value < floor {
            return Err(Error::new(
                "event_gap",
                "Events after this cursor have expired; start a new read to acknowledge the gap",
            ));
        }
        value
    } else {
        0
    };
    let mut args = vec![
        rusqlite::types::Value::Integer(position),
        rusqlite::types::Value::Integer(head),
    ];
    let mut clauses = filters(filter, &mut args)?;
    clauses.extend(["position>?1".to_owned(), "position<=?2".to_owned()]);
    let recent = after.is_none() && filter.since.is_none();
    let order = if recent { "DESC" } else { "ASC" };
    args.push(((limit + 1) as i64).into());
    let sql = format!(
        "SELECT position,received_at,event FROM operational_events WHERE {} ORDER BY position {order} LIMIT ?{}",
        clauses.join(" AND "),
        args.len()
    );
    let mut rows = db
        .prepare(&sql)?
        .query_map(crate::database::params_from_iter(args), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = if !recent && more {
        rows.last().unwrap().0
    } else {
        head
    };
    if recent {
        rows.reverse();
    }
    let events = rows
        .into_iter()
        .map(|(position, received, event)| record(&replica, position, received, event))
        .collect::<Result<_>>()?;
    let pending: i64 = db.query_row("SELECT count(*) FROM operational_event_outbox", [], |r| {
        r.get(0)
    })?;
    Ok(Page {
        events,
        cursor: cursor(&replica, next),
        has_more: !recent && more,
        expired_through: (floor > 0).then(|| cursor(&replica, floor)),
        replica,
        coverage: json!({"scope":"local_replica","pending_publication":pending,"fleet_completeness":"unknown"}),
    })
}

pub fn get(db: &Connection, id: &str) -> Result<Record> {
    let row = db.query_row("SELECT m.replica,e.position,e.received_at,e.event FROM operational_events e CROSS JOIN operational_event_meta m WHERE e.id=?1 AND m.id=1", [id],
        |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<String>>(3)?))).optional()?;
    let (replica, position, received, event) =
        row.ok_or_else(|| Error::new("not_found", "Event not found on this replica"))?;
    record(
        &replica,
        position,
        received,
        event.ok_or_else(|| Error::new("event_gap", "This event's payload has expired"))?,
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Retirement {
    id: String,
    record: String,
}

fn retire_records(db: &Connection, items: &[Retirement]) -> Result<()> {
    for item in items {
        if item.id.is_empty()
            || item.id.len() > 256
            || item.record.len() != 64
            || !item.record.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::invalid("Invalid event retirement"));
        }
        let known = db
            .query_row(
                "SELECT record FROM operational_events WHERE id=?1",
                [&item.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        if known.as_ref().is_some_and(|r| r != &item.record) {
            return Err(Error::conflict(
                "Event retirement does not match its identity",
            ));
        }
        db.execute("INSERT INTO operational_events(id,record,received_at) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET event=NULL,type=NULL,origin=NULL,time=NULL,project=NULL,subject=NULL,pruned=0", params![item.id,item.record,now()])?;
        db.execute("UPDATE operational_event_meta SET floor=max(floor,(SELECT position FROM operational_events WHERE id=?1)) WHERE id=1", [&item.id])?;
        db.execute(
            "DELETE FROM operational_event_outbox WHERE id=?1",
            [&item.record],
        )?;
    }
    Ok(())
}

pub(crate) fn retire(db: &Connection, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    if ids.len() > 128 {
        return Err(Error::invalid("Too many event retirements"));
    }
    let tx = db.unchecked_transaction()?;
    let mut items = Vec::new();
    for id in ids {
        if let Some(record) = tx
            .query_row(
                "SELECT record FROM operational_events WHERE id=?1 AND event IS NOT NULL",
                [id],
                |r| r.get(0),
            )
            .optional()?
        {
            items.push(Retirement {
                id: id.clone(),
                record,
            });
        }
    }
    if !items.is_empty() {
        let payload = json!({"protocol":2,"kind":RETIRED,"events":items}).to_string();
        retire_records(&tx, &items)?;
        tx.execute(
            "INSERT OR IGNORE INTO operational_event_outbox VALUES(?1,?2)",
            params![hey_boss_sync::digest(payload.as_bytes()), payload],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Shares the existing immutable journal and SSH exchange. Exporting twice
/// after a crash uses the same content hash, so publication is idempotent.
pub(crate) fn export(
    db: &Connection,
    engine: &mut hey_boss_sync::Engine,
) -> hey_boss_sync::Result<usize> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM operational_event_outbox)",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(0);
    }
    let tx = db.unchecked_transaction()?;
    let rows = tx
        .prepare("SELECT id,payload FROM operational_event_outbox ORDER BY rowid LIMIT 128")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, payload) in &rows {
        if engine.put(payload)? != *id {
            return Err(Error::conflict("Event outbox failed its integrity check").into());
        }
        tx.execute("DELETE FROM operational_event_outbox WHERE id=?1", [id])?;
    }
    tx.commit()?;
    Ok(rows.len())
}

/// Called inside the application projection transaction. It never queues a
/// received event for publication: the CRDT journal already forwards it.
pub(crate) fn receive(db: &Connection, payload: &str) -> Result<bool> {
    if payload.is_empty() {
        return Ok(false);
    }
    // Most journal records are ordinary revisions. Inspect their header without
    // allocating another complete copy of every issue body and artifact.
    #[derive(Deserialize)]
    struct Header<'a> {
        #[serde(borrow)]
        kind: Option<&'a str>,
    }
    let header: Header<'_> = serde_json::from_str(payload)?;
    let Some(kind) = header.kind.filter(|k| matches!(*k, KIND | RETIRED)) else {
        return Ok(false);
    };
    let value: Value = serde_json::from_str(payload)?;
    if value["protocol"] != 2 {
        return Err(Error::invalid("Unsupported event transport protocol"));
    }
    if kind == KIND {
        let event: Event = serde_json::from_value(value["event"].clone())?;
        validate(&event)?;
        let canonical = envelope(&event)?;
        if payload != canonical {
            return Err(Error::invalid("Event wire record is not canonical"));
        }
        insert(db, &event, &hey_boss_sync::digest(payload.as_bytes()))?;
    } else {
        let items: Vec<Retirement> = serde_json::from_value(value["events"].clone())?;
        if items.is_empty() || items.len() > 128 {
            return Err(Error::invalid("Invalid event retirement batch"));
        }
        retire_records(db, &items)?;
    }
    Ok(true)
}

pub(crate) fn collect(
    db: &Connection,
    engine: &mut hey_boss_sync::Engine,
) -> hey_boss_sync::Result<usize> {
    let expired = db.prepare("SELECT id FROM operational_events WHERE event IS NOT NULL AND received_at<?1 ORDER BY received_at,position LIMIT 128")?.query_map([now()-RETENTION_MS], |r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    retire(db, &expired)?;
    // Publish retirement facts before pruning their original payloads. Compact
    // identities remain permanently, including when an offline peer replays.
    export(db, engine)?;
    let retired = db.prepare("SELECT record FROM operational_events WHERE event IS NULL AND pruned=0 AND NOT EXISTS(SELECT 1 FROM operational_event_outbox) ORDER BY position LIMIT 128")?.query_map([], |r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if !retired.is_empty() {
        engine.prune(&retired)?;
        let tx = db.unchecked_transaction()?;
        for id in &retired {
            tx.execute(
                "UPDATE operational_events SET pruned=1 WHERE record=?1 AND event IS NULL",
                [id],
            )?;
        }
        tx.commit()?;
    }
    Ok(expired.len() + retired.len())
}

#[cfg(test)]
mod tests;
