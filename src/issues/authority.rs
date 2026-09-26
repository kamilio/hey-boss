//! Deliberately narrow policy for issue operations sent through the fleet connection.
use super::{BatchAssignment, Error, Operation, Request, Result};

pub(crate) fn validate(request: &Request) -> Result<()> {
    match &request.operation {
        Operation::View { .. } | Operation::Allocation { .. } => return Ok(()),
        Operation::Edit {
            draft: None | Some(true),
            if_version: Some(version),
            ..
        } if *version > 0 => {}
        Operation::Reopen {
            if_version: Some(version),
            ..
        } if *version > 0 => {}
        Operation::Batch { edits }
            if edits
                .iter()
                .all(|edit| matches!(edit.assignment, BatchAssignment::Keep)) => {}
        _ => {
            return Err(Error::invalid(
                "--supervisor supports view, allocation, version-guarded title/body/label edits, drafting and reopening unassigned, unreserved issues, and label-only batches with assignment: keep. Other lifecycle, claim, assignment and reservation changes are not supported; nothing was saved. Inspect support with hey-boss fleet capabilities",
            ));
        }
    }
    if request.request_id.is_none() {
        return Err(Error::invalid(
            "--supervisor changes require --request-id; retry uncertain writes with the same ID and identical operation",
        ));
    }
    Ok(())
}

/// Called under the authority's write lock, after checking durable retry receipts.
/// No expiry or missing heartbeat proves an unfinished attempt safe to release.
pub(super) fn guard_reopen(
    db: &crate::database::Connection,
    project: &str,
    number: i64,
) -> Result<()> {
    let protected: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM issues WHERE project_id=?1 AND number=?2 AND assignee IS NOT NULL) OR EXISTS(SELECT 1 FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2) OR EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL)",
        rusqlite::params![project, number], |row| row.get(0),
    )?;
    if protected {
        return Err(Error::conflict(
            "Supervisor reopen requires an unassigned, unreserved issue with no unfinished worker attempt. Ownership and reservations were preserved; inspect issue allocation through --supervisor",
        ));
    }
    Ok(())
}
