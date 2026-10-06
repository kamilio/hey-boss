//! Authoritative client budgets, keyed by upstream model rather than alias.
use crate::config::Config;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Conservative Pi compatibility defaults, not verified upstream maxima.
pub const DEFAULT_BUDGET: ModelBudget = ModelBudget {
    context_window: 128_000,
    max_tokens: 16_384,
    keep_recent_tokens: 20_000,
    reserve_tokens: 16_384,
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelBudget {
    pub context_window: u64,
    pub max_tokens: u64,
    pub keep_recent_tokens: u64,
    pub reserve_tokens: u64,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keep_recent_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserve_tokens: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRegistry {
    pub defaults: ModelBudget,
    #[serde(default)]
    pub models: BTreeMap<String, BudgetOverride>,
}

impl ModelBudget {
    fn validate(self, name: &str) -> Result<()> {
        let values = [
            self.context_window,
            self.max_tokens,
            self.keep_recent_tokens,
            self.reserve_tokens,
        ];
        if values.iter().any(|v| *v > 9_007_199_254_740_991)
            || self.context_window < 4
            || self.max_tokens == 0
            || self.max_tokens >= self.context_window
            || self.reserve_tokens < self.max_tokens
            || self.keep_recent_tokens > self.context_window / 2
            || self.reserve_tokens > self.context_window / 2
            || self.keep_recent_tokens.saturating_add(self.reserve_tokens) >= self.context_window
        {
            bail!(
                "Invalid model_registry budget for {name}: require positive context/output, output <= reserve <= half context, retention <= half context, and retention + reserve < context (JS-safe integers)"
            );
        }
        Ok(())
    }

    fn intersect(self, other: Self) -> Self {
        Self {
            context_window: self.context_window.min(other.context_window),
            max_tokens: self.max_tokens.min(other.max_tokens),
            keep_recent_tokens: self.keep_recent_tokens.min(other.keep_recent_tokens),
            reserve_tokens: self.reserve_tokens.min(other.reserve_tokens),
        }
    }
}

impl ModelRegistry {
    fn budget(&self, target: &str) -> ModelBudget {
        let target = hey_proxy::fallback::canonical(target);
        let Some(entry) = self.models.get(target) else {
            return self.defaults;
        };
        ModelBudget {
            context_window: entry.context_window.unwrap_or(self.defaults.context_window),
            max_tokens: entry.max_tokens.unwrap_or(self.defaults.max_tokens),
            keep_recent_tokens: entry
                .keep_recent_tokens
                .unwrap_or(self.defaults.keep_recent_tokens),
            reserve_tokens: entry.reserve_tokens.unwrap_or(self.defaults.reserve_tokens),
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.defaults.validate("defaults")?;
        for name in self.models.keys() {
            if name.is_empty()
                || name.starts_with("openai/")
                || name.chars().any(char::is_whitespace)
            {
                bail!(
                    "model_registry keys must be upstream model names (omit the optional openai/ prefix)"
                );
            }
            self.budget(name).validate(name)?;
        }
        Ok(())
    }

    /// Routing rewrites aliases once. Fallbacks are already upstream names and
    /// must not be rewritten as aliases again. Include every reasoning branch.
    pub fn resolve(&self, config: &Config, id: &str) -> ModelBudget {
        let mut pending = if let Some(alias) = config.alias_for(id, "/v1/responses") {
            let mut targets = vec![alias.to.as_deref().unwrap_or(&alias.from)];
            targets.extend(
                alias
                    .reasoning_routes
                    .values()
                    .map(|route| route.to.as_str()),
            );
            targets
        } else {
            vec![id]
        };
        let mut seen = BTreeSet::new();
        let mut resolved: Option<ModelBudget> = None;
        while let Some(target) = pending.pop() {
            if !seen.insert(hey_proxy::fallback::canonical(target)) {
                continue;
            }
            let budget = self.budget(target);
            resolved = Some(resolved.map_or(budget, |prior| prior.intersect(budget)));
            pending.extend(
                hey_proxy::fallback::targets(&config.fallbacks, target)
                    .iter()
                    .map(String::as_str),
            );
        }
        resolved.unwrap_or(self.defaults)
    }

    pub fn configured_max_tokens(&self, config: &Config, id: &str) -> Option<u64> {
        let mut pending = if let Some(alias) = config.alias_for(id, "/v1/responses") {
            let mut targets = vec![alias.to.as_deref().unwrap_or(&alias.from)];
            targets.extend(
                alias
                    .reasoning_routes
                    .values()
                    .map(|route| route.to.as_str()),
            );
            targets
        } else {
            vec![id]
        };
        let mut seen = BTreeSet::new();
        let mut has_explicit = false;
        while let Some(target) = pending.pop() {
            let canonical = hey_proxy::fallback::canonical(target);
            if !seen.insert(canonical) {
                continue;
            }
            if self
                .models
                .get(canonical)
                .and_then(|e| e.max_tokens)
                .is_some()
            {
                has_explicit = true;
            }
            pending.extend(
                hey_proxy::fallback::targets(&config.fallbacks, target)
                    .iter()
                    .map(String::as_str),
            );
        }
        has_explicit.then(|| self.resolve(config, id).max_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_roundtrip_preserves_registry_and_partial_entries_follow_defaults() {
        let value = json!({
            "listen":"127.0.0.1:8080",
            "aliases":[{"from":"alias", "to":"openai/backend"}],
            "model_registry":{
                "defaults":DEFAULT_BUDGET,
                "models":{"backend":{"context_window":256000}}
            }
        });
        let config: Config = serde_json::from_value(value.clone()).unwrap();
        config.validate().unwrap();
        let saved = serde_json::to_value(&config).unwrap();
        assert_eq!(saved["model_registry"], value["model_registry"]);
        let mut restored: Config = serde_json::from_value(saved).unwrap();
        let registry = restored.model_registry.as_mut().unwrap();
        registry.defaults.max_tokens = 8192;
        registry.defaults.reserve_tokens = 10000;
        registry.defaults.keep_recent_tokens = 15000;
        registry.validate().unwrap();
        let resolved = restored
            .model_registry
            .as_ref()
            .unwrap()
            .resolve(&restored, "alias");
        assert_eq!(resolved.context_window, 256000);
        assert_eq!(resolved.max_tokens, 8192);
        assert_eq!(resolved.keep_recent_tokens, 15000);
        assert_eq!(resolved.reserve_tokens, 10000);
    }
}
