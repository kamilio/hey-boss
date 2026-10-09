//! Durable scheduled definitions and occurrence decisions; no execution timer.
pub(crate) mod files;
pub mod schedule;

use crate::issues::{Error, Result, identifier};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    pub cron: String,
    pub timezone: String,
    pub harness: String,
    pub model: String,
}
impl Definition {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.name, "job name", 256)?;
        identifier(&self.model, "logical model", 256)?;
        serde_json::from_value::<crate::agent_runtime::Provider>(serde_json::json!(self.harness))
            .map_err(|_| Error::invalid("Job harness must be codex, claude, or pi"))?;
        schedule::Schedule::parse(&self.cron, &self.timezone)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub job_id: String,
    pub project_id: String,
    pub revision: i64,
    pub definition: Definition,
    /// SHA-256 of the exact UTF-8 bytes in the managed .md file.
    pub instruction_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Create {
        id: String,
        definition: Definition,
        markdown: String,
        enabled: bool,
    },
    Edit {
        id: String,
        if_revision: i64,
        definition: Definition,
        markdown: Option<String>,
    },
    SetEnabled {
        id: String,
        if_revision: i64,
        enabled: bool,
    },
    Delete {
        id: String,
        if_revision: i64,
    },
    View {
        id: String,
    },
    List {
        after: Option<String>,
        limit: usize,
        #[serde(default)]
        include_deleted: bool,
    },
    Preview {
        cron: String,
        timezone: String,
        after: i64,
        through: i64,
        limit: usize,
    },
    Next {
        id: String,
        after: i64,
        through: i64,
        limit: usize,
    },
    History {
        id: String,
        before: Option<i64>,
        limit: usize,
    },
    Revision {
        id: String,
        revision: i64,
    },
    Run {
        id: String,
        run_id: String,
    },
}
impl Operation {
    pub fn writes(&self) -> bool {
        matches!(
            self,
            Self::Create { .. } | Self::Edit { .. } | Self::SetEnabled { .. } | Self::Delete { .. }
        )
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Create {
                id,
                definition,
                markdown,
                ..
            } => {
                validate_id(id)?;
                definition.validate()?;
                files::validate(markdown)?;
            }
            Self::Edit {
                id,
                if_revision,
                definition,
                markdown,
            } => {
                validate_id(id)?;
                revision(*if_revision)?;
                definition.validate()?;
                if let Some(markdown) = markdown {
                    files::validate(markdown)?;
                }
            }
            Self::SetEnabled {
                id, if_revision, ..
            }
            | Self::Delete { id, if_revision } => {
                validate_id(id)?;
                revision(*if_revision)?;
            }
            Self::View { id } => validate_id(id)?,
            Self::List { after, limit, .. } => {
                page(*limit)?;
                if let Some(id) = after {
                    validate_id(id)?;
                }
            }
            Self::Preview {
                cron,
                timezone,
                after,
                through,
                limit,
            } => {
                schedule::Schedule::parse(cron, timezone)?.preview(*after, *through, *limit)?;
            }
            Self::Next {
                id,
                after,
                through,
                limit,
            } => {
                validate_id(id)?;
                range(*after, *through, *limit)?;
            }
            Self::History { id, before, limit } => {
                validate_id(id)?;
                page(*limit)?;
                if before.is_some_and(|v| v <= 0) {
                    return Err(Error::invalid("Invalid history cursor"));
                }
            }
            Self::Revision { id, revision: r } => {
                validate_id(id)?;
                revision(*r)?;
            }
            Self::Run { id, run_id } => {
                validate_id(id)?;
                validate_id(run_id)?;
            }
        }
        Ok(())
    }
}
fn revision(value: i64) -> Result<()> {
    if value <= 0 || value == i64::MAX {
        Err(Error::invalid(
            "Revision must be positive and incrementable",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn page(limit: usize) -> Result<()> {
    if !(1..=100).contains(&limit) {
        Err(Error::invalid("Page limit must be 1–100"))
    } else {
        Ok(())
    }
}
fn range(after: i64, through: i64, limit: usize) -> Result<()> {
    if !(1..=schedule::MAX_PREVIEW).contains(&limit)
        || through
            .checked_sub(after)
            .is_none_or(|n| !(0..=schedule::MAX_RANGE_MS).contains(&n))
    {
        return Err(Error::invalid("Invalid preview bounds"));
    }
    Ok(())
}
pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::invalid(
            "Job/run ID must contain 1–128 ASCII letters, digits, hyphens or underscores",
        ));
    }
    Ok(())
}

/// Prepared outside the writer; the commit checks the revision and cursor again.
#[derive(Debug, Clone)]
pub struct Occurrence {
    pub snapshot: Snapshot,
    pub scheduled_at: i64,
    pub observed_at: i64,
    pub(crate) expected_next: Option<i64>,
    pub(crate) next_at: Option<i64>,
    pub(crate) request_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub sequence: i64,
    pub id: String,
    pub snapshot: Snapshot,
    pub scheduled_at: i64,
    pub trigger: String,
    pub request_key: Option<String>,
    pub state: String,
    pub reason: Option<String>,
    pub task_number: Option<i64>,
    pub machine: Option<String>,
    pub session_id: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}
