//! Complete job/step evidence for bounded groups of immutable first attempts.
use crate::{Client, Error, Freshness, Result};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

// 20 * 100 jobs * 100 steps stays below GitHub's 500,000-node query bound.
pub(super) const BATCH_SIZE: usize = 20;
const QUERY: &str = r#"query ReleaseWorkflowJobs($ids: [ID!]!) {
  nodes(ids: $ids) {
    __typename
    ... on WorkflowRun {
      id databaseId runAttempt updatedAt
      checkSuite {
        commit { oid }
        repository { nameWithOwner }
        status conclusion updatedAt
        checkRuns(first: 100, filterBy: {checkType: ALL}) {
          totalCount pageInfo { hasNextPage }
          nodes {
            databaseId name status conclusion completedAt
            steps(first: 100) {
              totalCount pageInfo { hasNextPage }
              nodes { number name status conclusion }
            }
          }
        }
      }
    }
  }
}"#;
const VERSIONS: &str = r#"query ReleaseWorkflowJobVersions($ids: [ID!]!) {
  nodes(ids: $ids) {
    ... on WorkflowRun { id databaseId runAttempt updatedAt }
  }
}"#;

pub(super) fn eligible(run: &Value) -> bool {
    // CheckRun has no attempt selector. Repeated attempts retain the REST API's
    // explicit attempt identity, including partial reruns of failed jobs only.
    run["run_attempt"] == 1
        && run["status"] == "completed"
        && run["node_id"].as_str().is_some_and(|s| !s.is_empty())
}

fn memo_key(client: &Client, repository: &str, run: &Value) -> String {
    let version = crate::digest(
        &json!([
            run["id"],
            run["run_attempt"],
            run["head_sha"],
            run["status"],
            run["conclusion"],
            run["updated_at"]
        ])
        .to_string(),
    );
    // Use the established completed-jobs namespace for repository generation
    // invalidation, with a distinct format/version from the general CI memo.
    format!(
        "completed-jobs://{}/{repository}/{}/{}#release-v1-{version}",
        client.hostname(),
        run["id"],
        run["run_attempt"]
    )
}

pub(super) async fn cached(
    client: &Client,
    repository: &str,
    run: &Value,
) -> Result<Option<Vec<Value>>> {
    if run["status"] != "completed" {
        return Ok(None);
    }
    let Some(response) = client
        .peek_derived(&memo_key(client, repository, run))
        .await?
    else {
        return Ok(None);
    };
    if crate::now_ms().saturating_sub(response.validated_at_ms) >= 86400 * 1000 {
        return Ok(None);
    }
    if response.data.to_string().len() > client.collection_limit() {
        return Err(Error::Invalid(
            "release jobs exceed collection byte limit".into(),
        ));
    }
    // A freshly validated parent certifies this immutable version, just as for
    // REST completed-job pages. Looking up a memo is not a new validation.
    Ok(Some(response.decode()?))
}

pub(super) async fn save(
    client: &Client,
    repository: &str,
    run: &Value,
    jobs: &[Value],
) -> Result<()> {
    if run["status"] == "completed"
        && (!jobs.is_empty() || crate::report::settled_cancelled(run))
        && jobs.iter().all(|j| j["status"] == "completed")
    {
        client
            .save_derived(&memo_key(client, repository, run), json!(jobs))
            .await?;
    }
    Ok(())
}

pub(super) async fn fetch<'a>(
    client: &Client,
    runs: &[&'a Value],
    freshness: Freshness,
    prior_bytes: usize,
) -> Result<Vec<(&'a Value, Vec<Value>)>> {
    let mut by_node = BTreeMap::new();
    for run in runs {
        let id = text(&run["node_id"])?;
        if by_node.insert(id, *run).is_some() {
            return Err(invalid());
        }
    }
    let ids: Vec<_> = by_node.keys().copied().collect();
    let response = client.graphql(QUERY, json!({"ids":ids}), freshness).await?;
    let bytes = prior_bytes.saturating_add(response.data.to_string().len());
    if bytes > client.collection_limit() {
        return Err(Error::Invalid(
            "release jobs exceed collection byte limit".into(),
        ));
    }
    let nodes = response.data["data"]["nodes"]
        .as_array()
        .ok_or_else(invalid)?;
    if nodes.len() != runs.len() {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for node in nodes {
        let id = text(&node["id"])?;
        let run = *by_node.get(id).ok_or_else(invalid)?;
        if !seen.insert(id) {
            return Err(invalid());
        }
        let suite = &node["checkSuite"];
        if node["__typename"] != "WorkflowRun"
            || node["databaseId"] != run["id"]
            || node["runAttempt"] != run["run_attempt"]
            || suite["commit"]["oid"] != run["head_sha"]
            || !text(&suite["repository"]["nameWithOwner"])?
                .eq_ignore_ascii_case(text(&run["repository"]["full_name"])?)
            || timestamp(&node["updatedAt"])? != timestamp(&run["updated_at"])?
            || text(&suite["status"])?.to_ascii_lowercase() != text(&run["status"])?
            || conclusion(&suite["conclusion"])? != run["conclusion"]
        {
            return Err(invalid());
        }
        if timestamp(&suite["updatedAt"])? != timestamp(&run["updated_at"])? {
            // A suite can lag the run. Use the explicit REST attempt rather
            // than mixing those two versions of the evidence.
            continue;
        }
        if let Some(jobs) = decode_jobs(run, &suite["checkRuns"])? {
            result.push((run, jobs));
        }
    }
    if !result.is_empty() {
        // GraphQL resolves fields independently. A rerun starting between the
        // run selector and job connection must not relabel new jobs as attempt
        // one. Confirm every consumed version after collecting its evidence.
        let response = client
            .graphql(VERSIONS, json!({"ids":ids}), Freshness::Revalidate)
            .await?;
        if bytes.saturating_add(response.data.to_string().len()) > client.collection_limit() {
            return Err(Error::Invalid(
                "release jobs exceed collection byte limit".into(),
            ));
        }
        let nodes = response.data["data"]["nodes"]
            .as_array()
            .ok_or_else(invalid)?;
        if nodes.len() != runs.len() {
            return Err(invalid());
        }
        let mut seen = HashSet::new();
        for node in nodes {
            let id = text(&node["id"])?;
            let run = *by_node.get(id).ok_or_else(invalid)?;
            if !seen.insert(id)
                || node["databaseId"] != run["id"]
                || node["runAttempt"] != run["run_attempt"]
                || timestamp(&node["updatedAt"])? != timestamp(&run["updated_at"])?
            {
                return Err(invalid());
            }
        }
    }
    Ok(result)
}

fn connection(value: &Value) -> Result<Option<&Vec<Value>>> {
    let count = value["totalCount"].as_u64().ok_or_else(invalid)?;
    let more = value["pageInfo"]["hasNextPage"]
        .as_bool()
        .ok_or_else(invalid)?;
    let nodes = value["nodes"].as_array().ok_or_else(invalid)?;
    if more {
        return Ok(None);
    }
    if count != nodes.len() as u64 || count > 100 {
        return Err(invalid());
    }
    Ok(Some(nodes))
}

fn decode_jobs(run: &Value, value: &Value) -> Result<Option<Vec<Value>>> {
    let Some(nodes) = connection(value)? else {
        return Ok(None);
    };
    let mut jobs = Vec::new();
    let mut ids = HashSet::new();
    let mut complete = true;
    for node in nodes {
        let id = positive(&node["databaseId"])?;
        if !ids.insert(id) {
            return Err(invalid());
        }
        let name = text(&node["name"])?;
        let state = status(&node["status"])?;
        let result = conclusion(node.get("conclusion").ok_or_else(invalid)?)?;
        let completed = node.get("completedAt").ok_or_else(invalid)?;
        if !completed.is_null() {
            timestamp(completed)?;
        }
        let Some(raw_steps) = connection(&node["steps"])? else {
            complete = false;
            continue;
        };
        let mut steps = Vec::new();
        let mut numbers = HashSet::new();
        for step in raw_steps {
            let number = positive(&step["number"])?;
            if !numbers.insert(number) {
                return Err(invalid());
            }
            steps.push(json!({"number":number,"name":text(&step["name"])?,"status":status(&step["status"])?,"conclusion":conclusion(step.get("conclusion").ok_or_else(invalid)?)?}));
        }
        jobs.push(json!({"id":id,"run_id":run["id"],"run_attempt":run["run_attempt"],"head_sha":run["head_sha"],"name":name,"status":state,"conclusion":result,"completed_at":completed,"steps":steps}));
    }
    Ok(complete.then_some(jobs))
}
fn text(value: &Value) -> Result<&str> {
    value.as_str().filter(|s| !s.is_empty()).ok_or_else(invalid)
}
fn positive(value: &Value) -> Result<u64> {
    value.as_u64().filter(|n| *n > 0).ok_or_else(invalid)
}
fn status(value: &Value) -> Result<String> {
    let status = text(value)?.to_ascii_lowercase();
    if !matches!(
        status.as_str(),
        "completed" | "in_progress" | "queued" | "pending" | "waiting" | "requested"
    ) {
        return Err(invalid());
    }
    Ok(status)
}
fn conclusion(value: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let result = text(value)?.to_ascii_lowercase();
    if !matches!(
        result.as_str(),
        "success"
            | "failure"
            | "cancelled"
            | "skipped"
            | "neutral"
            | "timed_out"
            | "action_required"
            | "startup_failure"
    ) {
        return Err(invalid());
    }
    Ok(json!(result))
}
fn timestamp(value: &Value) -> Result<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(text(value)?).map_err(|_| invalid())
}
fn invalid() -> Error {
    Error::Invalid("missing or mismatched workflow job evidence".into())
}
