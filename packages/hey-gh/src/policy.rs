//! Required-status-check policy is distinct from general observed CI.
use crate::repository::{segment, validate_branch};
use crate::{CiReport, Client, Error, Freshness, Result, SourceError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
mod branch;
mod rules;
mod selectors;
mod timings;
use timings::{Phase, Timings};
#[cfg(test)]
mod timing_tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequiredCheck {
    pub context: String,
    pub app_id: Option<i64>,
    /// satisfied, failure, pending, missing, or unknown.
    pub state: String,
    pub sha: Option<String>,
    pub url: Option<String>,
    /// Stable identity of the current failed result(s), including rerun IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_key: Option<String>,
}

/// Policy provenance is independent of the PR's immediate diff base.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyIdentity {
    pub branch: String,
    pub stack: Option<Value>,
}

pub(crate) fn policy_identity(pr: &Value) -> Result<PolicyIdentity> {
    let base = pr["base"]["ref"]
        .as_str()
        .ok_or_else(|| Error::Invalid("PR lacks base branch".into()))?;
    validate_branch(base)?;
    let Some(stack) = pr.get("stack").filter(|v| !v.is_null()) else {
        return Ok(PolicyIdentity {
            branch: base.into(),
            stack: None,
        });
    };
    let invalid = || Error::Invalid("PR advertises incomplete native stack metadata".into());
    let branch = stack["base"]["ref"].as_str().ok_or_else(invalid)?;
    validate_branch(branch)?;
    if !["id", "number", "position", "size"]
        .iter()
        .all(|key| stack[*key].as_u64().is_some_and(|v| v > 0))
        || stack["position"].as_u64() > stack["size"].as_u64()
        || !stack["base"]["sha"]
            .as_str()
            .is_some_and(crate::repository::valid_sha)
    {
        return Err(invalid());
    }
    Ok(PolicyIdentity {
        branch: branch.into(),
        stack: Some(
            json!({"id":stack["id"],"number":stack["number"],"position":stack["position"],"size":stack["size"],"base":{"ref":branch,"sha":stack["base"]["sha"]}}),
        ),
    })
}

pub(crate) fn identity_matches(pr: &Value, identity: Option<&PolicyIdentity>) -> bool {
    match identity {
        Some(identity) => policy_identity(pr).is_ok_and(|current| current == *identity),
        // Legacy reports are valid only without advertised native membership.
        None => pr["stack"].is_null(),
    }
}

fn classic_checks_disabled(branch: &Value) -> bool {
    let protection = &branch["protection"];
    let checks = &protection["required_status_checks"];
    // `protected` includes rulesets. Only this explicit, internally consistent
    // classic-policy absence can replace the otherwise repeated 404 probe.
    protection["enabled"] == false
        && checks["enforcement_level"] == "off"
        && checks["contexts"].as_array().is_some_and(Vec::is_empty)
        && checks["checks"].as_array().is_some_and(Vec::is_empty)
        && (checks["strict"].is_null() || checks["strict"] == false)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequiredChecksReport {
    pub repository: String,
    pub pull_number: u64,
    pub head_sha: String,
    pub base_branch: String,
    #[serde(default)]
    pub policy_identity: Option<PolicyIdentity>,
    /// Resolved tip of the effective policy branch (the trunk for native stacks).
    #[serde(default)]
    pub policy_sha: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_validation_at_ms: Option<u64>,
    #[serde(default)]
    pub validations: Vec<crate::ResourceValidation>,
}

impl Client {
    // CI may already know this personal payload's collection selectors changed.
    // Use that cached observation only to schedule a personal refresh, never as
    // policy evidence or a substitute for personal metadata/confirmation.
    async fn policy_seed_superseded(
        &self,
        repository: &str,
        number: u64,
        seed: &crate::Response,
    ) -> Result<bool> {
        if !self.ci_uses_installation(repository) {
            return Ok(false);
        }
        let Some(latest) = self.peek_ci_pull_request(repository, number).await? else {
            return Ok(false);
        };
        Ok(latest.validated_at_ms > seed.validated_at_ms
            && latest.validated_at_ms <= crate::now_ms()
            && ([
                "/node_id",
                "/number",
                "/head/sha",
                "/base/sha",
                "/base/ref",
                "/base/repo/id",
                "/base/repo/node_id",
                "/base/repo/full_name",
                "/merge_commit_sha",
            ]
            .iter()
            .any(|field| latest.data.pointer(field) != seed.data.pointer(field))
                || policy_identity(&latest.data).ok() != policy_identity(&seed.data).ok()))
    }

    // Admission only: avoid an independent policy rotation competing to hydrate
    // cold/stale CI. The actual policy read still validates every source and
    // final selector normally. This probe never dispatches GitHub requests.
    pub(crate) async fn policy_ci_cached(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<bool> {
        let Freshness::MaxAge(age) = freshness else {
            return Ok(true);
        };
        // A small completion record lets cold candidates yield without loading
        // full PR/CI payloads. Its wrapper clock is only a scheduling hint, not
        // source evidence; identity and freshness are still checked below.
        let completion = self
            .peek_derived(&format!(
                "account-status-validated:ci:{}/{number}",
                repository.to_ascii_lowercase()
            ))
            .await?;
        if completion.is_none_or(|record| {
            record.validated_at_ms == 0
                || !crate::now_ms()
                    .checked_sub(record.validated_at_ms)
                    .is_some_and(|elapsed| (elapsed as u128) < age.as_millis())
        }) {
            return Ok(false);
        }
        crate::report::VALIDATIONS
            .scope(
                std::cell::RefCell::new(Vec::new()),
                crate::report::ci_discovery_scope(
                    repository,
                    number,
                    crate::entity::scope(async {
                        let Some(pr) = self.peek_ci_pull_request(repository, number).await? else {
                            return Ok(false);
                        };
                        let Some(head) = pr.data["head"]["sha"]
                            .as_str()
                            .filter(|sha| crate::repository::valid_sha(sha))
                        else {
                            return Ok(false);
                        };
                        let merge = pr.data["merge_commit_sha"]
                            .as_str()
                            .filter(|sha| crate::repository::valid_sha(sha));
                        let cached = self
                            .stored_pr_snapshot(
                                &format!("ci://{}/{repository}/{number}", self.hostname()),
                                repository,
                                pr.data["node_id"].as_str(),
                            )
                            .await?;
                        if cached.is_none_or(|ci| {
                            ci["head_sha"] != head
                                || ci["merge_sha"].as_str() != merge
                                || !ci["errors"].as_array().is_some_and(Vec::is_empty)
                        }) {
                            return Ok(false);
                        }
                        crate::entity::set(self.pr_owner(repository, number, &pr.data).await?);
                        // Use the same freshness/version proofs as the pending
                        // policy read. Offline reports intentionally retain old
                        // REST clocks too, even when a fresh version proves that
                        // payload unchanged; those clocks cannot gate admission.
                        let ci = crate::client::CACHE_PROBE
                            .scope(
                                (),
                                self.required_ci_report(repository, head, merge, freshness),
                            )
                            .await?;
                        Ok(ci.errors.is_empty()
                            && crate::report::VALIDATIONS.with(|records| {
                                let records = records.borrow();
                                !records.is_empty()
                                    && records.iter().all(|record| {
                                        record.validated_at_ms > 0
                                            && crate::now_ms()
                                                .checked_sub(record.validated_at_ms)
                                                .is_some_and(|age_ms| {
                                                    (age_ms as u128) < age.as_millis()
                                                })
                                    })
                            }))
                    }),
                ),
            )
            .await
    }

    async fn initial_policy_pr(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<crate::Response> {
        if number > 0
            && let Freshness::MaxAge(age) = freshness
            && !age.is_zero()
        {
            match self
                .peek_get(&format!("repos/{repository}/pulls/{number}"))
                .await
            {
                Ok(cached)
                    if cached.data["node_id"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty())
                        && cached.data["head"]["sha"]
                            .as_str()
                            .is_some_and(crate::repository::valid_sha)
                        && policy_identity(&cached.data).is_ok() =>
                {
                    if self
                        .policy_seed_superseded(repository, number, &cached)
                        .await?
                    {
                        return self
                            .pull_request(repository, number, Freshness::Revalidate)
                            .await;
                    }
                    if cached.data["stack"].is_null()
                        && cached.data["mergeable"].as_bool() != Some(true)
                        && !selectors::merged_seed(&cached.data, repository, number)
                        && cached.data["merge_commit_sha"]
                            .as_str()
                            .is_some_and(crate::repository::valid_sha)
                    {
                        // An uncertain merge can retain an obsolete merge SHA
                        // and cannot use GraphQL confirmation. Refresh before
                        // collecting that merge, under the final freshness bound.
                        // Clean/head-only seeds can still collect while peers
                        // refresh metadata; native stacks keep their final check.
                        return self
                            .pull_request(
                                repository,
                                number,
                                Freshness::MaxAge(age.min(Duration::from_secs(15))),
                            )
                            .await;
                    }
                    // This is only a collection seed, never fresh evidence.
                    // Final PR validation still enforces the caller's age and
                    // retries if node, head, base, merge or stack changed.
                    return Ok(cached);
                }
                Ok(_) | Err(Error::CacheMiss) => {}
                Err(error) => return Err(error),
            }
        }
        self.pull_request(repository, number, freshness).await
    }

    pub async fn required_checks_for_pr(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<RequiredChecksReport> {
        crate::client::validate_repository(repository)?;
        let mut timings = Timings::new(repository, number);
        let result = crate::client::INTERACTIVE_READ
            .scope(
                self.policy_priority(repository, number),
                crate::report::VALIDATIONS.scope(
                    std::cell::RefCell::new(Vec::new()),
                    crate::report::ci_discovery_scope(
                        repository,
                        number,
                        crate::entity::scope(async {
                            tokio::time::timeout(
                                self.report_timeout(),
                                self.collect_required_checks(
                                    repository,
                                    number,
                                    freshness,
                                    &mut timings,
                                ),
                            )
                            .await
                            .map_err(|_| Error::Deadline)?
                        }),
                    ),
                ),
            )
            .await;
        timings.finish(&result);
        result
    }
    // These are private error records, never successful REST responses. The
    // credential-scoped cache prevents N PRs sharing a base from repeating the
    // same inaccessible policy reads. Explicit refresh probes permissions again.
    async fn policy_get(&self, path: &str, freshness: Freshness) -> Result<crate::Response> {
        let key = format!("policy-error://{}/{path}", self.hostname());
        let previous = self.peek_derived(&key).await?;
        let max_age = match freshness {
            Freshness::MaxAge(age) => age.as_millis().min(90_000) as u64,
            _ => 90_000,
        };
        if !matches!(freshness, Freshness::Revalidate)
            && let Some(cached) = &previous
            && (matches!(freshness, Freshness::CachedOnly)
                || crate::now_ms().saturating_sub(cached.validated_at_ms) < max_age)
            && let Some(status) = cached.data["status"]
                .as_u64()
                .filter(|s| matches!(s, 403 | 404))
        {
            crate::report::record_validation(&key, cached);
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
                    if let Some(cached) = self.peek_derived(&key).await? {
                        crate::report::record_validation(&key, &cached);
                    }
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
        timings: &mut Timings,
    ) -> Result<RequiredChecksReport> {
        let lock = self.report_lock(&format!(
            "required-checks:{}#{number}",
            repository.to_ascii_lowercase()
        ));
        let repository_spelling = self.pr_repository_spelling(repository, number).await?;
        let repository = repository_spelling.as_str();
        timings.enter(Phase::Lock);
        let _guard = lock.lock().await;
        let mut retry_seed = None;
        for attempt in 0..2 {
            timings.enter(Phase::Seed);
            let freshness = if attempt == 0 {
                freshness
            } else {
                Freshness::Revalidate
            };
            crate::entity::clear();
            let pr = match retry_seed.take() {
                Some(confirmed) => confirmed,
                None => {
                    self.initial_policy_pr(repository, number, freshness)
                        .await?
                }
            };
            crate::entity::set(self.pr_owner(repository, number, &pr.data).await?);
            let base = pr.data["base"]["ref"]
                .as_str()
                .ok_or_else(|| Error::Invalid("PR lacks base branch".into()))?
                .to_owned();
            validate_branch(&base)?;
            let identity = policy_identity(&pr.data)?;
            let head = pr.data["head"]["sha"]
                .as_str()
                .filter(|sha| crate::repository::valid_sha(sha))
                .ok_or_else(|| Error::Invalid("PR lacks immutable head SHA".into()))?;
            let merge = pr.data["merge_commit_sha"]
                .as_str()
                .filter(|sha| crate::repository::valid_sha(sha));
            let branch_path = format!("repos/{repository}/branches/{}", segment(&base));
            let policy_path = format!("repos/{repository}/branches/{}", segment(&identity.branch));
            let protection_path = format!("{policy_path}/protection/required_status_checks");
            let rules_path = format!(
                "repos/{repository}/rules/branches/{}",
                segment(&identity.branch)
            );
            timings.enter(Phase::Policy);
            let ((branch, protection_res), rules_first_res) = tokio::join!(
                async {
                    // Keep the optional selector's future out of the nested
                    // account collector's stack frame.
                    let branch =
                        Box::pin(self.policy_branch(repository, &identity.branch, freshness)).await;
                    let protection = if branch
                        .as_ref()
                        .is_ok_and(|r| classic_checks_disabled(&r.data))
                    {
                        None
                    } else {
                        Some(self.policy_get(&protection_path, freshness).await)
                    };
                    (branch, protection)
                },
                Box::pin(self.policy_rules(repository, &identity.branch, freshness)),
            );
            let mut errors = Vec::new();
            let direct_branch = if identity.branch == base {
                branch.clone()
            } else {
                self.get(&branch_path, freshness).await
            };
            if let Ok(branch) = &branch
                && !branch.data["commit"]["sha"]
                    .as_str()
                    .is_some_and(crate::repository::valid_sha)
            {
                errors.push(source("policy_branch", "policy branch lacks immutable SHA"));
            }
            let protected = branch
                .as_ref()
                .ok()
                .and_then(|r| r.data["protected"].as_bool());
            let policy_sha = branch
                .as_ref()
                .ok()
                .and_then(|r| r.data["commit"]["sha"].as_str())
                .filter(|sha| crate::repository::valid_sha(sha))
                .map(str::to_owned);
            if let Err(e) = &branch {
                errors.push(source("policy_branch", e));
            }
            let base_sha = direct_branch
                .as_ref()
                .ok()
                .and_then(|r| r.data["commit"]["sha"].as_str())
                .filter(|sha| crate::repository::valid_sha(sha))
                .map(str::to_owned);
            if base_sha.is_none() {
                errors.push(source("base_branch", "diff base tip unavailable"));
            }
            let mut requirements = BTreeSet::new();
            let mut strict = false;
            match protection_res {
                None => {}
                Some(Ok(r)) => {
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
                // is absent. Without explicit disabled metadata, generic/masked
                // 404 and access denial on a protected branch remain unknown.
                Some(Err(Error::GitHub {
                    status: 404,
                    message,
                })) if protected == Some(false) || message == "Branch not protected" => {}
                Some(Err(e)) => errors.push(source("branch_protection", e)),
            }
            let rules_result = match rules_first_res {
                Ok(first) if first.link.is_none() && first.data.is_array() => {
                    Ok(first.data.as_array().cloned().unwrap_or_default())
                }
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
            // A definitive empty policy does not depend on CI. In particular,
            // unavailable optional checks must not delay a not_required result.
            // Uncertain policy still collects and retains its source errors.
            timings.enter(Phase::Ci);
            let checks = if requirements.is_empty() && errors.is_empty() {
                Vec::new()
            } else {
                let ci = self
                    .required_ci_report(repository, head, merge, freshness)
                    .await?;
                errors.extend(ci.errors.iter().cloned());
                evaluate(&ci, &requirements)
            };
            timings.enter(Phase::Ancestry);
            let up_to_date = if strict
                && let Some(base_sha) = branch
                    .as_ref()
                    .ok()
                    .and_then(|r| r.data["commit"]["sha"].as_str())
                    .filter(|s| crate::repository::valid_sha(s))
            {
                match self
                    .get(
                        &format!("repos/{repository}/compare/{base_sha}...{head}?per_page=1"),
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
            timings.enter(Phase::Confirmation);
            let (final_rest_pr, confirmed_opt) = if matches!(freshness, Freshness::CachedOnly) {
                (None, None)
            } else {
                // Other readers may have refreshed these selectors while CI
                // and policy were collected. Bound the newest cached evidence,
                // not the initial copies; compare identities below either way.
                // Native stack membership still requires an explicit check.
                let pr_policy = match freshness {
                    Freshness::MaxAge(age) if identity.stack.is_none() => {
                        Freshness::MaxAge(age.min(Duration::from_secs(15)))
                    }
                    _ => Freshness::Revalidate,
                };
                let branch_policy = match freshness {
                    Freshness::MaxAge(age) => Freshness::MaxAge(age.min(Duration::from_secs(30))),
                    _ => Freshness::Revalidate,
                };
                // Only the final selectors get completion priority. Collection
                // still queues normally, and the scheduler alternates these
                // confirmations with other work under the same quota limits.
                let (pr_res, branch_res) = crate::client::COMPLETION_VALIDATION
                    .scope((), async {
                        tokio::join!(
                            self.confirm_policy_pr(repository, number, &pr, pr_policy),
                            Box::pin(self.policy_branch(
                                repository,
                                &identity.branch,
                                branch_policy
                            )),
                        )
                    })
                    .await;
                (pr_res?, Some(branch_res))
            };
            let final_pr = final_rest_pr.as_ref().unwrap_or(&pr);
            if pr.data["node_id"] != final_pr.data["node_id"]
                || pr.data["head"]["sha"] != final_pr.data["head"]["sha"]
                // Repository counters and pushed_at can change for unrelated
                // PRs. Confirm the base's identity, branch and commit instead.
                || ["ref", "sha"].iter().any(|field| pr.data["base"][field] != final_pr.data["base"][field])
                || ["id", "node_id", "full_name"].iter().any(|field| pr.data["base"]["repo"][field] != final_pr.data["base"]["repo"][field])
                || pr.data["merge_commit_sha"] != final_pr.data["merge_commit_sha"]
                || identity != policy_identity(&final_pr.data)?
            {
                // This confirmation already supplies the new selectors. Use
                // its full REST body to seed the retry instead of immediately
                // fetching it again. CI/policy and final confirmation still
                // revalidate under the new entity identity on the next pass.
                retry_seed = final_rest_pr;
                continue;
            }
            if let Some(confirmed) = confirmed_opt {
                if let (Ok(before), Ok(after)) = (&branch, &confirmed) {
                    if before.data["commit"]["sha"] != after.data["commit"]["sha"]
                        || before.data["protected"] != after.data["protected"]
                        || before.data["protection"] != after.data["protection"]
                    {
                        continue;
                    }
                } else if let Err(e) = confirmed {
                    errors.push(source("base_confirmation", e));
                }
                if identity.branch != base {
                    match crate::client::COMPLETION_VALIDATION
                        .scope((), self.get(&branch_path, Freshness::Revalidate))
                        .await
                    {
                        Ok(after)
                            if after.data["commit"]["sha"].as_str() != base_sha.as_deref() =>
                        {
                            continue;
                        }
                        Err(e) => errors.push(source("base_confirmation", e)),
                        _ => {}
                    }
                }
            }
            timings.enter(Phase::Publication);
            let state = if errors.is_empty() { state } else { "unknown" };
            let validations = crate::report::VALIDATIONS.with(|records| records.borrow().clone());
            let observed_at_ms = crate::now_ms();
            let mut report = RequiredChecksReport {
                repository: repository.into(),
                pull_number: number,
                head_sha: head.into(),
                base_branch: base,
                policy_identity: Some(identity),
                policy_sha,
                base_sha,
                pr_base_sha: pr.data["base"]["sha"]
                    .as_str()
                    .filter(|sha| crate::repository::valid_sha(sha))
                    .map(str::to_owned),
                merge_sha: merge.map(str::to_owned),
                state: state.into(),
                strict,
                up_to_date,
                checks,
                rules,
                errors,
                cursor: String::new(),
                pull_request_state: final_pr.data["state"].as_str().map(str::to_owned),
                observed_at_ms: Some(observed_at_ms),
                oldest_validation_at_ms: validations.iter().map(|r| r.validated_at_ms).min(),
                validations,
            };
            let value = json!({"repository":report.repository,"pull_number":number,"head_sha":report.head_sha,"base_branch":report.base_branch,"base_sha":report.base_sha,"policy_identity":report.policy_identity,"policy_sha":report.policy_sha,"pr_base_sha":report.pr_base_sha,"merge_sha":report.merge_sha,"state":report.state,"strict":strict,"up_to_date":up_to_date,"checks":report.checks,"rules":report.rules,"errors":report.errors});
            if value.to_string().len() > self.collection_limit() {
                return Err(Error::Invalid(
                    "required-check report exceeds collection limit".into(),
                ));
            }
            let suffix = format!("{}/{repository}/{number}", self.hostname());
            let mut observations = vec![(format!("required_checks://{suffix}"), value)];
            if let Some(final_pr) = &final_rest_pr {
                // Publish the confirmed lifecycle/selectors, not a full CI
                // result. Cached policy reads must not overwrite newer metadata.
                // A GraphQL selector check never republishes the seeded body.
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
                timings.enter(Phase::StatusPublication);
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
            failure_key: failure_key(&ci.head_sha, name, *app, &sha, latest.into_values()),
            sha: (state != "missing").then_some(sha),
            url,
        });
    }
    result
}

pub(crate) fn failure_key<'a>(
    head: &str,
    context: &str,
    app: Option<i64>,
    sha: &str,
    results: impl Iterator<Item = &'a Value>,
) -> Option<String> {
    let failures: Vec<_> = results
        .filter(|c| {
            matches!(c["state"].as_str(), Some("failure" | "error"))
                || (c["status"] == "completed"
                    && matches!(
                        c["conclusion"].as_str(),
                        Some(
                            "failure"
                                | "cancelled"
                                | "timed_out"
                                | "action_required"
                                | "stale"
                                | "startup_failure"
                        )
                    ))
        })
        .map(|c| {
            json!([
                c["id"],
                c["name"],
                c["context"],
                c["conclusion"],
                c["state"],
                c["app"]["id"],
                c["started_at"],
                c["completed_at"]
            ])
        })
        .collect();
    (!failures.is_empty())
        .then(|| crate::digest(&json!([head, context, app, sha, failures]).to_string()))
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

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_failure_keys_track_reruns_without_job_or_review_reads() {
        let mut ci: CiReport = serde_json::from_value(json!({
            "head_sha":"head","merge_sha":null,"check_runs":[{"id":1,"name":"test","head_sha":"head","app":{"id":3},"status":"completed","conclusion":"failure"}],
            "commit_statuses":[],"workflow_runs":[],"jobs":[],"summary":{"state":"failure","successful":0,"failed":1,"pending":0,"unknown":0,"skipped":0},"failures":[],"errors":[]
        })).unwrap();
        let requirements = BTreeSet::from([("test".to_owned(), Some(3))]);
        let key = |ci: &CiReport| {
            serde_json::to_value(evaluate(ci, &requirements)).unwrap()[0]["failure_key"].clone()
        };
        let first = key(&ci);
        assert!(first.as_str().is_some_and(|s| !s.is_empty()));
        ci.check_runs[0]["details_url"] = json!("https://github.com/o/r/actions/runs/1");
        assert_eq!(key(&ci), first);
        ci.check_runs[0]["id"] = json!(2);
        assert_ne!(key(&ci), first);
        let second = key(&ci);
        ci.check_runs[0]["started_at"] = json!("2026-09-29T02:00:00Z");
        ci.check_runs[0]["completed_at"] = json!("2026-09-29T02:01:00Z");
        assert_ne!(key(&ci), second, "A provider can rerun the same check ID");
        ci.check_runs.push(json!({"id":3,"name":"test","head_sha":"head","app":{"id":3},"status":"in_progress","conclusion":null}));
        assert!(key(&ci).is_null(), "Superseded failures must disappear");
    }
}
