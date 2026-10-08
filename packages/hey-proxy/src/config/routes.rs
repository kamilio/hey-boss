//! File routing contract. Forwarding and quota policy consume these snapshots separately.
use super::*;
use std::sync::Arc;

pub type Overrides = BTreeMap<String, BTreeMap<String, Alias>>;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_shape: Option<ApiShape>,
    pub legs: Vec<Leg>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Leg {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, rename = "override", skip_serializing_if = "Option::is_none")]
    pub override_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    IncludedSubscription,
    PayPerToken,
}

/// Safe diagnostic metadata, never credentials or credential paths.
#[derive(Debug, Serialize)]
pub struct ResolvedLeg {
    pub config_revision: String,
    pub source_model: String,
    pub upstream_model: String,
    pub provider: String,
    pub implementation: &'static str,
    pub billing_mode: BillingMode,
    pub reasoning: Option<String>,
}

/// Holds the complete immutable config for the lifetime of a request, even if a
/// later attempt selects a different leg after the watcher publishes a new config.
pub struct Plan {
    config: Arc<Config>,
    route: usize,
    effort: Option<String>,
}

impl Plan {
    pub fn len(&self) -> usize {
        self.config.routes[self.route].legs.len()
    }

    /// Select the provider first, then run its explicitly referenced rewrite once.
    /// The returned config has no aliases, routes, overrides or legacy fallbacks;
    /// adapters must send upstream_model and reasoning from the returned metadata.
    pub fn select(&self, index: usize) -> Result<(Config, ResolvedLeg)> {
        let resolved = self.resolve(index)?;
        let account = &self.config.accounts[&resolved.provider];
        Ok((account.apply(&self.config), resolved))
    }

    /// Resolve safe route metadata without copying credentials or selecting a transport.
    pub fn resolve(&self, index: usize) -> Result<ResolvedLeg> {
        self.config.routes[self.route].resolve(&self.config, index, self.effort.as_deref())
    }
}

impl Route {
    /// Shared by forwarding and local diagnostics; never opens credential stores.
    pub(crate) fn resolve(
        &self,
        config: &Config,
        index: usize,
        effort: Option<&str>,
    ) -> Result<ResolvedLeg> {
        let leg = self.legs.get(index).context("Unknown route leg")?;
        let account = config
            .accounts
            .get(&leg.provider)
            .context("Unknown route provider")?;
        let model = leg.model.as_deref().unwrap_or(&self.model);
        let rewrite = leg
            .override_name
            .as_ref()
            .map(|name| {
                config
                    .overrides
                    .get(&leg.provider)
                    .and_then(|overrides| overrides.get(name))
                    .context("Unknown provider override")
            })
            .transpose()?;
        let upstream = rewrite
            .and_then(|alias| alias.destination(effort).0)
            .unwrap_or(model);
        let reasoning = rewrite
            .and_then(|alias| alias.reasoning.as_deref())
            .or(effort);
        let resolved = ResolvedLeg {
            config_revision: config.revision.clone(),
            source_model: self.model.clone(),
            upstream_model: upstream.into(),
            provider: leg.provider.clone(),
            implementation: account.implementation(),
            billing_mode: if account.auth() == "subscription" {
                BillingMode::IncludedSubscription
            } else {
                BillingMode::PayPerToken
            },
            reasoning: reasoning.map(str::to_owned),
        };
        Ok(resolved)
    }
}

fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 256
        && !model.chars().any(|c| c.is_whitespace() || c.is_control())
}
fn overlap(a: Option<ApiShape>, b: Option<ApiShape>) -> bool {
    a.is_none() || b.is_none() || a == b
}

impl Config {
    pub(crate) fn route_index(&self, model: &str, path: &str) -> Option<usize> {
        if self.mode == Mode::Client {
            return None;
        }
        self.routes.iter().position(|route| {
            route.model == model
                && route
                    .api_shape
                    .is_none_or(|s| Some(s) == ApiShape::from_route_path(path))
        })
    }

    /// None delegates to the existing legacy alias/fallback path. A matching named
    /// route owns the entire chain; legacy rewrites must never be run afterward.
    pub fn route_plan(
        self: &Arc<Self>,
        model: &str,
        path: &str,
        effort: Option<&str>,
    ) -> Option<Plan> {
        self.route_index(model, path).map(|route| Plan {
            config: self.clone(),
            route,
            effort: effort.map(str::to_owned),
        })
    }

    pub(super) fn validate_routes(&self) -> Result<()> {
        anyhow::ensure!(
            self.routes.len() <= 1024,
            "At most 1024 model routes are supported"
        );
        anyhow::ensure!(
            self.mode != Mode::Client || (self.routes.is_empty() && self.overrides.is_empty()),
            "Client relays cannot own routing policy"
        );
        anyhow::ensure!(
            self.overrides.len() <= 128,
            "Too many override provider scopes"
        );
        for (provider, overrides) in &self.overrides {
            anyhow::ensure!(
                self.accounts.contains_key(provider),
                "Unknown override provider"
            );
            anyhow::ensure!(overrides.len() <= 1024, "Too many provider overrides");
            for (name, alias) in overrides {
                anyhow::ensure!(accounts::valid_name(name), "Invalid override name");
                // Reuse the legacy alias validator, but with no credential projects.
                let check = Config {
                    aliases: vec![alias.clone()],
                    ..Config::default()
                };
                check.validate_routing()?;
                anyhow::ensure!(
                    valid_model(&alias.from)
                        && alias.to.as_deref().is_none_or(valid_model)
                        && alias.reasoning_routes.values().all(|r| valid_model(&r.to)),
                    "Invalid override model"
                );
            }
        }
        for (i, route) in self.routes.iter().enumerate() {
            anyhow::ensure!(valid_model(&route.model), "Invalid route model");
            anyhow::ensure!(
                !self.routes[..i]
                    .iter()
                    .any(|other| other.model == route.model
                        && overlap(other.api_shape, route.api_shape)),
                "Overlapping model routes"
            );
            anyhow::ensure!(
                !route.legs.is_empty() && route.legs.len() <= hey_proxy::fallback::MAX_ATTEMPTS,
                "Routes require 1..16 provider legs"
            );
            let mut seen = HashSet::new();
            for leg in &route.legs {
                anyhow::ensure!(
                    self.accounts.contains_key(&leg.provider),
                    "Unknown route provider"
                );
                let model = leg.model.as_deref().unwrap_or(&route.model);
                anyhow::ensure!(valid_model(model), "Invalid route leg model");
                anyhow::ensure!(
                    seen.insert((&leg.provider, model, &leg.override_name)),
                    "Duplicate route leg"
                );
                if let Some(name) = &leg.override_name {
                    let alias = self
                        .overrides
                        .get(&leg.provider)
                        .and_then(|overrides| overrides.get(name))
                        .context("Unknown provider override")?;
                    anyhow::ensure!(
                        alias.from == model
                            && (alias.api_shape.is_none() || alias.api_shape == route.api_shape),
                        "Override must match its route leg model and API shape"
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
