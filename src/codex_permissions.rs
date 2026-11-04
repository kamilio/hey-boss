//! Explicit Auto and YOLO policies. Workers can also inherit the harness configuration.
use serde_json::Value;
use std::process::Command;

pub(crate) const INTERACTIVE_FLAG: &str = "--approve-for-me";

pub(crate) fn apply(command: &mut Command) -> &mut Command {
    command.args([
        "-c",
        "approval_policy=\"on-request\"",
        "-c",
        "approvals_reviewer=\"auto_review\"",
        "-c",
        "sandbox_mode=\"workspace-write\"",
    ])
}

/// Resume must override the saved thread's policy as well as process defaults.
pub(crate) fn thread(mut params: Value) -> Value {
    params["approvalPolicy"] = "on-request".into();
    params["approvalsReviewer"] = "auto_review".into();
    params["sandbox"] = "workspace-write".into();
    params
}

pub(crate) fn apply_yolo(command: &mut Command) -> &mut Command {
    command.args([
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "approvals_reviewer=\"user\"",
        "-c",
        "sandbox_mode=\"danger-full-access\"",
    ])
}

pub(crate) fn yolo_thread(mut params: Value) -> Value {
    params["approvalPolicy"] = "never".into();
    params["approvalsReviewer"] = "user".into();
    params["sandbox"] = "danger-full-access".into();
    params
}

pub(crate) fn confirmation_schema(params: &Value) -> bool {
    if params
        .get("mode")
        .is_some_and(|mode| !matches!(mode.as_str(), Some("form" | "openai/form")))
    {
        return false;
    }
    let Some(schema) = params["requestedSchema"].as_object() else {
        return false;
    };
    // An empty form asks only for a decision. Reject fields, defaults and
    // unrecognized constraints rather than guessing or collecting their values.
    schema.get("type").is_some_and(|value| value == "object")
        && schema
            .get("properties")
            .is_some_and(|value| value.as_object().is_some_and(|fields| fields.is_empty()))
        && schema.iter().all(|(key, value)| match key.as_str() {
            "type" | "properties" => true,
            "required" => value.as_array().is_some_and(|fields| fields.is_empty()),
            "additionalProperties" => value.is_boolean(),
            "$schema" | "title" | "description" => value.is_string(),
            _ => false,
        })
}
