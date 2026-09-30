use hey_proxy::gemini::CARRIER_PREFIX;
use serde_json::Value;

/// Gemini's native replay capsule cannot be decrypted by an OpenAI endpoint.
/// Remove only that foreign reasoning item from the outgoing HTTP attempt, and
/// drop item ids hey-proxy minted for Gemini output: OpenAI validates their
/// prefixes and the ids are optional. All visible messages, tool calls/results
/// and review instructions remain intact.
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
    let mut changed = input.len() != before;
    for item in input.iter_mut() {
        if item["id"].as_str().is_some_and(gemini_item_id)
            && let Some(item) = item.as_object_mut()
        {
            item.remove("id");
            changed = true;
        }
    }
    changed
}

/// Ids minted for Gemini output: `<prefix>_<32 hex>[_<round>]_<index>`.
/// OpenAI's own item ids carry no further underscore after the prefix.
fn gemini_item_id(id: &str) -> bool {
    let mut parts = id.split('_');
    let (Some(prefix), Some(hex)) = (parts.next(), parts.next()) else {
        return false;
    };
    let rest: Vec<&str> = parts.collect();
    !prefix.is_empty()
        && prefix.bytes().all(|b| b.is_ascii_lowercase())
        && hex.len() == 32
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
        && matches!(rest.len(), 1 | 2)
        && rest
            .iter()
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests;
