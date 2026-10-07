use super::request::{native_tool_name, readable_tool_prefix};
use super::{ConvertedRequest, ReasoningCodec, Tool};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn resolve_tool<'a>(native: &str, tools: &'a BTreeMap<String, Tool>) -> Option<&'a Tool> {
    if let Some(tool) = tools.get(native) {
        return Some(tool);
    }
    let qualified = |tool: &Tool| {
        if let Some(ns) = &tool.namespace {
            format!("{ns}.{}", tool.name)
        } else {
            tool.name.clone()
        }
    };
    for tool in tools.values() {
        let full = qualified(tool);
        if full == native || native_tool_name(&full) == native {
            return Some(tool);
        }
    }
    let matches: Vec<&Tool> = tools
        .values()
        .filter(|tool| {
            let full = qualified(tool);
            let readable = readable_tool_prefix(&full);
            let leaf_readable = readable_tool_prefix(&tool.name);
            tool.name == native
                || readable == native
                || leaf_readable == native
                || format!("hey_{readable}") == native
                || format!("hey_{leaf_readable}") == native
                || native.starts_with(&format!("hey_{readable}_"))
                || native.starts_with(&format!("hey_{leaf_readable}_"))
        })
        .collect();
    (matches.len() == 1).then(|| matches[0])
}

pub fn usage(native: &Value) -> Option<Value> {
    let u = native.get("usageMetadata")?;
    let input = u.get("promptTokenCount")?.as_u64()?;
    let thoughts = u
        .get("thoughtsTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = u
        .get("candidatesTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(thoughts);
    Some(
        json!({"input_tokens":input,"input_tokens_details":{"cached_tokens":u.get("cachedContentTokenCount").and_then(Value::as_u64).unwrap_or(0)},
        "output_tokens":output,"output_tokens_details":{"reasoning_tokens":thoughts},
        "total_tokens":u.get("totalTokenCount").and_then(Value::as_u64).unwrap_or(input.saturating_add(output)),"gemini":u}),
    )
}

pub(crate) fn output_item(
    part: &Value,
    index: usize,
    request: &ConvertedRequest,
    id: &str,
) -> Result<Option<Value>> {
    // Empty native text can carry essential signatures but has no visible
    // Responses message. Retain it in the authenticated native turn.
    if part.get("text").and_then(Value::as_str) == Some("") {
        return Ok(None);
    }
    if part.get("thought").and_then(Value::as_bool) == Some(true)
        || (part.get("text").is_none()
            && part.get("functionCall").is_none()
            && part.get("thoughtSignature").is_some()
            && part.as_object().is_some_and(|o| o.len() == 1))
    {
        return Ok(None);
    }
    if let Some(call) = part.get("functionCall") {
        if call.get("partialArgs").is_some() || call["willContinue"] == true {
            bail!("Unassembled Gemini function arguments");
        }
        let native = call["name"]
            .as_str()
            .ok_or_else(|| anyhow!("Gemini function call name missing"))?;
        let args = call.get("args").cloned().unwrap_or(json!({}));
        let Some(tool) = resolve_tool(native, &request.tools) else {
            if request.suppress_hallucinated_calls {
                let text = format!(
                    "[Tool call `{native}` skipped because tool calling is disabled for this turn: {}]",
                    serde_json::to_string(&args).unwrap_or_else(|_| "{}".into())
                );
                return Ok(Some(json!({
                    "id": format!("msg_{id}_{index}"),
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": text, "annotations": []}]
                })));
            }
            bail!("Gemini called undeclared tool {native}");
        };
        if tool
            .validator
            .as_ref()
            .is_some_and(|schema| !schema.is_valid(&args))
        {
            bail!("Gemini tool arguments did not satisfy the strict JSON schema");
        }
        if tool.deferred {
            bail!("Gemini called a tool before it was loaded");
        }
        let call_id = call
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("call_{id}_{index}"));
        if let Some(execution) = &tool.search {
            return Ok(Some(
                json!({"id":format!("ts_{id}_{index}"),"type":"tool_search_call","execution":execution,
                "status":"completed","call_id":if execution == "client" {json!(call_id)} else {Value::Null},"arguments":args}),
            ));
        }
        let mut item = json!({"id":format!("{}_{id}_{index}",if tool.custom {"ctc"} else {"fc"}),"type":if tool.custom {"custom_tool_call"} else {"function_call"},"status":"completed","call_id":call_id,"name":tool.name});
        if let Some(namespace) = &tool.namespace {
            item["namespace"] = json!(namespace);
        }
        if tool.custom {
            item["input"] = json!(
                args.get("input")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("Gemini custom tool requires input string"))?
            );
        } else {
            item["arguments"] = json!(serde_json::to_string(&args)?);
        }
        Ok(Some(item))
    } else {
        let content = if let Some(text) = part.get("text").and_then(Value::as_str) {
            json!({"type":"output_text","text":text,"annotations":[]})
        } else {
            json!({"type":"gemini_part","part":part})
        };
        Ok(Some(
            json!({"id":format!("msg_{id}_{index}"),"type":"message","role":"assistant","status":"completed","content":[content]}),
        ))
    }
}

pub(crate) fn output_items(
    parts: &[Value],
    request: &ConvertedRequest,
    id: &str,
) -> Result<Vec<Value>> {
    let mut output = Vec::new();
    for part in parts {
        if let Some(item) = output_item(part, output.len() + 1, request, id)? {
            output.push(item);
        }
    }
    Ok(output)
}

/// Exact native parts and all native response metadata remain available. The
/// authenticated reasoning item carries the model turn for stateless replay.
pub fn convert_response(
    native: &Value,
    request: &ConvertedRequest,
    codec: &ReasoningCodec,
    id: &str,
) -> Result<Value> {
    super::validate::response(native)?;
    if native.get("error").is_some() {
        return Ok(json!({"id":format!("resp_{id}"),"object":"response",
            "status":"failed","model":request.response_model,"output":[],
            "error":super::ResponseError::from_native(native, None).error,
            "usage":usage(native),"gemini":native}));
    }
    let candidates = native.get("candidates").and_then(Value::as_array);
    if candidates.is_some_and(|c| c.len() > 1) {
        bail!("Multiple Gemini candidates require separate Responses; refusing to drop candidates");
    }
    let candidate = candidates.and_then(|c| c.first());
    let empty_parts = json!([]);
    let parts_value = candidate
        .and_then(|c| c.pointer("/content/parts"))
        .unwrap_or(&empty_parts);
    let parts = parts_value.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut items = output_items(parts, request, id)?;
    let summary: Vec<_> = parts
        .iter()
        .filter(|p| p["thought"] == true)
        .filter_map(|p| p["text"].as_str())
        .map(|text| json!({"type":"summary_text","text":text}))
        .collect();
    let reason = candidate
        .and_then(|c| c.get("finishReason"))
        .and_then(Value::as_str);
    let blocked = native
        .pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str);
    let (mut status, mut error, incomplete) = match (reason, blocked) {
        (_, Some(block)) => (
            "failed",
            json!({"type":"invalid_request_error","code":"invalid_prompt","gemini_code":"gemini_prompt_blocked","message":format!("Gemini blocked the prompt: {block}")}),
            Value::Null,
        ),
        (Some("STOP"), None) => ("completed", Value::Null, Value::Null),
        (Some("MAX_TOKENS"), None) => (
            "incomplete",
            Value::Null,
            json!({"reason":"max_output_tokens"}),
        ),
        (
            Some(
                reason @ ("SAFETY"
                | "RECITATION"
                | "BLOCKLIST"
                | "PROHIBITED_CONTENT"
                | "SPII"
                | "IMAGE_SAFETY"
                | "IMAGE_PROHIBITED_CONTENT"),
            ),
            None,
        ) => (
            "failed",
            json!({"type":"invalid_request_error","code":"invalid_prompt","gemini_code":"gemini_finish_error","message":format!("Gemini finish reason: {reason}")}),
            Value::Null,
        ),
        (Some(reason), None) => (
            "failed",
            json!({"type":"server_error","code":"server_error","gemini_code":"gemini_finish_error","message":format!("Gemini finish reason: {reason}")}),
            Value::Null,
        ),
        (None, None) => (
            "failed",
            json!({"type":"server_error","code":"server_error","gemini_code":"gemini_missing_completion","message":"Gemini ended without a finish reason; retry the request"}),
            Value::Null,
        ),
    };
    if status == "completed"
        && let Some(schema) = &request.output_schema
        && !parts.iter().any(|p| p.get("functionCall").is_some())
    {
        let text: String = parts
            .iter()
            .filter(|p| p["thought"] != true)
            .filter_map(|p| p["text"].as_str())
            .collect();
        let valid = parts.iter().all(|part| {
            part["thought"] == true
                || part.get("text").is_some()
                || part
                    .as_object()
                    .is_some_and(|p| p.len() == 1 && p.contains_key("thoughtSignature"))
        }) && serde_json::from_str::<Value>(&text)
            .is_ok_and(|value| schema.is_valid(&value));
        if !valid {
            status = "failed";
            error = json!({"code":"gemini_schema_mismatch","message":"Gemini output did not satisfy the requested JSON schema"});
        }
    }
    if status != "completed" {
        for item in &mut items {
            item["status"] = json!("incomplete");
        }
    }
    let carrier = codec.seal(&request.model, parts_value, &json!(items))?;
    let mut output = vec![
        json!({"id":format!("rs_{id}"),"type":"reasoning","summary":summary,"encrypted_content":carrier}),
    ];
    output.extend(items);
    let mut response = json!({"id":format!("resp_{id}"),"object":"response","created_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
        "status":status,"model":request.response_model,"output":output,"usage":usage(native),"error":error,"incomplete_details":incomplete,
        "parallel_tool_calls":true,"store":false,"gemini":native});
    response
        .as_object_mut()
        .unwrap()
        .extend(request.response_fields.clone());
    Ok(response)
}
