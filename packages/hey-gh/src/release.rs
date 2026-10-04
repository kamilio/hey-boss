//! Explicit release tracking; no agent assignment, prompting, or GitHub writes.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod collect;
pub mod queue;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub repository: String,
    pub branch: String,
    pub target: TargetKind,
    pub gates: Vec<Gate>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Commit,
    PullRequest,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Validation,
    Deployment,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub name: String,
    pub workflow: String,
    pub purpose: Purpose,
    pub jobs: Vec<Job>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    /// Exact Actions job name, or a prefix followed by one trailing '*'.
    pub name: String,
    #[serde(default = "one")]
    pub count: usize,
    /// Each named step must actually have succeeded; skipped does not count.
    #[serde(default)]
    pub steps: Vec<String>,
    /// Counts disambiguate repeated composite-action step names.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub step_counts: std::collections::BTreeMap<String, usize>,
    /// Explicit project-specific no-change proof for a skipped deployment step.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub unchanged_steps: std::collections::BTreeMap<String, String>,
}
fn one() -> usize {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub project: Project,
    pub targets: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Batch {
    pub observed_at_ms: u64,
    pub reports: Vec<Report>,
    pub validations: Vec<crate::ResourceValidation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub target: String,
    pub commit: Option<String>,
    /// awaiting_merge, not_merged, not_on_branch, watching, failed, verified,
    /// recovered, or unknown. Verified means the configured evidence passed;
    /// successor verification does not prove the original commit was healthy.
    pub state: String,
    pub branch_sha: Option<String>,
    pub gates: Vec<GateReport>,
    pub errors: Vec<String>,
}
impl Report {
    fn new(target: &str) -> Self {
        Self {
            target: target.into(),
            commit: None,
            state: "unknown".into(),
            branch_sha: None,
            gates: vec![],
            errors: vec![],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateReport {
    pub name: String,
    pub purpose: Purpose,
    pub satisfied: bool,
    pub history_complete: bool,
    /// First confirmation in the inspected history; never workflow updated_at.
    pub confirmation: Option<RunRecord>,
    pub runs: Vec<RunRecord>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: u64,
    pub attempt: u64,
    pub sha: String,
    pub url: String,
    /// exact or successor, established by Git ancestry, never timestamps.
    pub coverage: String,
    #[serde(flatten)]
    pub verdict: Verdict,
}

/// GitHub comparison responses are scoped to two immutable SHAs by the caller.
pub fn contains(base: &str, head: &str, comparison: &Value) -> bool {
    base == head
        || (comparison["base_commit"]["sha"] == base
            && comparison["merge_base_commit"]["sha"] == base
            && matches!(comparison["status"].as_str(), Some("ahead" | "identical")))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Passed,
    Unchanged,
    Failed,
    Cancelled,
    Pending,
    Skipped,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Verdict {
    pub state: RunState,
    #[serde(default)]
    pub workflow_conclusion: Option<String>,
    pub completed_at: Option<String>,
    pub failed_jobs: Vec<String>,
    pub missing: Vec<String>,
}

pub fn assess(gate: &Gate, run: &Value, jobs: &[Value]) -> Verdict {
    let mut v = Verdict {
        state: RunState::Unknown,
        workflow_conclusion: run["conclusion"].as_str().map(str::to_owned),
        completed_at: None,
        failed_jobs: vec![],
        missing: vec![],
    };
    let mut skipped = false;
    let mut unchanged = false;
    let mut pending = false;
    let mut completed = Vec::new();
    let mut job_ids = std::collections::HashSet::new();
    let mut invalid = jobs.iter().any(|j| !job_ids.insert(j["id"].as_u64()))
        || !matches!(
            run["status"].as_str(),
            Some("completed" | "in_progress" | "queued" | "pending" | "waiting" | "requested")
        )
        || (run["status"] == "completed"
            && !matches!(
                run["conclusion"].as_str(),
                Some(
                    "success"
                        | "failure"
                        | "cancelled"
                        | "skipped"
                        | "neutral"
                        | "timed_out"
                        | "action_required"
                        | "startup_failure"
                )
            ));
    for requirement in &gate.jobs {
        let selected: Vec<_> = jobs
            .iter()
            .filter(|job| {
                job["name"].as_str().is_some_and(|name| {
                    requirement
                        .name
                        .strip_suffix('*')
                        .map_or(name == requirement.name, |prefix| name.starts_with(prefix))
                })
            })
            .collect();
        if selected.len() != requirement.count {
            v.missing.push(requirement.name.clone());
        }
        for job in selected {
            if job["run_id"] != run["id"]
                || job["head_sha"] != run["head_sha"]
                || job["run_attempt"] != run["run_attempt"]
                || job["id"].as_u64().is_none()
            {
                invalid = true;
                continue;
            }
            let mut items = vec![job];
            for step_name in &requirement.steps {
                let steps: Vec<_> = job["steps"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|step| step["name"] == *step_name)
                    .collect();
                if steps.len() != requirement.step_counts.get(step_name).copied().unwrap_or(1) {
                    v.missing
                        .push(format!("{} / {step_name}", requirement.name));
                }
                for step in steps {
                    let alternative = requirement.unchanged_steps.get(step_name).and_then(|name| {
                        job["steps"].as_array()?.iter().find(|s| {
                            s["name"] == *name
                                && s["status"] == "completed"
                                && s["conclusion"] == "success"
                        })
                    });
                    if gate.purpose == Purpose::Deployment
                        && step["status"] == "completed"
                        && step["conclusion"] == "skipped"
                        && let Some(alternative) = alternative
                    {
                        unchanged = true;
                        items.push(alternative);
                    } else {
                        items.push(step);
                    }
                }
            }
            let mut failed = false;
            for item in items {
                if item["status"] != "completed" {
                    pending = true;
                    continue;
                }
                match item["conclusion"].as_str() {
                    Some("success") => {}
                    Some("failure" | "timed_out" | "action_required" | "startup_failure") => {
                        failed = true
                    }
                    Some("skipped" | "neutral") => skipped = true,
                    Some("cancelled") => pending = true,
                    _ => invalid = true,
                }
            }
            if failed {
                v.failed_jobs.push(job["name"].as_str().unwrap().to_owned());
            }
            match job["completed_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            {
                Some(time) => completed.push(time),
                None if job["conclusion"] == "success" => invalid = true,
                _ => {}
            }
        }
    }
    v.failed_jobs.sort();
    v.failed_jobs.dedup();
    let satisfied = !invalid
        && v.failed_jobs.is_empty()
        && !skipped
        && v.missing.is_empty()
        && !pending
        && !gate.jobs.is_empty();
    v.state = if invalid {
        RunState::Unknown
    } else if !satisfied && run["status"] == "completed" && run["conclusion"] == "cancelled" {
        RunState::Cancelled
    } else if !v.failed_jobs.is_empty() {
        RunState::Failed
    } else if skipped {
        RunState::Skipped
    } else if !v.missing.is_empty() {
        if run["status"] == "completed" {
            RunState::Unknown
        } else {
            RunState::Pending
        }
    } else if pending {
        RunState::Pending
    } else if gate.jobs.is_empty() {
        RunState::Unknown
    } else if unchanged {
        RunState::Unchanged
    } else {
        RunState::Passed
    };
    if matches!(v.state, RunState::Passed | RunState::Unchanged) {
        v.completed_at = completed
            .into_iter()
            .max()
            .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    v
}

impl Project {
    pub fn validate(&self) -> Result<()> {
        crate::client::validate_repository(&self.repository)?;
        crate::repository::validate_branch(&self.branch)?;
        if self.gates.len() > 20 || !self.gates.iter().any(|g| g.purpose == Purpose::Validation) {
            return Err(Error::Invalid(
                "release project needs a validation gate and at most 20 gates".into(),
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for gate in &self.gates {
            if gate.name.is_empty()
                || gate.name.len() > 100
                || !names.insert(&gate.name)
                || gate.workflow.is_empty()
                || gate.workflow.len() > 200
                || !gate
                    .workflow
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                || !(gate.workflow.ends_with(".yml") || gate.workflow.ends_with(".yaml"))
                || gate.jobs.is_empty()
                || gate.jobs.len() > 100
            {
                return Err(Error::Invalid("invalid or duplicate release gate".into()));
            }
            for job in &gate.jobs {
                if job.name.is_empty()
                    || job.name.len() > 300
                    || job.name == "*"
                    || job.name.trim_end_matches('*').contains('*')
                    || job.name.ends_with("**")
                    || !(1..=100).contains(&job.count)
                    || job.steps.len() > 100
                    || job
                        .steps
                        .iter()
                        .any(|step| step.is_empty() || step.len() > 500)
                {
                    return Err(Error::Invalid("invalid release job requirement".into()));
                }
                if !job.unchanged_steps.is_empty()
                    && (gate.purpose != Purpose::Deployment
                        || job.unchanged_steps.iter().any(|(step, alternative)| {
                            !job.steps.contains(step)
                                || alternative.is_empty()
                                || alternative.len() > 500
                                || step == alternative
                        }))
                {
                    return Err(Error::Invalid("unchanged proofs must name a required deployment step and a distinct confirmation step".into()));
                }
                if job
                    .step_counts
                    .iter()
                    .any(|(step, count)| !job.steps.contains(step) || !(1..=100).contains(count))
                {
                    return Err(Error::Invalid(
                        "step counts must name required steps and be within 1..100".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
