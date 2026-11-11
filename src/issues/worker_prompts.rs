//! Editable prompt defaults and single-pass variable expansion.
use super::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

macro_rules! prompts {
    ($(($key:ident, $constant:ident, $file:literal, $title:literal, $help:literal)),* $(,)?) => {
        $(pub const $constant: &str = include_str!(concat!("prompts/", $file)).trim_ascii_end();)*
        #[derive(Debug, Clone, Default, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct PromptOverrides {
            // Accept old saved sections without applying or exposing them.
            #[serde(skip_serializing)]
            pub error_task: Option<String>,
            #[serde(skip_serializing)]
            pub goal_continue: Option<String>,
            #[serde(skip_serializing)]
            pub handoff: Option<String>,
            #[serde(skip_serializing)]
            pub layout: Option<String>,
            #[serde(skip_serializing)]
            pub resume: Option<String>,
            #[serde(skip_serializing)]
            pub dependencies: Option<String>,
            #[serde(skip_serializing)]
            pub plan_document: Option<String>,
            #[serde(skip_serializing)]
            pub subtask: Option<String>,
            #[serde(skip_serializing)]
            pub github: Option<String>,
            #[serde(skip_serializing)]
            pub steering: Option<String>,
            #[serde(skip_serializing)]
            pub dependency_update: Option<String>,
            #[serde(skip_serializing)]
            pub prompt_update: Option<String>,
            // Accept legacy saved settings, but never apply or expose Chief wrappers.
            #[serde(skip_serializing)]
            pub chief_wrapper: Option<String>,
            $(#[serde(skip_serializing_if = "Option::is_none")] pub $key: Option<String>,)*
        }
        impl PromptOverrides {
            pub fn get(&self, key: &str) -> &str {
                match key {$(stringify!($key) => self.$key.as_deref().unwrap_or($constant),)* _ => unreachable!("Unknown prompt section: {key}")}
            }
            pub fn validate(&self) -> Result<()> {
                $(if let Some(text) = &self.$key {
                    if text.len() > 32000 || text.trim().is_empty() {
                        return Err(Error::invalid("Workflow prompts must contain 1–32000 bytes, or null to use the default"));
                    }
                })*
                Ok(())
            }
        }
        pub(crate) fn prompt_filename(key: &str) -> Option<&'static str> {
            match key { "worker" => Some("worker.md"), $(stringify!($key) => Some($file),)* _ => None }
        }
        pub fn prompt_defaults() -> Value { json!({$(stringify!($key): $constant,)*}) }
        pub fn prompt_sections() -> Value { json!([$({"key":stringify!($key),"path":$file,"title":$title,"help":$help},)*]) }
    };
}

prompts! {
    (plan, DEFAULT_PLAN_PROMPT, "plan.md", "Planning", ""),
    (worktree, DEFAULT_WORKTREE_PROMPT, "workspace-worktree.md", "Dedicated worktree", ""),
    (checkout, DEFAULT_CHECKOUT_PROMPT, "workspace-checkout.md", "Existing checkout", ""),
    (prs, DEFAULT_PRS_PROMPT, "delivery-pr.md", "Pull request", ""),
    (main, DEFAULT_MAIN_PROMPT, "delivery-main.md", "Push to main", ""),
}

pub fn render(text: &str, mut value: impl FnMut(&str) -> Option<String>) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let Some(end) = rest[start + 2..].find("}}") else {
            output.push_str(&rest[start..]);
            return output;
        };
        let end = start + 2 + end;
        if let Some(value) = value(rest[start + 2..end].trim()) {
            output.push_str(&value);
        } else {
            output.push_str(&rest[start..end + 2]);
        }
        rest = &rest[end + 2..];
    }
    output.push_str(rest);
    output
}
