//! Reuse recent batched selectors without refreshing unrelated REST metadata.
use crate::{Client, Error, Freshness, Response, Result, Source, now_ms};
use serde_json::Value;

pub(super) enum CiMetadata {
    Rest(Response),
    Discovery { cached: Response, validated_at: u64 },
}

impl CiMetadata {
    pub(super) fn data(&self) -> &Value {
        match self {
            Self::Rest(response) => &response.data,
            Self::Discovery { cached, .. } => &cached.data,
        }
    }

    pub(super) fn validated_at(&self) -> u64 {
        match self {
            Self::Rest(response) => response.validated_at_ms,
            Self::Discovery { validated_at, .. } => *validated_at,
        }
    }

    pub(super) fn rest_observation(&self) -> Option<&Response> {
        match self {
            Self::Rest(response) => Some(response),
            Self::Discovery { .. } => None,
        }
    }
}

impl Client {
    pub(super) async fn ci_metadata(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<CiMetadata> {
        if let Freshness::MaxAge(age) = freshness
            && !age.is_zero()
            && let Some(metadata) = self.discovery_ci_metadata(repository, number, age).await?
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
        Ok(CiMetadata::Rest(
            self.pull_request(repository, number, freshness).await?,
        ))
    }

    async fn discovery_ci_metadata(
        &self,
        repository: &str,
        number: u64,
        age: std::time::Duration,
    ) -> Result<Option<CiMetadata>> {
        let cached = match self
            .peek_get(&format!("repos/{repository}/pulls/{number}"))
            .await
        {
            Ok(response) => response,
            Err(Error::CacheMiss) => return Ok(None),
            Err(error) => return Err(error),
        };
        // Already fresh REST evidence needs no account-wide cache lookup.
        if now_ms()
            .checked_sub(cached.validated_at_ms)
            .is_some_and(|elapsed| (elapsed as u128) < age.as_millis())
        {
            return Ok(None);
        }
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
        let Some(validated_at) = scan.data["validatedAtByPr"]
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
        if validated_at == 0
            || validated_at < cached.validated_at_ms
            || now_ms()
                .checked_sub(validated_at)
                .is_none_or(|elapsed| elapsed as u128 >= age.as_millis())
        {
            return Ok(None);
        }
        let pr = &cached.data;
        let merge = &node["potentialMergeCommit"];
        let valid_sha = crate::repository::valid_sha;
        if node["state"] != "OPEN"
            || node["mergeable"] != "MERGEABLE"
            || pr["state"] != "open"
            || pr["merged"] == true
            || !node["id"].as_str().is_some_and(|id| !id.is_empty())
            || node["id"] != pr["node_id"]
            || !node["headRefOid"].as_str().is_some_and(valid_sha)
            || !node["baseRefOid"].as_str().is_some_and(valid_sha)
            || node["headRefOid"] != pr["head"]["sha"]
            || node["baseRefOid"] != pr["base"]["sha"]
            || !merge["oid"].as_str().is_some_and(valid_sha)
            || merge["oid"] != pr["merge_commit_sha"]
            || merge["parents"]["totalCount"] != 2
            || !merge["parents"]["nodes"].as_array().is_some_and(|parents| {
                parents.len() == 2
                    && parents
                        .iter()
                        .all(|parent| parent["oid"].as_str().is_some_and(valid_sha))
                    && parents
                        .iter()
                        .any(|parent| parent["oid"] == node["headRefOid"])
            })
        {
            return Ok(None);
        }
        // Native stacks merge against their trunk, which need not be the PR's
        // immediate diff base. Preserve the observed test-merge SHA and parents;
        // do not infer a policy branch from these selectors.
        Ok(Some(CiMetadata::Discovery {
            cached,
            validated_at,
        }))
    }
}
