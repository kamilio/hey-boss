use super::*;
use anyhow::{anyhow, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
const CARRIER: &str = "hey_messages_v1.";

pub(super) fn clean_headers(headers: &mut HeaderMap) {
    for name in [
        "content-length",
        "content-encoding",
        "etag",
        "content-md5",
        "digest",
    ] {
        headers.remove(name);
    }
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    if let Some(id) = headers.get("x-request-id").cloned() {
        headers.insert("request-id", id);
    }
}
pub(super) fn error_value(status: StatusCode, message: &str) -> Value {
    let kind = match status.as_u16() {
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        503 | 529 => "overloaded_error",
        400..=499 => "invalid_request_error",
        _ => "api_error",
    };
    json!({"type":"error","error":{"type":kind,"message":message}})
}
pub(super) fn encode_details(details: &[Value]) -> String {
    format!(
        "{CARRIER}{}",
        STANDARD.encode(serde_json::to_vec(details).unwrap())
    )
}
pub(super) fn decode_details(data: &str) -> Result<Vec<Value>> {
    let data = data.strip_prefix(CARRIER).ok_or_else(|| {
        anyhow!("Thinking replay requires the original signature/data from this Messages shim")
    })?;
    let decoded = STANDARD
        .decode(data)
        .map_err(|_| anyhow!("Invalid thinking replay data"))?;
    let details: Vec<Value> =
        serde_json::from_slice(&decoded).map_err(|_| anyhow!("Invalid thinking replay data"))?;
    ensure!(
        details.iter().all(Value::is_object),
        "Invalid thinking replay details"
    );
    Ok(details)
}
pub(super) fn reasoning_text(details: &[Value]) -> String {
    details
        .iter()
        .filter_map(|d| {
            d.get("summary")
                .or_else(|| d.get("text"))
                .and_then(Value::as_str)
        })
        .collect()
}
pub(super) fn thinking(details: &[Value]) -> Option<Value> {
    if details.is_empty() {
        return None;
    }
    let text = reasoning_text(details);
    let data = encode_details(details);
    Some(if text.is_empty() {
        json!({"type":"redacted_thinking","data":data})
    } else {
        json!({"type":"thinking","thinking":text,"signature":data})
    })
}
pub(super) fn usage(value: &Value) -> Value {
    let input = value["prompt_tokens"].as_u64().unwrap_or(0);
    let cached = value["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0);
    let written = value["prompt_tokens_details"]["cache_write_tokens"]
        .as_u64()
        .unwrap_or(0);
    // Anthropic input_tokens excludes cache reads/writes; Responses includes them.
    json!({"input_tokens":input.saturating_sub(cached).saturating_sub(written),
        "output_tokens":value["completion_tokens"].as_u64().unwrap_or(0),
        "cache_creation_input_tokens":written,"cache_read_input_tokens":cached})
}
pub(super) fn stop(reason: &str) -> Result<&'static str> {
    Ok(match reason {
        "stop" => "end_turn",
        "tool_calls" => "tool_use",
        "length" => "max_tokens",
        "content_filter" => "refusal",
        _ => bail!("Unsupported upstream finish reason"),
    })
}
pub(super) fn convert(value: &Value) -> Result<Value> {
    let choice = &value["choices"][0];
    let message = &choice["message"];
    let mut content = Vec::new();
    if let Some(details) = message["reasoning_details"].as_array()
        && let Some(block) = thinking(details)
    {
        content.push(block);
    }
    for key in ["content", "refusal"] {
        if let Some(text) = message[key].as_str().filter(|s| !s.is_empty()) {
            content.push(json!({"type":"text","text":text}));
        }
    }
    for call in message["tool_calls"].as_array().into_iter().flatten() {
        let input: Value =
            serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or(""))
                .map_err(|_| anyhow!("Upstream tool arguments are not complete JSON"))?;
        ensure!(
            input.is_object(),
            "Upstream tool arguments must be an object"
        );
        content.push(json!({"type":"tool_use","id":call["id"],"name":call["function"]["name"],"input":input}));
    }
    let stop = stop(choice["finish_reason"].as_str().unwrap_or(""))?;
    Ok(
        json!({"id":format!("msg_{}",value["id"].as_str().ok_or_else(|| anyhow!("Upstream reply lacks id"))?),
        "type":"message","role":"assistant","model":value["model"],"content":content,
        "stop_reason":stop,"stop_sequence":null,"usage":usage(&value["usage"])}),
    )
}
