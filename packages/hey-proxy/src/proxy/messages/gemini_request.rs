//! Messages -> native GenerateContent. No Responses/Chat intermediate format.
use super::*;
use anyhow::{anyhow, bail, ensure};
use hey_proxy::gemini::{ProviderConfig, ReasoningCodec, Thinking, native_tool_name};
use request::{cache, fields, parts, string};
use std::collections::BTreeMap;

pub(super) struct NativeRequest {
    pub model: String,
    pub body: Value,
    pub tools: BTreeMap<String, String>,
    pub stream: bool,
    pub parallel: bool,
    pub display_thinking: bool,
}
fn media(part: &Value) -> Result<Value> {
    cache(part)?;
    match string(part, "type")? {
        "text" => {
            fields(part, &["type", "text", "cache_control"])?;
            Ok(json!({"text":string(part,"text")?}))
        }
        "image" | "document" => {
            fields(part, &["type", "source", "title", "cache_control"])?;
            let source = &part["source"];
            fields(source, &["type", "media_type", "data"])?;
            let mime = string(source, "media_type")?;
            if source["type"] == "text" && mime == "text/plain" {
                return Ok(json!({"text":string(source,"data")?}));
            }
            ensure!(
                source["type"] == "base64",
                "Native Gemini media requires a base64 source"
            );
            ensure!(
                [
                    "image/jpeg",
                    "image/png",
                    "image/webp",
                    "image/gif",
                    "application/pdf"
                ]
                .contains(&mime),
                "Unsupported media type"
            );
            Ok(json!({"inlineData":{"mimeType":mime,"data":string(source,"data")?}}))
        }
        kind => bail!("Unsupported Messages block {kind}"),
    }
}
fn push(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    for part in parts {
        // Gemini cannot mix function responses with ordinary user parts in one
        // Content. Claude appends text/system reminders after tool results,
        // especially after compaction; retain a separate Content boundary.
        let response = part.get("functionResponse").is_some();
        if let Some(last) = contents.last_mut().filter(|c| {
            c["role"] == role
                && (role != "user"
                    || c["parts"]
                        .as_array()
                        .unwrap()
                        .last()
                        .is_some_and(|p| p.get("functionResponse").is_some() == response))
        }) {
            last["parts"].as_array_mut().unwrap().push(part);
        } else {
            contents.push(json!({"role":role,"parts":[part]}));
        }
    }
}
pub(super) fn visible(content: &[Value]) -> Vec<Value> {
    content
        .iter()
        .filter(|b| b["type"] != "redacted_thinking")
        .map(|b| {
            let mut b = b.clone();
            if let Some(object) = b.as_object_mut() {
                object.remove("cache_control");
            }
            if b["type"] == "thinking" {
                b["signature"] = json!("");
            }
            b
        })
        .collect()
}
pub(super) fn convert(
    input: &Value,
    model: &str,
    effort: Option<&str>,
    config: &ProviderConfig,
    codec: &ReasoningCodec,
    count: bool,
) -> Result<NativeRequest> {
    fields(
        input,
        &[
            "model",
            "messages",
            "system",
            "max_tokens",
            "stream",
            "temperature",
            "top_p",
            "top_k",
            "tools",
            "tool_choice",
            "stop_sequences",
            "metadata",
            "thinking",
            "output_config",
            "service_tier",
            "cache_control",
            "context_management",
        ],
    )?;
    let model = model
        .strip_prefix("gemini/")
        .unwrap_or(model)
        .strip_prefix("models/")
        .unwrap_or(model.strip_prefix("gemini/").unwrap_or(model))
        .to_owned();
    config.endpoint(&model, false)?;
    let max = if count {
        input["max_tokens"].as_u64().unwrap_or(1024)
    } else {
        input["max_tokens"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| anyhow!("max_tokens must be a positive integer"))?
    };
    ensure!(
        input.get("stream").is_none_or(Value::is_boolean),
        "stream must be boolean"
    );
    cache(input)?;
    if let Some(context) = input.get("context_management") {
        fields(context, &["edits"])?;
        let edits = context["edits"]
            .as_array()
            .ok_or_else(|| anyhow!("context_management.edits must be an array"))?;
        for edit in edits {
            fields(edit, &["type", "keep"])?;
            ensure!(
                edit["type"] == "clear_thinking_20251015" && edit["keep"] == "all",
                "Only clear_thinking with keep=all is supported; use client-side compaction"
            );
        }
    }

    let mut contents = Vec::new();
    let mut late_system = Vec::new();
    let mut history_calls = BTreeMap::<String, (String, Option<String>)>::new();
    let mut tools = BTreeMap::new();
    let mut declarations = Vec::new();
    for tool in input
        .get("tools")
        .map(|v| {
            v.as_array()
                .ok_or_else(|| anyhow!("tools must be an array"))
        })
        .transpose()?
        .into_iter()
        .flatten()
    {
        fields(
            tool,
            &[
                "name",
                "description",
                "input_schema",
                "type",
                "cache_control",
                "defer_loading",
                "strict",
            ],
        )?;
        cache(tool)?;
        ensure!(
            tool.get("type").is_none_or(|v| v == "custom"),
            "Only client-defined tools are supported"
        );
        ensure!(
            tool["input_schema"].is_object(),
            "Tool input_schema must be an object"
        );
        let name = string(tool, "name")?;
        let native = native_tool_name(name);
        ensure!(
            tools.insert(native.clone(), name.to_owned()).is_none(),
            "Duplicate tool name"
        );
        let mut declaration = json!({"name":native,"parametersJsonSchema":tool["input_schema"]});
        if let Some(description) = tool.get("description") {
            ensure!(description.is_string(), "Tool description must be text");
            declaration["description"] = description.clone();
        }
        declarations.push(declaration);
    }
    for message in input["messages"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("messages must be a nonempty array"))?
    {
        fields(message, &["role", "content"])?;
        let role = string(message, "role")?;
        ensure!(
            role == "user" || role == "assistant" || role == "system",
            "Messages role must be user or assistant"
        );
        let blocks = parts(&message["content"])?;
        if role == "system" {
            let marker = format!("hey-messages-instruction-{}", contents.len());
            late_system.push(json!({"text":format!("System instruction update, effective at conversation marker {marker} and for subsequent turns:")}));
            for block in blocks {
                ensure!(block["type"] == "text", "System messages require text");
                late_system.push(media(&block)?);
            }
            late_system.push(json!({"text":"End system instruction update."}));
            push(
                &mut contents,
                "user",
                vec![json!({"text":format!("[System instruction update marker: {marker}]")})],
            );
            continue;
        }

        if role == "assistant" {
            let carriers: Vec<_> = blocks
                .iter()
                .filter_map(|b| match b["type"].as_str() {
                    Some("thinking") => b["signature"].as_str().filter(|s| !s.is_empty()),
                    Some("redacted_thinking") => b["data"].as_str(),
                    _ => None,
                })
                .collect();
            if let Some(carrier) = carriers.first() {
                ensure!(
                    carriers.iter().all(|c| c == carrier),
                    "Conflicting Gemini turn carriers"
                );
                let replay = codec.open_messages(&model, carrier)?;
                ensure!(
                    replay["items"] == json!(visible(&blocks)),
                    "Altered Gemini assistant content; preserve original thinking/signature/tool blocks"
                );
                let native = replay["parts"]
                    .as_array()
                    .ok_or_else(|| anyhow!("Invalid native replay"))?
                    .clone();
                let calls: Vec<_> = native
                    .iter()
                    .filter_map(|p| p.get("functionCall"))
                    .collect();
                let visible_calls: Vec<_> =
                    blocks.iter().filter(|p| p["type"] == "tool_use").collect();
                ensure!(
                    calls.len() == visible_calls.len(),
                    "Invalid replay tool count"
                );
                for (call, block) in calls.iter().zip(visible_calls) {
                    history_calls.insert(
                        string(block, "id")?.into(),
                        (
                            string(call, "name")?.into(),
                            call["id"].as_str().map(str::to_owned),
                        ),
                    );
                }
                // Signed model turns retain their exact parts/boundaries.
                contents.push(json!({"role":"model","parts":native}));
                continue;
            }
        }
        let mut native = Vec::new();
        for block in blocks {
            cache(&block)?;
            match string(&block, "type")? {
                "tool_use" if role == "assistant" => {
                    fields(&block, &["type", "id", "name", "input", "cache_control"])?;
                    ensure!(block["input"].is_object(), "Tool input must be an object");
                    let name = native_tool_name(string(&block, "name")?);
                    let id = string(&block, "id")?;
                    ensure!(
                        history_calls
                            .insert(id.into(), (name.clone(), Some(id.into())))
                            .is_none(),
                        "Duplicate tool use id"
                    );
                    native.push(
                        json!({"functionCall":{"id":id,"name":name,"args":block["input"]},
                        "thoughtSignature":hey_proxy::gemini::IMPORTED_THOUGHT_SIGNATURE}),
                    );
                }
                "tool_result" if role == "user" => {
                    fields(
                        &block,
                        &[
                            "type",
                            "tool_use_id",
                            "content",
                            "is_error",
                            "cache_control",
                        ],
                    )?;
                    ensure!(
                        block.get("is_error").is_none_or(Value::is_boolean),
                        "is_error must be boolean"
                    );
                    let (name, native_id) = history_calls
                        .get(string(&block, "tool_use_id")?)
                        .ok_or_else(|| anyhow!("tool_result references an unknown tool_use"))?;
                    let mut result = Vec::new();
                    let mut images = Vec::new();
                    if let Some(content) = block.get("content") {
                        for b in parts(content)? {
                            let p = media(&b)?;
                            if let Some(text) = p["text"].as_str() {
                                result.push(text.to_owned());
                            } else {
                                images.push(p);
                            }
                        }
                    }
                    let mut response = json!({"name":name,"response":{(if block["is_error"]==true {"error"} else {"result"}):result.join("\n")}});
                    if let Some(id) = native_id {
                        response["id"] = json!(id);
                    }
                    if !images.is_empty() {
                        response["parts"] = json!(images);
                    }
                    native.push(json!({"functionResponse":response}));
                }
                "thinking" | "redacted_thinking" => {
                    bail!("Thinking replay requires a Gemini Messages signature from this proxy")
                }
                _ => native.push(media(&block)?),
            }
        }
        push(
            &mut contents,
            if role == "assistant" { "model" } else { "user" },
            native,
        );
    }
    ensure!(!contents.is_empty(), "messages cannot be empty");
    if contents.last().is_some_and(|c| c["role"] == "model") {
        push(&mut contents, "user", vec![json!({"text":"Continue."})]);
    }
    let mut body = json!({"contents":contents,"generationConfig":{"maxOutputTokens":max}});
    let mut system = Vec::new();
    if let Some(value) = input.get("system") {
        for b in parts(value)? {
            ensure!(b["type"] == "text", "system must contain text");
            system.push(media(&b)?);
        }
    }
    system.extend(late_system);
    if !declarations.is_empty() {
        body["tools"] = json!([{"functionDeclarations":declarations}]);
    }
    let mut mode = "AUTO";
    let mut allowed = Vec::new();
    let mut parallel = true;
    if let Some(choice) = input.get("tool_choice") {
        fields(choice, &["type", "name", "disable_parallel_tool_use"])?;
        mode = match string(choice, "type")? {
            "auto" => "AUTO",
            "any" => "ANY",
            "none" => "NONE",
            "tool" => {
                let name = native_tool_name(string(choice, "name")?);
                ensure!(
                    tools.contains_key(&name),
                    "tool_choice references undeclared tool"
                );
                allowed.push(name);
                "ANY"
            }
            _ => bail!("Unsupported tool_choice"),
        };
        if let Some(v) = choice.get("disable_parallel_tool_use") {
            ensure!(v.is_boolean(), "disable_parallel_tool_use must be boolean");
            parallel = v != true;
        }
    }
    if tools.is_empty() {
        mode = "NONE";
    }
    body["toolConfig"] = json!({"functionCallingConfig":{"mode":mode}});
    if !allowed.is_empty() {
        body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"] = json!(allowed);
    }
    if mode == "NONE" {
        system.push(json!({"text":"For this response, tool calling is disabled. Answer using text only; do not emit function calls. Historical tools are not available for this turn."}));
        tools.clear();
    } else if !parallel {
        system.push(json!({"text":"Call at most one tool in this response."}));
    }
    if !system.is_empty() {
        body["systemInstruction"] = json!({"parts":system});
    }
    for (from, to) in [
        ("temperature", "temperature"),
        ("top_p", "topP"),
        ("top_k", "topK"),
        ("stop_sequences", "stopSequences"),
    ] {
        if let Some(v) = input.get(from) {
            match from {
                "stop_sequences" => ensure!(
                    v.as_array().is_some_and(Vec::is_empty),
                    "Nonempty stop_sequences are unsupported: Gemini does not identify the matched sequence"
                ),
                "top_k" => ensure!(v.as_u64().is_some_and(|n| n > 0), "top_k must be positive"),
                _ => ensure!(
                    v.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n)),
                    "Invalid sampling parameter"
                ),
            }
            body["generationConfig"][to] = v.clone();
        }
    }
    let mut budget = None;
    if let Some(thinking) = input.get("thinking") {
        fields(thinking, &["type", "budget_tokens", "display"])?;
        ensure!(
            thinking
                .get("display")
                .is_none_or(|v| v == "omitted" || v == "summarized"),
            "Unsupported thinking display"
        );
        budget = Some(match string(thinking, "type")? {
            "disabled" => 0,
            "adaptive" => -1,
            "enabled" => thinking["budget_tokens"]
                .as_i64()
                .filter(|n| *n >= 1024 && *n < (max as i64))
                .ok_or_else(|| anyhow!("thinking budget must be >=1024 and below max_tokens"))?,
            _ => bail!("Unsupported thinking type"),
        });
    }
    let output_config = input.get("output_config");
    if let Some(c) = output_config {
        fields(c, &["effort", "format"])?;
    }
    let effort = effort.or_else(|| output_config.and_then(|c| c["effort"].as_str()));
    let level = matches!(config.thinking, Thinking::Level)
        || (matches!(config.thinking, Thinking::Auto) && model.starts_with("gemini-3"));
    if let Some(effort) = effort {
        budget = Some(match effort {
            "none" => 0,
            "minimal" => 128,
            "low" => 1024,
            "medium" => 8192,
            "high" => 24576,
            "xhigh" | "max" => -1,
            _ => bail!("Unsupported thinking effort"),
        });
    }
    if let Some(budget) = budget {
        let mut thinking = json!({"includeThoughts":budget!=0 && input.pointer("/thinking/display") != Some(&json!("omitted"))});
        if level {
            thinking["thinkingLevel"] = json!(match effort {
                Some("none" | "minimal") => "minimal",
                Some("low") => "low",
                Some("medium") => "medium",
                _ =>
                    if budget == 0 {
                        "minimal"
                    } else {
                        "high"
                    },
            });
        } else {
            thinking["thinkingBudget"] = json!(if budget > 0 {
                budget.min(max.saturating_sub(1) as i64)
            } else {
                budget
            });
        }
        body["generationConfig"]["thinkingConfig"] = thinking;
    }
    if let Some(format) = output_config.and_then(|c| c.get("format")) {
        fields(format, &["type", "schema"])?;
        ensure!(
            format["type"] == "json_schema" && format["schema"].is_object(),
            "output format requires a JSON schema"
        );
        body["generationConfig"]["responseMimeType"] = json!("application/json");
        body["generationConfig"]["responseJsonSchema"] = format["schema"].clone();
    }
    if let Some(tier) = input.get("service_tier") {
        ensure!(
            tier == "auto" || tier == "standard_only",
            "Unsupported Gemini service tier"
        );
    }
    Ok(NativeRequest {
        model,
        body,
        tools,
        parallel,
        display_thinking: input.pointer("/thinking/display") != Some(&json!("omitted")),
        stream: input["stream"] == true,
    })
}
