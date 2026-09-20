use super::*;

pub(super) fn command(provider: Provider, binary: &Path, resume: Option<&SessionRef>) -> Command {
    let mut command = Command::new(binary);
    match provider {
        Provider::Codex => {
            command.args(["app-server", "--listen", "stdio://"]);
        }
        Provider::Claude => {
            command.args([
                "--print",
                "--verbose",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--include-partial-messages",
                "--permission-prompt-tool",
                "stdio",
            ]);
            if let Some(saved) = resume {
                command.arg("--resume").arg(&saved.id);
            }
        }
        Provider::Pi => {
            command.args(["--mode", "rpc"]);
            if let Some(saved) = resume {
                command.arg("--session").arg(saved.path.as_ref().unwrap());
            }
        }
    }
    command
}
pub(super) fn request(provider: Provider, id: &str, method: &str, mut params: Value) -> Value {
    match provider {
        Provider::Codex => json!({"id":id,"method":method,"params":params}),
        Provider::Claude => {
            params["subtype"] = json!(method);
            json!({"type":"control_request","request_id":id,"request":params})
        }
        Provider::Pi => {
            params["type"] = json!(method);
            params["id"] = json!(id);
            params
        }
    }
}
pub(super) fn response(provider: Provider, id: &str, value: &Value) -> Option<io::Result<Value>> {
    match provider {
        Provider::Codex if value["id"].as_str() == Some(id) && value.get("method").is_none() => {
            Some(if let Some(error) = value.get("error") {
                Err(io::Error::other(format!("Codex rejected request: {error}")))
            } else {
                value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| io::Error::other("Codex response missing result"))
            })
        }
        Provider::Claude
            if value["type"] == "control_response"
                && value["response"]["request_id"].as_str() == Some(id) =>
        {
            Some(if value["response"]["subtype"] == "success" {
                Ok(value["response"]["response"].clone())
            } else {
                Err(io::Error::other(format!(
                    "Claude rejected request: {}",
                    value["response"]["error"]
                )))
            })
        }
        Provider::Pi if value["type"] == "response" && value["id"].as_str() == Some(id) => {
            Some(if value["success"] == true {
                Ok(value["data"].clone())
            } else {
                Err(io::Error::other(format!(
                    "Pi rejected request: {}",
                    value["error"]
                )))
            })
        }
        _ => None,
    }
}
pub(super) fn decision(provider: Provider, value: &Value, allow: bool) -> io::Result<Value> {
    match provider {
        Provider::Codex
            if matches!(
                value["method"].as_str(),
                Some("item/commandExecution/requestApproval" | "item/fileChange/requestApproval")
            ) =>
        {
            // New granular command approval schemas may omit accept. Never
            // manufacture a decision that was not offered by the server.
            if let Some(available) = value["params"]["availableDecisions"].as_array() {
                let expected = if allow { "accept" } else { "decline" };
                if !available.iter().any(|v| v.as_str() == Some(expected)) {
                    return Err(io::Error::other(
                        "The approval request does not offer that decision",
                    ));
                }
            }
            Ok(json!({"id":value["id"],"result":{"decision":if allow {"accept"} else {"decline"}}}))
        }
        Provider::Codex if value["method"] == "item/permissions/requestApproval" => {
            let requested = value["params"]
                .get("permissions")
                .filter(|v| v.is_object())
                .ok_or_else(|| io::Error::other("Codex omitted requested permissions"))?;
            Ok(
                json!({"id":value["id"],"result":{"permissions":if allow {requested.clone()} else {json!({})},"scope":"turn"}}),
            )
        }
        Provider::Claude if value["request"]["subtype"] == "can_use_tool" => {
            let answer = if allow {
                json!({"behavior":"allow","updatedInput":value["request"]["input"]})
            } else {
                json!({"behavior":"deny","message":"Declined by the controlling user"})
            };
            Ok(
                json!({"type":"control_response","response":{"subtype":"success","request_id":value["request_id"],"response":answer}}),
            )
        }
        _ => Err(io::Error::other(
            "This request requires provider-specific input, not a boolean approval",
        )),
    }
}
pub(super) fn input_response(
    provider: Provider,
    value: &Value,
    answer: Option<&str>,
) -> io::Result<Value> {
    if provider != Provider::Pi {
        return Err(io::Error::other(
            "This input request needs a provider-specific response",
        ));
    }
    let mut reply = json!({"type":"extension_ui_response","id":value["id"]});
    match (value["method"].as_str(), answer) {
        (Some("select" | "input" | "editor" | "confirm"), None) => reply["cancelled"] = json!(true),
        (Some("confirm"), Some("true")) => reply["confirmed"] = json!(true),
        (Some("confirm"), Some("false")) => reply["confirmed"] = json!(false),
        (Some("select"), Some(answer))
            if value["options"]
                .as_array()
                .is_some_and(|options| options.iter().any(|v| v.as_str() == Some(answer))) =>
        {
            reply["value"] = json!(answer)
        }
        (Some("input" | "editor"), Some(answer)) if answer.len() <= 32000 => {
            reply["value"] = json!(answer)
        }
        _ => {
            return Err(io::Error::other(
                "Unsupported or invalid extension input response",
            ));
        }
    }
    Ok(reply)
}
fn text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
impl AgentSession {
    pub(super) fn normalize(&mut self, value: Value) -> io::Result<()> {
        if self.events.len() >= 256 {
            self.uncertain = true;
            return Err(io::Error::other(
                "Agent event queue exceeded limit; drain events before issuing more controls",
            ));
        }
        match self.provider {
            Provider::Codex => self.codex(value),
            Provider::Claude => self.claude(value),
            Provider::Pi => self.pi(value),
        }
    }
    fn register(
        &mut self,
        id: String,
        value: Value,
        approval: bool,
        kind: String,
    ) -> io::Result<()> {
        if self.requests.len() >= 64 || self.requests.contains_key(&id) {
            return Err(io::Error::other(
                "Too many or duplicate pending agent requests",
            ));
        }
        self.events.push_back(if approval {
            Event::Approval {
                id: id.clone(),
                kind,
                payload: value.clone(),
            }
        } else {
            Event::Input {
                id: id.clone(),
                payload: value.clone(),
            }
        });
        self.requests.insert(id, value);
        Ok(())
    }
    pub(super) fn finish(
        &mut self,
        status: TurnStatus,
        output: String,
        structured_output: Option<Value>,
        usage: Value,
    ) -> io::Result<()> {
        let Some(id) = self.turn.take() else {
            return Ok(());
        };
        self.requests.clear();
        self.events.push_back(Event::TurnCompleted {
            id,
            status,
            output,
            structured_output,
            usage,
        });
        if let Some((id, text)) = self.queued_turns.pop_front() {
            self.output.clear();
            self.turn = Some(id.clone());
            self.send(&json!({"type":"user","session_id":self.session.as_ref().map(|s|s.id.as_str()).unwrap_or(""),"message":{"role":"user","content":text},"parent_tool_use_id":null}))?;
            self.events.push_back(Event::TurnStarted { id });
        }
        Ok(())
    }
    fn codex(&mut self, value: Value) -> io::Result<()> {
        let params = &value["params"];
        // Controls and approvals from another session must never be mixed in.
        if params["threadId"]
            .as_str()
            .is_some_and(|id| self.session.as_ref().is_none_or(|s| s.id != id))
        {
            return Ok(());
        }
        if params["turnId"]
            .as_str()
            .is_some_and(|id| self.turn.as_deref() != Some(id))
        {
            return Ok(());
        }
        let method = value["method"].as_str().unwrap_or("");
        if value.get("id").is_some() && !method.is_empty() {
            let id = serde_json::to_string(&value["id"])?;
            let approval = matches!(
                method,
                "item/commandExecution/requestApproval"
                    | "item/fileChange/requestApproval"
                    | "item/permissions/requestApproval"
            );
            return self.register(id, value.clone(), approval, method.into());
        }
        match method {
            "turn/started" => {
                let id = required(&params["turn"], "id")?;
                if self.turn.as_deref() != Some(&id) {
                    self.turn = Some(id.clone());
                    self.output.clear();
                    self.events.push_back(Event::TurnStarted { id });
                }
            }
            "item/agentMessage/delta" => {
                self.events.push_back(Event::TextDelta {
                    text: params["delta"].as_str().unwrap_or("").into(),
                });
            }
            "item/started" => {
                let item = &params["item"];
                match item["type"].as_str() {
                    Some("commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall") => {
                        self.events.push_back(Event::ToolStarted {
                            id: item["id"].as_str().unwrap_or("").into(),
                            name: item["type"].as_str().unwrap().into(),
                            input: item.clone(),
                        })
                    }
                    _ => self.events.push_back(Event::Other(value)),
                }
            }
            "item/completed" => {
                let item = &params["item"];
                if item["type"] == "agentMessage" {
                    self.output = item["text"].as_str().unwrap_or("").into();
                    self.events.push_back(Event::Message {
                        role: "assistant".into(),
                        text: self.output.clone(),
                    });
                } else {
                    self.events.push_back(Event::ToolCompleted {
                        id: item["id"].as_str().unwrap_or("").into(),
                        output: item.clone(),
                        failed: item["status"] == "failed",
                    });
                }
            }
            "turn/completed" if params["turn"]["id"].as_str() == self.turn.as_deref() => {
                let status = match params["turn"]["status"].as_str() {
                    Some("completed") => TurnStatus::Completed,
                    Some("interrupted") => TurnStatus::Interrupted,
                    _ => TurnStatus::Failed,
                };
                self.finish(
                    status,
                    self.output.clone(),
                    serde_json::from_str(&self.output).ok(),
                    params["turn"]["usage"].clone(),
                )?;
            }
            "thread/goal/updated" => self.events.push_back(Event::Goal(params["goal"].clone())),
            "serverRequest/resolved" => {
                let id = serde_json::to_string(&params["requestId"])?;
                self.requests.remove(&id);
                self.events.push_back(Event::RequestCancelled { id });
            }
            _ => self.events.push_back(Event::Other(value)),
        }
        Ok(())
    }
    fn claude(&mut self, value: Value) -> io::Result<()> {
        if let Some(id) = value["session_id"].as_str() {
            self.attach(id.into(), None)?;
        }
        match value["type"].as_str() {
            Some("system") => {
                if let Some(id) = value["task_id"].as_str() {
                    if value["subtype"] == "task_started"
                        && matches!(
                            value["task_type"].as_str(),
                            Some("local_agent" | "local_workflow")
                        )
                    {
                        self.tasks.insert(id.into());
                    } else if value["subtype"] == "task_notification"
                        || (value["subtype"] == "task_updated"
                            && matches!(
                                value["patch"]["status"].as_str(),
                                Some("completed" | "failed" | "stopped" | "killed")
                            ))
                    {
                        self.tasks.remove(id);
                    }
                }
                self.events.push_back(Event::Other(value));
            }
            Some("stream_event") => {
                if value["event"]["delta"]["type"] == "text_delta" {
                    self.events.push_back(Event::TextDelta {
                        text: value["event"]["delta"]["text"]
                            .as_str()
                            .unwrap_or("")
                            .into(),
                    });
                } else {
                    self.events.push_back(Event::Other(value));
                }
            }
            Some("assistant") => {
                // Forwarded subagent messages cannot overwrite the parent result.
                if !value["parent_tool_use_id"].is_null() {
                    self.events.push_back(Event::Other(value));
                    return Ok(());
                }
                let content = &value["message"]["content"];
                let message = text(content);
                if !message.is_empty() {
                    self.output = message.clone();
                    self.events.push_back(Event::Message {
                        role: "assistant".into(),
                        text: message,
                    });
                }
                for block in content
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "tool_use")
                {
                    self.events.push_back(Event::ToolStarted {
                        id: block["id"].as_str().unwrap_or("").into(),
                        name: block["name"].as_str().unwrap_or("").into(),
                        input: block["input"].clone(),
                    });
                }
            }
            Some("user") => {
                for block in value["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "tool_result")
                {
                    self.events.push_back(Event::ToolCompleted {
                        id: block["tool_use_id"].as_str().unwrap_or("").into(),
                        output: block["content"].clone(),
                        failed: block["is_error"] == true,
                    });
                }
            }
            Some("control_request") => {
                let kind = value["request"]["subtype"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                self.register(
                    required(&value, "request_id")?,
                    value,
                    kind == "can_use_tool",
                    kind,
                )?;
            }
            Some("control_cancel_request") => {
                let id = required(&value, "request_id")?;
                self.requests.remove(&id);
                self.events.push_back(Event::RequestCancelled { id });
            }
            Some("result") => {
                if !self.tasks.is_empty() {
                    self.events.push_back(Event::Other(value));
                    return Ok(());
                }
                let status = if self.interrupted {
                    TurnStatus::Interrupted
                } else if value["is_error"] == true || value["subtype"] != "success" {
                    TurnStatus::Failed
                } else {
                    TurnStatus::Completed
                };
                let output = value["result"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.output.clone());
                self.finish(
                    status,
                    output,
                    value.get("structured_output").cloned(),
                    value["usage"].clone(),
                )?;
            }
            _ => self.events.push_back(Event::Other(value)),
        }
        Ok(())
    }
    fn pi(&mut self, value: Value) -> io::Result<()> {
        match value["type"].as_str() {
            Some("message_update") if value["assistantMessageEvent"]["type"] == "text_delta" => {
                self.events.push_back(Event::TextDelta {
                    text: value["assistantMessageEvent"]["delta"]
                        .as_str()
                        .unwrap_or("")
                        .into(),
                })
            }
            Some("message_end") if value["message"]["role"] == "assistant" => {
                self.output = text(&value["message"]["content"]);
                self.status = match value["message"]["stopReason"].as_str() {
                    Some("aborted") => TurnStatus::Interrupted,
                    Some("error") => TurnStatus::Failed,
                    _ => TurnStatus::Completed,
                };
                self.events.push_back(Event::Message {
                    role: "assistant".into(),
                    text: self.output.clone(),
                });
            }
            Some("tool_execution_start") => self.events.push_back(Event::ToolStarted {
                id: value["toolCallId"].as_str().unwrap_or("").into(),
                name: value["toolName"].as_str().unwrap_or("").into(),
                input: value["args"].clone(),
            }),
            Some("tool_execution_end") => self.events.push_back(Event::ToolCompleted {
                id: value["toolCallId"].as_str().unwrap_or("").into(),
                output: value["result"].clone(),
                failed: value["isError"] == true,
            }),
            Some("extension_ui_request")
                if matches!(
                    value["method"].as_str(),
                    Some("select" | "confirm" | "input" | "editor")
                ) =>
            {
                self.register(required(&value, "id")?, value, false, String::new())?
            }
            Some("agent_settled") => {
                // agent_end and turn_end can precede automatic retry, compaction,
                // tool loops and queued instructions. Only settled is terminal.
                self.finish(
                    if self.interrupted {
                        TurnStatus::Interrupted
                    } else {
                        self.status.clone()
                    },
                    self.output.clone(),
                    serde_json::from_str(&self.output).ok(),
                    Value::Null,
                )?;
                self.refresh_pi()?;
            }
            _ => self.events.push_back(Event::Other(value)),
        }
        Ok(())
    }
}
