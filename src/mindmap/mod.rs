//! Project outlines and directed relationships, authored by CLI and viewed on the web.
use crate::issues::{BODY_LIMIT, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const READ_LIMIT: usize = 32 * 1024 * 1024;
/// Count serialized bytes without allocating another copy of a large map.
pub(crate) struct ReadBudget {
    bytes: usize,
}
impl Default for ReadBudget {
    fn default() -> Self {
        Self { bytes: 64 * 1024 }
    }
}
impl std::io::Write for ReadBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > READ_LIMIT.saturating_sub(self.bytes) {
            return Err(std::io::Error::other("Mindmap response exceeds 32 MiB"));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl ReadBudget {
    pub(crate) fn charge(&mut self, value: &Value) -> Result<()> {
        serde_json::to_writer(self, value).map_err(|_| Error::invalid(
            "Mindmap response exceeds 32 MiB; use show --bodies preview or none, view NODE for full text, or links NODE for one topic's relationships"
        ))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum BodyMode {
    #[default]
    Full,
    Preview,
    None,
}

/// Prepare a bounded display body without losing its saved source.
pub fn project_body(node: &mut Value, mode: BodyMode) {
    let body = node["body"].as_str().unwrap_or("");
    let has_body = node["has_body"] == true || !body.is_empty();
    let previously_truncated = node["body_truncated"] == true;
    let (body, truncated) = match mode {
        BodyMode::Full => (body.to_owned(), false),
        BodyMode::None => (String::new(), has_body),
        BodyMode::Preview => match body.char_indices().nth(512) {
            Some((index, _)) => (body[..index].to_owned(), true),
            None => (body.to_owned(), previously_truncated),
        },
    };
    node["has_body"] = json!(has_body);
    node["body_truncated"] = json!(truncated);
    node["body_html"] = json!(crate::markdown::render_fragment(&body));
    node["body"] = json!(body);
}

/// Restricted operations accepted by one atomic organization batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum BatchEdit {
    Edit {
        node: String,
        title: Option<String>,
        #[serde(default)]
        clear_label: bool,
    },
    Alias {
        node: String,
        alias: Option<String>,
    },
    Move {
        node: String,
        under: Option<String>,
        before: Option<String>,
        after: Option<String>,
    },
}
impl BatchEdit {
    pub(super) fn operation(&self) -> Operation {
        match self {
            Self::Edit {
                node,
                title,
                clear_label,
            } => Operation::Edit {
                node: node.clone(),
                title: title.clone(),
                body: None,
                clear_label: *clear_label,
                if_version: None,
            },
            Self::Alias { node, alias } => Operation::Alias {
                node: node.clone(),
                alias: alias.clone(),
                if_version: None,
            },
            Self::Move {
                node,
                under,
                before,
                after,
            } => Operation::Move {
                node: node.clone(),
                under: under.clone(),
                before: before.clone(),
                after: after.clone(),
                if_version: None,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Batch {
        edits: Vec<BatchEdit>,
        #[serde(default)]
        dry_run: bool,
        if_version: Option<i64>,
    },
    Show {
        #[serde(default)]
        body_mode: BodyMode,
    },
    View {
        node: String,
        #[serde(default)]
        body_mode: BodyMode,
    },
    Projects,
    Links {
        node: Option<String>,
    },
    Add {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_label: Option<String>,
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
        #[serde(default)]
        clear_label: bool,
        if_version: Option<i64>,
    },
    Alias {
        node: String,
        alias: Option<String>,
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
        !matches!(
            self,
            Self::Show { .. } | Self::View { .. } | Self::Projects | Self::Links { .. }
        )
    }
    pub fn validate(&self) -> Result<()> {
        let version = match self {
            Self::Batch {
                edits, if_version, ..
            } => {
                if edits.len() > 10000 {
                    return Err(Error::invalid("A batch supports at most 10000 edits"));
                }
                for edit in edits {
                    edit.operation().validate()?;
                }
                if serde_json::to_vec(edits)?.len() > BODY_LIMIT {
                    return Err(Error::invalid("Batch exceeds 1 MiB"));
                }
                *if_version
            }
            Self::Add {
                title,
                display_label,
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
                let title_limit = if kind == "pr" && reference.as_deref() == Some(title.as_str()) {
                    2048
                } else {
                    512
                };
                crate::issues::identifier(title, "node title", title_limit)?;
                if let Some(label) = display_label {
                    if kind != "issue" {
                        return Err(Error::invalid(
                            "Initial display labels require an issue node",
                        ));
                    }
                    crate::issues::identifier(label, "node title", 512)?;
                }
                validate_body(body)?;
                if let Some(alias) = alias {
                    validate_alias(alias)?;
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
                clear_label,
                if_version,
                ..
            } => {
                if *clear_label && (title.is_some() || body.is_some()) {
                    return Err(Error::invalid(
                        "--clear-label cannot be combined with --title or body edits",
                    ));
                }
                if title.is_none() && body.is_none() && !clear_label {
                    return Err(Error::invalid(
                        "edit requires --title, --body, --file or --clear-label",
                    ));
                }
                if let Some(title) = title {
                    // The store checks the ordinary label limit after resolving
                    // the node; a PR can restore its full URL as the label.
                    crate::issues::identifier(title, "node title", 2048)?;
                }
                if let Some(body) = body {
                    validate_body(body)?;
                }
                *if_version
            }
            Self::Alias {
                alias, if_version, ..
            } => {
                if let Some(alias) = alias {
                    validate_alias(alias)?;
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
                if let Some(text) = description
                    && text.len() > 16384
                {
                    return Err(Error::invalid("Link description exceeds 16 KiB"));
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
fn validate_alias(alias: &str) -> Result<()> {
    crate::issues::identifier(alias, "node alias", 128)?;
    if alias.contains(':') || alias.starts_with("n-") {
        return Err(Error::invalid(
            "Aliases cannot contain ':' or start with 'n-'",
        ));
    }
    Ok(())
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
pub fn enrich_notifications(graph: &mut Value, snapshot: Result<Value>) -> Result<()> {
    let mut budget = ReadBudget::default();
    for field in ["nodes", "external_nodes", "links"] {
        if let Some(items) = graph[field].as_array() {
            for item in items.iter().filter(|item| item["kind"] != "notification") {
                budget.charge(item)?;
            }
        }
    }
    let mode = serde_json::from_value(graph["body_mode"].clone()).unwrap_or(BodyMode::Full);
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
    let pending: std::collections::HashMap<String, Value> = tasks
        .into_iter()
        .filter(|task| task["status"] == "pending")
        .filter_map(|task| Some((task["taskID"].as_str()?.to_owned(), task)))
        .collect();
    let mut hidden = std::collections::HashSet::<String>::new();
    for field in ["nodes", "external_nodes"] {
        if let Some(nodes) = graph.get_mut(field).and_then(Value::as_array_mut) {
            for node in nodes.iter_mut().filter(|n| n["kind"] == "notification") {
                if let Some(task) = pending.get(node["reference"].as_str().unwrap_or("")) {
                    node["title"] = task["title"].clone();
                    node["body"] = json!(task["summary"].as_str().unwrap_or(""));
                    project_body(node, mode);
                    node["state"] = json!("pending");
                    node["available"] = json!(true);
                    budget.charge(node)?;
                } else {
                    hidden.insert(node["id"].as_str().unwrap_or("").to_owned());
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
            // Preserve nesting through completed ancestors, using their original parents.
            for node in nodes.iter_mut() {
                while hidden.contains(node["parent_id"].as_str().unwrap_or("")) {
                    node["parent_id"] = paths
                        .get(node["parent_id"].as_str().unwrap_or(""))
                        .and_then(|(parent, _, _)| parent.as_deref())
                        .map(|parent| json!(parent))
                        .unwrap_or(Value::Null);
                }
            }
            nodes.retain(|n| !hidden.contains(n["id"].as_str().unwrap_or("")));
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
        links.retain(|l| {
            !hidden.contains(l["from"].as_str().unwrap_or(""))
                && !hidden.contains(l["to"].as_str().unwrap_or(""))
        });
    }
    if let Some(selected) = graph.get("node") {
        let selected_id = selected["id"].clone();
        let projected = ["nodes", "external_nodes"]
            .iter()
            .filter_map(|field| graph[*field].as_array())
            .flatten()
            .find(|node| node["id"] == selected_id)
            .cloned();
        if let Some(node) = projected {
            graph["node"] = node;
        } else if hidden.contains(selected_id.as_str().unwrap_or("")) {
            graph.as_object_mut().unwrap().remove("node");
            graph["selected_node_id"] = selected_id;
        }
    }
    ReadBudget::default().charge(graph)
}

pub fn needs_inbox(graph: &Value) -> bool {
    ["nodes", "external_nodes"].iter().any(|field| {
        graph[*field]
            .as_array()
            .is_some_and(|nodes| nodes.iter().any(|n| n["kind"] == "notification"))
    })
}
