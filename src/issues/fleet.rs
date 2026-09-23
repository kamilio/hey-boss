//! Additive fleet metadata. No network filesystem or schema-version coupling.
use super::{Error, Result};
use crate::database::Connection;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

pub(crate) const INDEXES: &str = concat!(
    "CREATE INDEX IF NOT EXISTS fleet_row_local ON fleet_row_ids(table_name,local_id);",
    "CREATE INDEX IF NOT EXISTS fleet_outbox_retention ON fleet_outbox(seq DESC,coalesce(length(CAST(before_json AS BLOB)),0)+coalesce(length(CAST(after_json AS BLOB)),0));"
);

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS fleet_meta(id INTEGER PRIMARY KEY CHECK(id=1),role TEXT NOT NULL,node TEXT NOT NULL,syncing INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO fleet_meta VALUES(1,'standalone','',0);
CREATE TABLE IF NOT EXISTS fleet_allocations(project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,node TEXT NOT NULL,PRIMARY KEY(project_id,issue_number));
CREATE TABLE IF NOT EXISTS fleet_number_ranges(project_id TEXT PRIMARY KEY,first_number INTEGER NOT NULL,last_number INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS fleet_outbox(seq INTEGER PRIMARY KEY AUTOINCREMENT,table_name TEXT NOT NULL,before_json TEXT,after_json TEXT,created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS fleet_receipts(node TEXT NOT NULL,seq INTEGER NOT NULL,result TEXT NOT NULL,PRIMARY KEY(node,seq));
CREATE TABLE IF NOT EXISTS fleet_row_ids(origin TEXT NOT NULL,table_name TEXT NOT NULL,origin_id INTEGER NOT NULL,local_id INTEGER NOT NULL,PRIMARY KEY(origin,table_name,origin_id));
CREATE INDEX IF NOT EXISTS fleet_row_local ON fleet_row_ids(table_name,local_id);
CREATE TABLE IF NOT EXISTS fleet_conflicts(id TEXT PRIMARY KEY,node TEXT NOT NULL,seq INTEGER NOT NULL,table_name TEXT NOT NULL,data TEXT NOT NULL,reason TEXT NOT NULL,created_at INTEGER NOT NULL,resolved INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS fleet_subtask_receipts(node TEXT NOT NULL,seq INTEGER NOT NULL,project_id TEXT NOT NULL,child_number INTEGER NOT NULL,parent_number INTEGER NOT NULL,kind TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(node,seq));
CREATE INDEX IF NOT EXISTS fleet_subtask_receipt_key ON fleet_subtask_receipts(node,project_id,child_number,seq DESC);
CREATE TABLE IF NOT EXISTS fleet_deferred_subtasks(project_id TEXT NOT NULL,child_number INTEGER NOT NULL,row_json TEXT,PRIMARY KEY(project_id,child_number));
CREATE TABLE IF NOT EXISTS fleet_allocation_deadlines(project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,expires_at INTEGER NOT NULL,PRIMARY KEY(project_id,issue_number));
CREATE INDEX IF NOT EXISTS fleet_allocation_expiry ON fleet_allocation_deadlines(expires_at);
INSERT OR IGNORE INTO fleet_allocation_deadlines SELECT project_id,issue_number,CAST(strftime('%s','now') AS INTEGER)*1000+900000 FROM fleet_allocations;
CREATE TRIGGER IF NOT EXISTS fleet_allocation_started AFTER INSERT ON fleet_allocations BEGIN INSERT OR REPLACE INTO fleet_allocation_deadlines VALUES(NEW.project_id,NEW.issue_number,CAST(strftime('%s','now') AS INTEGER)*1000+900000); END;
CREATE TRIGGER IF NOT EXISTS fleet_allocation_removed AFTER DELETE ON fleet_allocations BEGIN DELETE FROM fleet_allocation_deadlines WHERE project_id=OLD.project_id AND issue_number=OLD.issue_number; END;
CREATE TRIGGER IF NOT EXISTS fleet_worker_deadline_started AFTER INSERT ON worker_runs WHEN NEW.claimed_at IS NULL AND NEW.finished_at IS NULL AND NEW.reservation_expires IS NOT NULL BEGIN UPDATE fleet_allocation_deadlines SET expires_at=NEW.reservation_expires WHERE project_id=NEW.project_id AND issue_number=NEW.issue_number AND EXISTS(SELECT 1 FROM fleet_allocations a WHERE a.project_id=NEW.project_id AND a.issue_number=NEW.issue_number AND a.node=NEW.machine); END;
CREATE TRIGGER IF NOT EXISTS fleet_worker_deadline_updated AFTER UPDATE OF reservation_expires ON worker_runs WHEN NEW.claimed_at IS NULL AND NEW.finished_at IS NULL AND NEW.reservation_expires IS NOT NULL BEGIN UPDATE fleet_allocation_deadlines SET expires_at=NEW.reservation_expires WHERE project_id=NEW.project_id AND issue_number=NEW.issue_number AND EXISTS(SELECT 1 FROM fleet_allocations a WHERE a.project_id=NEW.project_id AND a.issue_number=NEW.issue_number AND a.node=NEW.machine); END;
";

pub(crate) fn check_claim(
    db: &Connection,
    project: &str,
    number: i64,
    machine: &str,
    force: bool,
) -> Result<()> {
    if force {
        return Ok(());
    }
    db.execute("DELETE FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2 AND (SELECT role FROM fleet_meta WHERE id=1)='controller' AND EXISTS(SELECT 1 FROM fleet_allocation_deadlines d JOIN issues i ON i.project_id=d.project_id AND i.number=d.issue_number WHERE d.project_id=?1 AND d.issue_number=?2 AND d.expires_at<=CAST(strftime('%s','now') AS INTEGER)*1000 AND i.assignee IS NULL)", params![project,number])?;
    // Keep successful pickup cheap: host lookup and connection files are only
    // needed when explaining a denial, not while arbitrating ordinary claims.
    let blocked: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2 AND node<>?3) OR ((SELECT role FROM fleet_meta WHERE id=1)='agent' AND NOT EXISTS(SELECT 1 FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2 AND node=?3))", params![project,number,machine], |r|r.get(0))?;
    let expired: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM fleet_allocation_deadlines d JOIN issues i ON i.project_id=d.project_id AND i.number=d.issue_number WHERE d.project_id=?1 AND d.issue_number=?2 AND d.expires_at<=CAST(strftime('%s','now') AS INTEGER)*1000 AND i.assignee IS NULL)", params![project,number], |r|r.get(0))?;
    if !blocked && !expired {
        return Ok(());
    }
    let info = allocation(db, project, number, Some(machine))?;
    let code = match info["reason"].as_str() {
        Some("reserved_elsewhere") => "fleet_reserved",
        Some("allocation_missing") => "fleet_allocation_missing",
        Some("allocation_expired") => "fleet_allocation_expired",
        _ => return Ok(()),
    };
    let mut error = Error::new(
        code,
        format!(
            "{}\nInspect: {}\n{}\n--force is an explicit takeover override, not a synchronization or manual-resume step.",
            info["summary"].as_str().unwrap(),
            info["inspect_command"].as_str().unwrap(),
            info["recovery"].as_str().unwrap()
        ),
    );
    error.details = Some(info);
    Err(error)
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// A snapshot of fleet permission, separate from session ownership and worker locks.
/// Missing local allocation is not proof that the supervisor has no reservation.
pub(crate) fn allocation(
    db: &Connection,
    project: &str,
    number: i64,
    caller: Option<&str>,
) -> Result<Value> {
    let (role, node): (String, String) =
        db.query_row("SELECT role,node FROM fleet_meta WHERE id=1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let machine = caller.unwrap_or(&node);
    let reserved: Option<String> = db
        .query_row(
            "SELECT node FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2",
            params![project, number],
            |r| r.get(0),
        )
        .optional()?;
    let expires: Option<i64> = db.query_row("SELECT d.expires_at FROM fleet_allocation_deadlines d JOIN issues i ON i.project_id=d.project_id AND i.number=d.issue_number WHERE d.project_id=?1 AND d.issue_number=?2 AND i.assignee IS NULL", params![project,number], |r|r.get(0)).optional()?;
    let reason = match reserved.as_deref() {
        Some(_) if expires.is_some_and(|deadline| deadline <= super::worker::now()) => {
            "allocation_expired"
        }
        Some(owner) if owner != machine => "reserved_elsewhere",
        Some(_) => "allocated_here",
        None if role == "agent" => "allocation_missing",
        None => "unallocated",
    };
    let mut reserved_host: Option<String> = if reserved.as_deref() == Some(node.as_str()) {
        Some(super::identity::host())
    } else if let Some(owner) = &reserved {
        // Bound lookup to this issue's participants; never scan all saved agents.
        db.query_row("SELECT json_extract(metadata,'$.host') FROM agents WHERE id IN (SELECT assignee FROM issues WHERE project_id=?1 AND number=?2 UNION SELECT created_by FROM issues WHERE project_id=?1 AND number=?2) AND json_extract(metadata,'$.machine')=?3 ORDER BY last_seen DESC LIMIT 1", params![project,number,owner], |r|r.get(0)).optional()?.flatten()
    } else {
        None
    };
    let mut reserved_ssh_host: Option<String> = None;
    if let Some(owner) = &reserved
        && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='fleet_state')",
            [],
            |r| r.get::<_, bool>(0),
        )?
    {
        // Use the existing durable inventory, never a network request or a scan
        // of saved conversations. Connectivity here may be stale.
        let cached: Option<(Option<String>, Option<String>)> = db.query_row("SELECT json_extract(m.value,'$.hostname'),json_extract(m.value,'$.host') FROM fleet_state s,json_each(s.value) m WHERE s.key='machines' AND json_extract(m.value,'$.node')=?1 LIMIT 1", [owner], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((hostname, host)) = cached {
            reserved_host = hostname.or(reserved_host);
            reserved_ssh_host =
                host.filter(|h| h != "local" && crate::health::remote::valid_host(h));
        }
    }
    let command = format!(
        "hey-boss issue allocation {number} --project {}",
        quote(project)
    );
    let claim = format!("hey-boss issue claim {number} --project {}", quote(project));
    let summary = match reason {
        "allocation_expired" => format!(
            "Issue #{number}'s fleet reservation expired before an agent claimed it. Reconnect to the supervisor for a new reservation."
        ),
        "reserved_elsewhere" => format!(
            "Issue #{number} is reserved for fleet machine {}{}; caller machine is {machine}.",
            reserved.as_deref().unwrap(),
            reserved_host
                .as_ref()
                .map(|host| format!(" ({host})"))
                .unwrap_or_default()
        ),
        "allocation_missing" => format!(
            "Issue #{number} has no allocation in this companion's local replica (machine {machine}). The supervisor may have a newer allocation."
        ),
        "allocated_here" => format!("Issue #{number} is allocated to machine {machine}."),
        _ => format!("Issue #{number} has no fleet reservation in this store."),
    };
    let mut recovery = match reason {
        "reserved_elsewhere" => format!(
            "{}Resume on the reserved machine: {claim} --agent '<saved-agent-id>'. Unclaimed reservations expire with the worker startup or claim deadline. Claimed work remains protected. To move active work to another machine, ask Boss for a handoff; do not force a claim or change worker controls.",
            if role == "agent" {
                format!(
                    "This is a replica snapshot; check the supervisor for newer allocation: {command} --host '<supervisor-ssh-host>'. "
                )
            } else {
                String::new()
            }
        ),
        "allocation_missing" => format!(
            "Companions synchronize automatically while connected; there is no one-shot sync command. Inspect the authoritative supervisor: {command} --host '<supervisor-ssh-host>'. If it has no reservation or reserves this machine, resume through it: {claim} --host '<supervisor-ssh-host>' --agent '<saved-agent-id>'. A successful supervisor claim reserves this machine atomically. Wait for automatic synchronization and re-run the local inspection before offline work. If it reserves another machine, resume there or ask Boss for a handoff."
        ),
        _ => format!(
            "Resume a released manual claim: {claim} --agent '<saved-agent-id>'. Session ownership and active worker reservations still apply."
        ),
    };
    if reason == "reserved_elsewhere"
        && let Some(host) = &reserved_ssh_host
    {
        recovery = format!(
            "Connect to the reserved device: ssh {}. {recovery}",
            quote(host)
        );
    }
    Ok(
        json!({"reason":reason,"role":role,"store_machine":node,"caller_machine":machine,"reserved_machine":reserved,"reserved_host":reserved_host,"reserved_ssh_host":reserved_ssh_host,"expires_at":expires,"authoritative":role!="agent","connection":crate::fleet::worker_connection_path(&role, db.path()),"summary":summary,"inspect_command":command,"recovery":recovery}),
    )
}

/// Only an authoritative claim can reserve previously unallocated work. The
/// caller holds the issue write transaction, so allocation and ownership commit together.
pub(crate) fn reserve_manual_claim(
    db: &Connection,
    project: &str,
    number: i64,
    machine: &str,
) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO fleet_allocations(project_id,issue_number,node) SELECT ?1,?2,?3 FROM fleet_meta WHERE id=1 AND role='controller'", params![project,number,machine])?;
    Ok(())
}

pub(crate) fn check_create(db: &Connection, project: &str, number: i64) -> Result<()> {
    let allowed: bool = db.query_row("SELECT (SELECT role FROM fleet_meta WHERE id=1)<>'agent' OR EXISTS(SELECT 1 FROM fleet_number_ranges WHERE project_id=?1 AND ?2 BETWEEN first_number AND last_number)", params![project,number], |r|r.get(0))?;
    if !allowed {
        return Err(Error::conflict(
            "The offline issue-number allocation is exhausted. Reconnect to the fleet supervisor to replenish it.",
        ));
    }
    Ok(())
}
