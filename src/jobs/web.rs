//! Read-only runtime discovery for desktop and paired Jobs surfaces.
use crate::{
    agent_runtime::{AgentSession, Launch, ModelSelection, Provider},
    issues::{Error, Result},
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Discovery {
    provider: Option<Provider>,
    #[serde(default)]
    configured: Vec<String>,
}

pub(crate) fn runtime(value: Value) -> Result<Value> {
    let input: Discovery = serde_json::from_value(value)?;
    if input.configured.len() > 20 || input.configured.iter().any(|s| s.len() > 256) {
        return Err(Error::invalid("Too many or oversized configured model IDs"));
    }
    let status = crate::fleet::call(&json!({"kind":"overview"}));
    let (machines, service_error) = match status {
        Ok(value) => (
            json!(value["machines"].as_array().into_iter().flatten().map(|m| {
            json!({"host":m["host"],"node":m["node"],"state":m["state"],"heartbeat":m["heartbeat"],"jobs":m["jobs"]})
        }).collect::<Vec<_>>()),
            value.get("error").cloned(),
        ),
        Err(error) => (json!([]), Some(json!(error.to_string()))),
    };
    let machine = crate::issues::identity::machine()?;
    let mut result =
        json!({"ok":true,"machines":machines,"machine":machine,"service_error":service_error});
    if let Some(provider) = input.provider {
        let configured = input
            .configured
            .into_iter()
            .map(|value| {
                if provider == Provider::Pi {
                    let (route, id) = value.split_once('/').unwrap_or(("", &value));
                    ModelSelection {
                        id: id.into(),
                        route: (!route.is_empty()).then(|| route.into()),
                    }
                } else {
                    ModelSelection {
                        id: value,
                        route: None,
                    }
                }
            })
            .collect::<Vec<_>>();
        result["catalog"] = serde_json::to_value(AgentSession::discover_models(
            Launch {
                provider,
                model: None,
                binary: None,
                cwd: std::env::current_dir()?,
                resume: None,
                env: Default::default(),
                output_schema: None,
            },
            &configured,
        ))?;
    }
    Ok(result)
}
