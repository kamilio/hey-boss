//! Notice relationships and the local desktop Inbox bridge.
use crate::issues::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IssueReference {
    pub project: String,
    pub number: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}
impl IssueReference {
    pub fn validate(&self) -> Result<()> {
        crate::issues::identifier(&self.project, "issue project", 8192)?;
        if self.number <= 0 {
            return Err(Error::invalid("Issue number must be positive"));
        }
        if let Some(host) = &self.host
            && !crate::health::remote::valid_host(host)
        {
            return Err(Error::invalid("Invalid issue SSH host"));
        }
        Ok(())
    }
}
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    List,
    Count,
    Clear {
        task_ids: Vec<String>,
    },
    View {
        task_id: String,
    },
    Read {
        task_id: String,
    },
    Respond {
        task_id: String,
        answer: String,
    },
    Dismiss {
        task_id: String,
    },
    Comment {
        task_id: String,
        body: String,
        quote: Option<String>,
    },
    FinishReview {
        task_id: String,
    },
    Link {
        task_id: String,
        issue: Option<IssueReference>,
    },
    OpenLink {
        task_id: String,
    },
    OpenArtifact {
        project: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        host: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        issue: Option<i64>,
        #[serde(default)]
        node: Option<String>,
        #[serde(default)]
        file: Option<String>,
    },
}
impl Action {
    pub fn payload(&self) -> Result<Value> {
        let (command, id) = match self {
            Self::List | Self::Count => ("inbox_list", None),
            Self::Clear { .. } => ("inbox_clear", None),
            Self::View { task_id } => ("inbox_view", Some(task_id)),
            Self::Read { task_id } => ("inbox_read", Some(task_id)),
            Self::Respond { task_id, .. } => ("inbox_respond", Some(task_id)),
            Self::Dismiss { task_id } => ("inbox_dismiss", Some(task_id)),
            Self::Comment { task_id, .. } => ("inbox_comment", Some(task_id)),
            Self::FinishReview { task_id } => ("inbox_finish_review", Some(task_id)),
            Self::Link { task_id, .. } => ("inbox_link", Some(task_id)),
            Self::OpenLink { task_id } => ("inbox_open_link", Some(task_id)),
            Self::OpenArtifact { .. } => ("artifact_editor", None),
        };
        if let Some(id) = id {
            crate::issues::identifier(id, "task ID", 256)?;
        }
        let mut value = json!({"command":command,"sync":false,"task_id":id});
        match self {
            Self::Count => value["count_only"] = json!(true),
            Self::Clear { task_ids } => {
                if task_ids.is_empty() || task_ids.len() > 10000 {
                    return Err(Error::invalid("Select 1–10000 notices to clear"));
                }
                let mut seen = std::collections::HashSet::new();
                for id in task_ids {
                    crate::issues::identifier(id, "task ID", 256)?;
                    if !seen.insert(id) {
                        return Err(Error::invalid("Notice IDs must be unique"));
                    }
                }
                value["task_ids"] = json!(task_ids);
            }
            Self::Respond { answer, .. } => {
                if answer.trim().is_empty() || answer.len() > 65536 {
                    return Err(Error::invalid("Answer must contain 1–65536 bytes"));
                }
                value["question"] = json!(answer);
            }
            Self::Comment { body, quote, .. } => {
                if body.trim().is_empty() || body.len() > 16384 {
                    return Err(Error::invalid("Comment must contain 1–16384 bytes"));
                }
                value["question"] = json!(body);
                value["description"] = json!(quote);
            }
            Self::Link { issue, .. } => {
                if let Some(issue) = issue {
                    issue.validate()?;
                }
                value["issue"] = json!(issue);
            }
            Self::OpenArtifact {
                project,
                id,
                host,
                title,
                body,
                issue,
                node,
                file,
            } => {
                crate::issues::identifier(project, "artifact project", 8192)?;
                if let Some(id) = id.as_deref().filter(|s| !s.is_empty()) {
                    crate::issues::identifier(id, "artifact ID", 128)?;
                }
                if let Some(host) = host.as_deref().filter(|s| !s.is_empty())
                    && !crate::health::remote::valid_host(host)
                {
                    return Err(Error::invalid("Invalid artifact SSH host"));
                }
                if let Some(title) = title.as_deref().filter(|s| !s.is_empty()) {
                    crate::issues::identifier(title, "title", 512)?;
                }
                if let Some(body) = body
                    && body.len() > crate::issues::BODY_LIMIT
                {
                    return Err(Error::invalid("Markdown must be at most 1 MiB"));
                }
                if issue.is_some_and(|n| n <= 0) {
                    return Err(Error::invalid("Issue number must be positive"));
                }
                if let Some(node) = node.as_deref().filter(|s| !s.is_empty()) {
                    crate::issues::identifier(node, "mindmap node", 2048)?;
                }
                if let Some(file) = file.as_deref().filter(|s| !s.is_empty())
                    && (file.len() > 4096 || file.contains('\0'))
                {
                    return Err(Error::invalid("Invalid Markdown file path"));
                }
                value["project"] = json!(project);
                value["title"] = json!(title);
                value["question"] = json!(serde_json::to_string(&json!({
                    "project": project,
                    "id": id.as_deref().filter(|s| !s.is_empty()),
                    "host": host.as_deref().filter(|s| !s.is_empty()),
                    "title": title,
                    "body": body,
                    "issue": issue,
                    "node": node.as_deref().filter(|s| !s.is_empty()),
                    "file": file.as_deref().filter(|s| !s.is_empty()),
                }))?);
            }
            _ => {}
        }
        Ok(value)
    }
}
pub(crate) fn socket_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HEY_BOSS_INBOX_SOCKET") {
        return Ok(path.into());
    }
    if let Some(path) = std::env::var_os("HEY_BOSS_STATE_DIR") {
        return Ok(PathBuf::from(path).join("daemon.sock"));
    }
    if let Ok(executable) = std::env::current_exe().and_then(|p| p.canonicalize())
        && let Ok(state) = std::fs::read_to_string(executable.with_file_name("hey-boss.state"))
    {
        return Ok(PathBuf::from(state.trim()).join("daemon.sock"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        Error::new(
            "inbox_unavailable",
            "Start the hey-boss desktop app to use Inbox",
        )
    })?;
    Ok(PathBuf::from(home).join("Library/Application Support/hey-boss/daemon.sock"))
}
pub fn execute(action: &Action) -> Result<Value> {
    let payload = action.payload()?;
    let mut stream = UnixStream::connect(socket_path()?).map_err(|_| {
        Error::new(
            "inbox_unavailable",
            "Inbox is unavailable. Start or update the hey-boss desktop app, then refresh.",
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(&serde_json::to_vec(&payload)?)?;
    stream.shutdown(Shutdown::Write)?;
    let mut bytes = Vec::new();
    stream.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(Error::new(
            "inbox_unavailable",
            "Inbox response exceeds the supported size",
        ));
    }
    let reply: Value = serde_json::from_slice(&bytes)?;
    if reply["status"] != "ok" {
        return Err(Error::new(
            "inbox_error",
            reply["error"].as_str().unwrap_or("Inbox action failed"),
        ));
    }
    let result: Value = serde_json::from_str(reply["result"].as_str().ok_or_else(|| {
        Error::new(
            "inbox_unavailable",
            "Update the desktop app to use web Inbox",
        )
    })?)?;
    project_response(action, result)
}

fn project_response(action: &Action, mut result: Value) -> Result<Value> {
    if matches!(action, Action::Count) {
        // Older desktop daemons ignore count_only and return their full list.
        // Keep the web response compact while desktop/CLI upgrades converge.
        let unread = result["unread"]
            .as_u64()
            .ok_or_else(|| Error::invalid("Invalid Inbox unread count"))?;
        return Ok(json!({"ok":true,"unread":unread}));
    }
    if let Some(task) = result.get_mut("task") {
        let body = if task["kind"] == "update" || task["kind"] == "alert" {
            task["question"].as_str()
        } else {
            task["description"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| task["question"].as_str())
        };
        task["body_html"] = json!(crate::markdown::render_fragment(body.unwrap_or("")));
        if let Some(comments) = task["comments"].as_array_mut() {
            for comment in comments {
                comment["body_html"] = json!(crate::markdown::render_fragment(
                    comment["text"].as_str().unwrap_or("")
                ));
            }
        }
    }
    result["ok"] = json!(true);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unread_count_uses_a_compact_backward_compatible_inbox_request() {
        let action: Action = serde_json::from_value(json!({"action":"count"})).unwrap();
        let payload = action.payload().unwrap();
        assert_eq!(payload["command"], "inbox_list");
        assert_eq!(payload["count_only"], true);
        for reply in [
            json!({"unread":4}),
            json!({"unread":4,"changed":false,"tasks":[{"title":"Legacy notice","iconData":"x".repeat(200000)}]}),
        ] {
            assert_eq!(
                project_response(&action, reply).unwrap(),
                json!({"ok":true,"unread":4})
            );
        }
        for reply in [json!({}), json!({"unread":-1}), json!({"unread":"4"})] {
            assert!(project_response(&action, reply).is_err());
        }
    }

    #[test]
    fn clear_accepts_only_a_bounded_explicit_snapshot() {
        let action: Action =
            serde_json::from_value(json!({"action":"clear","task_ids":["one","two"]})).unwrap();
        let payload = action.payload().unwrap();
        assert_eq!(payload["command"], "inbox_clear");
        assert_eq!(payload["task_ids"], json!(["one", "two"]));
        for ids in [
            json!([]),
            json!([""]),
            json!(["one", "one"]),
            json!(vec!["one"; 10001]),
        ] {
            assert!(
                serde_json::from_value::<Action>(json!({"action":"clear","task_ids":ids}))
                    .unwrap()
                    .payload()
                    .is_err()
            );
        }
        assert!(serde_json::from_value::<Action>(json!({"action":"clear"})).is_err());
    }
    #[test]
    fn references_and_payloads_reject_invalid_or_unrelated_capabilities() {
        let reference = IssueReference {
            project: "github.com/example/repo".into(),
            number: 1,
            host: Some("devbox".into()),
        };
        assert!(reference.validate().is_ok());
        for issue in [
            IssueReference {
                number: 0,
                ..reference.clone()
            },
            IssueReference {
                project: "   ".into(),
                ..reference.clone()
            },
            IssueReference {
                host: Some("-oProxyCommand=x".into()),
                ..reference.clone()
            },
        ] {
            assert!(issue.validate().is_err());
        }
        assert!(serde_json::from_value::<Action>(json!({"action":"secret"})).is_err());
        assert!(
            Action::Respond {
                task_id: "task".into(),
                answer: " ".into()
            }
            .payload()
            .is_err()
        );
        assert_eq!(
            Action::Link {
                task_id: "task".into(),
                issue: None
            }
            .payload()
            .unwrap()["issue"],
            Value::Null
        );
        assert_eq!(
            Action::Link {
                task_id: "task".into(),
                issue: Some(reference)
            }
            .payload()
            .unwrap()["command"],
            "inbox_link"
        );
        let open: Action = serde_json::from_value(json!({
            "action": "open_artifact",
            "project": "named:Atlas",
            "id": "a-1234",
            "host": "devbox",
            "title": "Design Doc",
            "body": "# Design",
            "issue": 12
        }))
        .unwrap();
        let payload = open.payload().unwrap();
        assert_eq!(payload["command"], "artifact_editor");
        assert_eq!(payload["project"], "named:Atlas");
        let parsed: Value = serde_json::from_str(payload["question"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["id"], "a-1234");
        assert_eq!(parsed["host"], "devbox");
        assert_eq!(parsed["issue"], 12);
        assert!(
            serde_json::from_value::<Action>(json!({
                "action": "open_artifact",
                "project": "named:Atlas",
                "host": "-oProxyCommand=evil"
            }))
            .unwrap()
            .payload()
            .is_err()
        );
    }
}
