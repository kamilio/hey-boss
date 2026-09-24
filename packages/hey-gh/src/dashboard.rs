//! Account-wide PR status and a replayable feed of complete PR replacements.
use crate::{Client, Error, Freshness, Result, Watch, WatchKind, digest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

// PR updatedAt versions mutable PR metadata, not CI or mergeability. These
// sources still use their own validations and immutable commit references.
const VERSIONED_PR_FIELDS: &[&str] = &[
    "state",
    "closedAt",
    "mergedAt",
    "title",
    "isDraft",
    "body",
    "labels",
    "assignees",
    "updatedAt",
];

fn version_order(incoming: &Value, current: &Value) -> Option<Ordering> {
    let incoming = chrono::DateTime::parse_from_rfc3339(incoming.as_str()?).ok()?;
    let current = chrono::DateTime::parse_from_rfc3339(current.as_str()?).ok()?;
    Some(incoming.cmp(&current))
}

pub(crate) const DISCOVERY_CACHE: &str = "account-discovery-complete:v1";

// Account cycles publish once after each PR's work. Nested individual reads
// must not emit partial intermediate rows or overwrite discovery metadata.
tokio::task_local! { static ACCOUNT_PUBLICATION: (); }

const MY_PRS: &str = r#"query MyOpenPullRequests($after: String) {
  viewer { pullRequests(first: 100, after: $after, states: OPEN,
    orderBy: {field: CREATED_AT, direction: ASC}) {
    totalCount nodes { id number title url state isDraft createdAt updatedAt
      headRefName headRefOid baseRefName baseRefOid mergeable mergeStateStatus reviewDecision
      commits(last: 1) { nodes { commit { oid statusCheckRollup {
        state
      } } } }
      author { login } repository { nameWithOwner } }
    pageInfo { hasNextPage endCursor }
  } }
}"#;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrStatusChange {
    pub cursor: String,
    pub changed_fields: Vec<String>,
    pub observed_at_ms: u64,
    /// baseline, opened, updated, closed, merged, reopened, or removed.
    pub kind: String,
    pub activity: Vec<Value>,
    /// Full replacement, including removed=true for a PR leaving the open set.
    pub pull_request: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrStatusPage {
    /// Populated on bootstrap; empty for cursor reads.
    pub pull_requests: Vec<Value>,
    pub changes: Vec<PrStatusChange>,
    pub cursor: String,
    pub has_more: bool,
    pub complete: bool,
    pub errors: Vec<String>,
    /// Coverage of this page's rows, not proof of a complete repository roster.
    /// None when reading an envelope from an older daemon.
    #[serde(default)]
    pub coverage: Option<PrStatusCoverage>,
    #[serde(default)]
    pub account_discovery: AccountDiscoveryHealth,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrStatusCoverage {
    pub repository: Option<String>,
    pub returned_rows: usize,
    /// Includes source evidence omitted by a row projection.
    pub returned_rows_complete: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDiscoveryHealth {
    /// Last-known scan result, not freshness; None means no known result.
    pub complete: Option<bool>,
    pub last_poll_at_ms: Option<u64>,
    pub last_success_at_ms: Option<u64>,
    pub errors: Vec<String>,
}

impl PrStatusPage {
    pub(crate) fn update_coverage(&mut self, repository: Option<&str>) {
        let mut rows = self
            .pull_requests
            .iter()
            .chain(self.changes.iter().map(|change| &change.pull_request));
        self.coverage = Some(PrStatusCoverage {
            repository: repository.map(str::to_owned),
            returned_rows: self.pull_requests.len() + self.changes.len(),
            returned_rows_complete: rows.all(|row| row["complete"] == true),
        });
    }

    pub(crate) fn record_discovery_error(&mut self, error: &str) {
        self.complete = false;
        self.account_discovery.complete = Some(false);
        if !self
            .account_discovery
            .errors
            .iter()
            .any(|known| known == error)
        {
            self.account_discovery.errors.push(error.to_owned());
        }
        let error = format!("discovery: {error}");
        if !self.errors.contains(&error) {
            self.errors.push(error);
        }
    }
}

/// Work completed by the latest account hydration cycle. This is progress,
/// not a freshness, completeness, or merge-readiness conclusion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRefreshCycle {
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub total: usize,
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub interrupted: usize,
    pub deferred: usize,
    pub cycle_budget_exhausted: bool,
}

fn key(node: &Value) -> Result<(String, u64)> {
    let repo = node["repository"]["nameWithOwner"]
        .as_str()
        .ok_or_else(|| Error::Invalid("PR repository missing from discovery".into()))?;
    crate::client::validate_repository(repo)?;
    let number = node["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::Invalid("PR number missing from discovery".into()))?;
    Ok((repo.to_ascii_lowercase(), number))
}

fn normalized_page_clocks(clocks: BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    let mut normalized = BTreeMap::new();
    for (key, clock) in clocks {
        let value = normalized.entry(key.to_ascii_lowercase()).or_insert(0);
        *value = (*value).max(clock);
    }
    normalized
}

// Validation clocks are internal evidence, not semantic PR fields. Keeping
// each page's clock prevents older REST metadata from winning over a later page.
struct Discovery {
    pulls: Vec<Value>,
    validated_at: u64,
    validated_by_pr: BTreeMap<String, u64>,
}

impl Client {
    /// Read private, durable account-cycle progress without GitHub requests or
    /// observation-cursor changes. Discovery-only preparation does not replace it.
    pub async fn account_refresh_cycle(
        &self,
        ci_only: bool,
    ) -> Result<Option<AccountRefreshCycle>> {
        self.derived(&format!(
            "account-status-cycle:{}",
            if ci_only { "ci" } else { "details" }
        ))
        .await?
        .map(|response| response.decode())
        .transpose()
    }
    pub(crate) fn roster_resource(&self) -> String {
        format!("my-open-prs://{}", self.hostname())
    }
    pub(crate) async fn account_pr_validated_at(
        &self,
        repo: &str,
        number: u64,
        ci_only: bool,
    ) -> Result<Option<u64>> {
        self.derived(&format!(
            "account-status-validated:{}:{}/{number}",
            if ci_only { "ci" } else { "details" },
            repo.to_ascii_lowercase(),
        ))
        .await?
        .map(|response| response.decode())
        .transpose()
    }
    fn status_prefix(&self) -> String {
        format!("pr-status://{}/", self.hostname())
    }

    /// Fully paginate the viewer's authored open PR connection (no Search cap).
    pub async fn all_my_open_pull_requests(&self, freshness: Freshness) -> Result<Vec<Value>> {
        Ok(self.discover_my_open_pull_requests(freshness).await?.pulls)
    }

    async fn discover_my_open_pull_requests(&self, freshness: Freshness) -> Result<Discovery> {
        let deadline = tokio::time::Instant::now() + self.report_timeout();
        let lock = self.report_lock(DISCOVERY_CACHE);
        let _guard = if matches!(freshness, Freshness::CachedOnly) {
            None
        } else {
            Some(
                tokio::time::timeout_at(deadline, lock.lock())
                    .await
                    .map_err(|_| Error::Deadline)?,
            )
        };
        let cached = tokio::time::timeout_at(deadline, self.derived(DISCOVERY_CACHE))
            .await
            .map_err(|_| Error::Deadline)??;
        let legacy_success_at_ms = cached.as_ref().map(|response| response.fetched_at_ms);
        if let Some(cached) = cached {
            let reusable = match freshness {
                Freshness::CachedOnly => true,
                Freshness::MaxAge(age) => {
                    crate::now_ms().saturating_sub(cached.fetched_at_ms) < age.as_millis() as u64
                }
                Freshness::Revalidate => false,
            };
            if reusable {
                return Ok(Discovery {
                    pulls: cached.data["pulls"]
                        .as_array()
                        .cloned()
                        .ok_or_else(|| Error::Storage("invalid cached account discovery".into()))?,
                    validated_at: cached.data["validatedAtMs"].as_u64().ok_or_else(|| {
                        Error::Storage("invalid account discovery validation time".into())
                    })?,
                    validated_by_pr: normalized_page_clocks(
                        cached
                            .data
                            .get("validatedAtByPr")
                            .map(|value| serde_json::from_value(value.clone()))
                            .transpose()
                            .map_err(|_| {
                                Error::Storage("invalid account discovery page times".into())
                            })?
                            .unwrap_or_default(),
                    ),
                });
            }
        }
        if matches!(freshness, Freshness::CachedOnly) {
            return tokio::time::timeout_at(deadline, self.scan_my_open_pull_requests(freshness))
                .await
                .unwrap_or(Err(Error::Deadline));
        }
        let generation = self.begin_discovery(legacy_success_at_ms).await?;
        let result = if tokio::time::Instant::now() >= deadline {
            Err(Error::Deadline)
        } else {
            tokio::time::timeout_at(deadline, self.scan_my_open_pull_requests(freshness))
                .await
                .unwrap_or(Err(Error::Deadline))
        };
        let collection = result.as_ref().ok().map(|scan| {
            json!({
                "pulls": scan.pulls, "validatedAtMs": scan.validated_at,
                "validatedAtByPr": scan.validated_by_pr,
            })
        });
        // Finish the small durable transaction after the network budget. A
        // deadline must not drop its failure record; no GitHub work starts here.
        self.finish_discovery(
            generation,
            collection,
            result.as_ref().err().map(ToString::to_string),
        )
        .await?;
        result
    }

    async fn scan_my_open_pull_requests(&self, freshness: Freshness) -> Result<Discovery> {
        let started = tokio::time::Instant::now();
        let mut after = Value::Null;
        let mut cursors = BTreeSet::new();
        let mut pulls = BTreeMap::new();
        let mut bytes = 0usize;
        let mut total = None;
        let mut validated_at = u64::MAX;
        let mut validated_by_pr = BTreeMap::new();
        for page in 0..1000 {
            let response = self
                .graphql(MY_PRS, json!({"after":after}), freshness)
                .await?;
            validated_at = validated_at.min(response.validated_at_ms);
            bytes = bytes.saturating_add(response.data.to_string().len());
            if bytes > self.collection_limit() {
                return Err(Error::Invalid(
                    "PR discovery exceeds collection limit".into(),
                ));
            }
            let conn = &response.data["data"]["viewer"]["pullRequests"];
            let count = conn["totalCount"]
                .as_u64()
                .ok_or_else(|| Error::Invalid("PR discovery total missing".into()))?;
            if total.replace(count).is_some_and(|old| old != count) {
                return Err(Error::Invalid(
                    "open PR set changed during discovery; retry".into(),
                ));
            }
            let nodes = conn["nodes"]
                .as_array()
                .ok_or_else(|| Error::Invalid("PR discovery nodes missing".into()))?;
            for node in nodes {
                let identity = key(node)?;
                validated_by_pr.insert(
                    format!("{}/{}", identity.0, identity.1),
                    response.validated_at_ms,
                );
                if node["state"] != "OPEN" || pulls.insert(identity, node.clone()).is_some() {
                    return Err(Error::Invalid(
                        "invalid or repeated PR in discovery; retry".into(),
                    ));
                }
            }
            match conn["pageInfo"]["hasNextPage"].as_bool() {
                Some(false) => {
                    if pulls.len() as u64 != count {
                        return Err(Error::Invalid("PR discovery count mismatch; retry".into()));
                    }
                    let pulls: Vec<_> = pulls.into_values().collect();
                    tracing::info!(
                        pages = page + 1,
                        pull_requests = pulls.len(),
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "account discovery scan completed"
                    );
                    return Ok(Discovery {
                        pulls,
                        validated_at,
                        validated_by_pr,
                    });
                }
                Some(true) => {
                    let next = conn["pageInfo"]["endCursor"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| Error::Invalid("PR discovery cursor missing".into()))?;
                    if nodes.is_empty() || !cursors.insert(next.to_owned()) {
                        return Err(Error::Invalid("PR discovery pagination cycle".into()));
                    }
                    after = json!(next);
                }
                None => return Err(Error::Invalid("PR discovery page info missing".into())),
            }
        }
        Err(Error::Invalid("PR discovery exceeds 1000 pages".into()))
    }

    pub async fn save_account_watch(&self, interval_seconds: u64) -> Result<Watch> {
        if !(10..=86400).contains(&interval_seconds) {
            return Err(Error::Invalid("interval must be 10..86400 seconds".into()));
        }
        let watch = Watch {
            id: digest("account-open-prs"),
            repository: String::new(),
            pull_number: 0,
            interval_seconds,
            kind: WatchKind::Account,
            branches: Vec::new(),
            all_branches: false,
        };
        self.persist_watch(&watch).await?;
        Ok(watch)
    }

    /// CI and detail loops keep independent retry sets. A discovery failure never
    /// turns a previous roster into an empty set or stops REST CI for known PRs.
    pub async fn refresh_pr_status(
        &self,
        freshness: Freshness,
        ci_only: bool,
    ) -> Result<Vec<String>> {
        self.collect_pr_status(freshness, ci_only, false, false, false)
            .await
    }

    /// Batched metadata/head checks first; monitors hydrate detailed sources.
    pub async fn prepare_pr_status(&self, freshness: Freshness) -> Result<Vec<String>> {
        self.collect_pr_status(freshness, false, true, false, false)
            .await
    }

    pub(crate) async fn hydrate_pr_status_details(
        &self,
        freshness: Freshness,
    ) -> Result<Vec<String>> {
        self.collect_pr_status(freshness, false, false, true, true)
            .await
    }

    pub(crate) async fn hydrate_pr_status_ci(&self, freshness: Freshness) -> Result<Vec<String>> {
        self.collect_pr_status(freshness, true, false, false, true)
            .await
    }

    async fn collect_pr_status(
        &self,
        freshness: Freshness,
        ci_only: bool,
        seed_only: bool,
        details_only: bool,
        background: bool,
    ) -> Result<Vec<String>> {
        crate::client::BACKGROUND_READ
            .scope(
                (),
                ACCOUNT_PUBLICATION.scope(
                    (),
                    self.collect_pr_status_inner(
                        freshness,
                        ci_only,
                        seed_only,
                        details_only,
                        background,
                    ),
                ),
            )
            .await
    }

    async fn collect_pr_status_inner(
        &self,
        freshness: Freshness,
        ci_only: bool,
        seed_only: bool,
        details_only: bool,
        background: bool,
    ) -> Result<Vec<String>> {
        let mode = if ci_only { "ci" } else { "details" };
        let started = tokio::time::Instant::now();
        let started_at_ms = crate::now_ms();
        let deadline = tokio::time::Instant::now() + self.report_timeout();
        let lock = self.report_lock(&format!(
            "account-status:{}",
            if seed_only { "discovery" } else { mode }
        ));
        let _guard = tokio::time::timeout_at(deadline, lock.lock())
            .await
            .map_err(|_| Error::Deadline)?;
        let previous = self.stored_snapshot(&self.roster_resource()).await?;
        let established = previous.is_some();
        let previous = previous.unwrap_or(json!([]));
        let mut errors = vec![];
        let discovery_deadline = if established && !seed_only {
            deadline.min(tokio::time::Instant::now() + Duration::from_secs(30))
        } else {
            deadline
        };
        let (pulls, discovered, discovery_at, mut validated_by_pr) = match tokio::time::timeout_at(
            discovery_deadline,
            self.discover_my_open_pull_requests(if background {
                Freshness::CachedOnly
            } else {
                freshness
            }),
        )
        .await
        .unwrap_or(Err(Error::Deadline))
        {
            Ok(scan) => (scan.pulls, true, scan.validated_at, scan.validated_by_pr),
            Err(e) => {
                tracing::warn!(
                    mode,
                    seed_only,
                    error_code = e.diagnostic_code(),
                    "account discovery failed; keeping previous roster"
                );
                errors.push(format!("discovery: {e}"));
                if !matches!(freshness, Freshness::CachedOnly)
                    && matches!(
                        e,
                        Error::GraphQL {
                            access_denied: true,
                            ..
                        }
                    )
                    && let Err(error) = self
                        .recover_accessible_account_prs(
                            deadline.saturating_duration_since(tokio::time::Instant::now()),
                            matches!(freshness, Freshness::Revalidate),
                        )
                        .await
                {
                    tracing::warn!(
                        error_code = error.diagnostic_code(),
                        "REST account discovery unavailable; retaining known PRs"
                    );
                }
                (
                    previous.as_array().cloned().unwrap_or_default(),
                    false,
                    0,
                    BTreeMap::new(),
                )
            }
        };
        let (pulls, additive_roster) = self.overlay_accessible_account_prs(pulls).await?;
        if additive_roster {
            // REST additions were never validated by the GraphQL scan. Preserve
            // its per-page clocks only for nodes actually present in that scan.
            for node in &pulls {
                let (repo, number) = key(node)?;
                validated_by_pr
                    .entry(format!("{repo}/{number}"))
                    .or_insert(0);
            }
        }
        let authoritative_roster = discovered && !additive_roster;
        if !discovered && pulls.is_empty() {
            return Ok(errors);
        }
        let current: BTreeMap<_, _> = pulls
            .iter()
            .map(|n| Ok((key(n)?, n.clone())))
            .collect::<Result<_>>()?;
        let tracking_key = format!("account-status-pending:{mode}");
        let mut pending: BTreeMap<(String, u64), Value> = BTreeMap::new();
        if let Some(stored) = self.derived(&tracking_key).await? {
            for node in stored
                .data
                .as_array()
                .ok_or_else(|| Error::Storage("invalid account tracking".into()))?
            {
                pending.insert(key(node)?, node.clone());
            }
        }
        for node in previous.as_array().into_iter().flatten() {
            pending.insert(key(node)?, node.clone());
        }
        pending.extend(current.clone());
        // Persist before touching the roster, so closed/merged PR follow-ups
        // survive process death between discovery and their final observation.
        self.save_derived(&tracking_key, json!(pending.values().collect::<Vec<_>>()))
            .await?;
        if discovered || additive_roster {
            self.observe(&self.roster_resource(), &json!(pulls)).await?;
        }
        let mut retry = current.clone();
        let rotation_key = format!("account-status-next:{mode}");
        let next: Option<(String, u64)> = self
            .derived(&rotation_key)
            .await?
            .map(|r| r.decode())
            .transpose()?
            .flatten()
            .map(|(repo, number): (String, u64)| (repo.to_ascii_lowercase(), number));
        let mut work: Vec<_> = pending.into_iter().collect();
        if let Some(next) = next
            && let Some(index) = work.iter().position(|(key, _)| *key == next)
        {
            work.rotate_left(index);
        }
        let successors: BTreeMap<_, _> = work
            .iter()
            .zip(work.iter().cycle().skip(1))
            .take(work.len())
            .map(|((key, _), (next, _))| (key.clone(), next.clone()))
            .collect();
        let mut resume = None;
        let total = work.len();
        let mut attempted = 0usize;
        let mut failed = 0usize;
        let mut interrupted = 0usize;
        let mut deferred = 0usize;
        let mut work = work.into_iter();
        while let Some(((repo, number), node)) = work.next() {
            if !seed_only && tokio::time::Instant::now() >= deadline {
                // Unvisited PRs are deferred work, not failed observations.
                // Preserve terminal follow-ups as well as the current roster.
                resume.get_or_insert_with(|| (repo.clone(), number));
                let remaining = 1 + work.len();
                deferred = remaining;
                retry.insert((repo, number), node);
                retry.extend(work);
                errors.push(format!(
                    "refresh cycle budget exhausted; {remaining} PRs remain queued"
                ));
                break;
            }
            attempted += 1;
            let pr_started_at_ms = crate::now_ms();
            // A large roster must not spend an entire cycle on one PR. Keep
            // the existing rotation and retry sets, but bound background work
            // per PR so later lifecycle/CI reads get a turn in this cycle.
            let pr_deadline = if background {
                deadline.min(tokio::time::Instant::now() + Duration::from_secs(5))
            } else {
                deadline
            };
            let previously_terminal = if seed_only {
                self.stored_pr_snapshot(
                    &format!("metadata://{}/{repo}/{number}", self.hostname()),
                    &repo,
                    node["id"].as_str(),
                )
                .await?
                .is_some_and(|v| {
                    v["pull_request"]["state"] == "closed" || v["pull_request"]["merged"] == true
                })
            } else {
                false
            };
            let mut cycle_interrupted = false;
            let refresh = async {
                if seed_only
                    && authoritative_roster
                    && (!current.contains_key(&(repo.clone(), number)) || previously_terminal)
                {
                    tokio::time::timeout_at(
                        deadline.min(tokio::time::Instant::now() + Duration::from_secs(5)),
                        async {
                            let response = self
                                .pull_request(
                                    &repo,
                                    number,
                                    if matches!(freshness, Freshness::CachedOnly) {
                                        freshness
                                    } else {
                                        Freshness::Revalidate
                                    },
                                )
                                .await?;
                            let conflicts = match response.data["mergeable"].as_bool() {
                                Some(true) => "clean",
                                Some(false) => "conflicting",
                                None => "unknown",
                            };
                            self.observe(
                                &format!("metadata://{}/{repo}/{number}", self.hostname()),
                                &json!({"pull_request":response.data,"conflicts":conflicts}),
                            )
                            .await?;
                            Ok(())
                        },
                    )
                    .await
                    .unwrap_or(Err(Error::Deadline))
                } else if seed_only {
                    Ok(())
                } else if ci_only {
                    let result = tokio::time::timeout_at(pr_deadline, async {
                        let report = self.ci_for_pr(&repo, number, freshness).await?;
                        if !report.complete {
                            return Err(Error::Invalid(format!(
                                "incomplete CI: {}",
                                json!(report.data.errors)
                            )));
                        }
                        Ok(())
                    })
                    .await
                    .unwrap_or_else(|_| {
                        cycle_interrupted = true;
                        Err(Error::Deadline)
                    });
                    if result.is_ok() && tokio::time::Instant::now() < pr_deadline {
                        // Policy is best effort in the CI loop. A slow policy read
                        // must not turn already observed CI into a timeout failure.
                        let policy_deadline =
                            pr_deadline.min(tokio::time::Instant::now() + Duration::from_secs(1));
                        if let Err(error) = tokio::time::timeout_at(
                            policy_deadline,
                            self.required_checks_for_pr(&repo, number, freshness),
                        )
                        .await
                        .unwrap_or(Err(Error::Deadline))
                        {
                            tracing::warn!(repository=%repo,number,error_code=error.diagnostic_code(),"account required-check refresh failed");
                        }
                    }
                    result
                } else {
                    tokio::time::timeout_at(pr_deadline, async {
                        if details_only {
                            let errors = self.refresh_pr_details(&repo, number, freshness).await?;
                            if !errors.is_empty() {
                                return Err(Error::Invalid(format!(
                                    "incomplete details: {}",
                                    json!(errors)
                                )));
                            }
                            return Ok(());
                        }
                        let report = self.pr_report(&repo, number, freshness).await?;
                        if !report.complete {
                            return Err(Error::Invalid(format!(
                                "incomplete PR: {}",
                                json!({"sources":report.data.errors,"ci":report.data.ci.errors})
                            )));
                        }
                        Ok(())
                    })
                    .await
                    .unwrap_or_else(|_| {
                        cycle_interrupted = true;
                        Err(Error::Deadline)
                    })
                }
            };
            let result = crate::client::REQUEST_DEADLINE
                .scope(background.then_some(pr_deadline), refresh)
                .await;
            // The scheduler may deliver its local deadline just before the
            // outer timer fires, including a coalesced request from the other
            // hydration lane. Both outcomes retain local interruption health.
            if background && matches!(result, Err(Error::Deadline)) {
                cycle_interrupted = true;
            }
            if !seed_only && result.is_ok() && !matches!(freshness, Freshness::CachedOnly) {
                // This conservative clock comes from the request freshness
                // bound, never from publication time or cached availability.
                let validated_at = pr_started_at_ms.saturating_sub(match freshness {
                    Freshness::MaxAge(age) => age.as_millis() as u64,
                    _ => 0,
                });
                let mut modes = vec![mode];
                if !ci_only && !details_only {
                    modes.push("ci");
                }
                for mode in modes {
                    self.save_derived(
                        &format!("account-status-validated:{mode}:{repo}/{number}"),
                        json!(validated_at),
                    )
                    .await?;
                }
            }
            if !seed_only && matches!(result, Err(Error::Deadline)) && resume.is_none() {
                resume = successors.get(&(repo.clone(), number)).cloned();
            }
            if let Err(error) = &result {
                if cycle_interrupted {
                    interrupted += 1;
                    tracing::info!(repository=%repo,number,mode,"PR refresh interrupted by local budget; retry queued");
                } else {
                    failed += 1;
                    tracing::warn!(repository=%repo,number,mode,error_code=error.diagnostic_code(),"PR status refresh failed");
                }
            }
            let error = if cycle_interrupted {
                Some(if pr_deadline < deadline {
                    "PR refresh budget exhausted; retry queued; prior evidence retained".into()
                } else {
                    "refresh cycle budget exhausted; retry queued; prior evidence retained".into()
                })
            } else {
                result.err().map(|e| e.to_string())
            };
            if let Some(error) = &error {
                errors.push(format!("{repo}#{number}: {error}"));
                retry.insert((repo.clone(), number), node.clone());
            }
            let mut health = vec![(if seed_only { "discovery" } else { mode }, error)];
            if !seed_only && !ci_only && !details_only && health[0].1.is_none() {
                // A successful combined refresh validated CI too. It can
                // recover a prior CI interruption; detail-only work cannot.
                health.push(("ci", None));
            }
            self.publish_pr_status(
                (&repo, number),
                (
                    &node,
                    validated_by_pr
                        .get(&format!("{repo}/{number}"))
                        .copied()
                        .unwrap_or(discovery_at),
                ),
                authoritative_roster.then(|| !current.contains_key(&(repo.clone(), number))),
                established,
                &health,
            )
            .await?;
            // GitHub's list and detail endpoints can converge at different
            // times. Keep probing a disappearance until its terminal state is
            // known; a successful stale "open" response is not a final close.
            if authoritative_roster && !current.contains_key(&(repo.clone(), number)) {
                let status = self
                    .stored_snapshot(&format!("{}{repo}/{number}", self.status_prefix()))
                    .await?
                    .unwrap_or(Value::Null);
                if seed_only
                    || !matches!(
                        status["pullRequest"]["state"].as_str(),
                        Some("CLOSED" | "MERGED")
                    )
                {
                    retry.insert((repo, number), node);
                }
            }
        }
        self.save_derived(&tracking_key, json!(retry.values().collect::<Vec<_>>()))
            .await?;
        let finished_at_ms = crate::now_ms();
        if !seed_only {
            self.save_derived(&rotation_key, json!(resume)).await?;
            self.save_derived(
                &format!("account-status-cycle:{mode}"),
                json!(AccountRefreshCycle {
                    started_at_ms,
                    finished_at_ms,
                    total,
                    attempted,
                    succeeded: attempted - failed - interrupted,
                    failed,
                    interrupted,
                    deferred,
                    cycle_budget_exhausted: deferred > 0
                        || (interrupted > 0 && tokio::time::Instant::now() >= deadline),
                }),
            )
            .await?;
        }
        tracing::info!(
            mode,
            seed_only,
            details_only,
            started_at_ms,
            finished_at_ms,
            // A cached roster can be usable during an active discovery error.
            // This is availability, not successful upstream validation.
            roster_available = discovered || additive_roster,
            roster_cached_only = background || matches!(freshness, Freshness::CachedOnly),
            roster_additive_only = additive_roster,
            total,
            attempted,
            succeeded = attempted - failed - interrupted,
            failed,
            interrupted,
            deferred,
            cycle_budget_exhausted =
                deferred > 0 || (interrupted > 0 && tokio::time::Instant::now() >= deadline),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "account refresh cycle finished"
        );
        Ok(errors)
    }

    /// Reconcile only an already tracked PR. Individual observations neither
    /// discover an account roster nor add another author's PR to that roster.
    pub(crate) async fn publish_individual_pr_status(
        &self,
        repository: &str,
        number: u64,
        health: &[(&str, Option<String>)],
    ) -> Result<()> {
        if ACCOUNT_PUBLICATION.try_with(|_| ()).is_ok() {
            return Ok(());
        }
        let resource = format!("{}{repository}/{number}", self.status_prefix());
        if let Some(snapshot) = self.stored_snapshot(&resource).await? {
            self.publish_pr_status(
                (repository, number),
                (&snapshot["pullRequest"], 0),
                None,
                true,
                health,
            )
            .await?;
        }
        Ok(())
    }

    async fn publish_pr_status(
        &self,
        identity: (&str, u64),
        discovery: (&Value, u64),
        removed: Option<bool>,
        established: bool,
        health: &[(&str, Option<String>)],
    ) -> Result<()> {
        let (repo, number) = identity;
        let (discovered, discovery_at) = discovery;
        let resource = format!("{}{repo}/{number}", self.status_prefix());
        let lock = self.report_lock(&resource.to_ascii_lowercase());
        let _guard = lock.lock().await;
        let old_event = self
            .stored_snapshot(&resource)
            .await?
            .unwrap_or(Value::Null);
        let old = &old_event["pullRequest"];
        let mut removed = removed.unwrap_or(old["removed"] == true);
        let suffix = format!("{}/{repo}/{number}", self.hostname());
        let expected_node = self.expected_pr_node(repo, number).await?;
        let owner = crate::entity::current().unwrap_or(crate::store::PrOwner {
            repository: repo.to_ascii_lowercase(),
            number,
            node_id: expected_node
                .clone()
                .or_else(|| discovered["id"].as_str().map(str::to_owned)),
            generation: self.repository_generation(repo).await?,
        });
        if discovery_at > 0
            && expected_node
                .as_deref()
                .zip(discovered["id"].as_str())
                .is_some_and(|(expected, incoming)| expected != incoming)
        {
            // A delayed scan must not undo a newer accepted entity identity.
            return Ok(());
        }
        let metadata = self
            .stored_pr_snapshot(
                &format!("metadata://{suffix}"),
                repo,
                discovered["id"].as_str(),
            )
            .await?;
        let pr = &metadata
            .as_ref()
            .map_or(Value::Null, |v| v["pull_request"].clone());
        let old_clock = self.status_validation_clock(&resource).await?;
        // REST never supplies reviewDecision or mergeStateStatus. Its newer
        // metadata clock must not suppress separately validated GraphQL fields.
        let old_discovery_clock = self
            .status_validation_clock(&format!("{resource}#discovery"))
            .await?;
        let discovery_barrier = if old_discovery_clock > 0 {
            old_discovery_clock
        } else {
            old_clock
        };
        let identity_changed = (discovery_at > 0
            || expected_node.as_deref() == discovered["id"].as_str())
            && discovered["id"]
                .as_str()
                .zip(old["id"].as_str())
                .is_some_and(|(current, old)| current != old);
        if identity_changed {
            // Removal and terminal follow-ups belong to the retired node, not
            // the independently validated positive now using its selector.
            removed = false;
        }
        let discovered_version = (!identity_changed)
            .then(|| version_order(&discovered["updatedAt"], &old["updatedAt"]))
            .flatten();
        if discovered_version == Some(Ordering::Less) && old["removed"] == true {
            // An older listing cannot prove that an unresolved disappearance
            // has ended, even when it briefly includes the PR again.
            removed = true;
        }
        let mut row = if !old.is_null()
            && old_clock > discovery_at
            && !identity_changed
            && discovered_version != Some(Ordering::Greater)
        {
            old.clone()
        } else {
            discovered.clone()
        };
        if discovered_version == Some(Ordering::Less) {
            for field in VERSIONED_PR_FIELDS {
                if let Some(value) = old.get(field) {
                    row[field] = value.clone();
                }
            }
        }
        let graph_ci = discovered["commits"]["nodes"][0]["commit"]["statusCheckRollup"].clone();
        row.as_object_mut()
            .ok_or_else(|| Error::Invalid("invalid discovered PR".into()))?
            .remove("commits");
        row["headCiState"] = graph_ci["state"].clone();
        let metadata_at = match self
            .get(
                &format!("repos/{repo}/pulls/{number}"),
                Freshness::CachedOnly,
            )
            .await
        {
            Ok(response) if response.data == *pr => response.validated_at_ms,
            Ok(_) => 0,
            Err(Error::CacheMiss) => 0,
            Err(error) => return Err(error),
        };
        let discovery_clock = discovery_at.max(old_clock);
        let metadata_identity_matches = row["id"]
            .as_str()
            .zip(pr["node_id"].as_str())
            .is_none_or(|(current, metadata)| current == metadata);
        let metadata_version = version_order(&pr["updated_at"], &row["updatedAt"]);
        let metadata_newer = metadata_identity_matches
            && (metadata_at > discovery_clock || metadata_version == Some(Ordering::Greater));
        // Matching refs permit reuse of commit-bound evidence, but do not make
        // older titles, drafts or mergeability newer than discovery metadata.
        let metadata_current = metadata_identity_matches
            && (metadata_newer
                || row["headRefOid"]
                    .as_str()
                    .is_none_or(|sha| pr["head"]["sha"] == sha)
                    && row["baseRefOid"]
                        .as_str()
                        .is_none_or(|sha| pr["base"]["sha"] == sha));
        for field in [
            "mergedAt",
            "closedAt",
            "additions",
            "deletions",
            "baseRefOid",
        ] {
            if row.get(field).is_none() {
                row[field] = Value::Null;
            }
        }
        // A stable shape independent of which discovery page contained this PR.
        let repository_name = old["repository"]["nameWithOwner"]
            .as_str()
            .filter(|old| old.eq_ignore_ascii_case(repo))
            .or_else(|| {
                discovered["repository"]["nameWithOwner"]
                    .as_str()
                    .filter(|name| name.eq_ignore_ascii_case(repo))
            })
            .unwrap_or(repo);
        row["repository"] = json!({"nameWithOwner":repository_name});
        // A merge is irreversible for a GitHub PR node. Never let a later
        // stale open listing reopen the same node; a different node ID is a
        // different PR even when a recreated repository reuses its number.
        let known_merged = old["state"] == "MERGED"
            && old["id"]
                .as_str()
                .zip(row["id"].as_str())
                .is_some_and(|(old, current)| old == current)
            || pr["merged"] == true
                && pr["node_id"]
                    .as_str()
                    .zip(row["id"].as_str())
                    .is_some_and(|(metadata, current)| metadata == current);
        let state = if known_merged {
            "MERGED"
        } else if metadata_version == Some(Ordering::Less) && !row["state"].is_null() {
            if removed && row["state"] == "OPEN" {
                // Disappearance is evidence of removal, not proof that an
                // older terminal response describes the latest PR version.
                "UNKNOWN"
            } else {
                row["state"].as_str().unwrap_or("UNKNOWN")
            }
        } else if !metadata_newer && !row["state"].is_null() && !removed {
            row["state"].as_str().unwrap_or("UNKNOWN")
        } else if pr["merged"].as_bool() == Some(true) || !pr["merged_at"].is_null() {
            "MERGED"
        } else if pr["state"] == "closed" {
            "CLOSED"
        } else if removed {
            "UNKNOWN"
        } else {
            "OPEN"
        };
        let state = state.to_owned();
        row["state"] = json!(state);
        for (out, input) in [
            ("title", "title"),
            ("url", "html_url"),
            ("isDraft", "draft"),
            ("createdAt", "created_at"),
            ("updatedAt", "updated_at"),
            ("mergedAt", "merged_at"),
            ("closedAt", "closed_at"),
            ("additions", "additions"),
            ("deletions", "deletions"),
            ("body", "body"),
            ("labels", "labels"),
            ("assignees", "assignees"),
        ] {
            if let Some(value) = pr.get(input).filter(|_| {
                metadata_current
                    && (known_merged
                        || !matches!(out, "closedAt" | "mergedAt")
                        || metadata_version != Some(Ordering::Less))
                    && (metadata_newer
                        && (!VERSIONED_PR_FIELDS.contains(&out)
                            || metadata_version != Some(Ordering::Less))
                        || row.get(out).is_none_or(Value::is_null))
            }) {
                row[out] = value.clone();
            }
        }
        if let Some(login) = pr["user"]["login"].as_str() {
            row["author"] = json!({"login":login});
        }
        for (out, path) in [
            ("headRefName", ("head", "ref")),
            ("headRefOid", ("head", "sha")),
            ("baseRefName", ("base", "ref")),
            ("baseRefOid", ("base", "sha")),
        ] {
            if let Some(value) = pr.get(path.0).and_then(|v| v.get(path.1)).filter(|_| {
                metadata_current && (metadata_newer || row.get(out).is_none_or(Value::is_null))
            }) {
                row[out] = value.clone();
            }
        }
        if metadata_current
            && (metadata_newer || row["id"].is_null())
            && let Some(id) = pr["node_id"].as_str()
        {
            row["id"] = json!(id);
        }
        let matching_graph_refs = ["headRefOid", "baseRefOid"]
            .iter()
            .all(|field| discovered[field] == row[field]);
        let discovery_current = discovery_at > 0
            && matching_graph_refs
            && (identity_changed || discovery_at >= discovery_barrier);
        let old_graph_matches = !identity_changed
            && ["headRefOid", "baseRefOid"]
                .iter()
                .all(|field| old[field] == row[field]);
        for field in ["reviewDecision", "mergeStateStatus"] {
            row[field] = if discovery_current {
                discovered[field].clone()
            } else if old_graph_matches {
                old[field].clone()
            } else {
                Value::Null
            };
        }
        let graph_clock = if discovery_current {
            discovery_at
        } else if identity_changed {
            0
        } else {
            old_discovery_clock
        };
        if state == "OPEN" {
            row["closedAt"] = Value::Null;
            row["mergedAt"] = Value::Null;
        }
        row["conflicts"] = if metadata_current
            && (metadata_newer || discovered.get("mergeable").is_none())
            && let Some(metadata) = &metadata
        {
            metadata["conflicts"].clone()
        } else {
            json!(match row["mergeable"].as_str() {
                Some("MERGEABLE") => "clean",
                Some("CONFLICTING") => "conflicting",
                _ => "unknown",
            })
        };
        row["mergeable"] = json!(match row["conflicts"].as_str() {
            Some("clean") => "MERGEABLE",
            Some("conflicting") => "CONFLICTING",
            _ => "UNKNOWN",
        });
        let mut available = metadata.is_some() && metadata_current;
        for (out, source, field) in [
            ("ci", "ci", None),
            ("comments", "comments", Some("comments")),
            ("reviewComments", "review_comments", Some("review_comments")),
            ("reviews", "reviews", Some("reviews")),
            ("reviewThreads", "review_threads", Some("review_threads")),
            ("reviewStatus", "review_status", None),
        ] {
            let snapshot = self
                .stored_pr_snapshot(&format!("{source}://{suffix}"), repo, row["id"].as_str())
                .await?
                .filter(|v| {
                    source != "ci"
                        || (metadata_current
                            && (row["headRefOid"].is_null() || v["head_sha"] == row["headRefOid"])
                            && (pr.is_null() || v["merge_sha"] == pr["merge_commit_sha"]))
                });
            available &= snapshot.is_some();
            row[out] = snapshot.map_or(Value::Null, |v| {
                field.map_or_else(|| v.clone(), |f| v[f].clone())
            });
        }
        let graph_ci = if !removed
            && (graph_ci.is_null()
                || discovered["commits"]["nodes"][0]["commit"]["oid"] == row["headRefOid"])
        {
            graph_ci
        } else {
            Value::Null
        };
        row["headCiState"] = if graph_ci.is_null() && discovered["headRefOid"] == row["headRefOid"]
        {
            discovered["headCiState"].clone()
        } else {
            graph_ci["state"].clone()
        };
        let policy = self
            .stored_pr_snapshot(
                &format!("required_checks://{suffix}"),
                repo,
                row["id"].as_str(),
            )
            .await?;
        // PR-associated base OIDs and the resolved branch tip are separate
        // evidence. Checking both avoids relabeling policy after a base move
        // without incorrectly equating the two sources' selectors.
        let policy_base_tip = if policy.is_some()
            && let Some(branch) = row["baseRefName"].as_str()
        {
            match self
                .get(
                    &format!(
                        "repos/{repo}/branches/{}",
                        crate::repository::segment(branch)
                    ),
                    Freshness::CachedOnly,
                )
                .await
            {
                Ok(response) => response.data["commit"]["sha"]
                    .as_str()
                    .filter(|sha| crate::repository::valid_sha(sha))
                    .map(str::to_owned),
                Err(Error::CacheMiss) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        row["requiredChecks"] = policy
            .filter(|policy| {
                metadata_current
                    && policy["head_sha"] == row["headRefOid"]
                    && row["baseRefName"].is_string()
                    && policy["base_branch"] == row["baseRefName"]
                    && row["baseRefOid"].is_string()
                    && policy["pr_base_sha"] == row["baseRefOid"]
                    && policy_base_tip.as_deref().is_some_and(|sha| policy["base_sha"] == sha)
                    // A missing legacy field differs from a known absent merge.
                    && policy.get("merge_sha").is_some()
                    && policy["merge_sha"] == pr["merge_commit_sha"]
            })
            .unwrap_or(Value::Null);
        row["statusCheckRollup"] = if row["ci"].is_null() {
            graph_ci["contexts"]["nodes"]
                .as_array()
                .map_or(json!([]), |v| json!(v))
        } else {
            rollup(&row["ci"])
        };
        row["statusCheckRollupComplete"] = json!(if row["ci"].is_null() {
            graph_ci["contexts"]["totalCount"]
                .as_u64()
                .is_some_and(|n| {
                    row["statusCheckRollup"]
                        .as_array()
                        .is_some_and(|v| v.len() as u64 == n)
                })
        } else {
            true
        });
        let mut source_errors = if identity_changed {
            serde_json::Map::new()
        } else {
            old["sourceErrors"].as_object().cloned().unwrap_or_default()
        };
        for (mode, error) in health {
            source_errors.remove(*mode);
            if let Some(error) = error {
                source_errors.insert((*mode).into(), json!(error));
            }
        }
        source_errors.remove("state");
        let selector_changed = expected_node
            .as_deref()
            .zip(row["id"].as_str())
            .is_some_and(|(expected, current)| expected != current);
        if selector_changed {
            row["state"] = json!("UNKNOWN");
            removed = true;
            source_errors.insert(
                "state".into(),
                json!("PR selector belongs to another entity; original lifecycle unresolved"),
            );
        }
        if state == "UNKNOWN" {
            source_errors.insert(
                "state".into(),
                json!("PR lifecycle state unresolved; retry queued"),
            );
        }
        row["complete"] = json!(available && source_errors.is_empty());
        row["sourceErrors"] = json!(source_errors);
        row["removed"] = json!(removed || matches!(state.as_str(), "CLOSED" | "MERGED"));
        let clock = discovery_clock.max(if metadata_current { metadata_at } else { 0 });
        if row == *old {
            self.observe_validated_status(&resource, &old_event, clock, graph_clock, false, &owner)
                .await?;
            return Ok(());
        }
        let kind = if identity_changed {
            "opened"
        } else if row["state"] == "MERGED" && old["state"] != "MERGED" {
            "merged"
        } else if row["state"] == "CLOSED" && old["state"] != "CLOSED" {
            "closed"
        } else if row["removed"] == true && old["removed"] != true {
            "removed"
        } else if old["removed"] == true && row["removed"] != true {
            "reopened"
        } else if old.is_null() && established {
            "opened"
        } else if old.is_null() {
            "baseline"
        } else {
            "updated"
        };
        let mut activity = activity(if identity_changed { &Value::Null } else { old }, &row);
        if matches!(kind, "closed" | "merged" | "reopened" | "removed") {
            activity.insert(
                0,
                json!({"kind":kind,"mergedAt":row["mergedAt"],"closedAt":row["closedAt"]}),
            );
        }
        let fields: BTreeSet<_> = old
            .as_object()
            .into_iter()
            .flat_map(|o| o.keys())
            .chain(row.as_object().into_iter().flat_map(|o| o.keys()))
            .cloned()
            .collect();
        let changed_fields: Vec<_> = fields.into_iter().filter(|f| old[f] != row[f]).collect();
        self.observe_validated_status(&resource, &json!({"pullRequest":row,"kind":kind,"activity":activity,"changedFields":changed_fields}), clock, graph_clock, true, &owner).await?;
        Ok(())
    }

    /// HTTP preflight must not decode a page that will be decoded again later.
    pub(crate) async fn validate_pr_status_cursor(
        &self,
        repository: Option<&str>,
        cursor: &str,
    ) -> Result<()> {
        if let Some(repo) = repository {
            crate::client::validate_repository(repo)?;
        }
        self.validate_change_cursor(raw_pr_cursor(&pr_cursor_scope(repository), cursor)?)
            .await
    }

    /// Repository selection is part of the cursor's scope; changing it requires
    /// a new bootstrap. Limit bounds scanned events, so empty hasMore pages are
    /// valid when unrelated source/branch observations occupy the shared feed.
    pub async fn pr_status_page(
        &self,
        repository: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
        wait: Duration,
    ) -> Result<PrStatusPage> {
        self.pr_status_page_projected(repository, cursor, limit, wait, None)
            .await
    }

    pub(crate) async fn pr_status_page_projected(
        &self,
        repository: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
        wait: Duration,
        fields: Option<&[&str]>,
    ) -> Result<PrStatusPage> {
        if let Some(repo) = repository {
            crate::client::validate_repository(repo)?;
        }
        if !(1..=1000).contains(&limit) || wait > Duration::from_secs(30) {
            return Err(Error::Invalid(
                "limit must be 1..1000 and wait <=30 seconds".into(),
            ));
        }
        let scope = pr_cursor_scope(repository);
        let wrap = |raw: &str| format!("pr1:{scope}:{raw}");
        let matches = |row: &Value| {
            repository.is_none_or(|r| {
                row["repository"]["nameWithOwner"]
                    .as_str()
                    .is_some_and(|v| r.eq_ignore_ascii_case(v))
            })
        };
        let prefix = self.status_prefix();
        let selection_prefix =
            repository.map_or_else(|| prefix.clone(), |repo| format!("{prefix}{repo}/"));
        if let Some(cursor) = cursor {
            let raw = raw_pr_cursor(&scope, cursor)?;
            let deadline = tokio::time::Instant::now() + wait;
            let mut position = raw.to_owned();
            loop {
                let page = self
                    .wait_changes_prefix(
                        Some(&position),
                        limit,
                        deadline.saturating_duration_since(tokio::time::Instant::now()),
                        &selection_prefix,
                        fields,
                    )
                    .await?;
                let changes: Vec<_> = page
                    .changes
                    .into_iter()
                    .filter(|c| c.resource.starts_with(&prefix) && matches(&c.data["pullRequest"]))
                    .map(|c| PrStatusChange {
                        cursor: wrap(&c.cursor),
                        changed_fields: c.data["changedFields"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect(),
                        observed_at_ms: c.observed_at_ms,
                        kind: c.data["kind"].as_str().unwrap_or("updated").to_owned(),
                        activity: c.data["activity"].as_array().cloned().unwrap_or_default(),
                        pull_request: c.data["pullRequest"].clone(),
                    })
                    .collect();
                position = page.next_cursor;
                if !changes.is_empty() || page.has_more || tokio::time::Instant::now() >= deadline {
                    return self
                        .with_discovery_health(
                            repository,
                            PrStatusPage {
                                pull_requests: vec![],
                                changes,
                                cursor: wrap(&position),
                                has_more: page.has_more,
                                complete: true,
                                errors: vec![],
                                coverage: None,
                                account_discovery: AccountDiscoveryHealth::default(),
                            },
                        )
                        .await;
                }
            }
        }
        let page = self.bootstrap_prefix(&prefix, fields).await?;
        let pulls: Vec<_> = page
            .snapshots
            .into_iter()
            .filter(|s| {
                s.resource.starts_with(&prefix)
                    && s.data["pullRequest"]["removed"] != true
                    && s.data["pullRequest"]["state"] == "OPEN"
                    && matches(&s.data["pullRequest"])
            })
            .map(|s| s.data["pullRequest"].clone())
            .collect();
        let complete = pulls.iter().all(|p| p["complete"] == true);
        self.with_discovery_health(
            repository,
            PrStatusPage {
                pull_requests: pulls,
                changes: vec![],
                cursor: wrap(&page.cursor),
                has_more: false,
                complete,
                errors: vec![],
                coverage: None,
                account_discovery: AccountDiscoveryHealth::default(),
            },
        )
        .await
    }

    async fn with_discovery_health(
        &self,
        repository: Option<&str>,
        mut page: PrStatusPage,
    ) -> Result<PrStatusPage> {
        page.update_coverage(repository);
        page.complete &= page
            .changes
            .iter()
            .all(|change| change.pull_request["complete"] == true);
        if let Some(health) = self.discovery_health().await? {
            page.account_discovery.last_poll_at_ms = health.last_poll_at_ms;
            page.account_discovery.last_success_at_ms = health.last_success_at_ms;
            page.account_discovery.complete = health.last_success_at_ms.map(|_| true);
            if let Some(error) = health.last_error {
                page.record_discovery_error(&error);
            }
        }
        Ok(page)
    }
}

fn pr_cursor_scope(repository: Option<&str>) -> String {
    digest(&format!(
        "pr-status:{}",
        repository.unwrap_or("").to_ascii_lowercase()
    ))
}

fn raw_pr_cursor<'a>(scope: &str, cursor: &'a str) -> Result<&'a str> {
    cursor
        .strip_prefix(&format!("pr1:{scope}:"))
        .ok_or_else(|| Error::Invalid("PR cursor does not match this repository selection".into()))
}

fn rollup(ci: &Value) -> Value {
    let mut results = vec![];
    for c in ci["check_runs"].as_array().into_iter().flatten() {
        results.push(json!({"__typename":"CheckRun","name":c["name"],"status":c["status"].as_str().map(str::to_ascii_uppercase),"conclusion":c["conclusion"].as_str().map(str::to_ascii_uppercase),"detailsUrl":c["details_url"]}));
    }
    for s in ci["commit_statuses"].as_array().into_iter().flatten() {
        results.push(json!({"__typename":"StatusContext","context":s["context"],"state":s["state"].as_str().map(str::to_ascii_uppercase),"targetUrl":s["target_url"]}));
    }
    json!(results)
}

fn activity(old: &Value, new: &Value) -> Vec<Value> {
    if old.is_null() {
        return vec![];
    }
    let mut events = vec![];
    for (field, kind) in [
        ("headRefOid", "commits_pushed"),
        ("conflicts", "conflicts_changed"),
        ("ci", "ci_changed"),
        ("reviewStatus", "review_status_changed"),
        ("requiredChecks", "required_checks_changed"),
    ] {
        if old[field] != new[field] {
            if field == "ci" {
                events.push(json!({"kind":kind,"before":old[field]["summary"]["state"],"after":new[field]["summary"]["state"]}));
            } else if field == "headRefOid" || field == "conflicts" {
                events.push(json!({"kind":kind,"before":old[field],"after":new[field]}));
            } else {
                events.push(json!({"kind":kind}));
            }
        }
    }
    for (field, added, edited, deleted) in [
        (
            "comments",
            "comment_added",
            "comment_edited",
            "comment_deleted",
        ),
        (
            "reviewComments",
            "review_comment_added",
            "review_comment_edited",
            "review_comment_deleted",
        ),
        (
            "reviews",
            "review_submitted",
            "review_changed",
            "review_deleted",
        ),
    ] {
        // A missing/failed collection is not evidence of deletion.
        if !old[field].is_array() || !new[field].is_array() {
            continue;
        }
        let before: BTreeMap<_, _> = old[field]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| !c["id"].is_null())
            .map(|c| (c["id"].to_string(), c))
            .collect();
        let after: BTreeMap<_, _> = new[field]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| !c["id"].is_null())
            .map(|c| (c["id"].to_string(), c))
            .collect();
        for (id, comment) in &after {
            let kind = match before.get(id) {
                None => added,
                Some(previous)
                    if previous["body"] != comment["body"]
                        || (field == "reviews" && previous["state"] != comment["state"]) =>
                {
                    edited
                }
                _ => continue,
            };
            events.push(json!({"kind":kind,"source":field,"id":comment["id"],"author":comment["user"],"body":comment["body"],"state":comment["state"],"url":comment["html_url"]}));
        }
        for (id, comment) in &before {
            if !after.contains_key(id) {
                events.push(json!({"kind":deleted,"source":field,"id":comment["id"]}));
            }
        }
    }
    let threads: BTreeMap<_, _> = old["reviewThreads"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| (t["id"].to_string(), t))
        .collect();
    for thread in new["reviewThreads"].as_array().into_iter().flatten() {
        if let Some(previous) = threads.get(&thread["id"].to_string())
            && previous["isResolved"] != thread["isResolved"]
        {
            events.push(json!({"kind":if thread["isResolved"] == true {"thread_resolved"} else {"thread_reopened"},"id":thread["id"]}));
        }
    }
    events
}

#[cfg(test)]
mod incremental_tests {
    use super::*;

    #[test]
    fn older_envelopes_do_not_invent_coverage_or_discovery_success() {
        let page: PrStatusPage = serde_json::from_value(json!({
            "pullRequests":[], "changes":[], "cursor":"old", "hasMore":false,
            "complete":true, "errors":[],
        }))
        .unwrap();
        assert!(page.coverage.is_none());
        assert!(page.account_discovery.complete.is_none());
        assert!(page.account_discovery.last_success_at_ms.is_none());
    }

    #[tokio::test]
    async fn filtered_cursor_pages_return_empty_has_more_and_long_poll_resumes_after_source_events()
    {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            crate::Config {
                cache_path: dir.path().join("cache.sqlite"),
                ..crate::Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let initial = client
            .pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap();
        client
            .observe(
                "comments://github.com/acme/demo/1",
                &json!({"body":"x".repeat(2048)}),
            )
            .await
            .unwrap();
        let row = json!({"repository":{"nameWithOwner":"acme/demo"},"number":1,"state":"OPEN","removed":false,"complete":true});
        client
            .observe(
                "pr-status://github.com/acme/demo/1",
                &json!({"pullRequest":row,"kind":"opened"}),
            )
            .await
            .unwrap();
        let empty = tokio::time::timeout(
            Duration::from_secs(1),
            client.pr_status_page(None, Some(&initial.cursor), 1, Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(empty.changes.is_empty());
        assert!(
            empty.has_more,
            "a filtered empty scan page must return immediately for draining"
        );
        let page = client
            .pr_status_page(None, Some(&empty.cursor), 1000, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(page.changes.len(), 1);
        assert_eq!(page.changes[0].kind, "opened");
        assert!(!page.has_more);
        let waiting = client.clone();
        let cursor = page.cursor.clone();
        let poll = tokio::spawn(async move {
            waiting
                .pr_status_page(None, Some(&cursor), 1000, Duration::from_secs(2))
                .await
                .unwrap()
        });
        client
            .observe(
                "comments://github.com/acme/demo/1",
                &json!({"body":"later unrelated update"}),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        client
            .observe(
                "pr-status://github.com/acme/demo/1",
                &json!({"pullRequest":row,"kind":"ci"}),
            )
            .await
            .unwrap();
        let page = poll.await.unwrap();
        assert_eq!(page.changes.len(), 1);
        assert_eq!(page.changes[0].kind, "ci");
        assert!(!page.has_more);
    }
}
