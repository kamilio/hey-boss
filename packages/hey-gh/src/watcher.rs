//! Stable wake-up signals from observed CI and the separate required-check policy.
use crate::{Report, RequiredChecksReport};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub head: String,
    pub blocking: Vec<String>,
    pub completed: Option<String>,
    pub feedback: Vec<String>,
    pub evidence: Value,
}

/// The watcher supports canonical GitHub pull-request links.
pub fn pull_request_selector(url: &str) -> Option<(String, u64)> {
    let tail = url.strip_prefix("https://github.com/")?;
    let parts: Vec<_> = tail.trim_end_matches('/').split('/').collect();
    if parts.len() != 4
        || parts[2] != "pull"
        || parts[..2].iter().any(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
    {
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

fn ci_signals(
    repository: &str,
    number: u64,
    pull_request: &Value,
    ci: &crate::CiReport,
    policy: &RequiredChecksReport,
) -> Observation {
    let current = pull_request["state"] == "open"
        && pull_request["head"]["sha"] == ci.head_sha
        && policy.head_sha == ci.head_sha
        && policy.merge_sha == ci.merge_sha
        && policy.repository.eq_ignore_ascii_case(repository)
        && policy.pull_number == number
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
    if current {
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
    let complete = current
        && ci.summary.pending == 0
        && ci.summary.unknown == 0
        && !matches!(policy.state.as_str(), "unknown" | "pending" | "missing")
        && !(checks.is_empty() && statuses.is_empty() && workflows.is_empty())
        && checks.iter().all(|c| c["status"] == "completed")
        && ci.workflow_runs.iter().all(|c| c["status"] == "completed")
        && ci
            .commit_statuses
            .iter()
            .all(|c| matches!(c["state"].as_str(), Some("success" | "failure" | "error")));
    let completed = complete.then(|| {
        fingerprint(&json!([
            ci.head_sha,
            selected(
                &checks,
                &["id", "name", "status", "conclusion", "head_sha", "app"]
            ),
            selected(&statuses, &["id", "context", "state"]),
            selected(
                &workflows,
                &["id", "run_attempt", "status", "conclusion", "head_sha"]
            )
        ]))
    });
    Observation {
        head: ci.head_sha.clone(),
        blocking,
        completed,
        feedback: Vec::new(),
        evidence: json!({"repository":repository,"number":number,"head":ci.head_sha,
            "complete":false,"ci_complete":complete,"checks":checks,"statuses":statuses,"workflows":workflows,
            "required":policy.checks,"required_state":policy.state,"failures":ci.failures,
            "policy_errors":policy.errors,"ci_errors":ci.errors}),
    }
}

/// Policy collection reads check results and statuses, but never workflow jobs
/// or reviews. Its failure identities therefore wake work before those sources.
pub fn observe_required(policy: &RequiredChecksReport) -> Observation {
    let mut blocking = Vec::new();
    if policy.pull_request_state.as_deref() == Some("open") && policy.errors.is_empty() {
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
    Observation {
        head: policy.head_sha.clone(),
        blocking,
        completed: None,
        feedback: Vec::new(),
        evidence: json!({"repository":policy.repository,"number":policy.pull_number,"head":policy.head_sha,
            "complete":false,"required":policy.checks,"required_state":policy.state,"policy_errors":policy.errors}),
    }
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
    let ci = &pr.ci;
    let mut observation = ci_signals(&pr.repository, pr.number, &pr.pull_request, ci, policy);
    let complete = observation.completed.is_some() && report.complete && pr.errors.is_empty();
    if !complete {
        observation.completed = None;
    }
    let reviews = selected(
        &pr.reviews,
        &["id", "state", "body", "commit_id", "html_url"],
    );
    let comments = selected(
        &pr.review_comments,
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
    // Bound retained/displayed review bodies. Identity above still includes all
    // findings, so truncation never masks a newly published or edited review.
    fn excerpt(rows: Vec<Value>) -> Vec<Value> {
        rows.into_iter()
            .rev()
            .take(20)
            .map(|mut row| {
                if let Some(body) = row["body"].as_str() {
                    row["body"] = json!(body.chars().take(1000).collect::<String>());
                }
                row
            })
            .collect()
    }
    observation.feedback = feedback;
    observation.evidence.as_object_mut().expect("CI evidence is an object").extend(
        json!({"complete":complete,"reviews":excerpt(reviews),"review_comments":excerpt(comments),
            "comments":excerpt(discussion),"unresolved_threads":pr.review_status.unresolved_threads,
            "conflicts":pr.conflicts,"source_errors":pr.errors}).as_object().unwrap().clone()
    );
    observation
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
