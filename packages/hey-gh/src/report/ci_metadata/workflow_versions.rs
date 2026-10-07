//! Revalidate a complete REST workflow roster with the commit's current suites.
use super::status_versions::Cached;
use crate::{Client, Error, Freshness, Response, Result, now_ms};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

type Timestamp = chrono::DateTime<chrono::FixedOffset>;

fn text(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}
fn id(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| *n > 0)
}
fn timestamp(value: &Value) -> Option<Timestamp> {
    let value = value.as_str().filter(|s| s.len() <= 64)?;
    let at = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    (at.timestamp() >= 0).then_some(at)
}
fn outcome(status: &Value, conclusion: &Value) -> Option<(String, Option<String>)> {
    let status = status.as_str()?.to_ascii_lowercase();
    let conclusion = match conclusion {
        Value::Null => None,
        Value::String(s) => Some(s.to_ascii_lowercase()),
        _ => return None,
    };
    if (status == "completed"
        && matches!(
            conclusion.as_deref(),
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
        ))
        || (matches!(
            status.as_str(),
            "queued" | "in_progress" | "requested" | "waiting" | "pending"
        ) && conclusion.is_none())
    {
        Some((status, conclusion))
    } else {
        None
    }
}

#[derive(PartialEq, Eq)]
struct Version {
    id: u64,
    suite: u64,
    attempt: u64,
    number: u64,
    workflow: u64,
    event: String,
    path: String,
    name: String,
    title: String,
    created: Timestamp,
    updated: Timestamp,
    outcome: (String, Option<String>),
}
impl Version {
    fn accepts(&self, run: &Value) -> bool {
        let Some(attempt) = id(&run["run_attempt"]) else {
            return false;
        };
        let Some(updated) = timestamp(&run["updated_at"]) else {
            return false;
        };
        attempt >= self.attempt
            && updated >= self.updated
            && (attempt > self.attempt
                || updated > self.updated
                || outcome(&run["status"], &run["conclusion"]).as_ref() == Some(&self.outcome))
    }
    fn graphql(suite: &Value, run: &Value, sha: &str) -> Option<Self> {
        let suite_id = id(&suite["databaseId"])?;
        let updated = timestamp(&run["updatedAt"])?;
        // Suite/run updates may arrive separately. A queued rerun or late job
        // cannot validate the old attempt while these versions disagree.
        if run["checkSuite"]["databaseId"] != suite_id
            || run["checkSuite"]["commit"]["oid"] != sha
            || timestamp(&suite["updatedAt"])? != updated
        {
            return None;
        }
        let created = timestamp(&run["createdAt"])?;
        if created > updated {
            return None;
        }
        Some(Self {
            id: id(&run["databaseId"])?,
            suite: suite_id,
            attempt: id(&run["runAttempt"])?,
            number: id(&run["runNumber"])?,
            workflow: id(&run["workflow"]["databaseId"])?,
            event: text(&run["event"])?,
            path: text(&run["file"]["path"])?,
            name: text(&run["workflow"]["name"])?,
            title: text(&run["displayTitle"])?,
            created,
            updated,
            outcome: outcome(suite.get("status")?, suite.get("conclusion")?)?,
        })
    }
    fn rest(run: &Value, sha: &str) -> Option<Self> {
        if run["head_sha"] != sha {
            return None;
        }
        Some(Self {
            id: id(&run["id"])?,
            suite: id(&run["check_suite_id"])?,
            attempt: id(&run["run_attempt"])?,
            number: id(&run["run_number"])?,
            workflow: id(&run["workflow_id"])?,
            event: text(&run["event"])?,
            path: text(&run["path"])?,
            name: text(&run["name"])?,
            title: text(&run["display_title"])?,
            created: timestamp(&run["created_at"])?,
            updated: timestamp(&run["updated_at"])?,
            outcome: outcome(run.get("status")?, run.get("conclusion")?)?,
        })
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct Versions {
    sha: String,
    runs: BTreeMap<String, Version>,
}

pub(super) fn reusable(response: &Response, sha: &str, freshness: Freshness) -> bool {
    response.data["workflow_runs"]
        .as_array()
        .is_some_and(|runs| runs.iter().all(|run| Version::rest(run, sha).is_some()))
        && response.validated_at_ms > 0
        && now_ms()
            .checked_sub(response.validated_at_ms)
            .is_some_and(|age| matches!(freshness, Freshness::CachedOnly) || age < 86_400_000)
}

impl Versions {
    pub(super) fn regresses(&self, runs: &[Value]) -> bool {
        runs.iter().any(|run| {
            self.runs.values().any(|expected| {
                run["id"] == expected.id && run["head_sha"] == self.sha && !expected.accepts(run)
            })
        })
    }
    pub(super) fn validate(&self, runs: &[Value]) -> Result<()> {
        for expected in self.runs.values() {
            let observed = runs
                .iter()
                .find(|run| run["id"] == expected.id && run["head_sha"] == self.sha);
            if observed.is_none_or(|run| !expected.accepts(run)) {
                return Err(Error::Invalid(
                    "workflow versions disagree with newer CI metadata".into(),
                ));
            }
        }
        Ok(())
    }
    pub(super) fn from_commit(commit: &Value, sha: &str) -> Option<Self> {
        if commit["__typename"] != "Commit" || commit["oid"] != sha {
            return None;
        }
        let suites = &commit["checkSuites"];
        let nodes = suites["nodes"].as_array()?;
        if suites["pageInfo"]["hasNextPage"] != false
            || suites["totalCount"].as_u64()? != nodes.len() as u64
            || nodes.len() > 24
        {
            return None;
        }
        let mut suite_nodes = BTreeSet::new();
        let mut suite_ids = BTreeSet::new();
        let mut run_ids = BTreeSet::new();
        let mut runs = BTreeMap::new();
        for suite in nodes {
            if !suite_nodes.insert(text(&suite["id"])?)
                || !suite_ids.insert(id(&suite["databaseId"])?)
            {
                return None;
            }
            let run = suite.get("workflowRun")?;
            if run.is_null() {
                continue;
            }
            let version = Version::graphql(suite, run, sha)?;
            if !run_ids.insert(version.id) || runs.insert(text(&run["id"])?, version).is_some() {
                return None;
            }
        }
        Some(Self {
            sha: sha.into(),
            runs,
        })
    }
    pub(super) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
    fn matches(&self, runs: &[Value]) -> bool {
        if runs.len() != self.runs.len() {
            return false;
        }
        let mut seen = BTreeSet::new();
        runs.iter().all(|run| {
            let Some(node) = text(&run["node_id"]) else {
                return false;
            };
            seen.insert(node.clone())
                && self
                    .runs
                    .get(&node)
                    .zip(Version::rest(run, &self.sha))
                    .is_some_and(|(a, b)| a == &b)
        })
    }
    pub(super) async fn cached(
        &self,
        client: &Client,
        path: &str,
        at: u64,
        freshness: Freshness,
    ) -> Result<Cached> {
        use crate::report::VALIDATIONS;
        let (result, validations) = VALIDATIONS
            .scope(std::cell::RefCell::new(Vec::new()), async {
                let result = client
                    .pages(path, Some("workflow_runs"), Freshness::CachedOnly)
                    .await;
                (result, VALIDATIONS.with(|r| r.take()))
            })
            .await;
        let values = match result {
            Ok(values) => values,
            Err(Error::CacheMiss) => return Ok(Cached::Unavailable),
            Err(error) => return Err(error),
        };
        let now = now_ms();
        if validations.is_empty()
            || validations.iter().any(|v| {
                v.validated_at_ms == 0 || v.validated_at_ms > at || v.validated_at_ms > now
            })
        {
            return Ok(Cached::Unavailable);
        }
        if !self.matches(&values) {
            return Ok(Cached::Changed);
        }
        if !matches!(freshness, Freshness::CachedOnly)
            && validations
                .iter()
                .any(|v| now - v.validated_at_ms >= 86_400_000)
        {
            return Ok(Cached::Unavailable);
        }
        if matches!(freshness, Freshness::CachedOnly) {
            let _ = VALIDATIONS.try_with(|r| r.borrow_mut().extend(validations));
        }
        Ok(Cached::Matching(values))
    }
}
