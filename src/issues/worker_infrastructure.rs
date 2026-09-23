//! Approval infrastructure failures are holds, never implementation failures.
use serde_json::Value;

pub(super) const STATE: &str = "infrastructure_blocked";
pub(super) const GUIDANCE: &str = "Approval service unavailable. Automatic pickup is held; this run does not consume an implementation retry. The saved session, checkout and issue history are retained. Restore the approval service, then reopen the issue to resume the saved session. This hold grants no permissions and does not bypass approval.";

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
            Value::String(s) if approval_unavailable(s) => Some(s.chars().take(4000).collect()),
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
}
