//! Hardcoded Auto permission policy for every Codex session Hey Boss launches.
//! Keep approvals interactive so Codex's reviewer can approve or deny requests.
//! Never rewrite the user's global config or silently fall back to Full Access.
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
