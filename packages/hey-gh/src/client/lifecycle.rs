//! Bounded personal-account lifecycle reads; never a substitute for REST/CI metadata.
use super::{Client, Freshness, validate_repository};
use crate::{Error, Result, Source, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const PR_LIFECYCLE_BATCH_LIMIT: usize = 25;
pub(crate) const CACHE_TAG: &str = "pr-lifecycle-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrLifecycleState {
    Open,
    Closed,
    Merged,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrLifecycle {
    pub number: u64,
    pub node_id: String,
    pub state: PrLifecycleState,
    pub title: String,
    pub author_id: Option<i64>,
    pub merged_at: Option<String>,
}
impl PrLifecycle {
    pub fn merged_at_ms(&self) -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(self.merged_at.as_deref()?)
            .ok()
            .map(|at| at.timestamp_millis())
            .filter(|at| *at > 0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrLifecycleError {
    pub number: u64,
    pub code: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrLifecycleBatch {
    pub repository: String,
    pub pull_requests: Vec<PrLifecycle>,
    pub errors: Vec<PrLifecycleError>,
    pub complete: bool,
    pub fetched_at_ms: u64,
    pub validated_at_ms: u64,
    pub source: Source,
}

pub(crate) fn numbers(repository: &str, input: &[u64]) -> Result<Vec<u64>> {
    validate_repository(repository)?;
    let mut numbers = input.to_vec();
    numbers.sort_unstable();
    if numbers.is_empty()
        || numbers.len() > PR_LIFECYCLE_BATCH_LIMIT
        || numbers.iter().any(|n| !(1..=i32::MAX as u64).contains(n))
        || numbers.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(Error::Invalid(
            "lifecycle batches require 1..25 distinct positive GraphQL PR numbers".into(),
        ));
    }
    Ok(numbers)
}

fn body(repository: &str, numbers: &[u64]) -> Value {
    let (owner, repo) = repository.split_once('/').expect("validated repository");
    let mut variables = json!({"owner":owner,"repo":repo});
    let mut params = String::new();
    let mut fields = String::new();
    for (index, number) in numbers.iter().enumerate() {
        params.push_str(&format!(", $n{index}: Int!"));
        variables[format!("n{index}")] = json!(number);
        fields.push_str(&format!("p{index}: pullRequest(number: $n{index}) {{ id number state merged title mergedAt closedAt updatedAt repository {{ id nameWithOwner }} author {{ __typename ... on User {{ databaseId }} ... on Bot {{ databaseId }} }} }}\n"));
    }
    json!({"query":format!("query PrLifecycles($owner: String!, $repo: String!{params}) {{ repository(owner: $owner, name: $repo) {{ id nameWithOwner {fields} }} }}"),"variables":variables})
}

fn request_numbers(request: &Value) -> Option<(String, Vec<u64>)> {
    let variables = request["variables"].as_object()?;
    let repository = format!(
        "{}/{}",
        variables.get("owner")?.as_str()?,
        variables.get("repo")?.as_str()?
    );
    let count = variables.len().checked_sub(2)?;
    if !(1..=PR_LIFECYCLE_BATCH_LIMIT).contains(&count) {
        return None;
    }
    let input = (0..count)
        .map(|index| variables.get(&format!("n{index}"))?.as_u64())
        .collect::<Option<Vec<_>>>()?;
    let canonical = numbers(&repository, &input).ok()?;
    (body(&repository, &canonical) == *request).then_some((repository, canonical))
}

fn scoped_errors(data: &Value, count: usize) -> Option<BTreeMap<usize, &'static str>> {
    let mut denied = BTreeMap::new();
    for error in data["errors"].as_array()? {
        let code = match error["type"].as_str()? {
            "FORBIDDEN" => "access_denied",
            "NOT_FOUND" => "not_found",
            _ => return None,
        };
        let path = error["path"].as_array()?;
        if path.len() < 2 || path[0] != "repository" {
            return None;
        }
        let alias = path[1].as_str()?;
        let index: usize = alias.strip_prefix('p')?.parse().ok()?;
        if index >= count
            || alias != format!("p{index}")
            || !path[2..].iter().all(|part| {
                part.as_u64().is_some() || part.as_str().is_some_and(|part| !part.is_empty())
            })
        {
            return None;
        }
        denied.insert(index, code);
    }
    (!denied.is_empty()).then_some(denied)
}

// Only this tagged, fixed read may retain partial success. Generic GraphQL
// callers still receive their existing operation error and cannot share this cache.
pub(crate) fn capture_partial(
    key: &str,
    request: Option<&Value>,
    mut data: Value,
) -> Option<Value> {
    let base = key.split("#repository-generation=").next()?;
    if !base.ends_with(&format!("#{CACHE_TAG}")) {
        return None;
    }
    let (repository, numbers) = request_numbers(request?)?;
    let repo = &data["data"]["repository"];
    if !repo["nameWithOwner"]
        .as_str()?
        .eq_ignore_ascii_case(&repository)
        || repo["id"].as_str()?.is_empty()
    {
        return None;
    }
    for (index, _) in scoped_errors(&data, numbers.len())? {
        // An error anywhere under a PR invalidates that entire PR, even if
        // GitHub supplied enough other fields to look terminal.
        data["data"]["repository"][format!("p{index}")] = Value::Null;
    }
    Some(data)
}

fn timestamp(value: &Value) -> bool {
    value.as_str().is_some_and(|v| {
        chrono::DateTime::parse_from_rfc3339(v).is_ok_and(|at| at.timestamp_millis() > 0)
    })
}
fn decode(node: &Value, repository: &str, repo_id: &Value, number: u64) -> Option<PrLifecycle> {
    if node["number"] != number
        || node["repository"]["id"] != *repo_id
        || !node["repository"]["nameWithOwner"]
            .as_str()?
            .eq_ignore_ascii_case(repository)
        || !timestamp(&node["updatedAt"])
    {
        return None;
    }
    let node_id = node["id"].as_str().filter(|id| !id.is_empty())?.to_owned();
    let state = match (node["state"].as_str()?, node["merged"].as_bool()?) {
        ("OPEN", false)
            if node.get("closedAt") == Some(&Value::Null)
                && node.get("mergedAt") == Some(&Value::Null) =>
        {
            PrLifecycleState::Open
        }
        ("CLOSED", false)
            if timestamp(&node["closedAt"]) && node.get("mergedAt") == Some(&Value::Null) =>
        {
            PrLifecycleState::Closed
        }
        ("MERGED", true) if timestamp(&node["closedAt"]) && timestamp(&node["mergedAt"]) => {
            PrLifecycleState::Merged
        }
        _ => return None,
    };
    let author = node.get("author")?;
    let author_id = matches!(author["__typename"].as_str(), Some("User" | "Bot"))
        .then(|| author["databaseId"].as_i64().filter(|id| *id > 0))
        .flatten();
    Some(PrLifecycle {
        number,
        node_id,
        state,
        title: node["title"].as_str()?.into(),
        author_id,
        merged_at: node["mergedAt"].as_str().map(str::to_owned),
    })
}

impl Client {
    async fn lifecycle_superseded(
        &self,
        repository: &str,
        record: &PrLifecycle,
        validated_at_ms: u64,
    ) -> Result<bool> {
        let mut metadata = vec![
            self.peek_get(&format!("repos/{repository}/pulls/{}", record.number))
                .await,
        ];
        if self.ci_uses_installation(repository) {
            metadata.push(
                self.ci_pr_response(repository, record.number, Freshness::CachedOnly)
                    .await,
            );
        }
        for response in metadata {
            let response = match response {
                Ok(response) => response,
                Err(Error::CacheMiss) => continue,
                Err(error) => return Err(error),
            };
            if response.validated_at_ms < validated_at_ms || response.validated_at_ms > now_ms() {
                continue;
            }
            let data = &response.data;
            if data["number"] != record.number
                || !data["base"]["repo"]["full_name"]
                    .as_str()
                    .is_some_and(|repo| repo.eq_ignore_ascii_case(repository))
            {
                continue;
            }
            let state = match (data["state"].as_str(), data["merged"].as_bool()) {
                (Some("open"), Some(false)) => PrLifecycleState::Open,
                (Some("closed"), Some(false)) => PrLifecycleState::Closed,
                (Some("closed"), Some(true)) => PrLifecycleState::Merged,
                _ => continue,
            };
            if state != record.state
                || data["node_id"]
                    .as_str()
                    .is_some_and(|id| id != record.node_id)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Reads only lifecycle/title/authorship, using personal GraphQL quota.
    /// Does not register watches, populate REST metadata, or establish CI readiness.
    pub async fn pr_lifecycles(
        &self,
        repository: &str,
        input: &[u64],
        freshness: Freshness,
    ) -> Result<PrLifecycleBatch> {
        let started = std::time::Instant::now();
        let numbers = numbers(repository, input)?;
        let repository = repository.to_ascii_lowercase();
        let generation = self.repository_generation(&repository).await?;
        let request = body(&repository, &numbers);
        let response = self
            .request_versioned(
                self.0.config.graphql_url.to_string(),
                Some(request.clone()),
                freshness,
                Some(CACHE_TAG),
                false,
            )
            .await?;
        if response.validated_at_ms == 0
            || response.validated_at_ms > now_ms().saturating_add(30_000)
            || self.repository_generation(&repository).await? != generation
        {
            return Err(crate::entity::changed());
        }
        let denied = if response
            .data
            .get("errors")
            .is_some_and(|errors| errors.as_array().is_none_or(|errors| !errors.is_empty()))
        {
            scoped_errors(&response.data, numbers.len())
                .ok_or_else(|| Error::Invalid("Unscoped lifecycle errors".into()))?
        } else {
            BTreeMap::new()
        };
        let repo = &response.data["data"]["repository"];
        if !repo["nameWithOwner"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(&repository))
            || !repo["id"].as_str().is_some_and(|id| !id.is_empty())
        {
            return Err(Error::Invalid(
                "Lifecycle repository identity is unavailable".into(),
            ));
        }
        let mut duplicate_ids = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for index in 0..numbers.len() {
            if !denied.contains_key(&index)
                && let Some(id) = repo[format!("p{index}")]["id"].as_str()
                && !ids.insert(id)
            {
                duplicate_ids.insert(id);
            }
        }
        let mut pull_requests = Vec::new();
        let mut errors = Vec::new();
        for (index, number) in numbers.into_iter().enumerate() {
            let node = &repo[format!("p{index}")];
            let record = decode(node, &repository, &repo["id"], number);
            let mut code = denied.get(&index).copied();
            if code.is_none()
                && record
                    .as_ref()
                    .is_none_or(|record| duplicate_ids.contains(record.node_id.as_str()))
            {
                code = Some("invalid_metadata");
            }
            if let Some(code) = code {
                errors.push(PrLifecycleError {
                    number,
                    code: code.into(),
                });
                continue;
            }
            let record = record.unwrap();
            if self
                .lifecycle_superseded(&repository, &record, response.validated_at_ms)
                .await?
            {
                errors.push(PrLifecycleError {
                    number,
                    code: "metadata_changed".into(),
                });
                continue;
            }
            let accepted = match self.expected_pr_node(&repository, number).await? {
                Some(expected) => expected == record.node_id,
                None if matches!(response.source, Source::Cache) => false,
                None => {
                    self.0
                        .store
                        .accept_rest_identity(
                            &self.0.scope,
                            &repository,
                            number,
                            &record.node_id,
                            response.validated_at_ms,
                            &[
                                format!("pr-status://{}/{repository}/{number}", self.hostname()),
                                format!("metadata://{}/{repository}/{number}", self.hostname()),
                            ],
                        )
                        .await?
                }
            };
            let owner = crate::store::PrOwner {
                repository: repository.clone(),
                number,
                node_id: Some(record.node_id.clone()),
                generation,
            };
            if accepted && self.0.store.owner_is_current(&self.0.scope, &owner).await? {
                pull_requests.push(record);
            } else {
                errors.push(PrLifecycleError {
                    number,
                    code: "identity_changed".into(),
                });
            }
        }
        if self.repository_generation(&repository).await? != generation {
            return Err(crate::entity::changed());
        }
        tracing::info!(repository=%repository, requested=input.len(), succeeded=pull_requests.len(), unresolved=errors.len(), elapsed_ms=started.elapsed().as_millis() as u64, source=?response.source, "PR lifecycle batch completed");
        Ok(PrLifecycleBatch {
            repository,
            pull_requests,
            complete: errors.is_empty(),
            errors,
            fetched_at_ms: response.fetched_at_ms,
            validated_at_ms: response.validated_at_ms,
            source: response.source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_success_requires_the_fixed_tagged_query_and_scoped_known_errors() {
        let request = body("acme/demo", &[1, 2]);
        let key =
            format!("https://api.github.com/graphql#hash#{CACHE_TAG}#repository-generation=7");
        let data = json!({"data":{"repository":{"id":"R_demo","nameWithOwner":"acme/demo","p0":{"marker":"retained"},"p1":{"marker":"must be removed"}}},"errors":[{"type":"FORBIDDEN","path":["repository","p1","author"]}]});
        let retained = capture_partial(&key, Some(&request), data.clone()).unwrap();
        assert_eq!(
            retained["data"]["repository"]["p0"],
            data["data"]["repository"]["p0"]
        );
        assert!(retained["data"]["repository"]["p1"].is_null());
        assert_eq!(retained["errors"], data["errors"]);
        for path in [
            json!([]),
            json!(["repository"]),
            json!(["other", "p1"]),
            json!(["repository", "p99"]),
            json!(["repository", "p01"]),
            json!(["repository", 1]),
            json!(["repository", "p1", ""]),
        ] {
            let mut invalid = data.clone();
            invalid["errors"][0]["path"] = path;
            assert!(capture_partial(&key, Some(&request), invalid).is_none());
        }
        for kind in ["RATE_LIMITED", "UNAUTHORIZED", "INTERNAL", "UNKNOWN"] {
            let mut invalid = data.clone();
            invalid["errors"][0]["type"] = json!(kind);
            assert!(capture_partial(&key, Some(&request), invalid).is_none());
        }
        for field in ["query", "variables"] {
            let mut invalid = request.clone();
            invalid[field] = Value::Null;
            assert!(capture_partial(&key, Some(&invalid), data.clone()).is_none());
        }
        assert!(
            capture_partial("https://api.github.com/graphql#hash", Some(&request), data).is_none()
        );
    }
    async fn cached_fixture(installation: bool) -> (tempfile::TempDir, Client, u64) {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            super::super::Config {
                installation: installation.then(|| {
                    crate::AppInstallation::new(
                        "synthetic-client".into(),
                        42,
                        vec!["acme/demo".into()],
                        include_str!("../../tests/fixtures/github-app-test-key.pem"),
                    )
                    .unwrap()
                }),
                rest_url: "http://127.0.0.1:9/".parse().unwrap(),
                graphql_url: "http://127.0.0.1:9/graphql".parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                ..super::super::Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let stamp = now_ms() - 2000;
        let request = body("acme/demo", &[1]);
        let key = format!(
            "{}#{}#{CACHE_TAG}",
            client.0.config.graphql_url,
            crate::digest(&request.to_string())
        );
        let response = crate::Response {
            data: json!({"data":{"repository":{"id":"R_demo","nameWithOwner":"acme/demo","p0":{"id":"PR_1","number":1,"state":"OPEN","merged":false,"title":"PR","mergedAt":null,"closedAt":null,"updatedAt":"2026-10-01T00:00:00Z","author":{"databaseId":42},"repository":{"id":"R_demo","nameWithOwner":"acme/demo"}}}}}),
            fetched_at_ms: stamp,
            validated_at_ms: stamp,
            source: Source::Network,
            etag: None,
            last_modified: None,
            link: None,
        };
        client
            .0
            .store
            .put(&client.0.scope, &key, &response)
            .await
            .unwrap();
        client
            .0
            .store
            .accept_rest_identity(&client.0.scope, "acme/demo", 1, "PR_1", stamp, &[])
            .await
            .unwrap();
        (dir, client, stamp)
    }

    #[tokio::test]
    async fn newer_personal_or_installation_lifecycle_prevents_cached_rollback() {
        for installation in [false, true] {
            let (_dir, client, stamp) = cached_fixture(installation).await;
            let key = format!(
                "{}repos/acme/demo/pulls/1{}",
                client.0.config.rest_url,
                if installation {
                    "#installation-ci-pr"
                } else {
                    ""
                }
            );
            for (clock, blocked) in [
                (stamp - 1, false),
                (stamp, true),
                (stamp + 1, true),
                (now_ms() + 60_000, false),
            ] {
                client.0.store.put(&client.0.scope,&key,&crate::Response {
                    data:json!({"node_id":"PR_1","number":1,"state":"closed","merged":true,"base":{"repo":{"full_name":"acme/demo"}}}),
                    fetched_at_ms:clock,validated_at_ms:clock,source:Source::Network,etag:None,last_modified:None,link:None,
                }).await.unwrap();
                let report = client
                    .pr_lifecycles("acme/demo", &[1], Freshness::CachedOnly)
                    .await
                    .unwrap();
                assert_eq!(report.complete, !blocked);
                assert_eq!(report.pull_requests.len(), usize::from(!blocked));
                if blocked {
                    assert_eq!(report.errors[0].code, "metadata_changed");
                }
            }
        }
    }

    #[tokio::test]
    async fn repository_generation_change_during_cache_lookup_rejects_retired_lifecycle() {
        let (_dir, client, stamp) = cached_fixture(false).await;
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let resume = std::sync::Arc::new(tokio::sync::Notify::new());
        let reader = client.clone();
        let gate = (entered.clone(), resume.clone());
        let reading = tokio::spawn(async move {
            super::super::CACHE_LOOKUP_GATE
                .scope(
                    std::cell::RefCell::new(Some(gate)),
                    reader.pr_lifecycles("acme/demo", &[1], Freshness::CachedOnly),
                )
                .await
        });
        entered.notified().await;
        assert!(
            client
                .0
                .store
                .accept_rest_identity(&client.0.scope, "acme/demo", 1, "PR_new", stamp + 1000, &[])
                .await
                .unwrap()
        );
        resume.notify_one();
        assert!(matches!(reading.await.unwrap(), Err(Error::CacheMiss)));
    }
}
