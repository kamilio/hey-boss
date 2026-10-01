//! Editable prompt defaults and single-pass variable expansion.
use super::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

macro_rules! prompts {
    ($(($key:ident, $constant:ident, $title:literal, $help:literal)),* $(,)?) => {
        $(pub const $constant: &str = include_str!(concat!("prompts/", stringify!($key), ".md")).trim_ascii_end();)*
        #[derive(Debug, Clone, Default, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct PromptOverrides {
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
        pub fn prompt_defaults() -> Value { json!({$(stringify!($key): $constant,)*}) }
        pub fn prompt_sections() -> Value { json!([$({"key":stringify!($key),"title":$title,"help":$help},)*]) }
    };
}

prompts! {
    (layout, DEFAULT_LAYOUT_PROMPT, "Prompt sequence", "Arrange or omit {{task}}, {{subtask}}, {{dependencies}}, {{plan_document}}, {{github}}, {{workspace}}, {{delivery}}, {{handoff}}, {{resume}}. Empty sections disappear. Task selects Implementation or Plan; workspace and delivery follow the workflow settings."),
    (plan, DEFAULT_PLAN_PROMPT, "Planning", ""),
    (worktree, DEFAULT_WORKTREE_PROMPT, "Dedicated worktree", ""),
    (checkout, DEFAULT_CHECKOUT_PROMPT, "Existing checkout", ""),
    (prs, DEFAULT_PRS_PROMPT, "Pull request", ""),
    (main, DEFAULT_MAIN_PROMPT, "Push to main", ""),
    (handoff, DEFAULT_HANDOFF_PROMPT, "Ready handoff", "PR tasks that unblock dependencies before merge."),
    (subtask, DEFAULT_SUBTASK_PROMPT, "Subtask context", "Subtasks. Variables: {{subtask_position}}, {{subtask_total}}, {{parent_number}}, {{parent_title}}, {{parent_state}}, {{previous_subtask}}, {{next_subtask}}."),
    (dependencies, DEFAULT_DEPENDENCIES_PROMPT, "Prerequisites", "Tasks with dependencies. Variable: {{dependencies}}."),
    (resume, DEFAULT_RESUME_PROMPT, "Resume", "Resumed tasks."),
    (plan_document, DEFAULT_PLAN_DOCUMENT_PROMPT, "Plan document", "Tasks with a linked plan. Variable: {{plan_path}}."),
    (github, DEFAULT_GITHUB_PROMPT, "GitHub status", "Tasks with GitHub status, and live GitHub updates."),
    (steering, DEFAULT_STEERING_PROMPT, "New instructions", "Live instructions. Variables: {{scope}}, {{instruction}}."),
    (dependency_update, DEFAULT_DEPENDENCY_UPDATE_PROMPT, "Dependency updates", "Live dependency changes. Variable: {{instruction}}."),
    (prompt_update, DEFAULT_PROMPT_UPDATE_PROMPT, "Prompt updates", "Changed project instructions. Variable: {{instructions}}."),
    (goal_continue, DEFAULT_GOAL_CONTINUE_PROMPT, "Continue a goal", "A goal turn ends before a completion report."),
    (chief_wrapper, DEFAULT_CHIEF_WRAPPER_PROMPT, "Chief context", "Chief launches. Variables: {{project}}, {{prompt}}."),
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
