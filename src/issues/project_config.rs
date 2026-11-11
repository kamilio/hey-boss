//! Project configuration snapshots. The existing fleet journal transports these
//! immutable records alongside its last-write-wins settings projection.
use super::{Actor, Error, Operation as IssueOperation, Project, Result};
use crate::database::Connection;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS project_config_revisions(
 id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES projects(id),parent TEXT,
 sequence INTEGER NOT NULL,created_at INTEGER NOT NULL,actor TEXT NOT NULL,
 action TEXT NOT NULL,target TEXT,before_json TEXT NOT NULL,after_json TEXT NOT NULL,fields TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS project_config_history ON project_config_revisions(project_id,sequence DESC,id DESC);
CREATE TABLE IF NOT EXISTS project_config_checkouts(
 path TEXT PRIMARY KEY,project_id TEXT NOT NULL,base TEXT NOT NULL,error TEXT,updated_at INTEGER NOT NULL);
";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    History {
        #[serde(default)]
        offset: u32,
    },
    View {
        revision: String,
    },
    Apply {
        changes: Value,
        #[serde(default)]
        base: Option<String>,
    },
    Undo {
        revision: String,
        #[serde(default)]
        if_digest: Option<String>,
    },
    Restore {
        revision: String,
        #[serde(default)]
        if_digest: Option<String>,
    },
}
impl Operation {
    pub fn writes(&self) -> bool {
        matches!(
            self,
            Self::Apply { .. } | Self::Undo { .. } | Self::Restore { .. }
        )
    }
}

const FIELDS: &[&str] = &[
    "worker_model",
    "worker_reasoning",
    "worker_permissions",
    "paused",
    "pause_reason",
    "prompt",
    "prompt_overrides",
    "prs_enabled",
    "worktree_enabled",
    "drafts_enabled",
    "artifacts_enabled",
    "code_reviews_enabled",
];

pub(super) fn snapshot(settings: &Value) -> Value {
    Value::Object(
        FIELDS
            .iter()
            .map(|key| ((*key).into(), settings[*key].clone()))
            .collect(),
    )
}
pub(super) fn digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

// Individual prompt sections are independent edits, including resetting an
// override to its inherited default.
fn changes(before: &Value, after: &Value) -> Value {
    let mut changes = serde_json::Map::new();
    for key in FIELDS {
        if *key == "prompt_overrides" {
            for (section, _) in super::worker::prompt_defaults().as_object().unwrap() {
                if before[*key][section] != after[*key][section] {
                    changes.insert(
                        format!("prompt_overrides.{section}"),
                        json!({"before":before[*key][section],"after":after[*key][section]}),
                    );
                }
            }
        } else if before[*key] != after[*key] {
            changes.insert(
                (*key).into(),
                json!({"before":before[*key],"after":after[*key]}),
            );
        }
    }
    Value::Object(changes)
}

fn patch(snapshot: &mut Value, patch: &Value) -> Result<()> {
    for (key, value) in patch
        .as_object()
        .ok_or_else(|| Error::invalid("Configuration changes must be an object"))?
    {
        if let Some(section) = key.strip_prefix("prompt_overrides.") {
            if super::worker::prompt_defaults().get(section).is_none() {
                return Err(Error::invalid("Unknown prompt section"));
            }
            if value.is_null() {
                snapshot["prompt_overrides"]
                    .as_object_mut()
                    .unwrap()
                    .remove(section);
            } else {
                snapshot["prompt_overrides"][section] = value.clone();
            }
        } else if FIELDS.contains(&key.as_str()) && key != "prompt_overrides" {
            if value.is_null() {
                return Err(Error::invalid(format!("{key} cannot be null")));
            }
            snapshot[key] = value.clone();
        } else {
            return Err(Error::invalid(format!(
                "Unknown configuration field: {key}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn record(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    operation: &IssueOperation,
    before: &Value,
    after: &Value,
    now: i64,
) -> Result<Option<String>> {
    if before == after {
        return Ok(None);
    }
    let (mut parent, sequence): (Option<String>, i64) = db.query_row(
        "SELECT (SELECT id FROM project_config_revisions WHERE project_id=?1 ORDER BY sequence DESC,id DESC LIMIT 1),coalesce(max(sequence),0)+1 FROM project_config_revisions WHERE project_id=?1", [&project.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let (action, target) = match operation {
        IssueOperation::ProjectConfig {
            operation: Operation::Undo { revision, .. },
        } => ("undo", Some(revision.as_str())),
        IssueOperation::ProjectConfig {
            operation: Operation::Restore { revision, .. },
        } => ("restore", Some(revision.as_str())),
        IssueOperation::ProjectConfig {
            operation: Operation::Apply {
                base: Some(base), ..
            },
        } => {
            parent = Some(base.clone());
            ("edit", None)
        }
        _ => ("edit", None),
    };
    let id = super::worker::random_id()?;
    let author = json!({"id":actor.id,"kind":actor.kind,"session_id":actor.session_id,"machine":actor.machine,"host":actor.host,"source":actor.source});
    let fields = json!(
        changes(before, after)
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
    );
    db.execute(
        "INSERT INTO project_config_revisions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            id,
            project.id,
            parent,
            sequence,
            now,
            author.to_string(),
            action,
            target,
            before.to_string(),
            after.to_string(),
            fields.to_string()
        ],
    )?;
    Ok(Some(id))
}

pub(crate) fn execute(
    db: &Connection,
    project: &Project,
    operation: &Operation,
    actor: Option<&Actor>,
) -> Result<Value> {
    let settings = super::store::registry::project_settings(db, project)?;
    let current = snapshot(&settings);
    let current_digest = digest(&current);
    if let Operation::History { offset } = operation {
        let mut revisions = db.prepare("SELECT id,parent,sequence,created_at,actor,action,target,fields FROM project_config_revisions WHERE project_id=?1 ORDER BY sequence DESC,id DESC LIMIT 51 OFFSET ?2")?
            .query_map(params![project.id,offset], |r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,String>(7)?)))?
            .map(|row| { let (id,parent,sequence,at,actor,action,target,fields) = row?;
                Ok(json!({"id":id,"parent":parent,"sequence":sequence,"created_at":at,"actor":serde_json::from_str::<Value>(&actor)?,"action":action,"target":target,"fields":serde_json::from_str::<Value>(&fields)?}))
            }).collect::<Result<Vec<_>>>()?;
        let more = revisions.len() > 50;
        revisions.truncate(50);
        let copies = db
            .prepare(
                "SELECT path,error FROM project_config_checkouts WHERE project_id=?1 ORDER BY path",
            )?
            .query_map([&project.id], |r| {
                Ok(json!({"path":r.get::<_,String>(0)?,"error":r.get::<_,Option<String>>(1)?}))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        return Ok(
            json!({"ok":true,"project":project,"digest":current_digest,"revisions":revisions,"working_copies":copies,"next_offset":more.then_some(u64::from(*offset)+50)}),
        );
    }
    let mut desired = current.clone();
    match operation {
        Operation::Apply { changes, .. } => patch(&mut desired, changes)?,
        Operation::View { revision }
        | Operation::Undo { revision, .. }
        | Operation::Restore { revision, .. } => {
            let row: Option<(String,String)> = db.query_row("SELECT before_json,after_json FROM project_config_revisions WHERE project_id=?1 AND id=?2", params![project.id,revision], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (before, after) =
                row.ok_or_else(|| Error::new("not_found", "Configuration revision was not found"))?;
            let before: Value = serde_json::from_str(&before)?;
            let after: Value = serde_json::from_str(&after)?;
            let delta = changes(&before, &after);
            let inverse = Value::Object(
                delta
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v["before"].clone()))
                    .collect(),
            );
            patch(&mut desired, &inverse)?;
            if matches!(operation, Operation::View { .. }) {
                return Ok(
                    json!({"ok":true,"project":project,"revision":revision,"before":before,"after":after,"changes":delta,"undo_changes":changes(&current,&desired),"restore_changes":changes(&current,&after),"digest":current_digest}),
                );
            }
            let guard = match operation {
                Operation::Undo { if_digest, .. } | Operation::Restore { if_digest, .. } => {
                    if_digest
                }
                _ => unreachable!(),
            };
            if guard.as_ref().is_some_and(|v| v != &current_digest) {
                return Err(Error::conflict(
                    "Settings changed since the preview; refresh to review the current changes",
                ));
            }
            if matches!(operation, Operation::Restore { .. }) {
                desired = after;
            }
        }
        Operation::History { .. } => unreachable!(),
    }
    if desired == current {
        return Ok(settings);
    }
    desired["action"] = json!("configure_project");
    super::store::registry::execute(db, project, &serde_json::from_value(desired)?, actor)
}
