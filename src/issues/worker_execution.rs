//! Project-wide Codex execution choices. Empty values inherit the local harness configuration.
use super::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionSettings {
    pub model: String,
    pub reasoning: String,
    pub permissions: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inherited_execution_leaves_process_and_thread_configuration_untouched() {
        let execution = ExecutionSettings {
            permissions: String::new(),
            ..Default::default()
        };
        let mut command = Command::new("codex");
        execution.apply(&mut command, &json!({"labels":[]}));
        assert_eq!(command.get_args().count(), 0);
        assert_eq!(
            execution.thread(json!({"cwd":"/work"}), &json!({})),
            json!({"cwd":"/work"})
        );
        assert_eq!(
            execution.turn(json!({"threadId":"one"})),
            json!({"threadId":"one"})
        );
    }

    #[test]
    fn issue_yolo_overrides_project_permissions_and_model_is_a_literal_argument() {
        for permissions in ["", "auto", "yolo"] {
            let execution = ExecutionSettings {
                model: "custom-model".into(),
                reasoning: "high".into(),
                permissions: permissions.into(),
            };
            let issue = json!({"labels":["YOLO"]});
            let mut command = Command::new("codex");
            execution.apply(&mut command, &issue);
            let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
            assert!(args.contains(&"approval_policy=\"never\""));
            assert!(args.contains(&"sandbox_mode=\"danger-full-access\""));
            assert!(args.contains(&"model=\"custom-model\""));
            assert!(args.contains(&"model_reasoning_effort=\"high\""));
            assert_eq!(
                execution.thread(json!({}), &issue)["sandbox"],
                "danger-full-access"
            );
        }
    }
}

impl Default for ExecutionSettings {
    fn default() -> Self {
        Self {
            model: String::new(),
            reasoning: String::new(),
            permissions: "auto".into(),
        }
    }
}

impl ExecutionSettings {
    pub fn validate(&self) -> Result<()> {
        if self.model.len() > 512
            || self.model.starts_with('-')
            || self
                .model
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(Error::invalid(
                "Use a model ID without spaces, or leave it empty to inherit",
            ));
        }
        if ![
            "", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ]
        .contains(&self.reasoning.as_str())
        {
            return Err(Error::invalid(
                "Choose a supported reasoning level, or inherit",
            ));
        }
        if !["", "auto", "yolo"].contains(&self.permissions.as_str()) {
            return Err(Error::invalid(
                "Worker permissions must be auto, yolo, or empty to inherit",
            ));
        }
        Ok(())
    }

    pub(super) fn permissions<'a>(&'a self, issue: &Value) -> &'a str {
        if issue["labels"].as_array().is_some_and(|labels| {
            labels.iter().any(|label| {
                label
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case("yolo"))
            })
        }) {
            "yolo"
        } else {
            &self.permissions
        }
    }

    pub(super) fn apply(&self, command: &mut Command, issue: &Value) {
        match self.permissions(issue) {
            "auto" => {
                crate::codex_permissions::apply(command);
            }
            "yolo" => {
                crate::codex_permissions::apply_yolo(command);
            }
            _ => {}
        }
        for (key, value) in [
            ("model", &self.model),
            ("model_reasoning_effort", &self.reasoning),
        ] {
            if !value.is_empty() {
                command
                    .arg("-c")
                    .arg(format!("{key}={}", serde_json::to_string(value).unwrap()));
            }
        }
    }

    pub(super) fn thread(&self, mut params: Value, issue: &Value) -> Value {
        params = match self.permissions(issue) {
            "auto" => crate::codex_permissions::thread(params),
            "yolo" => crate::codex_permissions::yolo_thread(params),
            _ => params,
        };
        if !self.model.is_empty() {
            params["model"] = self.model.clone().into();
        }
        params
    }

    pub(super) fn turn(&self, mut params: Value) -> Value {
        if !self.model.is_empty() {
            params["model"] = self.model.clone().into();
        }
        if !self.reasoning.is_empty() {
            params["effort"] = self.reasoning.clone().into();
        }
        params
    }
}
