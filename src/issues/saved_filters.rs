use super::{Error, Project, Result};
use crate::database::Connection;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS project_issue_filters(
 project_id TEXT NOT NULL REFERENCES projects(id),name TEXT NOT NULL,
 filters TEXT NOT NULL CHECK(json_valid(filters)),PRIMARY KEY(project_id,name));";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    List,
    Save { name: String, url: String },
    Delete { name: String },
}

fn filters(url: &str, project: &Project) -> Result<Value> {
    let route = crate::routes::resolve(url)?;
    if route.entity != "issues" {
        return Err(Error::invalid("Use an issue list URL to save a filter"));
    }
    if route.project.as_ref().is_some_and(|id| id != &project.id) {
        return Err(Error::invalid(
            "The filter URL belongs to a different project",
        ));
    }
    let param = |name: &str| route.params.get(name).map(String::as_str).unwrap_or("");
    let state = match param("state") {
        state @ ("open" | "blocked" | "ready" | "closed" | "deleted" | "all") => state,
        _ => "open",
    };
    let states = ["open", "blocked", "ready", "closed"]
        .into_iter()
        .filter(|value| state == "all" && param("states").split(',').any(|v| v == *value))
        .collect::<Vec<_>>()
        .join(",");
    let min = param("min_launches");
    let min = min
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991 && n.to_string() == min)
        .map(|_| min)
        .unwrap_or("");
    Ok(json!({
        "state":state, "states":states,
        "owner":if param("owner").is_empty() { "all" } else { param("owner") },
        "created_by":param("created_by"), "label":param("label"), "search":param("search"),
        "blocked":if state == "blocked" && matches!(param("blocked"), "hold" | "dependencies") {param("blocked")} else {""},
        "draft":if param("draft") == "1" {"1"} else {""}, "min_launches":min,
    }))
}

pub(crate) fn list(db: &Connection, project: &str) -> Result<Vec<Value>> {
    db.prepare("SELECT name,filters FROM project_issue_filters WHERE project_id=?1 ORDER BY name")?
        .query_map([project], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .map(|row| {
            let (name, filters) = row?;
            Ok(json!({"name":name,"filters":serde_json::from_str::<Value>(&filters)?}))
        })
        .collect()
}

pub(crate) fn execute(db: &Connection, project: &Project, operation: &Operation) -> Result<Value> {
    match operation {
        Operation::List => {}
        Operation::Save { name, url } => {
            super::identifier(name, "filter name", 120)?;
            let filters = filters(url, project)?;
            db.execute(
                "INSERT INTO project_issue_filters(project_id,name,filters) VALUES(?1,?2,?3)
                ON CONFLICT(project_id,name) DO UPDATE SET filters=excluded.filters",
                params![project.id, name, filters.to_string()],
            )?;
        }
        Operation::Delete { name } => {
            super::identifier(name, "filter name", 120)?;
            db.execute(
                "DELETE FROM project_issue_filters WHERE project_id=?1 AND name=?2",
                params![project.id, name],
            )?;
        }
    }
    Ok(json!({"ok":true,"project":project,"saved_filters":list(db,&project.id)?}))
}
