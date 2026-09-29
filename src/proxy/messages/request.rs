use super::*;
use anyhow::{anyhow, bail, ensure};

pub(super) fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{key} must be a string"))
}
pub(super) fn fields(value: &Value, allowed: &[&str]) -> Result<()> {
    for key in value
        .as_object()
        .ok_or_else(|| anyhow!("Expected an object"))?
        .keys()
    {
        ensure!(
            allowed.contains(&key.as_str()),
            "Unsupported Messages field {key}"
        );
    }
    Ok(())
}
pub(super) fn cache(value: &Value) -> Result<()> {
    if let Some(cache) = value.get("cache_control") {
        fields(cache, &["type", "ttl"])?;
        ensure!(
            cache["type"] == "ephemeral",
            "Only ephemeral cache hints are supported"
        );
        ensure!(
            cache.get("ttl").is_none_or(|v| v == "5m" || v == "1h"),
            "Unsupported cache TTL"
        );
    }
    Ok(())
}
pub(super) fn parts(value: &Value) -> Result<Vec<Value>> {
    if let Some(text) = value.as_str() {
        return Ok(vec![json!({"type":"text","text":text})]);
    }
    Ok(value
        .as_array()
        .ok_or_else(|| anyhow!("content must be text or an array"))?
        .clone())
}
fn content(part: &Value, assistant: bool) -> Result<Value> {
    cache(part)?;
    match string(part, "type")? {
        "text" => {
            fields(part, &["type", "text", "cache_control"])?;
            Ok(json!({"type":"text","text":string(part,"text")?}))
        }
        "image" if !assistant => {
            fields(part, &["type", "source", "cache_control"])?;
            let source = &part["source"];
            let url = match string(source, "type")? {
                "base64" => {
                    fields(source, &["type", "media_type", "data"])?;
                    let media = string(source, "media_type")?;
                    ensure!(
                        ["image/jpeg", "image/png", "image/gif", "image/webp"].contains(&media),
                        "Unsupported image media type"
                    );
                    format!("data:{media};base64,{}", string(source, "data")?)
                }
                "url" => {
                    fields(source, &["type", "url"])?;
                    string(source, "url")?.to_owned()
                }
                _ => bail!("Unsupported image source"),
            };
            Ok(json!({"type":"image_url","image_url":{"url":url}}))
        }
        "document" if !assistant => {
            fields(part, &["type", "source", "title", "cache_control"])?;
            let source = &part["source"];
            fields(source, &["type", "media_type", "data"])?;
            match (string(source, "type")?, string(source, "media_type")?) {
                ("text", "text/plain") => Ok(json!({"type":"text","text":string(source,"data")?})),
                ("base64", "application/pdf") => Ok(json!({"type":"file","file":{
                    "filename":part.get("title").and_then(Value::as_str).unwrap_or("document.pdf"),
                    "file_data":format!("data:application/pdf;base64,{}",string(source,"data")?)}})),
                _ => bail!("Documents require text/plain text or a base64 PDF"),
            }
        }
        kind => bail!("Unsupported Messages content block {kind}"),
    }
}

pub(super) fn convert(input: &Value) -> Result<Value> {
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
        ],
    )?;
    ensure!(
        !string(input, "model")?.is_empty(),
        "model must not be empty"
    );
    ensure!(
        input["max_tokens"].as_u64().is_some_and(|n| n > 0),
        "max_tokens must be a positive integer"
    );
    ensure!(
        input.get("stream").is_none_or(Value::is_boolean),
        "stream must be boolean"
    );
    ensure!(
        input.get("top_k").is_none(),
        "top_k has no Responses equivalent"
    );
    ensure!(
        input
            .get("stop_sequences")
            .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty)),
        "stop_sequences have no Responses equivalent"
    );
    cache(input)?;
    let mut messages = Vec::new();
    if let Some(system) = input.get("system") {
        let mut text = Vec::new();
        for part in parts(system)? {
            ensure!(part["type"] == "text", "system must contain only text");
            text.push(content(&part, false)?);
        }
        messages.push(json!({"role":"system","content":text}));
    }
    for message in input["messages"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("messages must be a nonempty array"))?
    {
        fields(message, &["role", "content"])?;
        let role = string(message, "role")?;
        ensure!(
            ["user", "assistant"].contains(&role),
            "Messages roles must be user or assistant; use top-level system"
        );
        let assistant = role == "assistant";
        let mut visible = Vec::new();
        let mut calls = Vec::new();
        let mut details = Vec::new();
        for part in parts(&message["content"])? {
            cache(&part)?;
            match string(&part, "type")? {
                "tool_use" if assistant => {
                    fields(&part, &["type", "id", "name", "input", "cache_control"])?;
                    ensure!(
                        part["input"].is_object(),
                        "tool_use.input must be an object"
                    );
                    calls.push(json!({"id":string(&part,"id")?,"type":"function","function":{"name":string(&part,"name")?,"arguments":part["input"].to_string()}}));
                }
                "tool_result" if !assistant => {
                    fields(
                        &part,
                        &[
                            "type",
                            "tool_use_id",
                            "content",
                            "is_error",
                            "cache_control",
                        ],
                    )?;
                    ensure!(
                        part.get("is_error").is_none_or(Value::is_boolean),
                        "is_error must be boolean"
                    );
                    if !visible.is_empty() {
                        messages.push(json!({"role":role,"content":std::mem::take(&mut visible)}));
                    }
                    let mut result = Vec::new();
                    if part["is_error"] == true {
                        result.push(json!({"type":"text","text":"Tool returned an error:"}));
                    }
                    if let Some(value) = part.get("content") {
                        for p in parts(value)? {
                            result.push(content(&p, false)?);
                        }
                    }
                    messages.push(json!({"role":"tool","tool_call_id":string(&part,"tool_use_id")?,"content":result}));
                }
                "thinking" if assistant => {
                    fields(&part, &["type", "thinking", "signature"])?;
                    let recovered = response::decode_details(string(&part, "signature")?)?;
                    ensure!(
                        response::reasoning_text(&recovered) == string(&part, "thinking")?,
                        "Altered thinking block; preserve its original text and signature"
                    );
                    details.extend(recovered);
                }
                "redacted_thinking" if assistant => {
                    fields(&part, &["type", "data"])?;
                    details.extend(response::decode_details(string(&part, "data")?)?);
                }
                _ => visible.push(content(&part, assistant)?),
            }
        }
        if assistant || !visible.is_empty() {
            let mut output = json!({"role":role,"content":visible});
            if !calls.is_empty() {
                output["tool_calls"] = json!(calls);
            }
            if !details.is_empty() {
                output["reasoning_details"] = json!(details);
            }
            messages.push(output);
        }
    }
    let mut output = json!({"model":input["model"],"messages":messages,"max_completion_tokens":input["max_tokens"],"stream":input.get("stream").unwrap_or(&Value::Bool(false)),"stream_options":{"include_usage":true}});
    for key in ["temperature", "top_p"] {
        if let Some(value) = input.get(key) {
            let n = value
                .as_f64()
                .ok_or_else(|| anyhow!("{key} must be a number"))?;
            ensure!((0.0..=1.0).contains(&n), "{key} must be between 0 and 1");
            output[key] = value.clone();
        }
    }
    if let Some(metadata) = input.get("metadata") {
        fields(metadata, &["user_id"])?;
        if let Some(id) = metadata.get("user_id") {
            ensure!(id.is_string(), "metadata.user_id must be a string");
            output["metadata"] = metadata.clone();
        }
    }
    if let Some(tier) = input.get("service_tier") {
        ensure!(
            tier == "auto" || tier == "standard_only",
            "Messages service_tier must be auto or standard_only"
        );
        output["service_tier"] = json!(if tier == "standard_only" {
            "default"
        } else {
            "auto"
        });
    }
    if let Some(thinking) = input.get("thinking") {
        fields(thinking, &["type", "budget_tokens"])?;
        let effort = match string(thinking, "type")? {
            "disabled" => "none",
            "adaptive" => "medium",
            "enabled" => {
                let budget = thinking["budget_tokens"]
                    .as_u64()
                    .filter(|n| *n >= 1024 && *n < input["max_tokens"].as_u64().unwrap())
                    .ok_or_else(|| {
                        anyhow!("thinking.budget_tokens must be >=1024 and below max_tokens")
                    })?;
                if budget <= 2048 {
                    "low"
                } else if budget <= 8192 {
                    "medium"
                } else {
                    "high"
                }
            }
            _ => bail!("Unsupported thinking type"),
        };
        output["reasoning"] = json!({"effort":effort});
    }
    if let Some(config) = input.get("output_config") {
        fields(config, &["effort", "format"])?;
        if let Some(effort) = config.get("effort") {
            ensure!(
                input.pointer("/thinking/type") != Some(&json!("disabled")),
                "output_config.effort conflicts with disabled thinking"
            );
            ensure!(
                ["low", "medium", "high", "max"]
                    .iter()
                    .any(|v| effort == *v),
                "Unsupported output effort"
            );
            output["reasoning"] =
                json!({"effort":if effort == "max" {json!("xhigh")} else {effort.clone()}});
        }
        if let Some(format) = config.get("format") {
            fields(format, &["type", "schema"])?;
            ensure!(
                format["type"] == "json_schema" && format["schema"].is_object(),
                "output_config.format requires json_schema and an object schema"
            );
            output["response_format"] = json!({"type":"json_schema","json_schema":{"name":"response","strict":true,"schema":format["schema"]}});
        }
    }
    if let Some(tools) = input.get("tools") {
        let mut converted = Vec::new();
        for tool in tools
            .as_array()
            .ok_or_else(|| anyhow!("tools must be an array"))?
        {
            fields(
                tool,
                &[
                    "name",
                    "description",
                    "input_schema",
                    "type",
                    "cache_control",
                ],
            )?;
            cache(tool)?;
            ensure!(
                tool.get("type").is_none_or(|v| v == "custom"),
                "Only client-defined tools are supported"
            );
            ensure!(
                tool["input_schema"].is_object(),
                "input_schema must be an object"
            );
            let mut function = json!({"name":string(tool,"name")?,"parameters":tool["input_schema"],"strict":false});
            if let Some(description) = tool.get("description") {
                ensure!(description.is_string(), "tool description must be text");
                function["description"] = description.clone();
            }
            converted.push(json!({"type":"function","function":function}));
        }
        output["tools"] = json!(converted);
    }
    if let Some(choice) = input.get("tool_choice") {
        fields(choice, &["type", "name", "disable_parallel_tool_use"])?;
        output["tool_choice"] = match string(choice, "type")? {
            "auto" => json!("auto"),
            "any" => json!("required"),
            "none" => json!("none"),
            "tool" => json!({"type":"function","function":{"name":string(choice,"name")?}}),
            _ => bail!("Unsupported tool_choice type"),
        };
        if let Some(disabled) = choice.get("disable_parallel_tool_use") {
            ensure!(
                disabled.is_boolean(),
                "disable_parallel_tool_use must be boolean"
            );
            output["parallel_tool_calls"] = json!(disabled != true);
        }
    }
    Ok(output)
}
