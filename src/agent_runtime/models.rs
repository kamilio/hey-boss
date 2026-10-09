use super::*;

/// A logical model, independent of credentials, billing, or proxy backends.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModelSelection {
    pub id: String,
    /// Pi's configured provider entry point. Codex/Claude inherit their route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelChoice {
    pub selection: ModelSelection,
    pub label: String,
    pub configured: bool,
    /// Listed by the harness; this is not proof of inference entitlement.
    pub advertised: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub provider: Provider,
    pub models: Vec<ModelChoice>,
    /// Discovery failures remain visible even when configured choices survive.
    pub error: Option<String>,
}
fn valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
        && !value.starts_with('-')
}
pub(super) fn resolve(launch: &mut Launch) -> io::Result<()> {
    if let Some(saved) = &launch.resume {
        if let Some(requested) = &launch.model {
            if saved.model.as_ref() != Some(requested) {
                return Err(io::Error::other(
                    "Resume model differs from the saved model (or the legacy session has no model pin); resume with its saved selection",
                ));
            }
        }
        launch.model = saved.model.clone();
    }
    if let Some(model) = &launch.model {
        if !valid(&model.id) || model.route.as_ref().is_some_and(|r| !valid(r)) {
            return Err(io::Error::other(
                "Model ID and route must be nonempty identifiers without whitespace or control characters",
            ));
        }
        if (launch.provider == Provider::Pi) != model.route.is_some() {
            return Err(io::Error::other(
                "Pi requires an exact configured model route; Codex and Claude inherit their configured route and do not accept a route override",
            ));
        }
    }
    Ok(())
}
impl AgentSession {
    pub(super) fn start_session(&mut self) -> io::Result<()> {
        match self.provider {
            Provider::Codex => {
                let (method, mut params) = match &self.expected_session {
                    Some(s) => ("thread/resume", json!({"threadId":s.id})),
                    None => ("thread/start", json!({"ephemeral":false})),
                };
                if let Some(model) = &self.model {
                    params["model"] = json!(model.id);
                }
                let result = self.rpc(method, crate::codex_permissions::thread(params))?;
                self.verify_model(&result["model"], &Value::Null)?;
                self.attach(required(&result["thread"], "id")?, None)?;
            }
            Provider::Claude => {}
            Provider::Pi => {
                // Verify the saved file actually restored the pinned execution
                // identity before issuing any mutating model command.
                if self.expected_session.is_some() && self.model.is_some() {
                    self.refresh_pi()?;
                }
                if let Some(model) = self.model.clone() {
                    let result = self.rpc(
                        "set_model",
                        json!({"provider":model.route,"modelId":model.id}),
                    )?;
                    self.verify_model(&result["id"], &result["provider"])?;
                }
                self.refresh_pi()?;
            }
        }
        Ok(())
    }
    pub(super) fn verify_model(&mut self, id: &Value, route: &Value) -> io::Result<()> {
        if let Some(expected) = &self.model {
            if id.as_str() != Some(expected.id.as_str())
                || (self.provider == Provider::Pi && route.as_str() != expected.route.as_deref())
            {
                self.uncertain = true;
                return Err(io::Error::other(format!(
                    "{} did not confirm the requested logical model {}; use an exact supported model ID and check the configured route. No replacement session was started",
                    self.provider.name(),
                    expected.id
                )));
            }
        }
        self.model_verified = true;
        Ok(())
    }
    /// Query the harness without starting a turn or a Codex thread. Call outside
    /// database transactions; the owned discovery process is stopped on return.
    /// Pass saved/configured selections so custom proxy IDs remain editable.
    pub fn discover_models(mut launch: Launch, configured: &[ModelSelection]) -> ModelCatalog {
        let mut catalog = ModelCatalog {
            provider: launch.provider,
            models: Vec::new(),
            error: None,
        };
        for selection in configured {
            if !catalog.models.iter().any(|m| &m.selection == selection) {
                catalog.models.push(ModelChoice {
                    selection: selection.clone(),
                    label: selection.id.clone(),
                    configured: true,
                    advertised: false,
                });
            }
        }
        launch.model = None;
        launch.resume = None;
        launch.output_schema = None;
        let discovered = (|| {
            let mut client = Self::connect(launch)?;
            let models = client.read_models();
            let stopped = client.process.stop();
            let models = models?;
            stopped?;
            Ok::<_, io::Error>(models)
        })();
        match discovered {
            Ok(models) => {
                for model in models {
                    if let Some(existing) = catalog
                        .models
                        .iter_mut()
                        .find(|m| m.selection == model.selection)
                    {
                        existing.advertised = true;
                        existing.label = model.label;
                    } else {
                        catalog.models.push(model);
                    }
                }
            }
            Err(error) => {
                catalog.error = Some(format!(
                    "{} model discovery failed: {error}. Check the installed harness and its configured entry point",
                    catalog.provider.name()
                ))
            }
        }
        catalog
    }
    fn read_models(&mut self) -> io::Result<Vec<ModelChoice>> {
        let mut choices = Vec::new();
        let mut selections = BTreeSet::new();
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut cursor = Value::Null;
        let mut cursors = BTreeSet::new();
        for _ in 0..100 {
            if Instant::now() >= deadline {
                return Err(io::Error::other("Model discovery exceeded its deadline"));
            }
            let response = match self.provider {
                Provider::Codex => self.rpc(
                    "model/list",
                    json!({"limit":100,"includeHidden":true,"cursor":cursor}),
                )?,
                Provider::Claude => json!({"models": self.model_catalog}),
                Provider::Pi => self.rpc("get_available_models", json!({}))?,
            };
            let key = if self.provider == Provider::Codex {
                "data"
            } else {
                "models"
            };
            let entries = response[key].as_array().ok_or_else(|| io::Error::other("Harness did not return a model catalog; upgrade to a version supporting model discovery"))?;
            for entry in entries {
                let (id, route, label) = match self.provider {
                    Provider::Codex => (
                        required(entry, "model")?,
                        None,
                        entry["displayName"].as_str(),
                    ),
                    // Resolve moving aliases before presenting a pinnable choice.
                    Provider::Claude => (
                        required(entry, "resolvedModel").or_else(|_| required(entry, "value"))?,
                        None,
                        entry["displayName"].as_str(),
                    ),
                    Provider::Pi => (
                        required(entry, "id")?,
                        Some(required(entry, "provider")?),
                        entry["name"].as_str(),
                    ),
                };
                let selection = ModelSelection { id, route };
                if selections.insert(selection.clone()) {
                    choices.push(ModelChoice {
                        label: label.unwrap_or(&selection.id).into(),
                        selection,
                        configured: false,
                        advertised: true,
                    });
                }
            }
            if self.provider != Provider::Codex || response["nextCursor"].is_null() {
                return Ok(choices);
            }
            let next = required(&response, "nextCursor")?;
            if !cursors.insert(next.clone()) {
                return Err(io::Error::other(
                    "Model catalog repeated a pagination cursor",
                ));
            }
            cursor = json!(next);
        }
        Err(io::Error::other("Model catalog exceeded 100 pages"))
    }
}
