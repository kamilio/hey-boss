//! Additive fleet metadata. No network filesystem or schema-version coupling.
use super::{Error, Result};
use rusqlite::{Connection, params};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS fleet_meta(id INTEGER PRIMARY KEY CHECK(id=1),role TEXT NOT NULL,node TEXT NOT NULL,syncing INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO fleet_meta VALUES(1,'standalone','',0);
CREATE TABLE IF NOT EXISTS fleet_allocations(project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,node TEXT NOT NULL,PRIMARY KEY(project_id,issue_number));
CREATE TABLE IF NOT EXISTS fleet_number_ranges(project_id TEXT PRIMARY KEY,first_number INTEGER NOT NULL,last_number INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS fleet_outbox(seq INTEGER PRIMARY KEY AUTOINCREMENT,table_name TEXT NOT NULL,before_json TEXT,after_json TEXT,created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS fleet_receipts(node TEXT NOT NULL,seq INTEGER NOT NULL,result TEXT NOT NULL,PRIMARY KEY(node,seq));
CREATE TABLE IF NOT EXISTS fleet_row_ids(origin TEXT NOT NULL,table_name TEXT NOT NULL,origin_id INTEGER NOT NULL,local_id INTEGER NOT NULL,PRIMARY KEY(origin,table_name,origin_id));
CREATE TABLE IF NOT EXISTS fleet_conflicts(id TEXT PRIMARY KEY,node TEXT NOT NULL,seq INTEGER NOT NULL,table_name TEXT NOT NULL,data TEXT NOT NULL,reason TEXT NOT NULL,created_at INTEGER NOT NULL,resolved INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS fleet_subtask_receipts(node TEXT NOT NULL,seq INTEGER NOT NULL,project_id TEXT NOT NULL,child_number INTEGER NOT NULL,parent_number INTEGER NOT NULL,kind TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(node,seq));
CREATE INDEX IF NOT EXISTS fleet_subtask_receipt_key ON fleet_subtask_receipts(node,project_id,child_number,seq DESC);
CREATE TABLE IF NOT EXISTS fleet_deferred_subtasks(project_id TEXT NOT NULL,child_number INTEGER NOT NULL,row_json TEXT,PRIMARY KEY(project_id,child_number));
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
    let blocked: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2 AND node<>?3) OR ((SELECT role FROM fleet_meta WHERE id=1)='agent' AND NOT EXISTS(SELECT 1 FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2 AND node=?3))", params![project,number,machine], |r|r.get(0))?;
    if blocked {
        return Err(Error::conflict(
            "This issue is reserved for another fleet machine, or has not been allocated to this agent. Synchronize before pickup.",
        ));
    }
    Ok(())
}

pub(crate) fn check_create(db: &Connection, project: &str, number: i64) -> Result<()> {
    let allowed: bool = db.query_row("SELECT (SELECT role FROM fleet_meta WHERE id=1)<>'agent' OR EXISTS(SELECT 1 FROM fleet_number_ranges WHERE project_id=?1 AND ?2 BETWEEN first_number AND last_number)", params![project,number], |r|r.get(0))?;
    if !allowed {
        return Err(Error::conflict(
            "The offline issue-number allocation is exhausted. Reconnect to the fleet controller to replenish it.",
        ));
    }
    Ok(())
}
