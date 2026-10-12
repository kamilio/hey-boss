//! Draft eligibility shared by CLI, web, and project transfers.
use super::{Error, Project, Result};
use crate::database::Connection;
use rusqlite::params;

fn drafts_disabled(project: &Project) -> Error {
    Error::invalid(format!(
        "Project {:?} is not using drafts. Use regular tasks.",
        project.name
    ))
}

pub fn drafts_allowed(db: &Connection, project: &Project) -> Result<()> {
    let enabled: bool = db.query_row(
        "SELECT coalesce((SELECT drafts_enabled FROM project_settings WHERE project_id=?1),1)",
        [&project.id],
        |r| r.get(0),
    )?;
    if !enabled {
        return Err(drafts_disabled(project));
    }
    Ok(())
}
pub fn can_draft(
    db: &Connection,
    project: &Project,
    number: i64,
    state: &str,
    assignee: Option<&str>,
) -> Result<()> {
    drafts_allowed(db, project)?;
    let reserved: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL)
         OR EXISTS(SELECT 1 FROM fleet_allocations a
             LEFT JOIN fleet_allocation_deadlines d USING(project_id,issue_number)
             WHERE a.project_id=?1 AND a.issue_number=?2
               AND (d.expires_at IS NULL OR d.expires_at>?3))",
        params![project.id, number, super::worker::now()],
        |r| r.get(0),
    )?;
    if !matches!(state, "open" | "blocked") || assignee.is_some() || reserved {
        return Err(Error::conflict(
            "Only open or blocked, unassigned, unreserved issues can be drafted",
        ));
    }
    Ok(())
}
