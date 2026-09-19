//! Project outlines and directed relationships, authored by CLI and viewed on the web.
use crate::issues::{BODY_LIMIT, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Show,
    Projects,
    Links {
        node: Option<String>,
    },
    Add {
        title: String,
        body: String,
        kind: String,
        reference: Option<String>,
        reference_project: Option<String>,
        alias: Option<String>,
        under: Option<String>,
        if_version: Option<i64>,
    },
    Edit {
        node: String,
        title: Option<String>,
        body: Option<String>,
        if_version: Option<i64>,
    },
    Move {
        node: String,
        under: Option<String>,
        before: Option<String>,
        after: Option<String>,
        if_version: Option<i64>,
    },
    Remove {
        node: String,
        recursive: bool,
        if_version: Option<i64>,
    },
    Link {
        from: String,
        to: String,
        kind: String,
        description: Option<String>,
        if_version: Option<i64>,
    },
    Unlink {
        from: String,
        to: String,
        kind: String,
        if_version: Option<i64>,
    },
}
impl Operation {
    pub fn writes(&self) -> bool {
        !matches!(self, Self::Show | Self::Projects | Self::Links { .. })
    }
    pub fn validate(&self) -> Result<()> {
        let version = match self {
            Self::Add {
                title,
                body,
                kind,
                alias,
                reference,
                reference_project,
                if_version,
                ..
            } => {
                if !["text", "markdown", "issue", "pr", "notification"].contains(&kind.as_str()) {
                    return Err(Error::invalid("Unknown mindmap node kind"));
                }
                crate::issues::identifier(title, "node title", 512)?;
                validate_body(body)?;
                if let Some(alias) = alias {
                    crate::issues::identifier(alias, "node alias", 128)?;
                    if alias.contains(':') || alias.starts_with("n-") {
                        return Err(Error::invalid(
                            "Aliases cannot contain ':' or start with 'n-'",
                        ));
                    }
                }
                if ["issue", "pr", "notification"].contains(&kind.as_str()) && reference.is_none() {
                    return Err(Error::invalid("Reference node requires a reference"));
                }
                if ["text", "markdown"].contains(&kind.as_str())
                    && (reference.is_some() || reference_project.is_some())
                {
                    return Err(Error::invalid("Text nodes cannot have resource references"));
                }
                *if_version
            }
            Self::Edit {
                title,
                body,
                if_version,
                ..
            } => {
                if title.is_none() && body.is_none() {
                    return Err(Error::invalid("edit requires --title, --body or --file"));
                }
                if let Some(title) = title {
                    crate::issues::identifier(title, "node title", 512)?;
                }
                if let Some(body) = body {
                    validate_body(body)?;
                }
                *if_version
            }
            Self::Move {
                before,
                after,
                if_version,
                ..
            } => {
                if before.is_some() && after.is_some() {
                    return Err(Error::invalid("Use either --before or --after"));
                }
                *if_version
            }
            Self::Remove { if_version, .. } => *if_version,
            Self::Link {
                kind,
                description,
                if_version,
                ..
            } => {
                validate_kind(kind)?;
                if let Some(text) = description {
                    if text.len() > 16384 {
                        return Err(Error::invalid("Link description exceeds 16 KiB"));
                    }
                }
                *if_version
            }
            Self::Unlink {
                kind, if_version, ..
            } => {
                validate_kind(kind)?;
                *if_version
            }
            _ => None,
        };
        if version.is_some_and(|v| v < 0) {
            return Err(Error::invalid("Map version must be nonnegative"));
        }
        Ok(())
    }
}
fn validate_body(body: &str) -> Result<()> {
    if body.len() > BODY_LIMIT {
        Err(Error::invalid("Markdown exceeds 1 MiB"))
    } else {
        Ok(())
    }
}
fn validate_kind(kind: &str) -> Result<()> {
    crate::issues::identifier(kind, "link kind", 64)?;
    if kind == "pull-request" {
        return Err(Error::invalid(
            "pull-request links are automatic; use issue pr add/remove",
        ));
    }
    Ok(())
}

/// Decorate a graph with a fresh pending-only desktop Inbox snapshot. Failure is
/// explicit and never makes a saved notification look resolved.
pub fn enrich_notifications(graph: &mut Value, snapshot: Result<Value>) {
    let tasks = match snapshot {
        Ok(value) => match value.get("tasks").and_then(Value::as_array) {
            Some(tasks) => {
                graph["notifications"] = json!({"available":true});
                tasks.clone()
            }
            None => {
                graph["notifications"] =
                    json!({"available":false,"error":"Invalid Inbox snapshot"});
                Vec::new()
            }
        },
        Err(error) => {
            graph["notifications"] = json!({"available":false,"error":error.message});
            Vec::new()
        }
    };
    let mut hidden = Vec::new();
    for field in ["nodes", "external_nodes"] {
        if let Some(nodes) = graph.get_mut(field).and_then(Value::as_array_mut) {
            for node in nodes.iter_mut().filter(|n| n["kind"] == "notification") {
                let task = tasks.iter().find(|t| t["taskID"] == node["reference"]);
                if let Some(task) = task.filter(|t| t["status"] == "pending") {
                    node["title"] = task["title"].clone();
                    node["body"] = json!(task["summary"].as_str().unwrap_or(""));
                    node["body_html"] = json!(crate::markdown::render_fragment(
                        node["body"].as_str().unwrap_or("")
                    ));
                    node["state"] = json!("pending");
                    node["available"] = json!(true);
                } else {
                    hidden.push(node["id"].clone());
                }
            }
            // Order by original tree paths before promotion, so a completed
            // notice's children keep the notice's location among its siblings.
            let paths: std::collections::HashMap<String, (Option<String>, i64, i64)> = nodes
                .iter()
                .map(|n| {
                    (
                        n["id"].as_str().unwrap_or("").to_owned(),
                        (
                            n["parent_id"].as_str().map(str::to_owned),
                            n["position"].as_i64().unwrap_or(0),
                            n["created_at"].as_i64().unwrap_or(0),
                        ),
                    )
                })
                .collect();
            nodes.sort_by_cached_key(|node| {
                let mut current = node["id"].as_str().map(str::to_owned);
                let mut path = Vec::new();
                while let Some(id) = current {
                    let Some((parent, position, created)) = paths.get(&id) else {
                        break;
                    };
                    path.push((*position, *created, id));
                    current = parent.clone();
                    if path.len() > 32 {
                        break;
                    }
                }
                path.reverse();
                path
            });
            // Preserve nesting if a notice's ancestor has completed.
            let parents: Vec<_> = nodes
                .iter()
                .map(|n| (n["id"].clone(), n["parent_id"].clone()))
                .collect();
            for node in nodes.iter_mut() {
                while hidden.contains(&node["parent_id"]) {
                    node["parent_id"] = parents
                        .iter()
                        .find(|(id, _)| *id == node["parent_id"])
                        .map(|(_, p)| p.clone())
                        .unwrap_or(Value::Null);
                }
            }
            nodes.retain(|n| !hidden.contains(&n["id"]));
            let mut positions = std::collections::HashMap::<Option<String>, i64>::new();
            for node in nodes {
                let position = positions
                    .entry(node["parent_id"].as_str().map(str::to_owned))
                    .or_default();
                node["position"] = json!(*position);
                *position += 1;
            }
        }
    }
    if let Some(links) = graph.get_mut("links").and_then(Value::as_array_mut) {
        links.retain(|l| !hidden.contains(&l["from"]) && !hidden.contains(&l["to"]));
    }
}

pub fn needs_inbox(graph: &Value) -> bool {
    ["nodes", "external_nodes"].iter().any(|field| {
        graph[*field]
            .as_array()
            .is_some_and(|nodes| nodes.iter().any(|n| n["kind"] == "notification"))
    })
}
