//! Positive-only account discovery during GraphQL access denial. Search is
//! capped and can omit inaccessible repositories; absence never proves closure.
use crate::{Client, Error, Freshness, Result, dashboard::DISCOVERY_CACHE};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

const CACHE: &str = "account-discovery-additions:v1";
const DENIALS: &str = "account-discovery-candidate-denials:v1";
const MAX_PR_PROBES: usize = 5;
const MAX_PAGES: usize = 10;

#[derive(Deserialize, Serialize)]
struct Additions {
    // A new complete scan retires old additions even when its roster is empty.
    collection_epoch: Option<u64>,
    pulls: Vec<Value>,
}

fn identity(node: &Value) -> Result<(String, u64)> {
    let repository = node["repository"]["nameWithOwner"]
        .as_str()
        .ok_or_else(|| Error::Invalid("additive discovery repository missing".into()))?;
    crate::client::validate_repository(repository)?;
    let number = node["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::Invalid("additive discovery PR number missing".into()))?;
    Ok((repository.to_ascii_lowercase(), number))
}

impl Client {
    /// Overlay independently validated positive observations. This never
    /// updates discovery health or the complete collection's validation clock.
    pub(crate) async fn overlay_accessible_account_prs(
        &self,
        pulls: Vec<Value>,
    ) -> Result<(Vec<Value>, bool)> {
        let epoch = self
            .derived(DISCOVERY_CACHE)
            .await?
            .map(|r| r.fetched_at_ms);
        let Some(stored) = self.derived(CACHE).await? else {
            return Ok((pulls, false));
        };
        let additions: Additions = stored.decode()?;
        if additions.collection_epoch != epoch || additions.pulls.is_empty() {
            return Ok((pulls, false));
        }
        let mut merged = pulls
            .into_iter()
            .map(|node| Ok((identity(&node)?, node)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        for node in additions.pulls {
            let key = identity(&node)?;
            let accepted = self.expected_pr_node(&key.0, key.1).await?;
            if accepted.as_deref() == node["id"].as_str() {
                // Independently validated REST evidence can replace a retired
                // node at the same selector without claiming a GraphQL scan.
                merged.insert(key, node);
            } else {
                merged.entry(key).or_insert(node);
            }
        }
        Ok((merged.into_values().collect(), true))
    }

    /// Bounded, credential-scoped REST work for new accessible PRs only. Known
    /// PR updates continue through the independent CI/detail polling lanes.
    pub(crate) async fn recover_accessible_account_prs(
        &self,
        budget: Duration,
        force_probe: bool,
    ) -> Result<usize> {
        if budget.is_zero() {
            return Ok(0);
        }
        let deadline = tokio::time::Instant::now()
            + budget
                .min(self.report_timeout())
                .min(Duration::from_secs(30));
        let lock = self.report_lock(CACHE);
        let _guard = tokio::time::timeout_at(deadline, lock.lock())
            .await
            .map_err(|_| Error::Deadline)?;
        let complete = self.derived(DISCOVERY_CACHE).await?;
        let epoch = complete.as_ref().map(|r| r.fetched_at_ms);
        let mut retained = BTreeMap::new();
        if let Some(stored) = self.derived(CACHE).await? {
            let previous: Additions = stored.decode()?;
            if previous.collection_epoch == epoch {
                for node in previous.pulls {
                    retained.insert(identity(&node)?, node);
                }
            }
        }
        let mut known = retained.clone();
        for node in complete
            .as_ref()
            .and_then(|r| r.data["pulls"].as_array())
            .into_iter()
            .flatten()
        {
            known.insert(identity(node)?, node.clone());
        }
        if let Some(roster) = self.stored_snapshot(&self.roster_resource()).await? {
            for node in roster
                .as_array()
                .ok_or_else(|| Error::Storage("invalid account roster".into()))?
            {
                known.insert(identity(node)?, node.clone());
            }
        }
        let mut added = Vec::new();
        let mut probed = 0usize;
        let stored_denials: BTreeMap<String, u64> = self
            .derived(DENIALS)
            .await?
            .map(|r| r.decode())
            .transpose()?
            .unwrap_or_default();
        let old_denials = stored_denials.clone();
        // Legacy cooldowns used the search URL's repository spelling. Merge
        // aliases without shortening a denied candidate's retry window.
        let mut denials = BTreeMap::new();
        for (key, retry_at) in stored_denials {
            let value = denials.entry(key.to_ascii_lowercase()).or_insert(0);
            *value = (*value).max(retry_at);
        }
        denials.retain(|_, retry_at| *retry_at > crate::now_ms());
        let result = tokio::time::timeout_at(deadline, async {
            let viewer = self.get("user", Freshness::MaxAge(Duration::from_secs(3600))).await?;
            let login = viewer.data["login"].as_str().filter(|s| !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
                .ok_or_else(|| Error::Invalid("REST viewer login missing or invalid".into()))?;
            let query = format!("is:pr is:open author:{login}");
            let mut bytes = 0usize;
            for page in 1..=MAX_PAGES {
                let args = url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("q", &query).append_pair("sort", "updated").append_pair("order", "desc")
                    .append_pair("per_page", "100").append_pair("page", &page.to_string()).finish();
                let response = self.get(&format!("search/issues?{args}"), Freshness::MaxAge(Duration::from_secs(30))).await?;
                bytes = bytes.saturating_add(response.data.to_string().len());
                if bytes > self.collection_limit() { return Err(Error::Invalid("REST discovery exceeds collection limit".into())); }
                let items = response.data["items"].as_array().ok_or_else(|| Error::Invalid("REST discovery search items missing".into()))?;
                if response.data["total_count"].as_u64().is_none() || !response.data["incomplete_results"].is_boolean() || items.len() > 100 {
                    return Err(Error::Invalid("REST discovery search envelope invalid".into()));
                }
                for item in items {
                    let Some(path) = item["pull_request"]["url"].as_str() else { continue; };
                    let url = url::Url::parse(path).map_err(|_| Error::Invalid("REST discovery PR URL invalid".into()))?;
                    let segments: Vec<_> = url.path().split('/').collect();
                    let ["repos", owner, repo, "pulls", number] = segments.get(segments.len().saturating_sub(5)..).unwrap_or_default() else {
                        return Err(Error::Invalid("REST discovery PR URL shape invalid".into()));
                    };
                    let repository = format!("{owner}/{repo}");
                    crate::client::validate_repository(&repository)?;
                    let number: u64 = number.parse().map_err(|_| Error::Invalid("REST discovery PR selector invalid".into()))?;
                    if number == 0 || item["number"].as_u64() != Some(number) {
                        return Err(Error::Invalid("REST discovery PR selector mismatch".into()));
                    }
                    if let Some(node) = known.get(&(repository.to_ascii_lowercase(), number)) {
                        let expected = self.expected_pr_node(&repository, number).await?;
                        if expected.as_deref().is_none_or(|id| node["id"].as_str() == Some(id)) { continue; }
                    }
                    let candidate_key = format!("{}/{number}",repository.to_ascii_lowercase());
                    if !force_probe && denials.contains_key(&candidate_key) { continue; }
                    if probed == MAX_PR_PROBES { return Ok(()); }
                    probed += 1;
                    // get() validates origin/prefix before forwarding any token.
                    // Revalidate the PR: search can be stale or incomplete.
                    let response = match self.get(path, Freshness::Revalidate).await {
                        Ok(response) => response,
                        Err(error) => {
                            if matches!(error,Error::GitHub {status:403|404,..}) {
                                denials.insert(candidate_key,crate::now_ms().saturating_add(300_000));
                            }
                            tracing::warn!(error_code=error.diagnostic_code(), "REST discovery candidate unavailable; retaining known PRs");
                            continue;
                        }
                    };
                    let pr = &response.data;
                    if pr["number"].as_u64() != Some(number) || !pr["base"]["repo"]["full_name"].as_str().is_some_and(|name|name.eq_ignore_ascii_case(&repository)) {
                        return Err(Error::Invalid("REST discovery PR identity mismatch".into()));
                    }
                    if pr["state"] != "open" || pr["merged"] == true || !pr["user"]["login"].as_str().is_some_and(|author| author.eq_ignore_ascii_case(login)) { continue; }
                    let id = pr["node_id"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| Error::Invalid("REST discovery PR node missing".into()))?;
                    for field in ["head", "base"] {
                        if !pr[field]["sha"].as_str().is_some_and(crate::repository::valid_sha) || pr[field]["ref"].as_str().is_none() {
                            return Err(Error::Invalid("REST discovery PR refs missing".into()));
                        }
                    }
                    for field in ["created_at", "updated_at"] {
                        if pr[field].as_str().is_none_or(|s| chrono::DateTime::parse_from_rfc3339(s).is_err()) {
                            return Err(Error::Invalid("REST discovery PR timestamp invalid".into()));
                        }
                    }
                    if !pr["title"].is_string() || !pr["html_url"].is_string() || !pr["draft"].is_boolean() {
                        return Err(Error::Invalid("REST discovery PR metadata missing".into()));
                    }
                    let node = json!({"id":id,"number":number,"repository":{"nameWithOwner":pr["base"]["repo"]["full_name"]},"author":{"login":login},
                        "title":pr["title"],"url":pr["html_url"],"state":"OPEN","isDraft":pr["draft"],"createdAt":pr["created_at"],"updatedAt":pr["updated_at"],
                        "headRefName":pr["head"]["ref"],"headRefOid":pr["head"]["sha"],"baseRefName":pr["base"]["ref"],"baseRefOid":pr["base"]["sha"],
                        "mergeable":match pr["mergeable"].as_bool() {Some(true)=>"MERGEABLE",Some(false)=>"CONFLICTING",None=>"UNKNOWN"}});
                    known.insert((repository.to_ascii_lowercase(), number), node.clone());
                    added.push(node);
                }
                if items.len() < 100 { return Ok(()); }
            }
            Ok(())
        }).await;
        if let Err(error) = result.unwrap_or(Err(Error::Deadline)) {
            tracing::warn!(
                error_code = error.diagnostic_code(),
                "REST discovery incomplete; retaining validated additions and known PRs"
            );
        }
        let count = added.len();
        if denials != old_denials {
            self.save_derived(
                DENIALS,
                serde_json::to_value(&denials).map_err(|e| Error::Storage(e.to_string()))?,
            )
            .await?;
        }
        if count > 0 {
            for node in added {
                retained.insert(identity(&node)?, node);
            }
            let data = serde_json::to_value(Additions {
                collection_epoch: epoch,
                pulls: retained.into_values().collect(),
            })
            .map_err(|e| Error::Storage(e.to_string()))?;
            if data.to_string().len() > self.collection_limit() {
                return Err(Error::Invalid(
                    "REST discovery additions exceed collection limit".into(),
                ));
            }
            self.save_derived(CACHE, data).await?;
        }
        tracing::info!(
            added = count,
            probed,
            "REST account discovery finished; additions only, discovery health unchanged"
        );
        Ok(count)
    }
}
