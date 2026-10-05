//! Validate complete cached REST status lists against fresh selector versions.
use crate::{Client, Error, Freshness, Result, now_ms};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(PartialEq, Eq)]
struct Version {
    updated_at: chrono::DateTime<chrono::FixedOffset>,
    context: String,
    state: &'static str,
    description: Option<String>,
    target_url: Option<String>,
}

fn text(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn nullable(value: &Value) -> Option<Option<String>> {
    match value {
        Value::Null => Some(None),
        Value::String(s) => Some(Some(s.clone())),
        _ => None,
    }
}

fn timestamp(value: &Value) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let value = value.as_str().filter(|s| s.len() <= 64)?;
    let at = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    (at.timestamp() >= 0).then_some(at)
}

impl Version {
    fn graphql(value: &Value) -> Option<Self> {
        Some(Self {
            updated_at: timestamp(&value["updatedAt"])?,
            context: text(&value["context"])?,
            state: match value["state"].as_str()? {
                "ERROR" => "error",
                "FAILURE" => "failure",
                "PENDING" => "pending",
                "SUCCESS" => "success",
                _ => return None,
            },
            description: nullable(value.get("description")?)?,
            target_url: nullable(value.get("targetUrl")?)?,
        })
    }

    fn rest(value: &Value) -> Option<Self> {
        Some(Self {
            updated_at: timestamp(&value["updated_at"])?,
            context: text(&value["context"])?,
            state: match value["state"].as_str()? {
                "error" => "error",
                "failure" => "failure",
                "pending" => "pending",
                "success" => "success",
                _ => return None,
            },
            description: nullable(value.get("description")?)?,
            target_url: nullable(value.get("target_url")?)?,
        })
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct Versions(BTreeMap<String, Version>);

pub(super) enum Cached {
    Matching(Vec<Value>),
    Changed,
    Unavailable,
}

impl Versions {
    pub(super) fn has_fields(node: &Value, sha: &str) -> bool {
        std::iter::once(&node["potentialMergeCommit"])
            .chain(
                node["commits"]["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|n| &n["commit"]),
            )
            .filter(|commit| commit["oid"] == sha)
            .any(|commit| {
                commit["status"].get("contexts").is_some()
                    || commit["statusCheckRollup"]["contexts"]
                        .get("statusContextCount")
                        .is_some()
            })
    }

    fn commit(commit: &Value, sha: &str) -> Option<Self> {
        if commit["oid"] != sha || text(&commit["status"]["id"]).is_none() {
            return None;
        }
        let contexts = commit["status"]["contexts"].as_array()?;
        let count = commit["statusCheckRollup"]["contexts"]["statusContextCount"].as_u64()?;
        if contexts.is_empty() || contexts.len() as u64 != count {
            return None;
        }
        let mut versions = BTreeMap::new();
        for context in contexts {
            if versions
                .insert(text(&context["id"])?, Version::graphql(context)?)
                .is_some()
            {
                return None;
            }
        }
        Some(Self(versions))
    }

    pub(super) fn from_node(node: &Value, sha: &str) -> Option<Self> {
        let merge = &node["potentialMergeCommit"];
        if node["headRefOid"] == sha {
            let nodes = node["commits"]["nodes"].as_array()?;
            if nodes.len() != 1 {
                return None;
            }
            let versions = Self::commit(&nodes[0]["commit"], sha)?;
            if merge["oid"] == sha && Self::commit(merge, sha).as_ref() != Some(&versions) {
                return None;
            }
            Some(versions)
        } else {
            Self::commit(merge, sha)
        }
    }

    fn matches(&self, statuses: &[Value]) -> bool {
        if statuses.len() != self.0.len() {
            return false;
        }
        let mut nodes = BTreeSet::new();
        let mut ids = BTreeSet::new();
        statuses.iter().all(|value| {
            let Some(node) = text(&value["node_id"]) else {
                return false;
            };
            value["id"]
                .as_u64()
                .is_some_and(|id| id > 0 && ids.insert(id))
                && nodes.insert(node.clone())
                && self
                    .0
                    .get(&node)
                    .zip(Version::rest(value))
                    .is_some_and(|(expected, actual)| expected == &actual)
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
        // Stage these clocks: a failed shortcut must not add unused evidence.
        let (result, validations) = VALIDATIONS
            .scope(std::cell::RefCell::new(Vec::new()), async {
                let result = client
                    .pages(path, Some("statuses"), Freshness::CachedOnly)
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
        // Online validation belongs to the fresh immutable-version proof.
        // Offline reads keep the REST pages' original clocks as well.
        if matches!(freshness, Freshness::CachedOnly) {
            let _ = VALIDATIONS.try_with(|r| r.borrow_mut().extend(validations));
        }
        Ok(Cached::Matching(values))
    }
}
