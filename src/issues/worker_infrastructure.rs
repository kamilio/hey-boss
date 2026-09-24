//! Recognized service failures retain their cause and retry automatically.
use serde_json::Value;

pub(super) const STATE: &str = "infrastructure_blocked";
pub(super) const GUIDANCE: &str = "Approval service unavailable. This attempt failed and will retry automatically with exponential backoff. Its slot and claim are released; the saved session, checkout and issue history are retained. Retrying grants no permissions and never bypasses approval.";
const DATABASE_GUIDANCE: &str = "Database service unavailable. This attempt failed and will retry automatically with exponential backoff. Its slot and claim are released; the saved session, checkout and issue history are retained. Before repeating an uncertain mutation, read and reconcile the current issue state or reuse its original request ID for deduplication. Never blindly replay a write whose outcome is unknown.";
const PROXY_GUIDANCE: &str = "Model proxy unavailable. This attempt failed and will retry automatically with exponential backoff. Its slot and claim are released; the saved session, checkout and issue history are retained. A proxy recovery timeout is not an implementation result.";

pub(super) fn database_unavailable(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    // A sandbox/policy denial is not repaired by restoring the database.
    if text.contains("operation not permitted") || text.contains("permission denied") {
        return false;
    }
    [
        "database service transport failed:",
        "database service disconnected",
        "database result stream disconnected",
        "database transaction lost its connection",
        "database service unavailable:",
        "database service did not become ready",
        "database service exited during startup",
        "database service protocol mismatch",
    ]
    .iter()
    .any(|pattern| text.contains(pattern))
}

pub(super) fn unavailable(text: &str) -> Option<&'static str> {
    if database_unavailable(text) {
        return Some(DATABASE_GUIDANCE);
    }
    let lower = text.to_ascii_lowercase();
    if (lower.contains("hey-proxy") || lower.contains("model proxy"))
        && lower.contains("recovery")
        && lower.contains("budget")
        && lower.contains("exhausted")
    {
        return Some(PROXY_GUIDANCE);
    }
    approval_unavailable(text).then_some(GUIDANCE)
}

pub(super) fn approval_unavailable(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    // A denied command or policy rejection is not an outage. Recognize the
    // engine's pre-execution review failure and the captured agent reports,
    // without depending on the configured model's name.
    let review = text.contains("automatic approval review")
        || text.contains("approval review cannot execute");
    review
        && ((text.contains("404")
            && (text.contains("model")
                || text.contains("missing")
                || text.contains("cannot access configured")
                || text.contains("unavailable")))
            || text.contains("automatic approval review could not be completed")
            || text.contains("automatic approval review failed:")
            || text.contains("automatic approval review is unavailable")
            || text.contains("this is a review failure, not a determination"))
}

pub(super) fn tool_failure(item: &Value) -> Option<String> {
    let output = match item["type"].as_str()? {
        "commandExecution"
            if item["status"] == "failed" || item["exitCode"].as_i64().is_some_and(|c| c != 0) =>
        {
            &item["aggregatedOutput"]
        }
        "mcpToolCall"
            if item["status"] == "failed"
                || item["result"]["isError"] == true
                || !item["error"].is_null() =>
        {
            if !item["error"].is_null() {
                &item["error"]
            } else {
                &item["result"]
            }
        }
        "dynamicToolCall" if item["success"] == false => &item["contentItems"],
        _ => return None,
    };
    fn find(value: &Value) -> Option<String> {
        match value {
            Value::String(s) if unavailable(s).is_some() => Some(s.chars().take(4000).collect()),
            Value::Array(values) => values.iter().find_map(find),
            Value::Object(values) => values.values().find_map(find),
            _ => None,
        }
    }
    find(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn captured_service_failures_are_distinct_from_policy_denials() {
        for text in [
            "Automatic approval review failed: unexpected status 404 Not Found: The model `other-model` does not exist or you do not have access to it.",
            "Automatic approval review rejected publication twice because `wisp-alpha` is unavailable (HTTP 404).",
            "Approval review cannot execute: configured wisp-alpha model returns404.",
            "automatic approval review itself cannot access configured wisp-alpha (404)",
            "Automatic approval review failed: upstream returned 503 Service Unavailable.",
            "Automatic approval review could not be completed: connection timed out.",
        ] {
            assert!(approval_unavailable(text), "{text}");
        }
        for text in [
            "Automatic approval review rejected legacy-directory deletion because unique contents could not be verified.",
            "Automatic approval review rejected that step twice, interpreting AGENTS.md as prohibiting it.",
            "GitHub returned 404: missing pull request.",
            "The approval model is unavailable.",
        ] {
            assert!(!approval_unavailable(text), "{text}");
        }
    }

    #[test]
    fn only_failed_tool_outputs_supply_outage_evidence() {
        let text = "Automatic approval review failed: HTTP 404, model unavailable";
        for item in [
            json!({"type":"commandExecution","exitCode":1,"aggregatedOutput":text}),
            json!({"type":"mcpToolCall","status":"failed","error":{"message":text}}),
            json!({"type":"dynamicToolCall","success":false,"contentItems":[{"type":"inputText","text":text}]}),
        ] {
            assert_eq!(tool_failure(&item).as_deref(), Some(text));
        }
        for item in [
            json!({"type":"commandExecution","exitCode":0,"aggregatedOutput":text}),
            json!({"type":"mcpToolCall","status":"completed","arguments":{"text":text}}),
            json!({"type":"agentMessage","text":text}),
        ] {
            assert!(tool_failure(&item).is_none());
        }
    }

    #[test]
    fn database_and_proxy_transport_failures_are_not_implementation_failures() {
        for text in [
            "Database service disconnected; write outcome may be unknown; mutations are never automatically replayed.",
            "Database service transport failed: Broken pipe (os error 32). Write outcome may be unknown; mutations are never automatically replayed.",
            "Database transaction lost its connection; its outcome cannot be replayed automatically",
            "HTTP 504 from local hey-proxy: internal recovery time budget exhausted before a response could be forwarded.",
        ] {
            assert!(unavailable(text).is_some(), "{text}");
            assert!(
                tool_failure(
                    &json!({"type":"commandExecution","exitCode":1,"aggregatedOutput":text})
                )
                .is_some()
            );
            assert!(
                tool_failure(
                    &json!({"type":"commandExecution","exitCode":0,"aggregatedOutput":text})
                )
                .is_none()
            );
        }
        for text in [
            "HTTP 504 from application under test",
            "Broken pipe during unit test",
            "Database constraint failed: UNIQUE constraint",
            "hey-proxy returned HTTP 403: policy denied",
            "cargo test failed: expected 504 from hey-proxy, got 200",
        ] {
            assert!(unavailable(text).is_none(), "{text}");
        }
    }
}
