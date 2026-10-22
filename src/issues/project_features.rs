use super::{Error, Project, Result};
use crate::database::Connection;

pub(crate) enum Feature {
    Artifacts,
}

impl Feature {
    pub(crate) fn require(&self, db: &Connection, project: &Project) -> Result<()> {
        let (name, query) = match self {
            Self::Artifacts => (
                "artifacts",
                "SELECT coalesce((SELECT artifacts_enabled FROM project_settings WHERE project_id=?1),1)",
            ),
        };
        if !db.query_row(query, [&project.id], |row| row.get::<_, bool>(0))? {
            return Err(Error::invalid(format!(
                "Project {:?} is not using {name}. Use regular tasks.",
                project.name
            )));
        }
        Ok(())
    }
}
