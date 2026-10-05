//! A base/policy retry does not itself invalidate CI for unchanged commits.
use crate::{Freshness, entity, repository::valid_sha};
use serde_json::Value;

#[derive(PartialEq, Eq)]
pub(super) struct Identity {
    generation: u64,
    access_failure_epoch: u64,
    pr: String,
    repo_id: u64,
    repo_node: String,
    repo_name: String,
    head: String,
    merge: Option<String>,
}

impl Identity {
    pub(super) fn current(pr: &Value, access_failure_epoch: u64) -> Option<Self> {
        let owner = entity::current()?;
        let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        let repo = &pr["base"]["repo"];
        let pr_node = text(&pr["node_id"])?;
        let repo_name = text(&repo["full_name"])?;
        if owner.node_id.as_ref() != Some(&pr_node)
            || pr["number"] != owner.number
            || !repo_name.eq_ignore_ascii_case(&owner.repository)
        {
            return None;
        }
        let merge = match pr.get("merge_commit_sha")? {
            Value::Null => None,
            Value::String(sha) if valid_sha(sha) => Some(sha.clone()),
            _ => return None,
        };
        Some(Self {
            generation: owner.generation,
            access_failure_epoch,
            pr: pr_node,
            repo_id: repo["id"].as_u64().filter(|id| *id > 0)?,
            repo_node: text(&repo["node_id"])?,
            repo_name: repo_name.to_ascii_lowercase(),
            head: pr["head"]["sha"]
                .as_str()
                .filter(|sha| valid_sha(sha))?
                .to_owned(),
            merge,
        })
    }
}

pub(super) fn freshness(
    caller: Freshness,
    attempt: Freshness,
    previous: Option<&Identity>,
    current: Option<&Identity>,
) -> Freshness {
    // This only chooses an age bound. Recollect through the normal CI path so
    // new cache observations, access denials, expiry and generations still win.
    if matches!(caller, Freshness::MaxAge(age) if !age.is_zero())
        && previous.is_some()
        && previous == current
    {
        caller
    } else {
        attempt
    }
}
