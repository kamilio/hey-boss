use hey_proxy::gemini::CARRIER_PREFIX;
use serde_json::Value;

/// Gemini's native replay capsule cannot be decrypted by an OpenAI endpoint.
/// Remove only that foreign reasoning item from the outgoing HTTP attempt. All
/// visible messages, tool calls/results and review instructions remain intact.
/// Do this after routing, not in `rewrite`: fallback planning must keep the
/// original carrier in case a later attempt goes back to Gemini.
pub(super) fn for_openai(path: &str, request: &mut Value) -> bool {
    if !matches!(path.trim_end_matches('/'), "/v1/responses" | "/responses")
        || request["model"]
            .as_str()
            .is_none_or(|model| model.starts_with("gemini/"))
    {
        return false;
    }
    let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let before = input.len();
    input.retain(|item| {
        !(item["type"] == "reasoning"
            && item["encrypted_content"]
                .as_str()
                .is_some_and(|carrier| carrier.starts_with(CARRIER_PREFIX)))
    });
    input.len() != before
}

#[cfg(test)]
mod tests;
