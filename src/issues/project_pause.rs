use super::{Actor, Error, Project, Result};
use crate::database::Connection;
use rusqlite::OptionalExtension;

pub(super) fn reason(db: &Connection, project: &str) -> Result<Option<String>> {
    Ok(db
        .query_row(
            "SELECT pause_reason FROM project_settings WHERE project_id=?1 AND paused=1",
            [project],
            |r| r.get(0),
        )
        .optional()?)
}

pub(super) fn check_create(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    force: bool,
) -> Result<()> {
    if !force
        && actor.kind != "human"
        && let Some(reason) = reason(db, &project.id)?
    {
        return Err(Error::new(
            "project_paused",
            format!(
                "Project {:?} is paused: {reason}. To create this issue anyway, retry with --force. Worker pickup stays paused.",
                project.name
            ),
        ));
    }
    Ok(())
}
