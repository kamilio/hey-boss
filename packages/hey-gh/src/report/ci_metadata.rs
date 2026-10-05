//! Confirm CI selectors without refreshing unrelated REST metadata.
use crate::{Client, Error, Freshness, Response, Result, Source, now_ms};
use serde_json::Value;
use std::time::Duration;

pub(super) mod discovery;
mod status_versions;

struct VersionEvidence {
    versions: status_versions::Versions,
    at: u64,
    resource: String,
}

struct ListEvidence {
    empty: bool,
    at: u64,
    resource: String,
    versions: Option<VersionEvidence>,
}

#[derive(Clone, Copy)]
pub(super) enum CommitList {
    Statuses,
    Checks,
}

impl CommitList {
    fn field(self) -> &'static str {
        match self {
            Self::Statuses => "statuses",
            Self::Checks => "check_runs",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Statuses => "commit-statuses",
            Self::Checks => "check-runs",
        }
    }
}

fn list_empty(pr: &Value, sha: &str, list: CommitList) -> Option<bool> {
    let evidence = |commit: &Value| {
        if commit["oid"] != sha {
            return None;
        }
        match list {
            CommitList::Statuses => match commit.get("status")? {
                Value::Null => Some(true),
                value if value["id"].as_str().is_some_and(|id| !id.is_empty()) => Some(false),
                _ => None,
            },
            CommitList::Checks => match commit.get("statusCheckRollup")? {
                Value::Null => Some(true),
                value => value["contexts"]["checkRunCount"].as_u64().map(|n| n == 0),
            },
        }
    };
    let head = pr["commits"]["nodes"]
        .as_array()
        .filter(|nodes| nodes.len() == 1 && pr["headRefOid"] == sha)
        .and_then(|nodes| evidence(&nodes[0]["commit"]));
    let merge = evidence(&pr["potentialMergeCommit"]);
    match (head, merge) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), _) | (_, Some(true)) => Some(true),
        _ => None,
    }
}

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
    pub(super) async fn cached_ci_pr_seed(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Option<Response>> {
        if number > 0 && matches!(freshness, Freshness::MaxAge(age) if !age.is_zero()) {
            return Ok(self
                .peek_ci_pull_request(repository, number)
                .await?
                .filter(|cached| super::usable_pr_seed(&cached.data, number)));
        }
        Ok(None)
    }

    pub(super) async fn commit_list_from_metadata(
        &self,
        repository: &str,
        sha: &str,
        rest_path: &str,
        list: CommitList,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        if !matches!(freshness, Freshness::Revalidate)
            && !matches!(freshness, Freshness::MaxAge(age) if age.is_zero())
            && let Some(ListEvidence {
                empty,
                at,
                resource,
                versions,
            }) = self
                .cached_list_evidence(repository, sha, list, freshness)
                .await?
        {
            let rest = match self.peek_get(rest_path).await {
                Ok(rest) => Some(rest),
                Err(Error::CacheMiss) => None,
                Err(error) => return Err(error),
            };
            // A later REST response can observe newly posted checks/statuses.
            // At the same clock, positive evidence from either source wins;
            // an empty payload cannot certify contradictory presence metadata.
            if rest.as_ref().is_none_or(|rest| {
                rest.validated_at_ms < at || (!empty && rest.validated_at_ms == at)
            }) {
                if empty {
                    let _ = super::VALIDATIONS.try_with(|records| {
                        records.borrow_mut().push(super::ResourceValidation {
                            resource,
                            validated_at_ms: at,
                            source: Source::Cache,
                        })
                    });
                    return Ok(Vec::new());
                }
                let mut changed = false;
                if let Some(proof) = versions
                    && rest
                        .as_ref()
                        .is_some_and(|r| r.data["sha"] == sha && r.validated_at_ms <= proof.at)
                {
                    match proof
                        .versions
                        .cached(self, rest_path, proof.at, freshness)
                        .await?
                    {
                        status_versions::Cached::Matching(values) => {
                            let _ = super::VALIDATIONS.try_with(|records| {
                                records.borrow_mut().push(super::ResourceValidation {
                                    resource: proof.resource,
                                    validated_at_ms: proof.at,
                                    source: Source::Cache,
                                })
                            });
                            return Ok(values);
                        }
                        status_versions::Cached::Changed => changed = true,
                        status_versions::Cached::Unavailable => {}
                    }
                }
                if changed
                    || rest.as_ref().is_none_or(|rest| {
                        rest.data[list.field()]
                            .as_array()
                            .is_some_and(Vec::is_empty)
                    })
                {
                    // Newer metadata contradicts the old list. Offline
                    // reads cannot invent the missing full REST payload.
                    if matches!(freshness, Freshness::CachedOnly) {
                        return Err(Error::CacheMiss);
                    }
                    let values = self
                        .pages(rest_path, Some(list.field()), Freshness::Revalidate)
                        .await?;
                    if values.is_empty() {
                        return Err(Error::Invalid(format!(
                            "{} disagree with newer CI metadata",
                            list.label().replace('-', " ")
                        )));
                    }
                    return Ok(values);
                }
            }
        }
        self.pages(rest_path, Some(list.field()), freshness).await
    }

    async fn cached_list_evidence(
        &self,
        repository: &str,
        sha: &str,
        list: CommitList,
        freshness: Freshness,
    ) -> Result<Option<ListEvidence>> {
        let Some(owner) = crate::entity::current().filter(|owner| {
            owner.repository.eq_ignore_ascii_case(repository) && owner.node_id.is_some()
        }) else {
            return Ok(None);
        };
        // These are cache peeks only. CI never dispatches account discovery or
        // an extra point query to obtain this optional list evidence.
        let mut candidates = Vec::new();
        match self
            .ci_selector_response(repository, owner.number, Freshness::CachedOnly)
            .await
        {
            Ok(response) => candidates.push((
                response.data["data"]["repository"]["pullRequest"].clone(),
                response.validated_at_ms,
                "graphql",
            )),
            Err(Error::CacheMiss) => {}
            Err(error) => return Err(error),
        }
        if let Some((node, at)) = discovery::read(self, repository, owner.number).await {
            candidates.push((node, at, "my-open-prs"));
        }
        let mut latest = None;
        let mut versions = None;
        let mut version_at = 0;
        for (node, at, source) in candidates {
            let Some(elapsed) = now_ms().checked_sub(at).filter(|_| at > 0) else {
                continue;
            };
            if matches!(freshness, Freshness::MaxAge(age) if elapsed as u128 >= age.as_millis())
                || node["id"].as_str() != owner.node_id.as_deref()
                || node["number"] != owner.number
                || !node["repository"]["nameWithOwner"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(repository))
            {
                continue;
            }
            let Some(empty) = list_empty(&node, sha, list) else {
                continue;
            };
            // Full rosters use their own page/point clock. Legacy presence
            // alone cannot renew a proof. Newer malformed version evidence or
            // disagreement at the same clock must not certify an older list.
            if matches!(list, CommitList::Statuses)
                && status_versions::Versions::has_fields(&node, sha)
            {
                let proof = status_versions::Versions::from_node(&node, sha);
                if at > version_at {
                    version_at = at;
                    versions = proof.map(|proof| VersionEvidence {
                        versions: proof,
                        at,
                        resource: format!(
                            "{source}://{}/{repository}/pulls/{}#commit-status-versions:{sha}",
                            self.hostname(),
                            owner.number
                        ),
                    });
                } else if at == version_at
                    && versions.as_ref().map(|v| &v.versions) != proof.as_ref()
                {
                    versions = None;
                }
            }
            // A newer nonempty observation must defeat an older empty one.
            // Conflicting observations at the same clock also retain REST.
            if latest.as_ref().is_none_or(|&(_, old_at, _)| at >= old_at) {
                if matches!(latest, Some((false, old_at, _)) if old_at == at) {
                    continue;
                }
                latest = Some((empty, at, source));
            }
        }
        Ok(latest.map(|(empty, at, source)| ListEvidence {
            empty,
            at,
            resource: format!(
                "{source}://{}/{repository}/pulls/{}#{}:{sha}",
                self.hostname(),
                owner.number,
                list.label()
            ),
            versions: versions.filter(|_| !empty),
        }))
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
            && let Some(cached) = self
                .cached_ci_pr_seed(repository, number, freshness)
                .await?
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
                let installation = self.ci_uses_installation(repository);
                // The installation route is authoritative and retains the caller's
                // normal deadline. Personal GraphQL remains an optional shortcut
                // with time reserved for complete REST metadata on a stall.
                // Only consumed selector evidence belongs in the report's
                // validation clocks; malformed or mismatched optional reads
                // must not age a successful REST fallback.
                let (result, validations) = super::VALIDATIONS
                    .scope(std::cell::RefCell::new(Vec::new()), async {
                        let query = self.ci_selectors(repository, number, freshness);
                        let result = if installation {
                            query.await
                        } else {
                            tokio::time::timeout(Duration::from_secs(2), query)
                                .await
                                .unwrap_or(Err(Error::Deadline))
                        };
                        (
                            result,
                            super::VALIDATIONS.with(|records| records.borrow().clone()),
                        )
                    })
                    .await;
                match result {
                    Ok(response)
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
                    Err(error) if installation => return Err(error),
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
                    _ => {}
                }
                // Changed or ambiguous selectors must not certify the seed.
                return Ok(CiMetadata::Rest(
                    self.ci_pull_request(repository, number, Freshness::Revalidate)
                        .await?,
                ));
            }
        }
        Ok(CiMetadata::Rest(
            self.ci_pull_request(repository, number, freshness).await?,
        ))
    }

    async fn discovery_ci_metadata(
        &self,
        repository: &str,
        number: u64,
        age: std::time::Duration,
        cached: &Response,
    ) -> Result<Option<CiMetadata>> {
        let Some((node, validated_at)) = discovery::read(self, repository, number).await else {
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
