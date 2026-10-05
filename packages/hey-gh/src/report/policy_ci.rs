//! Only attest the fields consumed by required-check evaluation. These proofs
//! never refresh full REST payloads or publish a full CI recovery.
use super::{CiReport, check_size, dedup_id, failed_results, record_validation, summarize};
use crate::{Client, Error, Freshness, Response, Result, now_ms};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) enum Proof {
    Matching(Box<CiReport>),
    Rest(Freshness),
}

struct Seed {
    sha: String,
    checks: Response,
    statuses: Response,
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.is_empty())
}

fn nullable(value: Option<&Value>) -> bool {
    value.is_some_and(|v| v.is_null() || v.is_string())
}

fn check_version(value: &Value, graph: bool, sha: &str) -> Option<(String, Value)> {
    let (id, name, status, conclusion, app, actual_sha, started, completed, url) = if graph {
        (
            &value["databaseId"],
            &value["name"],
            &value["status"],
            value.get("conclusion")?,
            &value["checkSuite"]["app"]["databaseId"],
            &value["checkSuite"]["commit"]["oid"],
            value.get("startedAt"),
            value.get("completedAt"),
            value.get("detailsUrl"),
        )
    } else {
        (
            &value["id"],
            &value["name"],
            &value["status"],
            value.get("conclusion")?,
            &value["app"]["id"],
            &value["head_sha"],
            value.get("started_at"),
            value.get("completed_at"),
            value.get("details_url"),
        )
    };
    let node = text(&value[if graph { "id" } else { "node_id" }])?;
    let status = status.as_str()?.to_ascii_lowercase();
    let conclusion = match conclusion {
        Value::Null => Value::Null,
        Value::String(s) => json!(s.to_ascii_lowercase()),
        _ => return None,
    };
    if id.as_u64().is_none_or(|id| id == 0)
        || text(name).is_none()
        || app.as_u64().is_none_or(|id| id == 0)
        || actual_sha != sha
        || !matches!(
            status.as_str(),
            "queued" | "in_progress" | "completed" | "waiting" | "requested" | "pending"
        )
        || (!conclusion.is_null()
            && !matches!(
                conclusion.as_str(),
                Some(
                    "success"
                        | "failure"
                        | "neutral"
                        | "cancelled"
                        | "skipped"
                        | "timed_out"
                        | "action_required"
                        | "stale"
                        | "startup_failure"
                )
            ))
        || !nullable(started)
        || !nullable(completed)
        || !nullable(url)
    {
        return None;
    }
    Some((
        node.to_owned(),
        json!([
            id, name, status, conclusion, app, sha, started, completed, url
        ]),
    ))
}

fn status_version(value: &Value, graph: bool) -> Option<(String, Value)> {
    let node = text(&value[if graph { "id" } else { "node_id" }])?;
    let state = value["state"].as_str()?.to_ascii_lowercase();
    let context = text(&value["context"])?;
    let updated = text(&value[if graph { "updatedAt" } else { "updated_at" }])?;
    let url = value.get(if graph { "targetUrl" } else { "target_url" });
    if !matches!(state.as_str(), "success" | "failure" | "error" | "pending")
        || !nullable(url)
        || (!graph && value["id"].as_u64().is_none_or(|id| id == 0))
    {
        return None;
    }
    Some((node.to_owned(), json!([context, state, updated, url])))
}

fn roster(
    values: &[Value],
    graph: bool,
    checks: bool,
    sha: &str,
) -> Option<BTreeMap<String, Value>> {
    let mut result = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for value in values {
        let (node, version) = if checks {
            check_version(value, graph, sha)?
        } else {
            status_version(value, graph)?
        };
        if result.insert(node, version).is_some() {
            return None;
        }
        if !graph && !ids.insert(value["id"].as_u64()?) {
            return None;
        }
    }
    Some(result)
}

fn complete_connection(value: &Value, limit: usize) -> Option<&[Value]> {
    let nodes = value["nodes"].as_array()?;
    (value["pageInfo"]["hasNextPage"] == false
        && nodes.len() <= limit
        && value["totalCount"].as_u64() == Some(nodes.len() as u64))
    .then_some(nodes.as_slice())
}

impl Seed {
    fn values(&self, checks: bool) -> Option<&[Value]> {
        let (response, field) = if checks {
            (&self.checks, "check_runs")
        } else {
            (&self.statuses, "statuses")
        };
        let values = response.data[field].as_array()?;
        (response.link.is_none()
            && response.data["total_count"].as_u64() == Some(values.len() as u64)
            && values.len() <= 100
            && (checks || response.data["sha"] == self.sha))
            .then_some(values.as_slice())
    }

    fn matches(&self, commit: &Value) -> bool {
        if commit["__typename"] != "Commit" || commit["oid"] != self.sha {
            return false;
        }
        let Some(checks) = self.values(true) else {
            return false;
        };
        let Some(statuses) = self.values(false) else {
            return false;
        };
        // Rollups omit some later workflow-generated checks, including on
        // merge commits. Only complete suite/run connections prove a roster.
        let Some(suites) = complete_connection(&commit["checkSuites"], 24) else {
            return false;
        };
        let mut graph_checks = Vec::new();
        let mut suite_nodes = BTreeSet::new();
        let mut suite_ids = BTreeSet::new();
        for suite in suites {
            let Some(id) = text(&suite["id"]) else {
                return false;
            };
            let Some(database_id) = suite["databaseId"].as_u64().filter(|id| *id > 0) else {
                return false;
            };
            if !suite_nodes.insert(id) || !suite_ids.insert(database_id) {
                return false;
            }
            let Some(runs) = complete_connection(&suite["checkRuns"], 32) else {
                return false;
            };
            if graph_checks.len() + runs.len() > 100
                || runs.iter().any(|run| run["__typename"] != "CheckRun")
            {
                return false;
            }
            graph_checks.extend(runs.iter().cloned());
        }
        // Status.contexts is an unpaginated list of the latest legacy statuses.
        let graph_statuses = match commit.get("status") {
            Some(Value::Null) => &[][..],
            Some(status) => match status["contexts"].as_array() {
                Some(contexts) if contexts.len() <= 100 => contexts.as_slice(),
                _ => return false,
            },
            None => return false,
        };
        [true, false].into_iter().all(|is_check| {
            roster(
                if is_check { checks } else { statuses },
                false,
                is_check,
                &self.sha,
            )
            .zip(roster(
                if is_check {
                    &graph_checks
                } else {
                    graph_statuses
                },
                true,
                is_check,
                &self.sha,
            ))
            .is_some_and(|(rest, graph)| rest == graph)
        })
    }
}

impl Client {
    async fn policy_ci_seeds(
        &self,
        repository: &str,
        head: &str,
        merge: Option<&str>,
    ) -> Result<Option<Vec<Seed>>> {
        let mut seeds = Vec::new();
        for sha in std::iter::once(head).chain(merge.filter(|m| *m != head)) {
            let checks = self
                .peek_get(&format!(
                    "repos/{repository}/commits/{sha}/check-runs?filter=latest&per_page=100"
                ))
                .await;
            let statuses = self
                .peek_get(&format!(
                    "repos/{repository}/commits/{sha}/status?per_page=100"
                ))
                .await;
            let (checks, statuses) = match (checks, statuses) {
                (Ok(checks), Ok(statuses)) => (checks, statuses),
                (Err(error), _) if !matches!(error, Error::CacheMiss) => return Err(error),
                (_, Err(error)) if !matches!(error, Error::CacheMiss) => return Err(error),
                _ => return Ok(None),
            };
            let seed = Seed {
                sha: sha.to_owned(),
                checks,
                statuses,
            };
            for is_check in [true, false] {
                if seed
                    .values(is_check)
                    .and_then(|values| roster(values, false, is_check, sha))
                    .is_none()
                {
                    return Ok(None);
                }
            }
            if [&seed.checks, &seed.statuses].iter().any(|r| {
                now_ms()
                    .checked_sub(r.validated_at_ms)
                    .is_none_or(|age| age >= 86_400_000)
                    || r.validated_at_ms == 0
            }) {
                return Ok(None);
            }
            seeds.push(seed);
        }
        check_size(
            seeds.iter().flat_map(|s| {
                s.values(true)
                    .unwrap()
                    .iter()
                    .chain(s.values(false).unwrap())
            }),
            self.collection_limit(),
        )?;
        Ok(Some(seeds))
    }

    pub(super) async fn unchanged_policy_ci(
        &self,
        repository: &str,
        head: &str,
        merge: Option<&str>,
        freshness: Freshness,
    ) -> Result<Proof> {
        crate::client::validate_repository(repository)?;
        if !super::valid_sha(head) || merge.is_some_and(|m| !super::valid_sha(m)) {
            return Err(Error::Invalid(
                "CI refs must be immutable commit SHAs".into(),
            ));
        }
        let Freshness::MaxAge(age) = freshness else {
            return Ok(Proof::Rest(freshness));
        };
        if age.is_zero() {
            return Ok(Proof::Rest(freshness));
        }
        let Some(seeds) = self.policy_ci_seeds(repository, head, merge).await? else {
            return Ok(Proof::Rest(freshness));
        };
        if seeds
            .iter()
            .flat_map(|s| [&s.checks, &s.statuses])
            .all(|r| (now_ms().saturating_sub(r.validated_at_ms) as u128) < age.as_millis())
        {
            return Ok(Proof::Rest(freshness));
        }
        drop(seeds);
        let (resource, proof) = match crate::client::optional_selector_read(
            self.policy_ci_response(repository, head, merge, freshness),
        )
        .await
        {
            Ok(proof) => proof,
            Err(Error::CacheMiss) if crate::client::CACHE_PROBE.try_with(|_| ()).is_ok() => {
                // Admission can still use the existing discovery/version
                // proofs when this optional query has never been cached.
                return Ok(Proof::Rest(freshness));
            }
            Err(
                error @ (Error::Auth(_)
                | Error::LocalAuth(_)
                | Error::Storage(_)
                | Error::Stopped
                | Error::GraphQL {
                    access_denied: true,
                    ..
                }
                | Error::GitHub {
                    status: 401 | 403, ..
                }),
            ) => return Err(error),
            _ => return Ok(Proof::Rest(Freshness::Revalidate)),
        };
        // A newer REST observation during the query wins over the original
        // cached seed. Entity/generation checks also run on these cache reads.
        let Some(seeds) = self.policy_ci_seeds(repository, head, merge).await? else {
            return Ok(Proof::Rest(Freshness::Revalidate));
        };
        let repo = &proof.data["data"]["repository"];
        if text(&repo["id"]).is_none()
            || !repo["nameWithOwner"]
                .as_str()
                .is_some_and(|r| r.eq_ignore_ascii_case(repository))
            || now_ms()
                .checked_sub(proof.validated_at_ms)
                .is_none_or(|elapsed| elapsed as u128 >= age.as_millis())
            || !seeds.iter().enumerate().all(|(index, s)| {
                s.matches(&repo[if index == 0 { "head" } else { "merge" }])
                    && s.checks.validated_at_ms <= proof.validated_at_ms
                    && s.statuses.validated_at_ms <= proof.validated_at_ms
            })
        {
            return Ok(Proof::Rest(Freshness::Revalidate));
        }
        let (mut checks, mut statuses) = (Vec::new(), Vec::new());
        for mut seed in seeds {
            let Value::Array(values) = seed.checks.data["check_runs"].take() else {
                unreachable!("validated check roster")
            };
            checks.extend(values);
            let Value::Array(values) = seed.statuses.data["statuses"].take() else {
                unreachable!("validated status roster")
            };
            for mut status in values {
                status["observed_sha"] = json!(seed.sha);
                statuses.push(status);
            }
        }
        dedup_id(&mut checks);
        dedup_id(&mut statuses);
        record_validation(&resource, &proof);
        Ok(Proof::Matching(Box::new(CiReport {
            head_sha: head.to_owned(),
            merge_sha: merge.map(str::to_owned),
            summary: summarize(&checks, &statuses, &[], &[], false),
            failures: failed_results(&checks, &statuses, &[], &[]),
            check_runs: checks,
            commit_statuses: statuses,
            workflow_runs: vec![],
            jobs: vec![],
            errors: vec![],
        })))
    }
}
