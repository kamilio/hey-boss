//! Required-status-check policy is distinct from general observed CI.
use crate::repository::{segment, validate_branch};
use crate::{CiReport, Client, Error, Freshness, Result, SourceError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequiredCheck {
    pub context: String,
    pub app_id: Option<i64>,
    /// satisfied, failure, pending, missing, or unknown.
    pub state: String,
    pub sha: Option<String>,
    pub url: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequiredChecksReport {
    pub repository: String,
    pub pull_number: u64,
    pub head_sha: String,
    pub base_branch: String,
    /// Immutable base tip and test-merge commit used for this conclusion.
    /// Legacy reports lack these selectors and must not be attached as current.
    pub base_sha: Option<String>,
    /// Base commit associated with the PR metadata, distinct from resolved tip.
    pub pr_base_sha: Option<String>,
    pub merge_sha: Option<String>,
    /// satisfied, failure, pending, missing, unknown, or not_required.
    /// This is NOT a claim that the PR can be merged.
    pub state: String,
    pub strict: bool,
    pub up_to_date: Option<bool>,
    pub checks: Vec<RequiredCheck>,
    pub rules: Vec<Value>,
    pub errors: Vec<SourceError>,
    pub cursor: String,
}

impl Client {
    pub async fn required_checks_for_pr(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<RequiredChecksReport> {
        crate::client::validate_repository(repository)?;
        crate::client::INTERACTIVE_READ
            .scope(
                self.policy_priority(repository, number),
                crate::entity::scope(async {
                    tokio::time::timeout(
                        self.report_timeout(),
                        self.collect_required_checks(repository, number, freshness),
                    )
                    .await
                    .map_err(|_| Error::Deadline)?
                }),
            )
            .await
    }
    // These are private error records, never successful REST responses. The
    // credential-scoped cache prevents N PRs sharing a base from repeating the
    // same inaccessible policy reads. Explicit refresh probes permissions again.
    async fn policy_get(&self, path: &str, freshness: Freshness) -> Result<crate::Response> {
        let key = format!("policy-error://{}/{path}", self.hostname());
        let previous = self.derived(&key).await?;
        if !matches!(freshness, Freshness::Revalidate)
            && let Some(cached) = &previous
            && (matches!(freshness, Freshness::CachedOnly)
                || crate::now_ms().saturating_sub(cached.validated_at_ms) < 300_000)
            && let Some(status) = cached.data["status"]
                .as_u64()
                .filter(|s| matches!(s, 403 | 404))
        {
            return Err(Error::GitHub {
                status: status as u16,
                message: cached.data["message"]
                    .as_str()
                    .unwrap_or("policy inaccessible")
                    .into(),
            });
        }
        match self.get(path, freshness).await {
            Ok(response) => {
                if previous.is_some_and(|p| p.data["status"].is_number()) {
                    self.save_derived(&key, json!({"status":null})).await?;
                }
                Ok(response)
            }
            Err(
                error @ Error::GitHub {
                    status: 403 | 404, ..
                },
            ) => {
                if let Error::GitHub { status, message } = &error {
                    self.save_derived(&key, json!({"status":status,"message":message}))
                        .await?;
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    async fn collect_required_checks(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<RequiredChecksReport> {
        let lock = self.report_lock(&format!(
            "required-checks:{}#{number}",
            repository.to_ascii_lowercase()
        ));
        let repository_spelling = self.pr_repository_spelling(repository, number).await?;
        let repository = repository_spelling.as_str();
        let _guard = lock.lock().await;
        for attempt in 0..2 {
            let freshness = if attempt == 0 {
                freshness
            } else {
                Freshness::Revalidate
            };
            crate::entity::clear();
            let pr = self.pull_request(repository, number, freshness).await?;
            crate::entity::set(self.pr_owner(repository, number, &pr.data).await?);
            let base = pr.data["base"]["ref"]
                .as_str()
                .ok_or_else(|| Error::Invalid("PR lacks base branch".into()))?
                .to_owned();
            validate_branch(&base)?;
            let head = pr.data["head"]["sha"]
                .as_str()
                .ok_or_else(|| Error::Invalid("PR lacks head SHA".into()))?;
            let merge = pr.data["merge_commit_sha"]
                .as_str()
                .filter(|sha| crate::repository::valid_sha(sha));
            let ci = self
                .required_ci_report(repository, head, merge, freshness)
                .await?;
            let mut errors = ci.errors.clone();
            let branch_path = format!("repos/{repository}/branches/{}", segment(&base));
            let branch = self.get(&branch_path, freshness).await;
            if let Ok(branch) = &branch
                && !branch.data["commit"]["sha"]
                    .as_str()
                    .is_some_and(crate::repository::valid_sha)
            {
                errors.push(source("base_branch", "base branch lacks immutable SHA"));
            }
            let protected = branch
                .as_ref()
                .ok()
                .and_then(|r| r.data["protected"].as_bool());
            let base_sha = branch
                .as_ref()
                .ok()
                .and_then(|r| r.data["commit"]["sha"].as_str())
                .filter(|sha| crate::repository::valid_sha(sha))
                .map(str::to_owned);
            if let Err(e) = &branch {
                errors.push(source("base_branch", e));
            }
            let mut requirements = BTreeSet::new();
            let mut strict = false;
            match self
                .policy_get(
                    &format!("{branch_path}/protection/required_status_checks"),
                    freshness,
                )
                .await
            {
                Ok(r) => {
                    if !r.data["strict"].is_boolean()
                        || (!r.data["contexts"].is_array() && !r.data["checks"].is_array())
                    {
                        errors.push(source(
                            "branch_protection",
                            "malformed required-check policy",
                        ));
                    } else {
                        strict = r.data["strict"] == true;
                        let mut named = BTreeSet::new();
                        if let Some(checks) = r.data["checks"].as_array() {
                            for check in checks {
                                match requirement(check, "context", "app_id") {
                                    Ok(pair) => {
                                        named.insert(pair.0.clone());
                                        requirements.insert(pair);
                                    }
                                    Err(e) => errors.push(source("branch_protection", e)),
                                }
                            }
                        }
                        if let Some(contexts) = r.data["contexts"].as_array() {
                            for value in contexts {
                                if let Some(name) = value.as_str() {
                                    if !named.contains(name) {
                                        requirements.insert((name.to_owned(), None));
                                    }
                                } else {
                                    errors.push(source("branch_protection", "invalid context"));
                                }
                            }
                        }
                    }
                }
                // Rulesets can mark a branch protected while legacy protection
                // is absent. Only the explicit GitHub message proves absence;
                // generic/masked 404 and access denial remain unknown.
                Err(Error::GitHub {
                    status: 404,
                    message,
                }) if protected == Some(false) || message == "Branch not protected" => {}
                Err(e) => errors.push(source("branch_protection", e)),
            }
            let rules_path = format!("repos/{repository}/rules/branches/{}", segment(&base));
            let rules_result = match self.policy_get(&rules_path, freshness).await {
                Ok(_) => {
                    self.pages(
                        &rules_path,
                        None,
                        if matches!(freshness, Freshness::CachedOnly) {
                            Freshness::CachedOnly
                        } else {
                            Freshness::MaxAge(Duration::from_secs(1))
                        },
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            let rules = match rules_result {
                Ok(rules) => rules,
                Err(e) => {
                    errors.push(source("rulesets", e));
                    Vec::new()
                }
            };
            for rule in &rules {
                if !rule["type"].is_string() {
                    errors.push(source("rulesets", "rule lacks type"));
                    continue;
                }
                if rule["type"] != "required_status_checks" {
                    continue;
                }
                if !rule["parameters"]["strict_required_status_checks_policy"].is_boolean() {
                    errors.push(source(
                        "rulesets",
                        "required-status-check rule lacks strict policy",
                    ));
                }
                strict |= rule["parameters"]["strict_required_status_checks_policy"] == true;
                if let Some(checks) = rule["parameters"]["required_status_checks"].as_array() {
                    for check in checks {
                        match requirement(check, "context", "integration_id") {
                            Ok(pair) => {
                                requirements.insert(pair);
                            }
                            Err(e) => errors.push(source("rulesets", e)),
                        }
                    }
                } else {
                    errors.push(source(
                        "rulesets",
                        "required-status-check rule lacks checks",
                    ));
                }
            }
            let up_to_date = if strict
                && let Some(base_sha) = branch
                    .as_ref()
                    .ok()
                    .and_then(|r| r.data["commit"]["sha"].as_str())
                    .filter(|s| crate::repository::valid_sha(s))
            {
                match self
                    .get(
                        &format!(
                            "repos/{repository}/compare/{base_sha}...{}?per_page=1",
                            ci.head_sha
                        ),
                        freshness,
                    )
                    .await
                {
                    Ok(r) => match r.data["merge_base_commit"]["sha"]
                        .as_str()
                        .filter(|s| crate::repository::valid_sha(s))
                    {
                        Some(sha) => Some(sha == base_sha),
                        None => {
                            errors.push(source(
                                "base_ancestry",
                                "comparison lacks immutable merge base",
                            ));
                            None
                        }
                    },
                    Err(e) => {
                        errors.push(source("base_ancestry", e));
                        None
                    }
                }
            } else {
                None
            };
            let checks = evaluate(&ci, &requirements);
            let state = if !errors.is_empty() || (strict && up_to_date.is_none()) {
                "unknown"
            } else if checks.iter().any(|c| c.state == "failure") {
                "failure"
            } else if checks.iter().any(|c| c.state == "unknown") {
                "unknown"
            } else if checks.iter().any(|c| c.state == "missing") {
                "missing"
            } else if checks.iter().any(|c| c.state == "pending")
                || (strict && up_to_date == Some(false))
            {
                "pending"
            } else if checks.is_empty() {
                "not_required"
            } else {
                "satisfied"
            };
            let final_pr = if matches!(freshness, Freshness::CachedOnly) {
                pr.clone()
            } else {
                self.pull_request(repository, number, Freshness::Revalidate)
                    .await?
            };
            if pr.data["node_id"] != final_pr.data["node_id"]
                || pr.data["head"]["sha"] != final_pr.data["head"]["sha"]
                || pr.data["base"] != final_pr.data["base"]
                || pr.data["merge_commit_sha"] != final_pr.data["merge_commit_sha"]
            {
                continue;
            }
            if !matches!(freshness, Freshness::CachedOnly) {
                let confirmed = self.get(&branch_path, Freshness::Revalidate).await;
                if let (Ok(before), Ok(after)) = (&branch, &confirmed) {
                    if before.data["commit"]["sha"] != after.data["commit"]["sha"] {
                        continue;
                    }
                } else if let Err(e) = confirmed {
                    errors.push(source("base_confirmation", e));
                }
            }
            let state = if errors.is_empty() { state } else { "unknown" };
            let mut report = RequiredChecksReport {
                repository: repository.into(),
                pull_number: number,
                head_sha: ci.head_sha,
                base_branch: base,
                base_sha,
                pr_base_sha: pr.data["base"]["sha"]
                    .as_str()
                    .filter(|sha| crate::repository::valid_sha(sha))
                    .map(str::to_owned),
                merge_sha: ci.merge_sha,
                state: state.into(),
                strict,
                up_to_date,
                checks,
                rules,
                errors,
                cursor: String::new(),
            };
            let value = json!({"repository":report.repository,"pull_number":number,"head_sha":report.head_sha,"base_branch":report.base_branch,"base_sha":report.base_sha,"pr_base_sha":report.pr_base_sha,"merge_sha":report.merge_sha,"state":report.state,"strict":strict,"up_to_date":up_to_date,"checks":report.checks,"rules":report.rules,"errors":report.errors});
            if value.to_string().len() > self.collection_limit() {
                return Err(Error::Invalid(
                    "required-check report exceeds collection limit".into(),
                ));
            }
            let suffix = format!("{}/{repository}/{number}", self.hostname());
            let mut observations = vec![(format!("required_checks://{suffix}"), value)];
            if !matches!(freshness, Freshness::CachedOnly) {
                // Publish the confirmed lifecycle/selectors, not a full CI
                // result. Cached policy reads must not overwrite newer metadata.
                let conflicts = match final_pr.data["mergeable"].as_bool() {
                    Some(true) => "clean",
                    Some(false) => "conflicting",
                    None => "unknown",
                };
                observations.push((
                    format!("metadata://{suffix}"),
                    json!({"pull_request":final_pr.data,"conflicts":conflicts}),
                ));
            }
            report.cursor = self.observe_many(&observations).await?;
            if !matches!(freshness, Freshness::CachedOnly) {
                self.publish_individual_pr_status(repository, number, &[])
                    .await?;
            }
            return Ok(report);
        }
        Err(Error::Invalid(
            "PR changed repeatedly during policy collection; retry".into(),
        ))
    }
}
fn source(name: &str, error: impl std::fmt::Display) -> SourceError {
    SourceError {
        source: name.into(),
        message: error.to_string(),
    }
}
fn requirement(value: &Value, context: &str, app: &str) -> Result<(String, Option<i64>)> {
    let name = value[context]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Invalid("required check lacks context".into()))?;
    let id = if value[app].is_null() {
        None
    } else {
        Some(
            value[app]
                .as_i64()
                .ok_or_else(|| Error::Invalid("invalid required check app id".into()))?,
        )
    };
    if id.is_some_and(|i| i < -1) {
        return Err(Error::Invalid("invalid required check app id".into()));
    }
    Ok((name.into(), id.filter(|i| *i != -1)))
}
fn evaluate(ci: &CiReport, requirements: &BTreeSet<(String, Option<i64>)>) -> Vec<RequiredCheck> {
    let mut result = Vec::new();
    for (name, app) in requirements {
        // A merge-commit result takes precedence only when this context exists
        // on that SHA. Never apply an unrelated merge check to a head context.
        let mut by_sha = BTreeMap::<String, Vec<&Value>>::new();
        for check in &ci.check_runs {
            if check["name"] == *name
                && app.is_none_or(|id| check["app"]["id"].as_i64() == Some(id))
            {
                let sha = check["head_sha"]
                    .as_str()
                    .unwrap_or(&ci.head_sha)
                    .to_owned();
                by_sha.entry(sha).or_default().push(check);
            }
        }
        for status in &ci.commit_statuses {
            if status["context"] == *name && app.is_none() {
                let sha = status["observed_sha"]
                    .as_str()
                    .unwrap_or(&ci.head_sha)
                    .to_owned();
                by_sha.entry(sha).or_default().push(status);
            }
        }
        let sha = ci
            .merge_sha
            .as_ref()
            .filter(|sha| by_sha.contains_key(*sha))
            .cloned()
            .unwrap_or_else(|| ci.head_sha.clone());
        let values = by_sha.remove(&sha).unwrap_or_default();
        // A context can be both a commit status and a check: both must pass.
        let mut latest = BTreeMap::<String, &Value>::new();
        for value in values {
            let key = if value.get("context").is_some() {
                "status".into()
            } else {
                format!("check:{}", value["app"]["id"])
            };
            if latest.get(&key).is_none_or(|old| {
                value["id"].as_u64().unwrap_or(0) > old["id"].as_u64().unwrap_or(0)
            }) {
                latest.insert(key, value);
            }
        }
        let mut state = "satisfied";
        let mut url = None;
        if latest.is_empty() {
            state = "missing";
        }
        for value in latest.values() {
            let observed = if value.get("context").is_some() {
                match value["state"].as_str() {
                    Some("success") => "satisfied",
                    Some("failure" | "error") => "failure",
                    Some("pending") => "pending",
                    _ => "unknown",
                }
            } else if value["status"] != "completed" {
                "pending"
            } else {
                match value["conclusion"].as_str() {
                    Some("success" | "skipped" | "neutral") => "satisfied",
                    Some(
                        "failure" | "cancelled" | "timed_out" | "action_required" | "stale"
                        | "startup_failure",
                    ) => "failure",
                    _ => "unknown",
                }
            };
            if rank(observed) > rank(state) {
                state = observed;
            }
            if url.is_none() {
                url = value["details_url"]
                    .as_str()
                    .or_else(|| value["target_url"].as_str())
                    .map(str::to_owned);
            }
        }
        result.push(RequiredCheck {
            context: name.clone(),
            app_id: *app,
            state: state.into(),
            sha: (!latest.is_empty()).then_some(sha),
            url,
        });
    }
    result
}
fn rank(state: &str) -> u8 {
    match state {
        "failure" => 4,
        "unknown" => 3,
        "pending" => 2,
        "missing" => 1,
        _ => 0,
    }
}
