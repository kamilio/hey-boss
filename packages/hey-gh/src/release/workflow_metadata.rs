//! Refresh known archived IDs; REST still owns discovery and job/step evidence.
use crate::{Client, Error, Freshness, Result};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

const QUERY: &str = r#"query ReleaseWorkflowMetadata($ids: [ID!]!) {
  nodes(ids: $ids) {
    __typename
    ... on WorkflowRun {
      id databaseId runAttempt event createdAt updatedAt
      file { path }
      checkSuite {
        commit { oid }
        status conclusion updatedAt
        branch { name }
        repository { nameWithOwner }
      }
    }
  }
}"#;

/// None requests the ordinary REST history path. Errors never bypass quota,
/// permission, deadline, or identity checks by falling back to another API.
pub(super) async fn refresh(
    client: &Client,
    archived: &[Value],
    freshness: Freshness,
    bytes: &mut usize,
) -> Result<Option<Vec<Value>>> {
    let mut by_node = BTreeMap::new();
    let mut run_ids = HashSet::new();
    for run in archived {
        let Some(node_id) = run["node_id"].as_str().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        if by_node.insert(node_id, run).is_some() || !run_ids.insert(run["id"].as_u64()) {
            return Ok(None);
        }
    }
    let ids: Vec<_> = by_node.keys().copied().collect();
    let mut refreshed = Vec::with_capacity(ids.len());
    let mut representable = true;
    for chunk in ids.chunks(100) {
        let response = client
            .graphql(QUERY, json!({"ids":chunk}), freshness)
            .await?;
        *bytes = bytes.saturating_add(response.data.to_string().len());
        if *bytes > client.collection_limit() {
            return Err(Error::Invalid(
                "release history exceeds collection byte limit".into(),
            ));
        }
        let nodes = response.data["data"]["nodes"]
            .as_array()
            .ok_or_else(invalid)?;
        if nodes.len() != chunk.len() {
            return Err(invalid());
        }
        let mut seen = HashSet::new();
        for node in nodes {
            let id = node["id"].as_str().ok_or_else(invalid)?;
            if !chunk.contains(&id) || !seen.insert(id) {
                return Err(invalid());
            }
            let old = by_node[id];
            let suite = &node["checkSuite"];
            let attempt = node["runAttempt"]
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(invalid)?;
            let updated = timestamp(&node["updatedAt"])?;
            let suite_updated = timestamp(&suite["updatedAt"])?;
            if node["__typename"] != "WorkflowRun"
                || node["databaseId"] != old["id"]
                || suite["commit"]["oid"] != old["head_sha"]
                || node["file"]["path"] != old["path"]
                || node["event"] != old["event"]
                || suite["repository"]["nameWithOwner"]
                    .as_str()
                    .zip(old["repository"]["full_name"].as_str())
                    .is_none_or(|(a, b)| !a.eq_ignore_ascii_case(b))
                || timestamp(&node["createdAt"])? != timestamp(&old["created_at"])?
                || attempt < old["run_attempt"].as_u64().ok_or_else(invalid)?
                || updated < timestamp(&old["updated_at"])?
            {
                return Err(invalid());
            }
            // GitHub may omit a deleted branch, and suite/run updates need not
            // be synchronized. Neither shape can manufacture REST metadata.
            if suite.get("branch") == Some(&Value::Null) {
                representable = false;
            } else if suite["branch"]["name"] != old["head_branch"] {
                return Err(invalid());
            }
            representable &= updated == suite_updated;
            let status = suite["status"]
                .as_str()
                .ok_or_else(invalid)?
                .to_ascii_lowercase();
            let conclusion = match suite.get("conclusion").ok_or_else(invalid)? {
                Value::Null => Value::Null,
                Value::String(value) => json!(value.to_ascii_lowercase()),
                _ => return Err(invalid()),
            };
            if !matches!(
                status.as_str(),
                "completed" | "in_progress" | "queued" | "pending" | "waiting" | "requested"
            ) || (status == "completed"
                && !matches!(
                    conclusion.as_str(),
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
                || (status != "completed" && !conclusion.is_null())
            {
                representable = false;
            }
            let mut run = old.clone();
            run["run_attempt"] = json!(attempt);
            run["updated_at"] = node["updatedAt"].clone();
            run["status"] = json!(status);
            run["conclusion"] = conclusion;
            refreshed.push(run);
        }
    }
    Ok(representable.then_some(refreshed))
}

fn timestamp(value: &Value) -> Result<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(value.as_str().ok_or_else(invalid)?).map_err(|_| invalid())
}
fn invalid() -> Error {
    Error::Invalid("missing or mismatched archived workflow metadata".into())
}
