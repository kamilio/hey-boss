//! Stable wake-up signals from observed CI and the separate required-check policy.
use crate::{Report, RequiredChecksReport};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
mod evidence;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub head: String,
    pub blocking: Vec<String>,
    pub completed: Option<String>,
    pub feedback: Vec<String>,
    pub evidence: Value,
    /// Exact policy inputs for local legacy migration; never a display payload.
    #[serde(skip)]
    pub policy_comparison: Option<PolicyComparison>,
}

/// The watcher supports canonical GitHub pull-request links.
pub fn pull_request_selector(url: &str) -> Option<(String, u64)> {
    let tail = url.strip_prefix("https://github.com/")?;
    let parts: Vec<_> = tail.strip_suffix('/').unwrap_or(tail).split('/').collect();
    if parts.len() != 4
        || parts[2] != "pull"
        || parts[..2].iter().any(|s| {
            s.is_empty()
                || matches!(*s, "." | "..")
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
    {
        return None;
    }
    if parts[3].starts_with('0') || !parts[3].bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let number = parts[3].parse::<u64>().ok().filter(|n| *n > 0)?;
    Some((format!("{}/{}", parts[0], parts[1]), number))
}

fn fingerprint(value: &Value) -> String {
    crate::digest(&value.to_string())
}

fn selected<'a>(rows: impl IntoIterator<Item = &'a Value>, fields: &[&str]) -> Vec<Value> {
    let mut values: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            Value::Object(
                fields
                    .iter()
                    .map(|field| {
                        (
                            (*field).into(),
                            if *field == "app" {
                                json!({"id":row["app"]["id"]})
                            } else {
                                row[*field].clone()
                            },
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    values.sort_by_cached_key(Value::to_string);
    values
}

fn metadata_matches(
    repository: &str,
    number: u64,
    pull_request: &Value,
    policy: &RequiredChecksReport,
) -> bool {
    !policy.head_sha.is_empty()
        && pull_request["head"]["sha"] == policy.head_sha
        && policy.repository.eq_ignore_ascii_case(repository)
        && policy.pull_number == number
        && crate::policy::identity_matches(pull_request, policy.policy_identity.as_ref())
        && pull_request["number"]
            .as_u64()
            .is_none_or(|value| value == number)
        && pull_request["base"]["repo"]["full_name"]
            .as_str()
            .is_none_or(|value| value.eq_ignore_ascii_case(repository))
        && pull_request["base"]["ref"]
            .as_str()
            .is_none_or(|value| value == policy.base_branch)
        && policy
            .pr_base_sha
            .as_deref()
            .zip(pull_request["base"]["sha"].as_str())
            .is_none_or(|(required, observed)| required == observed)
}

/// Select limited policy confirmation only when every watcher selector is
/// explicit and agrees. The caller must enforce its read freshness separately.
pub fn confirmed_metadata<'a>(
    repository: &str,
    number: u64,
    policy: &'a RequiredChecksReport,
) -> Option<&'a crate::PullRequestConfirmation> {
    let proof = policy.pull_request_confirmation.as_ref()?;
    let pr = &proof.selectors;
    let nonempty = |value: &Value| value.as_str().is_some_and(|s| !s.is_empty());
    let sha = |value: &Value| value.as_str().is_some_and(crate::repository::valid_sha);
    (policy.policy_identity.is_some()
        && policy.pull_request_state.as_deref() == Some("open")
        && pr["state"] == "open"
        && pr["merged"] == false
        && pr["mergeable"].is_boolean()
        && nonempty(&pr["node_id"])
        && pr["number"] == number
        && sha(&pr["head"]["sha"])
        && sha(&pr["base"]["sha"])
        && pr["base"]["ref"] == policy.base_branch
        && policy.pr_base_sha.as_deref() == pr["base"]["sha"].as_str()
        && pr["base"]["repo"]["id"].as_u64().is_some_and(|id| id > 0)
        && nonempty(&pr["base"]["repo"]["node_id"])
        && pr["base"]["repo"]["full_name"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(repository))
        && pr.get("stack").is_some()
        && match policy.merge_sha.as_deref() {
            Some(merge) => crate::repository::valid_sha(merge) && pr["merge_commit_sha"] == merge,
            None => pr.get("merge_commit_sha") == Some(&Value::Null),
        }
        && metadata_matches(repository, number, pr, policy))
    .then_some(proof)
}

fn metadata_conflicts(pull_request: &Value) -> &'static str {
    match pull_request["mergeable"].as_bool() {
        Some(false) => "conflicting",
        Some(true) => "clean",
        None => "unknown",
    }
}

fn ci_signals(
    repository: &str,
    number: u64,
    pull_request: &Value,
    ci: &crate::CiReport,
    policy: &RequiredChecksReport,
) -> Observation {
    let sources_match = metadata_matches(repository, number, pull_request, policy)
        && policy.head_sha == ci.head_sha
        && policy.merge_sha == ci.merge_sha;
    let current = pull_request["state"] == "open"
        && sources_match
        && policy.errors.is_empty()
        && ci.errors.is_empty();
    let checks = selected(
        crate::report::latest_checks(&ci.check_runs),
        &[
            "id",
            "name",
            "status",
            "conclusion",
            "head_sha",
            "started_at",
            "completed_at",
            "details_url",
            "app",
        ],
    );
    let statuses = selected(
        &ci.commit_statuses,
        &["id", "context", "state", "target_url"],
    );
    let workflows = selected(
        &ci.workflow_runs,
        &[
            "id",
            "run_attempt",
            "status",
            "conclusion",
            "head_sha",
            "html_url",
        ],
    );
    let mut blocking = Vec::new();
    if sources_match && pull_request["state"] == "open" && pull_request["mergeable"] == false {
        blocking.push(format!("conflict:{}", fingerprint(&json!(ci.head_sha))));
    }
    if current {
        blocking.extend(required_gaps(policy));
        for required in policy.checks.iter().filter(|c| c.state == "failure") {
            let Some(sha) = required.sha.as_deref() else {
                continue;
            };
            let mut latest = std::collections::BTreeMap::<String, &Value>::new();
            for check in ci.check_runs.iter().filter(|c| {
                c["name"] == required.context
                    && c["head_sha"].as_str().unwrap_or(&ci.head_sha) == sha
                    && required
                        .app_id
                        .is_none_or(|id| c["app"]["id"].as_i64() == Some(id))
            }) {
                let key = format!("check:{}", check["app"]["id"]);
                if latest
                    .get(&key)
                    .is_none_or(|old| old["id"].as_u64() < check["id"].as_u64())
                {
                    latest.insert(key, check);
                }
            }
            if required.app_id.is_none() {
                for status in ci.commit_statuses.iter().filter(|s| {
                    s["context"] == required.context
                        && s["observed_sha"].as_str().unwrap_or(&ci.head_sha) == sha
                }) {
                    if latest
                        .get("status")
                        .is_none_or(|old| old["id"].as_u64() < status["id"].as_u64())
                    {
                        latest.insert("status".into(), status);
                    }
                }
            }
            if let Some(key) = crate::policy::failure_key(
                &ci.head_sha,
                &required.context,
                required.app_id,
                sha,
                latest.into_values(),
            ) {
                blocking.push(key);
            }
        }
    }
    blocking.sort();
    blocking.dedup();
    let has_checks = !(checks.is_empty() && statuses.is_empty() && workflows.is_empty());
    let settled = current
        && ci.summary.pending == 0
        && ci.summary.unknown == 0
        && !matches!(policy.state.as_str(), "unknown" | "pending" | "missing")
        && (has_checks || (policy.state == "not_required" && policy.checks.is_empty()))
        && checks.iter().all(|c| c["status"] == "completed")
        && ci.workflow_runs.iter().all(|c| c["status"] == "completed")
        && ci
            .commit_statuses
            .iter()
            .all(|c| matches!(c["state"].as_str(), Some("success" | "failure" | "error")));
    let complete = settled && has_checks;
    let completed = complete.then(|| {
        fingerprint(&json!([
            ci.head_sha,
            selected(
                &checks,
                &[
                    "id",
                    "name",
                    "status",
                    "conclusion",
                    "head_sha",
                    "app",
                    "started_at",
                    "completed_at"
                ]
            ),
            selected(&statuses, &["id", "context", "state"]),
            selected(
                &workflows,
                &["id", "run_attempt", "status", "conclusion", "head_sha"]
            )
        ]))
    });
    let mut observation = Observation {
        policy_comparison: None,
        head: ci.head_sha.clone(),
        blocking,
        completed,
        feedback: Vec::new(),
        evidence: evidence::bounded(
            json!({"repository":repository,"number":number,"head":ci.head_sha,
            "sources_match":sources_match,"conflicts":metadata_conflicts(pull_request),
            "source_heads":{"pull_request":pull_request["head"]["sha"],"ci":ci.head_sha,"required":policy.head_sha},
            "source_merges":{"ci":ci.merge_sha,"required":policy.merge_sha},
            "source_bases":{"pull_request":{"ref":pull_request["base"]["ref"],"sha":pull_request["base"]["sha"]},"required":{"ref":policy.base_branch,"sha":policy.pr_base_sha}},
            "policy_identity":policy.policy_identity,"policy_sha":policy.policy_sha,
            "complete":false,"ci_complete":complete,"ci_settled":settled,"has_checks":has_checks,"checks":checks,"statuses":statuses,"workflows":workflows,
            "required":policy.checks,"required_state":policy.state,"failures":ci.failures,
            "policy_errors":policy.errors,"ci_errors":ci.errors}),
        ),
    };
    if current {
        attach_policy(&mut observation, policy);
    }
    observation
}

fn attach_policy(observation: &mut Observation, policy: &RequiredChecksReport) {
    let legacy = legacy_policy_input(policy);
    observation.evidence["policy_fingerprint"] = json!(policy_fingerprint(policy));
    observation.evidence["legacy_policy_fingerprint"] =
        json!(format!("policy:{}", fingerprint(&legacy)));
    observation.policy_comparison = Some(PolicyComparison { legacy });
}

fn policy_fingerprint(policy: &RequiredChecksReport) -> String {
    // Validation tips establish freshness, not new work. Keep them in source
    // matching and display evidence; compare the effective policy separately.
    let mut identity = policy
        .policy_identity
        .clone()
        .unwrap_or(crate::policy::PolicyIdentity {
            branch: policy.base_branch.clone(),
            stack: None,
        });
    if let Some(stack) = identity.stack.as_mut()
        && let Some(base) = stack["base"].as_object_mut()
    {
        base.remove("sha");
    }
    let mut rules: Vec<Value> = policy.rules.iter().map(|rule| {
        json!({"type":rule["type"],"parameters":canonical_rule_parameters(&rule["parameters"])})
    }).collect();
    rules.sort_by_cached_key(Value::to_string);
    rules.dedup();
    format!(
        "policy:v2:{}",
        fingerprint(&json!([
            policy.head_sha,
            policy.base_branch,
            identity,
            policy.strict,
            rules,
            policy
                .checks
                .iter()
                .map(|check| (&check.context, check.app_id))
                .collect::<std::collections::BTreeSet<_>>()
        ]))
    )
}

fn canonical_rule_parameters(value: &Value) -> Value {
    match value {
        Value::Array(values) => {
            let mut values: Vec<_> = values.iter().map(canonical_rule_parameters).collect();
            values.sort_by_cached_key(Value::to_string);
            values.dedup();
            json!(values)
        }
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), canonical_rule_parameters(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn legacy_policy_input(policy: &RequiredChecksReport) -> Value {
    let mut rules = policy.rules.clone();
    rules.sort_by_cached_key(Value::to_string);
    json!([
        policy.head_sha,
        policy.base_branch,
        policy.base_sha,
        policy.pr_base_sha,
        policy.policy_identity,
        policy.policy_sha,
        policy.strict,
        rules,
        policy
            .checks
            .iter()
            .map(|check| (&check.context, check.app_id))
            .collect::<std::collections::BTreeSet<_>>()
    ])
}

#[cfg(test)]
fn legacy_policy_fingerprint(policy: &RequiredChecksReport) -> String {
    format!("policy:{}", fingerprint(&legacy_policy_input(policy)))
}

/// Retains unbounded policy inputs only for the duration of a local observation.
#[derive(Clone, Debug)]
pub struct PolicyComparison {
    legacy: Value,
}

impl PolicyComparison {
    /// Prove that today's full policy reproduces the saved legacy identity when
    /// only validation tips are restored. A bounded check list is never proof.
    pub fn equivalent_legacy<'a>(&self, previous: &'a Value) -> Option<&'a str> {
        let old = previous["policy_fingerprint"].as_str()?;
        if !old.starts_with("policy:") || old.starts_with("policy:v2:") {
            return None;
        }
        let branch = previous["source_bases"]["required"]["ref"].as_str()?;
        let pr_base = previous["source_bases"]["required"]["sha"].as_str()?;
        let policy_sha = previous["policy_sha"].as_str()?;
        // Older evidence omitted the direct branch tip. It is recoverable only
        // when the direct base and effective policy branch are the same branch.
        let identity = previous.get("policy_identity")?;
        if identity["branch"].as_str() != Some(branch)
            || self.legacy[0] != previous["head"]
            || self.legacy[1] != branch
        {
            return None;
        }
        let mut candidate = self.legacy.clone();
        candidate[2] = json!(policy_sha);
        candidate[3] = json!(pr_base);
        candidate[5] = json!(policy_sha);
        // Native stack base SHA is freshness; membership and ordering are not.
        if let Some(base) = candidate[4]
            .get_mut("stack")
            .and_then(|stack| stack.get_mut("base"))
            .and_then(Value::as_object_mut)
        {
            base.insert("sha".into(), identity["stack"]["base"]["sha"].clone());
        }
        (format!("policy:{}", fingerprint(&candidate)) == old).then_some(old)
    }
}

fn required_gaps(policy: &RequiredChecksReport) -> Vec<String> {
    let mut gaps: Vec<_> = policy
        .checks
        .iter()
        .filter(|check| check.state == "missing")
        .map(|check| {
            format!(
                "required-missing:{}",
                fingerprint(&json!([policy.head_sha, check.context, check.app_id]))
            )
        })
        .collect();
    if policy.strict && policy.up_to_date == Some(false) {
        gaps.push(format!(
            "required-outdated:{}",
            fingerprint(&json!([
                policy.head_sha,
                policy.base_branch,
                policy.policy_sha
            ]))
        ));
    }
    gaps
}

/// Policy collection reads check results and statuses, but never workflow jobs
/// or reviews. Its failure identities therefore wake work before those sources.
pub fn observe_required(policy: &RequiredChecksReport) -> Observation {
    let mut blocking = Vec::new();
    if policy.pull_request_state.as_deref() == Some("open") && policy.errors.is_empty() {
        blocking.extend(required_gaps(policy));
        blocking.extend(
            policy
                .checks
                .iter()
                .filter(|check| check.state == "failure")
                .filter_map(|check| check.failure_key.clone()),
        );
    }
    blocking.sort();
    blocking.dedup();
    let mut observation = Observation {
        policy_comparison: None,
        head: policy.head_sha.clone(),
        blocking,
        completed: None,
        feedback: Vec::new(),
        evidence: evidence::bounded(
            json!({"repository":policy.repository,"number":policy.pull_number,"head":policy.head_sha,
            "complete":false,"required":policy.checks,"required_state":policy.state,"policy_errors":policy.errors,
            "policy_identity":policy.policy_identity,"policy_sha":policy.policy_sha,
            "source_bases":{"required":{"ref":policy.base_branch,"sha":policy.pr_base_sha}},"source_merges":{"required":policy.merge_sha}}),
        ),
    };
    if policy.pull_request_state.as_deref() == Some("open")
        && policy.errors.is_empty()
        && !policy.head_sha.is_empty()
    {
        attach_policy(&mut observation, policy);
    }
    observation
}

/// Conflicts are metadata evidence; optional workflows and review access cannot
/// delay them. The caller must validate timestamps before retaining this result.
pub fn observe_metadata(
    repository: &str,
    number: u64,
    pull_request: &Value,
    policy: &RequiredChecksReport,
) -> Observation {
    let mut observation = observe_required(policy);
    let sources_match = metadata_matches(repository, number, pull_request, policy)
        && pull_request["number"].as_u64() == Some(number)
        && pull_request["base"]["ref"] == policy.base_branch
        && policy
            .pr_base_sha
            .as_deref()
            .is_none_or(|sha| pull_request["base"]["sha"] == sha)
        && pull_request["base"]["repo"]["full_name"]
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(repository));
    observation.evidence["sources_match"] = json!(sources_match);
    observation.evidence["conflicts"] = json!(metadata_conflicts(pull_request));
    if !sources_match || pull_request["state"] != "open" {
        observation.blocking.clear();
    }
    if sources_match && pull_request["state"] == "open" && pull_request["mergeable"] == false {
        observation
            .blocking
            .push(format!("conflict:{}", fingerprint(&json!(policy.head_sha))));
    }
    observation
}

/// Detect required failures without collecting reviews. A completion signal is
/// reserved for a full, current PR snapshot so the agent gets its review findings.
pub fn observe_ci(
    repository: &str,
    number: u64,
    pull_request: &Value,
    ci: &crate::CiReport,
    policy: &RequiredChecksReport,
) -> Observation {
    let mut observation = ci_signals(repository, number, pull_request, ci, policy);
    observation.completed = None;
    observation
}

/// Report times are deliberately excluded from event identities. Reruns and
/// edited reviews change identities; rereading the same evidence does not.
pub fn observe(report: &Report, policy: &RequiredChecksReport) -> Observation {
    let pr = &report.data;
    observe_review_evidence(
        ReviewEvidence {
            repository: &pr.repository,
            number: pr.number,
            pull_request: &pr.pull_request,
            conflicts: &pr.conflicts,
            comments: &pr.comments,
            review_comments: &pr.review_comments,
            reviews: &pr.reviews,
            review_threads: &pr.review_threads,
            review_status: &pr.review_status,
            ci: &pr.ci,
            errors: &pr.errors,
            complete: report.complete,
        },
        policy,
    )
}

/// The watcher consumes the same evidence without requiring unused history.
pub fn observe_review_report(
    report: &crate::ReviewReport,
    policy: &RequiredChecksReport,
) -> Observation {
    let pr = &report.data;
    observe_review_evidence(
        ReviewEvidence {
            repository: &pr.repository,
            number: pr.number,
            pull_request: &pr.pull_request,
            conflicts: &pr.conflicts,
            comments: &pr.comments,
            review_comments: &pr.review_comments,
            reviews: &pr.reviews,
            review_threads: &pr.review_threads,
            review_status: &pr.review_status,
            ci: &pr.ci,
            errors: &pr.errors,
            complete: report.complete,
        },
        policy,
    )
}

struct ReviewEvidence<'a> {
    repository: &'a str,
    number: u64,
    pull_request: &'a Value,
    conflicts: &'a str,
    comments: &'a [Value],
    review_comments: &'a [Value],
    reviews: &'a [Value],
    review_threads: &'a [Value],
    review_status: &'a crate::ReviewStatus,
    ci: &'a crate::CiReport,
    errors: &'a [crate::SourceError],
    complete: bool,
}

fn observe_review_evidence(pr: ReviewEvidence<'_>, policy: &RequiredChecksReport) -> Observation {
    let ci = pr.ci;
    let mut observation = ci_signals(pr.repository, pr.number, pr.pull_request, ci, policy);
    if pr.complete
        && pr.errors.is_empty()
        && policy.errors.is_empty()
        && ci.errors.is_empty()
        && observation.evidence["sources_match"] == true
        && pr.pull_request["state"] == "open"
        && pr.conflicts == "conflicting"
    {
        let key = format!("conflict:{}", fingerprint(&json!(ci.head_sha)));
        if !observation.blocking.contains(&key) {
            observation.blocking.push(key);
        }
    }
    let complete =
        observation.evidence["ci_settled"] == true && pr.complete && pr.errors.is_empty();
    if !complete {
        observation.completed = None;
    }
    let reviews = selected(
        pr.reviews,
        &["id", "state", "body", "commit_id", "html_url"],
    );
    let comments = selected(
        pr.review_comments,
        &["id", "body", "commit_id", "path", "line", "html_url"],
    );
    let mut feedback = Vec::new();
    let by_author = |value: &Value| {
        pr.pull_request["user"]["login"]
            .as_str()
            .is_some_and(|author| {
                value["user"]["login"] == author || value["author"]["login"] == author
            })
    };
    let discussion = selected(
        &pr.comments
            .iter()
            .filter(|c| !by_author(c))
            .cloned()
            .collect::<Vec<_>>(),
        &["id", "body", "html_url"],
    );
    if complete {
        for review in pr.reviews.iter().filter(|r| {
            !by_author(r)
                && (r["state"] == "CHANGES_REQUESTED"
                    || (r["state"] == "COMMENTED"
                        && r["body"].as_str().is_some_and(|s| !s.trim().is_empty())))
        }) {
            feedback.push(fingerprint(&json!([
                "review",
                ci.head_sha,
                review["id"],
                review["state"],
                review["body"]
            ])));
        }
        for comment in &discussion {
            feedback.push(fingerprint(&json!(["comment", ci.head_sha, comment])));
        }
        for thread in pr
            .review_threads
            .iter()
            .filter(|t| t["isResolved"] == false && t["isOutdated"] != true)
        {
            for comment in thread["comments"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| !by_author(c))
            {
                feedback.push(fingerprint(&json!([
                    "thread",
                    ci.head_sha,
                    thread["id"],
                    comment["id"],
                    comment["body"]
                ])));
            }
        }
    }
    feedback.sort();
    feedback.dedup();
    observation.feedback = feedback;
    observation.evidence.as_object_mut().expect("CI evidence is an object").extend(
        json!({"complete":complete,"reviews":reviews.into_iter().rev().collect::<Vec<_>>(),"review_comments":comments.into_iter().rev().collect::<Vec<_>>(),
            "comments":discussion.into_iter().rev().collect::<Vec<_>>(),"unresolved_threads":pr.review_status.unresolved_threads,
            "conflicts":pr.conflicts,"source_errors":pr.errors}).as_object().unwrap().clone()
    );
    observation.evidence = evidence::bounded(observation.evidence);
    observation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_request_links_have_one_canonical_numeric_spelling() {
        for repository in ["../r", "./r", "o/.", "o/.."] {
            assert!(
                pull_request_selector(&format!("https://github.com/{repository}/pull/1")).is_none()
            );
        }
        assert_eq!(
            pull_request_selector("https://github.com/o/r/pull/42/"),
            Some(("o/r".into(), 42))
        );
        for tail in [
            "+1",
            "01",
            "1//",
            "1?query=true",
            "1#fragment",
            "18446744073709551616",
            "0",
        ] {
            assert_eq!(
                pull_request_selector(&format!("https://github.com/o/r/pull/{tail}")),
                None,
                "{tail}"
            );
        }
    }
    fn fixture() -> (Report, RequiredChecksReport) {
        let report = serde_json::from_value(json!({
            "data":{"repository":"o/r","number":1,"pull_request":{"state":"open","head":{"sha":"head"}},
            "conflicts":"clean","comments":[],"review_comments":[],"reviews":[],"timeline":[],"review_events":[],"review_threads":[],
            "review_status":{"requested_reviewers":[],"requested_teams":[],"latest_reviews":[],"approved_by":[],"changes_requested_by":[],"dismissed_reviews":[],"resolved_threads":0,"unresolved_threads":0,"outdated_threads":0},
            "ci":{"head_sha":"head","merge_sha":null,"check_runs":[{"id":1,"name":"test","app":{"id":1},"status":"completed","conclusion":"failure"}],"commit_statuses":[],"workflow_runs":[],"jobs":[],
            "summary":{"state":"failure","successful":0,"failed":1,"pending":1,"skipped":0,"unknown":0},"failures":[],"errors":[]},"errors":[]},
            "complete":true,"observed_at_ms":100,"oldest_validation_at_ms":100,"validations":[]
        })).unwrap();
        let policy = serde_json::from_value(json!({"repository":"o/r","pull_number":1,"head_sha":"head","base_branch":"main","state":"failure","strict":false,"up_to_date":true,"checks":[{"context":"test","app_id":1,"state":"failure","sha":"head","url":"https://github.com/o/r/actions/runs/1"}],"rules":[],"errors":[],"cursor":"unused"})).unwrap();
        (report, policy)
    }

    #[test]
    fn review_report_preserves_watcher_signals_without_timeline_history() {
        for case in [
            "feedback",
            "incomplete",
            "unknown_policy",
            "changed_head",
            "conflict",
        ] {
            let (mut full, mut policy) = fixture();
            full.data.ci.summary.pending = 0;
            full.data.pull_request["user"] = json!({"login":"author"});
            full.data.reviews.push(json!({"id":2,"state":"CHANGES_REQUESTED","body":"Fix race","user":{"login":"reviewer"}}));
            full.data
                .comments
                .push(json!({"id":3,"body":"Still broken","user":{"login":"reviewer"}}));
            full.data.review_threads.push(json!({"id":"thread","isResolved":false,"isOutdated":false,"comments":{"nodes":[{"id":"comment","body":"Unsafe","author":{"login":"reviewer"}}]}}));
            full.data
                .timeline
                .push(json!({"event":"unrelated history"}));
            match case {
                "incomplete" => full.complete = false,
                "unknown_policy" => policy.errors.push(crate::SourceError {
                    source: "policy".into(),
                    message: "unavailable".into(),
                }),
                "changed_head" => policy.head_sha = "previous".into(),
                "conflict" => {
                    full.data.conflicts = "conflicting".into();
                    full.data.pull_request["mergeable"] = json!(false);
                }
                _ => {}
            }
            let expected = observe(&full, &policy);
            let review = crate::ReviewReport::from(full);
            let actual = observe_review_report(&review, &policy);
            assert_eq!(
                serde_json::to_value(&actual).unwrap(),
                serde_json::to_value(&expected).unwrap(),
                "{case}"
            );
            if case == "feedback" {
                assert_eq!(actual.feedback.len(), 3);
            }
            if matches!(case, "incomplete" | "unknown_policy" | "changed_head") {
                assert!(actual.completed.is_none(), "{case}");
                assert!(actual.feedback.is_empty(), "{case}");
            }
        }
    }
    #[test]
    fn metadata_conflicts_do_not_wait_for_optional_ci_or_reviews() {
        let (mut report, mut policy) = fixture();
        policy.pr_base_sha = Some("base-tip".into());
        report.data.pull_request["mergeable"] = json!(false);
        report.data.pull_request["number"] = json!(1);
        report.data.pull_request["base"] =
            json!({"ref":"main","sha":"base-tip","repo":{"full_name":"o/r"}});
        let early = observe_metadata("o/r", 1, &report.data.pull_request, &policy);
        assert!(
            early
                .blocking
                .iter()
                .any(|key| key.starts_with("conflict:"))
        );
        let ci = observe_ci(
            "o/r",
            1,
            &report.data.pull_request,
            &report.data.ci,
            &policy,
        );
        assert_eq!(early.evidence["conflicts"], "conflicting");
        assert!(ci.blocking.iter().any(|key| key.starts_with("conflict:")));
        assert_eq!(ci.evidence["ci_settled"], false);
        assert!(early.completed.is_none());
        assert!(early.feedback.is_empty());
        for field in [
            "head",
            "base",
            "base_sha",
            "missing_base",
            "repository",
            "number",
            "missing_number",
            "closed",
            "unknown",
        ] {
            let mut changed = report.data.pull_request.clone();
            match field {
                "head" => changed["head"]["sha"] = json!("other"),
                "base" => changed["base"]["ref"] = json!("release"),
                "base_sha" => changed["base"]["sha"] = json!("new-tip"),
                "missing_base" => changed["base"]["sha"] = Value::Null,
                "repository" => changed["base"]["repo"]["full_name"] = json!("o/other"),
                "number" => changed["number"] = json!(2),
                "missing_number" => changed["number"] = Value::Null,
                "closed" => changed["state"] = json!("closed"),
                "unknown" => changed["mergeable"] = Value::Null,
                _ => unreachable!(),
            }
            let observed = observe_metadata("o/r", 1, &changed, &policy);
            assert!(
                !observed
                    .blocking
                    .iter()
                    .any(|key| key.starts_with("conflict:")),
                "{field}"
            );
        }
    }
    #[test]
    fn policy_refresh_identity_ignores_validation_tips_and_rule_order() {
        let (_, mut policy) = fixture();
        policy.rules = vec![json!({"type":"required_status_checks","ruleset_id":1,
            "parameters":{"strict_required_status_checks_policy":false,
            "required_status_checks":[{"context":"test","integration_id":1},{"context":"lint","integration_id":2}]}})];
        policy.policy_identity = Some(crate::policy::PolicyIdentity {
            branch: "main".into(),
            stack: Some(
                json!({"id":1,"number":2,"position":1,"size":2,"base":{"ref":"main","sha":"old"}}),
            ),
        });
        let before = policy_fingerprint(&policy);
        policy.base_sha = Some("new-base-tip".into());
        policy.pr_base_sha = Some("new-pr-base".into());
        policy.policy_sha = Some("new-policy-tip".into());
        policy
            .policy_identity
            .as_mut()
            .unwrap()
            .stack
            .as_mut()
            .unwrap()["base"]["sha"] = json!("new");
        policy.rules[0]["ruleset_id"] = json!(2);
        policy.rules[0]["parameters"]["required_status_checks"]
            .as_array_mut()
            .unwrap()
            .reverse();
        policy.observed_at_ms = Some(200);
        assert_eq!(policy_fingerprint(&policy), before);
        for kind in ["head", "branch", "strict", "app", "rule"] {
            let mut changed = policy.clone();
            match kind {
                "head" => changed.head_sha = "new-head".into(),
                "branch" => changed.policy_identity.as_mut().unwrap().branch = "release".into(),
                "strict" => changed.strict = true,
                "app" => changed.checks[0].app_id = Some(2),
                "rule" => changed.rules.push(json!({"type":"pull_request","parameters":{"required_approving_review_count":2}})),
                _ => unreachable!(),
            }
            assert_ne!(policy_fingerprint(&changed), before, "{kind}");
        }
    }
    #[test]
    fn legacy_policy_proof_uses_complete_rules_and_preserves_stack_identity() {
        for stacked in [false, true] {
            let (_, mut policy) = fixture();
            policy.pull_request_state = Some("open".into());
            policy.base_sha = Some("old-main".into());
            policy.policy_sha = Some("old-main".into());
            policy.pr_base_sha = Some("pr-base".into());
            policy.policy_identity = Some(crate::policy::PolicyIdentity {
                branch: "main".into(),
                stack: stacked.then(|| json!({"id":1,"number":2,"position":1,"size":2,"base":{"ref":"main","sha":"pr-base"}})),
            });
            policy.rules = vec![
                json!({"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"test","integration_id":1}]}}),
            ];
            // Display evidence omits most checks; a changed omitted requirement
            // must still fail the exact legacy proof.
            policy.checks = (0..300)
                .map(|n| {
                    let mut check = policy.checks[0].clone();
                    check.context = format!("check-{n}");
                    check
                })
                .collect();
            let mut previous = observe_required(&policy).evidence;
            previous["policy_fingerprint"] = json!(legacy_policy_fingerprint(&policy));
            assert!(previous["omitted"]["required"].as_u64().unwrap() > 0);
            let old = previous["policy_fingerprint"].as_str().unwrap();
            policy.base_sha = Some("advanced-main".into());
            policy.policy_sha = Some("advanced-main".into());
            if stacked {
                policy
                    .policy_identity
                    .as_mut()
                    .unwrap()
                    .stack
                    .as_mut()
                    .unwrap()["base"]["sha"] = json!("advanced-stack-base");
            }
            let observation = observe_required(&policy);
            assert_eq!(
                observation
                    .policy_comparison
                    .as_ref()
                    .unwrap()
                    .equivalent_legacy(&previous),
                Some(old)
            );
            assert!(
                serde_json::to_value(&observation)
                    .unwrap()
                    .get("policy_comparison")
                    .is_none()
            );
            for kind in ["rule", "omitted_check", "head", "branch", "strict", "stack"] {
                let mut changed = policy.clone();
                match kind {
                    "rule" => changed.rules[0]["parameters"]["new_requirement"] = json!(true),
                    "omitted_check" => changed.checks[299].app_id = Some(99),
                    "head" => changed.head_sha = "new-head".into(),
                    "branch" => changed.base_branch = "other".into(),
                    "strict" => changed.strict = !changed.strict,
                    "stack" => {
                        changed.policy_identity.as_mut().unwrap().stack = Some(
                            json!({"id":99,"number":2,"position":2,"size":3,"base":{"ref":"main","sha":"pr-base"}}),
                        )
                    }
                    _ => unreachable!(),
                }
                assert_eq!(
                    observe_required(&changed)
                        .policy_comparison
                        .as_ref()
                        .unwrap()
                        .equivalent_legacy(&previous),
                    None,
                    "{kind}/{stacked}"
                );
            }
        }
    }

    #[test]
    fn ci_only_observation_wakes_before_review_sources_are_read() {
        let (mut report, policy) = fixture();
        report.data.ci.summary.pending = 0;
        let early = observe_ci(
            &report.data.repository,
            report.data.number,
            &report.data.pull_request,
            &report.data.ci,
            &policy,
        );
        let full = observe(&report, &policy);
        assert_eq!(early.blocking, full.blocking);
        assert!(!early.blocking.is_empty());
        assert!(
            early.completed.is_none(),
            "Completion must include the review snapshot"
        );
        assert_eq!(early.evidence["ci_complete"], true);
        assert_eq!(early.evidence["complete"], false);
        assert!(full.completed.is_some());
        report.data.pull_request["head"]["sha"] = json!("new-head");
        let stale = observe_ci(
            &report.data.repository,
            report.data.number,
            &report.data.pull_request,
            &report.data.ci,
            &policy,
        );
        assert!(stale.blocking.is_empty());
        assert_eq!(stale.evidence["ci_complete"], false);
    }

    #[test]
    fn policy_failure_can_wake_without_waiting_for_workflow_job_details() {
        let (report, policy) = fixture();
        let mut data = serde_json::to_value(policy).unwrap();
        data["pull_request_state"] = json!("open");
        data["checks"][0]["failure_key"] =
            json!(observe(&report, &serde_json::from_value(data.clone()).unwrap()).blocking[0]);
        let policy = serde_json::from_value(data.clone()).unwrap();
        assert_eq!(
            observe_required(&policy).blocking,
            observe(&report, &policy).blocking
        );
        assert!(observe_required(&policy).completed.is_none());
        data["pull_request_state"] = json!("closed");
        assert!(
            observe_required(&serde_json::from_value(data.clone()).unwrap())
                .blocking
                .is_empty()
        );
        data["pull_request_state"] = json!("open");
        data["errors"] = json!([{"source":"policy","message":"Permission denied"}]);
        assert!(
            observe_required(&serde_json::from_value(data).unwrap())
                .blocking
                .is_empty()
        );
    }

    #[test]
    fn required_failure_wakes_before_optional_checks_finish() {
        let (r, p) = fixture();
        let observation = observe(&r, &p);
        assert_eq!(observation.blocking.len(), 1);
        assert!(observation.completed.is_none());
    }
    #[test]
    fn optional_failure_waits_for_completion_and_review_changes_wake_again() {
        let (mut r, mut p) = fixture();
        p.checks.clear();
        p.state = "not_required".into();
        assert!(observe(&r, &p).blocking.is_empty());
        assert!(observe(&r, &p).completed.is_none());
        r.data.ci.summary.pending = 0;
        let first = observe(&r, &p).completed.unwrap();
        r.observed_at_ms += 100;
        assert_eq!(observe(&r, &p).completed.as_ref(), Some(&first));
        r.data
            .reviews
            .push(json!({"id":5,"state":"CHANGES_REQUESTED","body":"fix race"}));
        assert_eq!(observe(&r, &p).completed.as_ref(), Some(&first));
        assert_eq!(observe(&r, &p).feedback.len(), 1);
    }
    #[test]
    fn checkless_prs_deliver_reviews_without_an_empty_completion_event() {
        let (mut report, mut policy) = fixture();
        report.data.ci.check_runs.clear();
        report.data.ci.summary.pending = 0;
        report.data.ci.summary.failed = 0;
        policy.checks.clear();
        policy.state = "not_required".into();
        let empty = observe(&report, &policy);
        assert!(empty.blocking.is_empty());
        assert!(empty.completed.is_none());
        assert!(empty.feedback.is_empty());
        assert_eq!(empty.evidence["ci_settled"], true);
        assert_eq!(empty.evidence["ci_complete"], false);
        assert_eq!(empty.evidence["has_checks"], false);
        assert_eq!(empty.evidence["complete"], true);
        report
            .data
            .reviews
            .push(json!({"id":5,"state":"CHANGES_REQUESTED","body":"fix race"}));
        let reviewed = observe(&report, &policy);
        assert!(reviewed.completed.is_none());
        assert_eq!(reviewed.feedback.len(), 1);
        report.observed_at_ms += 100;
        assert_eq!(observe(&report, &policy).feedback, reviewed.feedback);
        report.complete = false;
        assert!(observe(&report, &policy).feedback.is_empty());
        report.complete = true;
        for state in ["unknown", "pending", "missing", "failure", "satisfied"] {
            policy.state = state.into();
            assert!(observe(&report, &policy).feedback.is_empty(), "{state}");
        }
    }
    #[test]
    fn unknown_policy_stale_heads_and_incomplete_details_never_complete() {
        let (mut r, mut p) = fixture();
        r.data.ci.summary.pending = 0;
        p.head_sha = "old".into();
        assert!(observe(&r, &p).blocking.is_empty());
        assert!(observe(&r, &p).completed.is_none());
        p.head_sha = "head".into();
        r.complete = false;
        assert!(observe(&r, &p).completed.is_none());
        // Independent review hydration cannot hide a validated required failure.
        assert_eq!(observe(&r, &p).blocking.len(), 1);
        p.errors.push(crate::SourceError {
            source: "policy".into(),
            message: "denied".into(),
        });
        assert!(observe(&r, &p).blocking.is_empty());
    }
    #[test]
    fn mixed_heads_are_explicit_in_evidence_and_never_generate_signals() {
        let (mut report, mut policy) = fixture();
        report.data.ci.summary.pending = 0;
        policy.head_sha = "previous-head".into();
        let observed = observe(&report, &policy);
        assert_eq!(observed.evidence["sources_match"], false);
        assert_eq!(
            observed.evidence["source_heads"]["required"],
            "previous-head"
        );
        assert_eq!(observed.evidence["source_heads"]["ci"], "head");
        assert!(observed.blocking.is_empty());
        assert!(observed.completed.is_none());
        policy.head_sha = "head".into();
        policy.merge_sha = Some("previous-merge".into());
        assert_eq!(observe(&report, &policy).evidence["sources_match"], false);
        policy.merge_sha = None;
        report.data.pull_request["head"]["sha"] = json!("next-head");
        assert_eq!(observe(&report, &policy).evidence["sources_match"], false);
        report.data.pull_request["head"]["sha"] = json!("head");
        assert_eq!(observe(&report, &policy).evidence["sources_match"], true);
    }
    #[test]
    fn retargeted_base_and_wrong_pr_identity_do_not_reuse_required_policy() {
        let (mut report, mut policy) = fixture();
        report.data.ci.summary.pending = 0;
        report.data.pull_request["base"] =
            json!({"ref":"release","sha":"base","repo":{"full_name":"o/r"}});
        let retargeted = observe(&report, &policy);
        assert_eq!(retargeted.evidence["sources_match"], false);
        assert!(retargeted.blocking.is_empty());
        assert!(retargeted.completed.is_none());
        report.data.pull_request["base"]["ref"] = json!("main");
        policy.pr_base_sha = Some("previous-base".into());
        assert_eq!(observe(&report, &policy).evidence["sources_match"], false);
        policy.pr_base_sha = Some("base".into());
        report.data.pull_request["number"] = json!(2);
        assert_eq!(observe(&report, &policy).evidence["sources_match"], false);
        report.data.pull_request["number"] = json!(1);
        report.data.pull_request["base"]["repo"]["full_name"] = json!("another/repository");
        assert_eq!(observe(&report, &policy).evidence["sources_match"], false);
        report.data.pull_request["base"]["repo"]["full_name"] = json!("O/R");
        assert_eq!(observe(&report, &policy).evidence["sources_match"], true);
    }
    #[test]
    fn a_reused_check_id_with_a_new_attempt_wakes_again() {
        let (mut report, policy) = fixture();
        report.data.ci.summary.pending = 0;
        report.data.ci.check_runs[0]["started_at"] = json!("2026-09-29T01:00:00Z");
        report.data.ci.check_runs[0]["completed_at"] = json!("2026-09-29T01:01:00Z");
        let first = observe(&report, &policy);
        report.observed_at_ms += 100;
        assert_eq!(observe(&report, &policy).blocking, first.blocking);
        report.data.ci.check_runs[0]["started_at"] = json!("2026-09-29T02:00:00Z");
        report.data.ci.check_runs[0]["completed_at"] = json!("2026-09-29T02:01:00Z");
        let rerun = observe(&report, &policy);
        assert_ne!(rerun.blocking, first.blocking);
        assert_ne!(rerun.completed, first.completed);
    }

    #[test]
    fn rerun_is_a_new_failure_and_empty_ci_is_not_completion() {
        let (mut r, p) = fixture();
        let first = observe(&r, &p).blocking;
        r.data.ci.check_runs[0]["id"] = json!(2);
        assert_ne!(observe(&r, &p).blocking, first);
        r.data.ci.check_runs.clear();
        r.data.ci.summary.pending = 0;
        assert!(observe(&r, &p).completed.is_none());
    }

    #[test]
    fn resolving_feedback_and_author_replies_do_not_create_more_work() {
        let (mut r, p) = fixture();
        r.data.ci.summary.pending = 0;
        r.data.pull_request["user"] = json!({"login":"author"});
        r.data
            .comments
            .push(json!({"id":1,"body":"review finding","user":{"login":"reviewer"}}));
        r.data.review_threads.push(json!({"id":"thread","isResolved":false,"isOutdated":false,"comments":{"nodes":[{"id":"comment","body":"race","author":{"login":"reviewer"}}]}}));
        let before = observe(&r, &p);
        assert_eq!(before.feedback.len(), 2);
        r.data
            .comments
            .push(json!({"id":2,"body":"fixed","user":{"login":"author"}}));
        assert_eq!(observe(&r, &p).feedback, before.feedback);
        r.data.review_threads[0]["isResolved"] = json!(true);
        let after = observe(&r, &p);
        assert_eq!(after.completed, before.completed);
        assert!(
            after
                .feedback
                .iter()
                .all(|key| before.feedback.contains(key))
        );
    }

    #[test]
    fn unrelated_app_or_sha_does_not_change_a_required_failure_identity() {
        let (mut r, p) = fixture();
        r.data.ci.check_runs[0]["app"] = json!({"id":1});
        r.data.ci.check_runs[0]["head_sha"] = json!("head");
        let before = observe(&r, &p).blocking;
        r.data.ci.check_runs.push(
            json!({"id":2,"name":"test","status":"in_progress","app":{"id":2},"head_sha":"head"}),
        );
        r.data.ci.check_runs.push(json!({"id":3,"name":"test","status":"in_progress","app":{"id":1},"head_sha":"another"}));
        assert_eq!(observe(&r, &p).blocking, before);
    }

    #[test]
    fn superseded_check_runs_do_not_delay_or_change_completion() {
        let (mut r, mut p) = fixture();
        r.data.ci.summary.pending = 0;
        p.state = "satisfied".into();
        p.checks[0].state = "satisfied".into();
        r.data.ci.check_runs[0]["head_sha"] = json!("head");
        r.data.ci.check_runs[0]["status"] = json!("in_progress");
        r.data.ci.check_runs.push(json!({"id":2,"name":"test","app":{"id":1},"head_sha":"head","status":"completed","conclusion":"success"}));
        let completed = observe(&r, &p)
            .completed
            .expect("Latest successful check completed");
        r.data.ci.check_runs[0]["status"] = json!("completed");
        assert_eq!(observe(&r, &p).completed.as_ref(), Some(&completed));
        r.data.ci.check_runs.remove(0);
        assert_eq!(observe(&r, &p).completed.as_ref(), Some(&completed));
    }

    #[test]
    fn ci_metadata_does_not_repeat_completion() {
        let (mut r, p) = fixture();
        r.data.ci.summary.pending = 0;
        let before = observe(&r, &p).completed;
        assert!(before.is_some());
        r.data.ci.check_runs[0]["details_url"] = json!("https://github.com/o/r/actions/runs/1");
        r.data.ci.check_runs[0]["app"]["name"] = json!("Updated app name");
        assert_eq!(observe(&r, &p).completed, before);
    }

    #[test]
    fn large_evidence_is_bounded_without_hiding_events_in_omitted_details() {
        let (mut report, policy) = fixture();
        report.data.ci.summary.pending = 0;
        for id in 2..202 {
            report.data.ci.check_runs.push(json!({"id":id,"name":"Long name ".repeat(500),"app":{"id":id},"head_sha":"head","status":"completed","conclusion":"success"}));
            report
                .data
                .comments
                .push(json!({"id":id,"body":"界".repeat(5000),"user":{"login":"reviewer"}}));
        }
        report.data.ci.failures.push(crate::FailedResult {
            kind: "job".into(),
            name: "test".into(),
            conclusion: "failure".into(),
            url: None,
            failed_steps: (0..100)
                .map(|id| json!({"number":id,"name":"界".repeat(5000),"conclusion":"failure"}))
                .collect(),
        });
        let first = observe(&report, &policy);
        assert!(first.evidence.to_string().len() <= 64 * 1024);
        assert!(
            first.evidence["omitted"]["checks"]
                .as_u64()
                .is_some_and(|n| n > 0)
        );
        assert_eq!(first.evidence["required"][0]["state"], "failure");
        report.data.comments[0]["body"] = json!(format!("{}a later edit", "界".repeat(5000)));
        let edited = observe(&report, &policy);
        assert_ne!(
            first.feedback, edited.feedback,
            "Identity must use full feedback even when its display is truncated"
        );
        assert_eq!(first.blocking, edited.blocking);
    }
}
