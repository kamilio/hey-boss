//! Explicit, request-scoped Codex approvals over the existing desktop/SSH Inbox.
use super::{Error, Result};
use crate::{Client, Request};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use std::{io::Read, os::unix::net::UnixStream};

struct Prompt {
    question: String,
    description: String,
    choices: Vec<(String, Value)>,
}

fn prompt(method: &str, params: &Value) -> Result<Option<Prompt>> {
    let (question, choices) = match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            let question = if method == "item/fileChange/requestApproval" {
                "Allow these Codex file changes?"
            } else if !params["networkApprovalContext"].is_null() {
                "Allow this Codex network access?"
            } else if params["kind"] == "writeStdin" {
                "Allow Codex to send this process input?"
            } else {
                "Allow this Codex command?"
            };
            let available = params.get("availableDecisions").filter(|v| !v.is_null());
            if available.is_some_and(|v| !v.is_array()) {
                return Err(Error::invalid("Invalid Codex approval decisions"));
            }
            let choices = [
                ("Approve once", "accept"),
                ("Decline", "decline"),
                ("Cancel", "cancel"),
            ]
            .into_iter()
            .filter(|(_, decision)| {
                available.is_none_or(|v| v.as_array().unwrap().contains(&json!(decision)))
            })
            .map(|(label, decision)| (label.into(), json!({"decision":decision})))
            .collect::<Vec<_>>();
            if choices.is_empty() {
                return Err(Error::invalid(
                    "Codex offered no supported request-scoped decisions",
                ));
            }
            (question, choices)
        }
        "item/permissions/requestApproval" => {
            let permissions = params
                .get("permissions")
                .filter(|v| v.is_object())
                .ok_or_else(|| Error::invalid("Codex omitted requested permissions"))?;
            (
                "Allow these Codex permissions for this turn?",
                vec![
                    (
                        "Approve for this turn".into(),
                        json!({"permissions":permissions,"scope":"turn"}),
                    ),
                    ("Decline".into(), json!({"permissions":{},"scope":"turn"})),
                    ("Cancel".into(), json!({"permissions":{},"scope":"turn"})),
                ],
            )
        }
        _ => return Ok(None),
    };
    let mut context = String::new();
    for (field, label) in [
        ("reason", "Reason"),
        ("command", "Command"),
        ("cwd", "Working directory"),
        ("grantRoot", "Requested write root"),
    ] {
        if let Some(value) = params[field].as_str() {
            context.push_str(&format!("**{label}**\n\n{}\n", code(value)));
        }
    }
    if let Some(network) = params["networkApprovalContext"].as_object() {
        context.push_str("**Network access**\n\n");
        describe_access(&mut context, "", &Value::Object(network.clone()));
    }
    for field in ["additionalPermissions", "permissions"] {
        if !params[field].is_null() {
            context.push_str("\n**Requested access**\n\n");
            describe_access(&mut context, "", &params[field]);
        }
    }
    let scope = if method == "item/permissions/requestApproval" {
        "Approval grants only the permissions shown below for the current turn."
    } else {
        "Approval applies only to this request."
    };
    Ok(Some(Prompt {
        question: question.into(),
        description: format!(
            "{scope} Codex will continue in the same session after your decision.\n\n{context}"
        ),
        choices,
    }))
}

fn code(value: &str) -> String {
    // Indented text stays code even with Markdown fences or HTML in a command.
    value.lines().map(|line| format!("    {line}\n")).collect()
}
fn words(key: &str) -> String {
    let mut result = String::new();
    for c in key.chars() {
        if c.is_uppercase() {
            result.push(' ');
            result.extend(c.to_lowercase());
        } else if c == '_' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}
fn describe_access(output: &mut String, label: &str, value: &Value) {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                let key = words(key);
                let label = if label.is_empty() {
                    key
                } else {
                    format!("{label} · {key}")
                };
                describe_access(output, &label, value);
            }
        }
        Value::Array(values) => {
            for value in values {
                describe_access(output, label, value);
            }
        }
        Value::Null => {}
        _ => {
            let value = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            output.push_str(&code(&format!("{label}: {value}")));
            output.push('\n');
        }
    }
}

struct Pending {
    id: Value,
    item: Value,
    turn: Value,
    task: String,
    choices: Vec<(String, Value)>,
    reading: Option<Reading>,
}

// Read status incrementally: a slow SSH bridge must not block cancellation
// checks or other Codex callbacks. No additional threads or busy polling.
struct Reading {
    stream: UnixStream,
    bytes: Vec<u8>,
    started: Instant,
}
impl Reading {
    fn start(client: &Client, task: &str) -> Result<Self> {
        let pending = client.try_start(&Request::action("status", Some(task)))?;
        pending.stream.set_nonblocking(true)?;
        Ok(Self {
            stream: pending.stream,
            bytes: vec![],
            started: Instant::now(),
        })
    }
    fn receive(&mut self) -> Result<Option<crate::Response>> {
        let mut buffer = [0u8; 8192];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => return Ok(Some(serde_json::from_slice(&self.bytes)?)),
                Ok(n) => {
                    self.bytes.extend_from_slice(&buffer[..n]);
                    if self.bytes.len() > 8 * 1024 * 1024 {
                        return Err(Error::invalid("Inbox response exceeded 8 MiB"));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if self.started.elapsed() > Duration::from_secs(10) {
                        return Err(Error::new("blocked", "Inbox approval status timed out"));
                    }
                    return Ok(None);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Approvals {
    pending: Vec<Pending>,
    polled: Option<Instant>,
}
impl Approvals {
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn start(
        &mut self,
        value: &Value,
        job: &super::worker::Job,
        preview: Option<&Value>,
    ) -> Result<bool> {
        let Some(mut p) = prompt(value["method"].as_str().unwrap_or(""), &value["params"])? else {
            return Ok(false);
        };
        if self.pending.len() >= 64 || self.pending.iter().any(|p| p.id == value["id"]) {
            return Err(Error::invalid(
                "Too many or duplicate Codex approval requests",
            ));
        }
        if let Some(preview) = preview {
            if preview["previewTooLarge"] == true {
                return Err(Error::invalid(
                    "File diff exceeds 64 KiB; review the saved Codex session",
                ));
            }
            p.description.push_str("\n\n**Requested changes**\n\n");
            if let Some(changes) = preview["changes"].as_array() {
                for change in changes {
                    p.description
                        .push_str(&code(change["path"].as_str().unwrap_or("File change")));
                    p.description.push('\n');
                    p.description.push_str(&code(
                        change["diff"]
                            .as_str()
                            .unwrap_or("No diff supplied by Codex."),
                    ));
                    p.description.push('\n');
                }
            }
        }
        if p.description.len() > 65536 {
            return Err(Error::invalid(
                "Codex approval context exceeds 64 KiB; inspect the saved session",
            ));
        }
        let mut request = Request::action("ask", None);
        request.project = Some(job.project.name.clone());
        request.title = Some(format!("#{} · Codex approval", job.number()));
        request.question = Some(p.question);
        request.description = Some(p.description);
        request.options = Some(p.choices.iter().map(|(label, _)| label.clone()).collect());
        request.issue = Some(crate::notices::IssueReference {
            project: job.project.id.clone(),
            number: job.number(),
            host: None,
        });
        request.severity = Some(crate::Severity::Warning);
        request.icon = Some("code".into());
        let reply = Client::new(crate::notices::socket_path()?).try_send(&request)?;
        // Native asynchronous creation historically returns only the task ID.
        if !matches!(reply.status.as_deref(), None | Some("ok" | "pending"))
            || reply.task_id.is_empty()
        {
            return Err(Error::new(
                "blocked",
                "Codex approval could not be delivered to Inbox; resume the saved session or retry after connecting Hey Boss",
            ));
        }
        self.pending.push(Pending {
            id: value["id"].clone(),
            item: value["params"]["itemId"].clone(),
            turn: value["params"]["turnId"].clone(),
            task: reply.task_id,
            choices: p.choices,
            reading: None,
        });
        Ok(true)
    }

    pub fn poll(&mut self) -> Result<Vec<(Value, bool)>> {
        if self.pending.is_empty() {
            return Ok(vec![]);
        }
        let due = self
            .polled
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(1));
        if due {
            self.polled = Some(Instant::now());
        }
        let client = Client::new(crate::notices::socket_path()?);
        let mut responses = vec![];
        let mut index = 0;
        while index < self.pending.len() {
            let p = &mut self.pending[index];
            if p.reading.is_none() && due {
                p.reading = Some(Reading::start(&client, &p.task)?);
            }
            let Some(reply) = p
                .reading
                .as_mut()
                .map(Reading::receive)
                .transpose()?
                .flatten()
            else {
                index += 1;
                continue;
            };
            p.reading = None;
            if reply.task_id != p.task {
                return Err(Error::invalid("Inbox returned a different approval task"));
            }
            if reply.status.as_deref() == Some("pending") {
                index += 1;
                continue;
            }
            let answer = if reply.status.as_deref() == Some("ok") {
                reply.result.as_deref()
            } else {
                None
            };
            let response = answer
                .and_then(|answer| p.choices.iter().find(|(label, _)| label == answer))
                .map(|(_, value)| value.clone());
            // Free-text, dismissal, missing results and transport errors never approve.
            let cancelled = response.is_none() || answer == Some("Cancel");
            let response = response.unwrap_or_else(|| {
                if p.choices
                    .iter()
                    .any(|(_, v)| v.get("permissions").is_some())
                {
                    json!({"permissions":{},"scope":"turn"})
                } else {
                    json!({"decision":"cancel"})
                }
            });
            responses.push((json!({"id":p.id,"result":response}), cancelled));
            if cancelled {
                dismiss(&p.task);
            }
            self.pending.remove(index);
        }
        Ok(responses)
    }

    pub fn observe(&mut self, value: &Value) {
        let params = &value["params"];
        self.pending.retain(|p| {
            let resolved = match value["method"].as_str() {
                Some("serverRequest/resolved") => params["requestId"] == p.id,
                Some("item/completed") => !p.item.is_null() && params["item"]["id"] == p.item,
                Some("turn/completed") => !p.turn.is_null() && params["turn"]["id"] == p.turn,
                _ => false,
            };
            if resolved {
                dismiss(&p.task);
            }
            !resolved
        });
    }
}
fn dismiss(task: &str) {
    if let Ok(socket) = crate::notices::socket_path()
        && let Ok(pending) = Client::new(socket).try_start(&Request::action("hide", Some(task)))
    {
        let _ = pending
            .stream
            .set_read_timeout(Some(Duration::from_millis(250)));
        let _ = pending.try_wait();
    }
}
impl Drop for Approvals {
    fn drop(&mut self) {
        for p in &self.pending {
            dismiss(&p.task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_show_context_and_grant_only_this_request() {
        let p = prompt("item/commandExecution/requestApproval", &json!({"command":"cargo test", "cwd":"/repo", "reason":"Needs network", "additionalPermissions":{"network":true}})).unwrap().unwrap();
        assert!(p.description.contains("cargo test"));
        assert!(p.description.contains("/repo"));
        assert!(p.description.contains("Needs network"));
        assert!(p.description.contains("network: true"));
        assert_eq!(
            p.choices,
            vec![
                ("Approve once".into(), json!({"decision":"accept"})),
                ("Decline".into(), json!({"decision":"decline"})),
                ("Cancel".into(), json!({"decision":"cancel"}))
            ]
        );
    }

    #[test]
    fn advertised_decisions_do_not_expand_authority() {
        let p = prompt("item/commandExecution/requestApproval", &json!({"availableDecisions":["decline","cancel","acceptForSession",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["sh"]}}]})).unwrap().unwrap();
        assert_eq!(p.choices.len(), 2);
        assert!(p.choices.iter().all(|(_, v)| v["decision"] != "accept"));
        assert!(
            prompt(
                "item/commandExecution/requestApproval",
                &json!({"availableDecisions":[]})
            )
            .is_err()
        );
    }

    #[test]
    fn network_and_file_requests_describe_their_scope() {
        let p = prompt(
            "item/commandExecution/requestApproval",
            &json!({"networkApprovalContext":{"host":"registry.example.com","protocol":"https"}}),
        )
        .unwrap()
        .unwrap();
        assert!(p.question.contains("network"));
        assert!(p.description.contains("registry.example.com"));
        let p = prompt(
            "item/fileChange/requestApproval",
            &json!({"grantRoot":"/repo","reason":"Apply fix"}),
        )
        .unwrap()
        .unwrap();
        assert!(p.question.contains("file"));
        assert!(p.description.contains("/repo"));
    }

    #[test]
    fn permissions_require_an_explicit_turn_scoped_grant() {
        let requested = json!({"network":{"enabled":true},"fileSystem":{"write":["/repo"]}});
        let p = prompt(
            "item/permissions/requestApproval",
            &json!({"permissions":requested}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            p.choices[0].1,
            json!({"permissions":requested,"scope":"turn"})
        );
        assert_eq!(p.choices[1].1, json!({"permissions":{},"scope":"turn"}));
        assert!(prompt("item/permissions/requestApproval", &json!({})).is_err());
        assert!(prompt("unknown/request", &json!({})).unwrap().is_none());
    }

    #[test]
    fn status_reads_are_nonblocking_and_preserve_split_responses() {
        use std::io::Write;
        let (stream, mut server) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        let mut reading = Reading {
            stream,
            bytes: vec![],
            started: Instant::now(),
        };
        assert!(reading.receive().unwrap().is_none());
        server
            .write_all(br#"{"task_id":"notice","status":"ok","result":"Approve"#)
            .unwrap();
        assert!(reading.receive().unwrap().is_none());
        server.write_all(br#" once"}"#).unwrap();
        drop(server);
        let reply = reading.receive().unwrap().unwrap();
        assert_eq!(reply.result.as_deref(), Some("Approve once"));
    }

    #[test]
    fn stale_and_malformed_status_reads_fail_closed() {
        let (stream, _server) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        let mut reading = Reading {
            stream,
            bytes: vec![],
            started: Instant::now() - Duration::from_secs(11),
        };
        assert!(reading.receive().is_err());
        let (stream, server) = UnixStream::pair().unwrap();
        drop(server);
        let mut reading = Reading {
            stream,
            bytes: b"not-json".to_vec(),
            started: Instant::now(),
        };
        assert!(reading.receive().is_err());
    }

    #[test]
    fn command_context_cannot_escape_into_html_or_markdown() {
        let p = prompt(
            "item/commandExecution/requestApproval",
            &json!({"command":"```\n<script>evil()</script>\n[approve](https://evil.example)"}),
        )
        .unwrap()
        .unwrap();
        let html = crate::markdown::render_fragment(&p.description);
        assert!(!html.contains("<script>"));
        assert!(!html.contains("href=\"https://evil.example"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
