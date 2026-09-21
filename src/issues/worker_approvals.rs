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
    link: Option<String>,
}

fn prompt(method: &str, params: &Value) -> Result<Option<Prompt>> {
    if method == "mcpServer/elicitation/request" {
        // Form elicitations may request credentials; never collect them in Inbox.
        if params["mode"] != "url" {
            return Ok(None);
        }
        let url = params["url"]
            .as_str()
            .and_then(|s| reqwest::Url::parse(s).ok())
            .filter(|u| {
                u.scheme() == "https"
                    && u.host_str().is_some()
                    && u.username().is_empty()
                    && u.password().is_none()
            })
            .ok_or_else(|| {
                Error::invalid("MCP sign-in requires an HTTPS URL without embedded credentials")
            })?;
        let server = params["serverName"].as_str().unwrap_or("MCP server");
        let message = params["message"].as_str().unwrap_or("Sign in to continue.");
        return Ok(Some(Prompt {
            question: format!("{server} needs you to sign in"),
            description: format!(
                "Open the sign-in page on {} and finish there. Return here and choose ‘I've finished signing in’ to continue the same Codex session. Opening the link does not approve or answer this request. Never enter credentials in Hey Boss.\n\n**Server**\n\n{}\n**Server message**\n\n{}",
                url.host_str().unwrap(),
                code(server),
                code(message)
            ),
            link: Some(url.into()),
            choices: vec![
                (
                    "I've finished signing in".into(),
                    json!({"action":"accept","content":null}),
                ),
                ("Decline".into(), json!({"action":"decline","content":null})),
                ("Cancel".into(), json!({"action":"cancel","content":null})),
            ],
        }));
    }
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
        link: None,
    }))
}

fn input_prompts(method: &str, params: &Value) -> Result<Option<Vec<Prompt>>> {
    if !matches!(
        method,
        "tool/requestUserInput" | "item/tool/requestUserInput"
    ) {
        return prompt(method, params).map(|p| p.map(|p| vec![p]));
    }
    let questions = params["questions"]
        .as_array()
        .filter(|q| !q.is_empty() && q.len() <= 20)
        .ok_or_else(|| Error::invalid("Codex input requires 1–20 choice questions"))?;
    let mut ids = std::collections::HashSet::new();
    let mut prompts = vec![];
    for question in questions {
        let id = question["id"]
            .as_str()
            .filter(|s| !s.is_empty() && ids.insert(*s))
            .ok_or_else(|| Error::invalid("Codex question IDs must be nonempty and unique"))?;
        if question["isSecret"] == true {
            return Err(Error::invalid(
                "Secret input must not be collected in Inbox",
            ));
        }
        let options = question["options"]
            .as_array()
            .filter(|o| !o.is_empty() && o.len() <= 20)
            .ok_or_else(|| {
                Error::invalid(
                    "Codex input requires advertised choices; free text is not supported",
                )
            })?;
        let mut labels = std::collections::HashSet::new();
        let mut choices = vec![];
        let mut description = String::from(
            "Choose one of the requested options. Your answer applies only to this question in the current Codex session.\n\n",
        );
        for option in options {
            let label = option["label"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 512 && labels.insert(*s))
                .ok_or_else(|| Error::invalid("Codex choice labels must be nonempty and unique"))?;
            description.push_str(&code(&format!(
                "{label}: {}",
                option["description"].as_str().unwrap_or("")
            )));
            choices.push((label.into(), json!({"answers":{id:{"answers":[label]}}})));
        }
        prompts.push(Prompt {
            question: question["question"]
                .as_str()
                .unwrap_or("Choose an option")
                .into(),
            description,
            choices,
            link: None,
        });
    }
    Ok(Some(prompts))
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
    questions: Vec<Question>,
    response: Value,
}

struct Question {
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
        let Some(prompts) =
            input_prompts(value["method"].as_str().unwrap_or(""), &value["params"])?
        else {
            return Ok(false);
        };
        if self
            .pending
            .iter()
            .map(|p| p.questions.len())
            .sum::<usize>()
            + prompts.len()
            > 64
            || self.pending.iter().any(|p| p.id == value["id"])
        {
            return Err(Error::invalid(
                "Too many or duplicate Codex approval requests",
            ));
        }
        let mut pending = Pending {
            id: value["id"].clone(),
            item: value["params"]["itemId"].clone(),
            turn: value["params"]["turnId"].clone(),
            questions: vec![],
            response: json!({}),
        };
        // Pending owns delivered questions so partial delivery is cancelled on error.
        for mut p in prompts {
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
            request.title = Some(format!(
                "#{} · Codex {}",
                job.number(),
                if p.link.is_some() {
                    "sign-in"
                } else {
                    "approval"
                }
            ));
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
            request.link_url = p.link;
            request.link_label = request
                .link_url
                .as_ref()
                .map(|_| "Open sign-in page".into());
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
            pending.questions.push(Question {
                task: reply.task_id,
                choices: p.choices,
                reading: None,
            });
        }
        self.pending.push(pending);
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
            let mut question_index = 0;
            let mut cancelled = false;
            while question_index < p.questions.len() {
                let q = &mut p.questions[question_index];
                if q.reading.is_none() && due {
                    q.reading = Some(Reading::start(&client, &q.task)?);
                }
                let Some(reply) = q
                    .reading
                    .as_mut()
                    .map(Reading::receive)
                    .transpose()?
                    .flatten()
                else {
                    question_index += 1;
                    continue;
                };
                q.reading = None;
                if reply.task_id != q.task {
                    return Err(Error::invalid("Inbox returned a different approval task"));
                }
                if reply.status.as_deref() == Some("pending") {
                    question_index += 1;
                    continue;
                }
                let answer = if reply.status.as_deref() == Some("ok") {
                    reply.result.as_deref()
                } else {
                    None
                };
                let response = answer
                    .and_then(|answer| q.choices.iter().find(|(label, _)| label == answer))
                    .map(|(_, value)| value.clone());
                // Free text, dismissal and missing results never grant authority.
                cancelled = response.is_none() || answer == Some("Cancel");
                let response = response.unwrap_or_else(|| {
                    let shape = &q.choices[0].1;
                    if shape.get("permissions").is_some() {
                        json!({"permissions":{},"scope":"turn"})
                    } else if shape.get("answers").is_some() {
                        json!({"answers":{}})
                    } else if shape.get("action").is_some() {
                        json!({"action":"cancel","content":null})
                    } else {
                        json!({"decision":"cancel"})
                    }
                });
                if cancelled {
                    p.response = response;
                    break;
                }
                if let Some(answers) = response["answers"].as_object() {
                    if p.response.get("answers").is_none() {
                        p.response = json!({"answers":{}});
                    }
                    p.response["answers"]
                        .as_object_mut()
                        .unwrap()
                        .extend(answers.clone());
                } else {
                    p.response = response;
                }
                p.questions.remove(question_index);
            }
            if cancelled || p.questions.is_empty() {
                responses.push((json!({"id":p.id,"result":p.response}), cancelled));
                self.pending.remove(index);
            } else {
                index += 1;
            }
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
impl Drop for Pending {
    fn drop(&mut self) {
        for question in &self.questions {
            dismiss(&question.task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_sign_in_has_an_explicit_completion_and_safe_external_link() {
        let p = prompt("mcpServer/elicitation/request", &json!({"mode":"url","serverName":"Okta","message":"Sign in to continue.","url":"https://login.example.invalid/authorize?state=synthetic","elicitationId":"login-1"})).unwrap().unwrap();
        assert_eq!(
            p.link.as_deref(),
            Some("https://login.example.invalid/authorize?state=synthetic")
        );
        assert!(p.description.contains("login.example.invalid"));
        assert!(p.description.contains("Okta"));
        assert_eq!(
            p.choices[0],
            (
                "I've finished signing in".into(),
                json!({"action":"accept","content":null})
            )
        );
        assert_eq!(p.choices[1].1, json!({"action":"decline","content":null}));
        for url in [
            "javascript:alert(1)",
            "http://login.example.invalid",
            "https://user:password@login.example.invalid",
            "https:///",
            "file:///tmp/login",
        ] {
            assert!(
                prompt(
                    "mcpServer/elicitation/request",
                    &json!({"mode":"url","url":url,"message":"Sign in"})
                )
                .is_err(),
                "{url}"
            );
        }
        assert!(
            prompt(
                "mcpServer/elicitation/request",
                &json!({"mode":"form","requestedSchema":{"type":"object"}})
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn connector_choices_map_to_exact_question_ids_without_free_text_or_secrets() {
        for method in ["tool/requestUserInput", "item/tool/requestUserInput"] {
            let p = input_prompts(method, &json!({"questions":[{"id":"approval","header":"Okta","question":"Allow the requested action?","options":[{"label":"Accept","description":"Run once"},{"label":"Decline","description":"Do not run"}]}]})).unwrap().unwrap();
            assert_eq!(p.len(), 1);
            assert_eq!(
                p[0].choices[0].1,
                json!({"answers":{"approval":{"answers":["Accept"]}}})
            );
            assert!(p[0].description.contains("Run once"));
            assert!(input_prompts(method, &json!({"questions":[{"id":"secret","isSecret":true,"options":[{"label":"Accept"}]}]})).is_err());
            assert!(
                input_prompts(
                    method,
                    &json!({"questions":[{"id":"free","question":"Password?","options":null}]})
                )
                .is_err()
            );
            assert!(input_prompts(method, &json!({"questions":[{"id":"same","options":[{"label":"Accept"}]},{"id":"same","options":[{"label":"Decline"}]}]})).is_err());
        }
    }

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
