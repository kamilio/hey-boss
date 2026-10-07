//! Worker preference is a filter over route metadata, never a route rewrite.
use super::*;
use hey_proxy::usage::{MAX_WORKER_CANDIDATES, Runtime, WorkerCandidate};
use std::sync::Arc;

impl Config {
    pub(super) fn validate_workers(&self) -> Result<()> {
        anyhow::ensure!(
            self.worker_candidates.len() <= MAX_WORKER_CANDIDATES,
            "At most 128 worker candidates are supported"
        );
        anyhow::ensure!(
            self.mode != Mode::Client || self.worker_candidates.is_empty(),
            "Client relays cannot own worker preference policy"
        );
        let mut seen = HashSet::new();
        for candidate in &self.worker_candidates {
            anyhow::ensure!(
                self.accounts.contains_key(&candidate.provider),
                "Unknown worker provider"
            );
            anyhow::ensure!(
                self.routes.iter().any(|r| r.model == candidate.model),
                "Unknown worker model route"
            );
            anyhow::ensure!(
                seen.insert((
                    &candidate.provider,
                    &candidate.model,
                    candidate.runtime.as_str()
                )),
                "Duplicate worker candidate"
            );
            // An effort-dependent subscription rewrite cannot prove quota for one upstream model.
            for leg in self
                .routes
                .iter()
                .filter(|r| r.model == candidate.model)
                .flat_map(|r| &r.legs)
                .filter(|l| l.provider == candidate.provider)
            {
                if let Some(alias) = leg
                    .override_name
                    .as_ref()
                    .and_then(|n| self.overrides.get(&leg.provider)?.get(n))
                {
                    anyhow::ensure!(
                        alias.reasoning_routes.is_empty(),
                        "Worker candidates require an effort-independent subscription model"
                    );
                }
            }
        }
        Ok(())
    }

    pub(crate) fn worker_target(
        self: &Arc<Self>,
        candidate: &WorkerCandidate,
    ) -> Option<routes::ResolvedLeg> {
        let account = self.accounts.get(&candidate.provider)?;
        if account.auth() != "subscription" {
            return None;
        }
        let implementation = account.implementation();
        if !matches!(
            (candidate.runtime, implementation),
            (Runtime::Codex, "codex")
                | (Runtime::Claude, "claude")
                | (Runtime::Pi, "codex" | "claude")
        ) {
            return None;
        }
        let path = if implementation == "claude" {
            "/v1/messages"
        } else {
            "/v1/responses"
        };
        let plan = self.route_plan(&candidate.model, path, None)?;
        let mut matching = (0..plan.len())
            .filter_map(|i| plan.resolve(i).ok())
            .filter(|leg| leg.provider == candidate.provider);
        let leg = matching.next()?;
        // Multiple uses of one account for different models cannot be represented by one candidate.
        if matching.next().is_some() {
            return None;
        }
        Some(leg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn config() -> Config {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../../examples/routes.config.json")).unwrap();
        value["worker_candidates"] = json!([
            {"provider":"codex-work","model":"gpt-6.1-sol","runtime":"codex"},
            {"provider":"claude-personal","model":"claude-sonnet-5-5","runtime":"claude"}]);
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn preferences_roundtrip_without_reordering_routes() {
        let c = Arc::new(config());
        c.validate().unwrap();
        let before = serde_json::to_value(&c.routes).unwrap();
        let target = c.worker_target(&c.worker_candidates[0]).unwrap();
        assert_eq!(target.provider, "codex-work");
        assert_eq!(target.upstream_model, "gpt-6.1-sol");
        assert_eq!(before, serde_json::to_value(&c.routes).unwrap());
        let roundtrip: Config = serde_json::from_value(serde_json::to_value(&*c).unwrap()).unwrap();
        assert_eq!(roundtrip.worker_candidates, c.worker_candidates);
        let paid = WorkerCandidate {
            provider: "ultima".into(),
            model: "gpt-6-astra".into(),
            runtime: Runtime::Codex,
        };
        assert!(c.worker_target(&paid).is_none());
        let unreachable = WorkerCandidate {
            provider: "codex-work".into(),
            ..paid
        };
        assert!(c.worker_target(&unreachable).is_none());
        let incompatible = WorkerCandidate {
            runtime: Runtime::Claude,
            ..c.worker_candidates[0].clone()
        };
        assert!(c.worker_target(&incompatible).is_none());
    }
    #[test]
    fn worker_policy_validation_is_bounded_and_host_owned() {
        let mut c = config();
        c.worker_candidates.push(c.worker_candidates[0].clone());
        assert!(c.validate_workers().is_err());
        let mut c = config();
        c.worker_candidates[0].provider = "missing".into();
        assert!(c.validate_workers().is_err());
        let mut c = config();
        c.worker_candidates[0].model = "missing".into();
        assert!(c.validate_workers().is_err());
        let mut c = config();
        c.mode = Mode::Client;
        assert!(c.validate_workers().is_err());
    }
}
