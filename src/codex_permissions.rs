//! Auto is the default. Only Boss's issue-level web control grants YOLO
//! for a worker attempt; new and resumed threads use the same explicit policy.
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
