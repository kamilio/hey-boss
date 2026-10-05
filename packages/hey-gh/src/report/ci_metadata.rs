//! Confirm CI selectors without refreshing unrelated REST metadata.
use crate::{Client, Error, Freshness, Response, Result, Source, now_ms};
use serde_json::{Value, json};
use std::time::Duration;

const CI_SELECTORS: &str = r#"query CiSelectors($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      id number state merged mergeable headRefOid baseRefOid
      repository { nameWithOwner }
      commits(last: 1) { nodes { commit { oid status { id } } } }
      potentialMergeCommit { oid status { id } parents(first: 2) { totalCount nodes { oid } } }
    }
  }
}"#;

fn recent(response: &Response, age: Duration) -> bool {
    now_ms()
        .checked_sub(response.validated_at_ms)
        .is_some_and(|elapsed| (elapsed as u128) < age.as_millis())
}

fn matches_selectors(pr: &Value, node: &Value, repository: &str, number: u64) -> bool {
    let merge = &node["potentialMergeCommit"];
    let valid_sha = crate::repository::valid_sha;
    node["number"] == number
        && node["repository"]["nameWithOwner"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(repository))
        && node["state"] == "OPEN"
        && node["mergeable"] == "MERGEABLE"
        && pr["state"] == "open"
        && pr["merged"] != true
        && node["id"].as_str().is_some_and(|id| !id.is_empty())
        && node["id"] == pr["node_id"]
        && node["headRefOid"].as_str().is_some_and(valid_sha)
        && node["baseRefOid"].as_str().is_some_and(valid_sha)
        && node["headRefOid"] == pr["head"]["sha"]
        && node["baseRefOid"] == pr["base"]["sha"]
        && merge["oid"].as_str().is_some_and(valid_sha)
        && merge["oid"] == pr["merge_commit_sha"]
        && merge["parents"]["totalCount"] == 2
        && merge["parents"]["nodes"].as_array().is_some_and(|parents| {
            parents.len() == 2
                && parents
                    .iter()
                    .all(|parent| parent["oid"].as_str().is_some_and(valid_sha))
                && parents
                    .iter()
                    .any(|parent| parent["oid"] == node["headRefOid"])
        })
}

pub(super) enum CiMetadata {
    Rest(Response),
    Selectors { cached: Response, validated_at: u64 },
}

impl CiMetadata {
    pub(super) fn data(&self) -> &Value {
        match self {
            Self::Rest(response) => &response.data,
            Self::Selectors { cached, .. } => &cached.data,
        }
    }

    pub(super) fn validated_at(&self) -> u64 {
        match self {
            Self::Rest(response) => response.validated_at_ms,
            Self::Selectors { validated_at, .. } => *validated_at,
        }
    }

    pub(super) fn rest_observation(&self) -> Option<&Response> {
        match self {
            Self::Rest(response) => Some(response),
            Self::Selectors { .. } => None,
        }
    }
}

impl Client {
    pub(super) async fn empty_commit_statuses(
        &self,
        repository: &str,
        sha: &str,
        rest_path: &str,
        freshness: Freshness,
    ) -> Result<bool> {
        if matches!(freshness, Freshness::Revalidate)
            || matches!(freshness, Freshness::MaxAge(age) if age.is_zero())
        {
            return Ok(false);
        }
        let Some(owner) = crate::entity::current().filter(|owner| {
            owner.repository.eq_ignore_ascii_case(repository) && owner.node_id.is_some()
        }) else {
            return Ok(false);
        };
        let (repo_owner, repo) = repository.split_once('/').expect("validated repository");
        // Only reuse a query already made for metadata. Do not add a network
        // request, wait for another collector, or relabel a rollup as full CI.
        let response = match self
            .peek_graphql(
                CI_SELECTORS,
                json!({"owner":repo_owner,"repo":repo,"number":owner.number}),
            )
            .await
        {
            Ok(response) => response,
            Err(Error::CacheMiss) => return Ok(false),
            Err(error) => return Err(error),
        };
        if response.validated_at_ms == 0
            || response.validated_at_ms > now_ms()
            || matches!(freshness, Freshness::MaxAge(age) if !recent(&response, age))
        {
            return Ok(false);
        }
        let pr = &response.data["data"]["repository"]["pullRequest"];
        if pr["id"].as_str() != owner.node_id.as_deref()
            || pr["number"] != owner.number
            || !pr["repository"]["nameWithOwner"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(repository))
        {
            return Ok(false);
        }
        // Commit.status is null only when no legacy statuses exist. Missing
        // fields, null commits, and nonempty statuses are not empty evidence.
        // Nonempty statuses retain REST's numeric IDs and complete payloads.
        let empty =
            |commit: &Value| commit["oid"] == sha && commit.get("status") == Some(&Value::Null);
        let head_empty = pr["headRefOid"] == sha
            && pr["commits"]["nodes"]
                .as_array()
                .is_some_and(|nodes| nodes.len() == 1 && empty(&nodes[0]["commit"]));
        if !head_empty && !empty(&pr["potentialMergeCommit"]) {
            return Ok(false);
        }
        match self.peek_get(rest_path).await {
            // A later REST read can observe a newly posted status. Never let
            // older empty evidence hide it, including during offline reads.
            Ok(rest) if rest.validated_at_ms >= response.validated_at_ms => return Ok(false),
            Ok(_) | Err(Error::CacheMiss) => {}
            Err(error) => return Err(error),
        }
        super::record_validation(
            &format!(
                "graphql://{}/{repository}/pulls/{}#commit-statuses:{sha}",
                self.hostname(),
                owner.number
            ),
            &response,
        );
        Ok(true)
    }

    pub(super) async fn initial_ci_metadata(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<CiMetadata> {
        let pr = self.ci_metadata(repository, number, freshness).await?;
        crate::entity::set(self.pr_owner(repository, number, pr.data()).await?);
        // Lifecycle evidence must remain independent of slow CI sources.
        if super::can_publish() && pr.rest_observation().is_some() {
            self.observe(
                &format!("metadata://{}/{repository}/{number}", self.hostname()),
                &serde_json::json!({"conflicts":super::conflicts(pr.data()),"pull_request":pr.data()}),
            )
            .await?;
            self.publish_individual_pr_status(repository, number, &[])
                .await?;
        }
        Ok(pr)
    }

    pub(super) async fn ci_metadata(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<CiMetadata> {
        if let Freshness::MaxAge(age) = freshness
            && !age.is_zero()
            && let Some(cached) = self.cached_pr_seed(repository, number, freshness).await?
            && !recent(&cached, age)
        {
            if let Some(metadata) = self
                .discovery_ci_metadata(repository, number, age, &cached)
                .await?
            {
                let _ = super::VALIDATIONS.try_with(|records| {
                    records.borrow_mut().push(super::ResourceValidation {
                        resource: format!(
                            "my-open-prs://{}/{repository}/{number}#ci-selectors",
                            self.hostname()
                        ),
                        validated_at_ms: metadata.validated_at(),
                        source: Source::Cache,
                    });
                });
                return Ok(metadata);
            }
            if cached.data["mergeable"] == true
                && cached.data["merge_commit_sha"]
                    .as_str()
                    .is_some_and(crate::repository::valid_sha)
            {
                let (owner, repo) = repository.split_once('/').expect("validated repository");
                // Confirm just the CI selectors in the main account's GraphQL
                // quota. Preserve REST capacity for complete metadata/detail reads,
                // and leave time for REST if this optional path stalls.
                // Only consumed selector evidence belongs in the report's
                // validation clocks; malformed or mismatched optional reads
                // must not age a successful REST fallback.
                let (result, validations) = super::VALIDATIONS
                    .scope(std::cell::RefCell::new(Vec::new()), async {
                        let result = tokio::time::timeout(
                            Duration::from_secs(2),
                            self.graphql(
                                CI_SELECTORS,
                                json!({"owner":owner,"repo":repo,"number":number}),
                                freshness,
                            ),
                        )
                        .await;
                        (
                            result,
                            super::VALIDATIONS.with(|records| records.borrow().clone()),
                        )
                    })
                    .await;
                match result {
                    Ok(Ok(response))
                        if response.validated_at_ms >= cached.validated_at_ms
                            && recent(&response, age)
                            && response.data["data"]["repository"]["pullRequest"]["merged"]
                                == false
                            && matches_selectors(
                                &cached.data,
                                &response.data["data"]["repository"]["pullRequest"],
                                repository,
                                number,
                            ) =>
                    {
                        super::VALIDATIONS.with(|records| records.borrow_mut().extend(validations));
                        return Ok(CiMetadata::Selectors {
                            cached,
                            validated_at: response.validated_at_ms,
                        });
                    }
                    Ok(Err(
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
                    )) => return Err(error),
                    _ => {}
                }
                // Changed or ambiguous selectors must not certify the seed.
                return Ok(CiMetadata::Rest(
                    self.pull_request(repository, number, Freshness::Revalidate)
                        .await?,
                ));
            }
        }
        Ok(CiMetadata::Rest(
            self.pull_request(repository, number, freshness).await?,
        ))
    }

    async fn discovery_ci_metadata(
        &self,
        repository: &str,
        number: u64,
        age: std::time::Duration,
        cached: &Response,
    ) -> Result<Option<CiMetadata>> {
        let scan = match self.peek_derived(crate::dashboard::DISCOVERY_CACHE).await {
            Ok(Some(scan)) => scan,
            Ok(None) => return Ok(None),
            Err(error) => {
                // This optional account memo must not prevent an independent,
                // fully validated REST CI read from recovering.
                tracing::warn!(
                    error_code = error.diagnostic_code(),
                    "CI discovery memo unavailable; validating REST metadata"
                );
                return Ok(None);
            }
        };
        let Some(nodes) = scan.data["pulls"].as_array() else {
            return Ok(None);
        };
        let Some(node) = nodes.iter().find(|node| {
            node["number"] == number
                && node["repository"]["nameWithOwner"]
                    .as_str()
                    .is_some_and(|repo| repo.eq_ignore_ascii_case(repository))
        }) else {
            return Ok(None);
        };
        let key = format!("{repository}/{number}");
        let Some(mut validated_at) = scan.data["validatedAtByPr"]
            .as_object()
            .and_then(|clocks| {
                clocks
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(&key))
            })
            .and_then(|(_, clock)| clock.as_u64())
        else {
            return Ok(None);
        };
        let mut node = node.clone();
        let page_after = scan.data["pageAfterByPr"].as_object().and_then(|pages| {
            pages
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&key))
                .map(|(_, after)| after)
        });
        if let Some(after) = page_after
            .filter(|after| after.is_null() || after.as_str().is_some_and(|s| !s.is_empty()))
        {
            match self.cached_discovery_page(after.clone()).await {
                Ok(page) if page.validated_at_ms > validated_at => {
                    // A point observation does not need a complete account scan.
                    // The old cursor is only a hint: membership can move between
                    // pages, so require exactly one matching identity again.
                    let mut matches = page.data["data"]["viewer"]["pullRequests"]["nodes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|candidate| {
                            candidate["number"] == number
                                && candidate["repository"]["nameWithOwner"]
                                    .as_str()
                                    .is_some_and(|repo| repo.eq_ignore_ascii_case(repository))
                        });
                    let Some(candidate) = matches.next() else {
                        return Ok(None);
                    };
                    if matches.next().is_some() {
                        return Ok(None);
                    }
                    node = candidate.clone();
                    validated_at = page.validated_at_ms;
                }
                Ok(_) | Err(Error::CacheMiss) => {}
                Err(error) => {
                    tracing::warn!(
                        error_code = error.diagnostic_code(),
                        "CI discovery page unavailable; validating REST metadata"
                    );
                    return Ok(None);
                }
            }
        }
        if validated_at == 0
            || validated_at < cached.validated_at_ms
            || now_ms()
                .checked_sub(validated_at)
                .is_none_or(|elapsed| elapsed as u128 >= age.as_millis())
        {
            return Ok(None);
        }
        if !matches_selectors(&cached.data, &node, repository, number) {
            return Ok(None);
        }
        // Native stacks merge against their trunk, which need not be the PR's
        // immediate diff base. Preserve the observed test-merge SHA and parents;
        // do not infer a policy branch from these selectors.
        Ok(Some(CiMetadata::Selectors {
            cached: cached.clone(),
            validated_at,
        }))
    }
}
