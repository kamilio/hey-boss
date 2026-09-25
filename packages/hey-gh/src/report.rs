use crate::{Client, Error, Freshness, Result, client::validate_repository, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

// Nested PR/CI reads share this flag: contention makes the entire cached
// observation read-only, preventing stale writes over an active refresh.
tokio::task_local! { static PUBLICATION_READ_ONLY: Arc<AtomicBool>; }
// A background detail projection consumes cached CI but cannot certify that
// a failed independent CI poll has recovered. Unlike a caller's cached read,
// this background writer must wait for publication locks (within the report
// deadline), or a concurrent CI poll can silently discard its combined snapshot.
tokio::task_local! { static PRESERVE_CI_HEALTH: (); }

fn preserves_ci_health() -> bool {
    PRESERVE_CI_HEALTH.try_with(|_| ()).is_ok()
}

fn publication_scope() -> Arc<AtomicBool> {
    PUBLICATION_READ_ONLY
        .try_with(Arc::clone)
        .unwrap_or_default()
}

fn can_publish() -> bool {
    !PUBLICATION_READ_ONLY.with(|flag| flag.load(Ordering::Relaxed))
}

async fn acquire_report_lock(
    lock: Arc<tokio::sync::Mutex<()>>,
    freshness: Freshness,
) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    if matches!(freshness, Freshness::CachedOnly) && !preserves_ci_health() {
        match lock.try_lock_owned() {
            Ok(guard) => Some(guard),
            Err(_) => {
                PUBLICATION_READ_ONLY.with(|flag| flag.store(true, Ordering::Relaxed));
                None
            }
        }
    } else {
        Some(lock.lock_owned().await)
    }
}

tokio::task_local! { static VALIDATIONS: std::cell::RefCell<Vec<ResourceValidation>>; }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceValidation {
    pub resource: String,
    pub validated_at_ms: u64,
    pub source: crate::Source,
}
pub(crate) fn record_validation(resource: &str, response: &crate::Response) {
    let _ = VALIDATIONS.try_with(|records| {
        records.borrow_mut().push(ResourceValidation {
            resource: resource.into(),
            validated_at_ms: response.validated_at_ms,
            source: response.source.clone(),
        })
    });
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceError {
    pub source: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CiSummary {
    /// success, failure, running, skipped, or unknown. Missing sources can never
    /// produce a success claim. Required-check policy is not inferred.
    pub state: String,
    pub successful: usize,
    pub failed: usize,
    pub pending: usize,
    pub skipped: usize,
    pub unknown: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailedResult {
    pub kind: String,
    pub name: String,
    pub conclusion: String,
    pub url: Option<String>,
    pub failed_steps: Vec<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewStatus {
    pub requested_reviewers: Vec<Value>,
    pub requested_teams: Vec<Value>,
    pub latest_reviews: Vec<Value>,
    pub approved_by: Vec<String>,
    pub changes_requested_by: Vec<String>,
    pub dismissed_reviews: Vec<Value>,
    pub resolved_threads: usize,
    pub unresolved_threads: usize,
    pub outdated_threads: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CiReport {
    pub head_sha: String,
    pub merge_sha: Option<String>,
    pub check_runs: Vec<Value>,
    pub commit_statuses: Vec<Value>,
    pub workflow_runs: Vec<Value>,
    pub jobs: Vec<Value>,
    pub summary: CiSummary,
    pub failures: Vec<FailedResult>,
    pub errors: Vec<SourceError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CiObservation {
    pub data: CiReport,
    pub complete: bool,
    pub cursor: Option<String>,
    pub observed_at_ms: u64,
    pub oldest_validation_at_ms: u64,
    pub validations: Vec<ResourceValidation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrReport {
    pub repository: String,
    pub number: u64,
    /// Complete REST PR metadata, including draft/base/head/labels/mergeability.
    pub pull_request: Value,
    /// clean, conflicting, or unknown; blocking CI/reviews are separate facts.
    pub conflicts: String,
    pub comments: Vec<Value>,
    pub review_comments: Vec<Value>,
    pub reviews: Vec<Value>,
    pub timeline: Vec<Value>,
    pub review_events: Vec<Value>,
    pub review_threads: Vec<Value>,
    pub review_status: ReviewStatus,
    pub ci: CiReport,
    pub errors: Vec<SourceError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub data: PrReport,
    pub observed_at_ms: u64,
    /// Resume after this observation, without a snapshot/feed race.
    pub cursor: Option<String>,
    pub complete: bool,
    pub oldest_validation_at_ms: u64,
    pub validations: Vec<ResourceValidation>,
}

impl Client {
    // Keep failures before/after collection visible for already tracked PRs.
    // Remain inside the publication scope: contended cached reads and internal
    // detail/policy projections must not change independent source health.
    async fn individual_read_result<T>(
        &self,
        repository: &str,
        number: u64,
        mode: &'static str,
        future: impl std::future::Future<
            Output = std::result::Result<Result<T>, tokio::time::error::Elapsed>,
        >,
    ) -> Result<T> {
        let result = future.await.unwrap_or(Err(Error::Deadline));
        if let Err(error) = &result
            && can_publish()
            && !preserves_ci_health()
            && self.current_pr_owner_is_valid().await.unwrap_or(false)
            && let Err(record_error) = self
                .publish_individual_pr_status(
                    repository,
                    number,
                    &[(mode, Some(error.to_string()))],
                )
                .await
        {
            // Preserve the actual read failure even if diagnostics cannot be
            // committed. Never replace an upstream403 with a storage error.
            tracing::warn!(
                mode,
                error_code = record_error.diagnostic_code(),
                "PR read failure health could not be recorded; original error retained"
            );
        }
        result
    }
    /// Independent-watch detail polling spends no additional CI requests.
    /// Keep the combined source snapshot compatible for source-feed consumers,
    /// but cached CI projection must preserve the CI loop's failure health.
    pub(crate) async fn refresh_monitored_pr_details(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Vec<SourceError>> {
        crate::client::INTERACTIVE_READ
            .scope(
                self.report_priority("pr", repository, number, false),
                self.refresh_monitored_pr_details_inner(repository, number, freshness),
            )
            .await
    }

    async fn refresh_monitored_pr_details_inner(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Vec<SourceError>> {
        let result = tokio::time::timeout(self.report_timeout(), async {
            let lock = self.report_lock(&format!("{}#{number}", repository.to_ascii_lowercase()));
            let _guard = lock.lock().await;
            self.refresh_pr_details(repository, number, freshness).await
        })
        .await
        .unwrap_or(Err(Error::Deadline));
        let health = match &result {
            Ok(errors) => source_error_message(errors),
            Err(error) => Some(error.to_string()),
        };
        self.publish_individual_pr_status(repository, number, &[("details", health)])
            .await?;
        let errors = result?;
        if errors.is_empty() {
            match PRESERVE_CI_HEALTH
                .scope(
                    (),
                    self.pr_report(repository, number, Freshness::CachedOnly),
                )
                .await
            {
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    repository,
                    number,
                    error_code = error.diagnostic_code(),
                    "cached combined PR projection unavailable; independent detail sources retained"
                ),
            }
        }
        Ok(errors)
    }

    /// Hydrate conversation/review sources without waiting on CI. Publish each
    /// completed collection independently so later failures retain progress.
    pub(crate) async fn refresh_pr_details(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Vec<SourceError>> {
        crate::entity::scope(Box::pin(
            self.refresh_pr_details_inner(repository, number, freshness),
        ))
        .await
    }

    async fn refresh_pr_details_inner(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Vec<SourceError>> {
        let suffix = format!("{}/{repository}/{number}", self.hostname());
        let pr = self.pull_request(repository, number, freshness).await?;
        crate::entity::set(self.pr_owner(repository, number, &pr.data).await?);
        self.observe(
            &format!("metadata://{suffix}"),
            &json!({"pull_request":pr.data,"conflicts":conflicts(&pr.data)}),
        )
        .await?;
        let prefix = format!("repos/{repository}");
        let mut errors = Vec::new();
        let mut reviews = None;
        let mut threads = None;
        let sources = [
            (
                "comments",
                format!("{prefix}/issues/{number}/comments?per_page=100"),
            ),
            (
                "review_comments",
                format!("{prefix}/pulls/{number}/comments?per_page=100"),
            ),
            (
                "reviews",
                format!("{prefix}/pulls/{number}/reviews?per_page=100"),
            ),
            (
                "timeline",
                format!("{prefix}/issues/{number}/timeline?per_page=100"),
            ),
            ("review_events", String::new()),
            ("review_threads", String::new()),
        ];
        // Independent sources can share a queue round. Keep tiny embedded
        // queues sequential rather than making their own reads overload them.
        let width = if self.status().queue_capacity >= 32 {
            3
        } else {
            1
        };
        // Fetch issue comments first: an upstream stall in reviews must not
        // get ahead of new comments on the daemon's serial transport queue.
        for group in sources[..1].chunks(1).chain(sources[1..].chunks(width)) {
            let fetch = |index: usize| async move {
                if let Some((source, path)) = group.get(index) {
                    self.refresh_pr_detail_source(repository, number, source, path, freshness)
                        .await
                } else {
                    Ok(Vec::new())
                }
            };
            let (a, b, c) = tokio::join!(fetch(0), fetch(1), fetch(2));
            for ((source, _), result) in group.iter().zip([a, b, c]) {
                match result {
                    Ok(values) => {
                        if *source == "reviews" {
                            reviews = Some(values);
                        } else if *source == "review_threads" {
                            threads = Some(values);
                        }
                    }
                    Err(error) => {
                        errors.push(SourceError {
                            source: (*source).into(),
                            message: error.to_string(),
                        });
                    }
                }
            }
        }
        let final_pr = self
            .pull_request(
                repository,
                number,
                if matches!(freshness, Freshness::CachedOnly) {
                    freshness
                } else {
                    Freshness::Revalidate
                },
            )
            .await?;
        if pr.data["node_id"] != final_pr.data["node_id"] {
            return Err(crate::entity::changed());
        }
        self.observe(
            &format!("metadata://{suffix}"),
            &json!({"pull_request":final_pr.data,"conflicts":conflicts(&final_pr.data)}),
        )
        .await?;
        if let (Some(reviews), Some(threads)) = (reviews, threads) {
            self.observe(
                &format!("review_status://{suffix}"),
                &serde_json::to_value(review_status(&final_pr.data, &reviews, &threads))
                    .map_err(|error| Error::Invalid(error.to_string()))?,
            )
            .await?;
        }
        Ok(errors)
    }

    async fn refresh_pr_detail_source(
        &self,
        repository: &str,
        number: u64,
        source: &str,
        path: &str,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        let started = tokio::time::Instant::now();
        tracing::info!(
            repository,
            number,
            source,
            "PR detail source refresh started"
        );
        let result: Result<Vec<Value>> = async {
            let values = match source {
                "review_events" => self.review_events(repository, number, freshness).await?,
                "review_threads" => self.review_threads(repository, number, freshness).await?,
                _ => self.pages(path, None, freshness).await?,
            };
            check_size(values.iter(), self.collection_limit())?;
            let suffix = format!("{}/{repository}/{number}", self.hostname());
            // Publication belongs to the source future, not the batch join:
            // successful evidence survives cancellation of a stalled sibling.
            self.observe(&format!("{source}://{suffix}"), &json!({source:values}))
                .await?;
            Ok(values)
        }
        .await;
        match &result {
            Ok(values) => tracing::info!(
                repository,
                number,
                source,
                items = values.len(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "PR detail source refresh completed"
            ),
            Err(error) => tracing::warn!(
                repository,
                number,
                source,
                error_code = error.diagnostic_code(),
                "PR detail source refresh failed"
            ),
        }
        result
    }

    /// CI-only path never spends GraphQL quota or waits for review pagination.
    pub async fn ci_for_pr(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<CiObservation> {
        validate_repository(repository)?;
        let priority = if matches!(freshness, Freshness::CachedOnly) {
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
        } else {
            self.report_priority("ci", repository, number, crate::client::interactive_read())
        };
        crate::client::INTERACTIVE_READ
            .scope(
                priority,
                crate::entity::scope(Box::pin(
                    self.ci_for_pr_inner(repository, number, freshness),
                )),
            )
            .await
    }

    async fn ci_for_pr_inner(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<CiObservation> {
        validate_repository(repository)?;
        let lock = self.report_lock(&format!("{}#{number}:ci", repository.to_ascii_lowercase()));
        PUBLICATION_READ_ONLY.scope(publication_scope(), self.individual_read_result(repository, number, "ci", tokio::time::timeout(self.report_timeout(), async {
            let _guard = acquire_report_lock(lock, freshness).await;
            let repository_spelling = self.pr_repository_spelling(repository, number).await?;
            let repository = repository_spelling.as_str();
            VALIDATIONS.scope(std::cell::RefCell::new(Vec::new()), async {
                for attempt in 0..2 {
                    let policy = if attempt == 0 || matches!(freshness, Freshness::CachedOnly) {
                        freshness
                    } else {
                        Freshness::Revalidate
                    };
                    crate::entity::clear();
                    let pr = self.pull_request(repository, number, policy).await?;
                    crate::entity::set(self.pr_owner(repository, number, &pr.data).await?);
                    // Lifecycle evidence does not depend on finishing CI jobs.
                    if can_publish() {
                        self.observe(
                            &format!("metadata://{}/{repository}/{number}", self.hostname()),
                            &json!({"conflicts":conflicts(&pr.data),"pull_request":pr.data}),
                        ).await?;
                        self.publish_individual_pr_status(repository, number, &[]).await?;
                    }
                    let head = sha(&pr.data, "head")?;
                    let merge = pr.data["merge_commit_sha"].as_str().filter(|s| valid_sha(s));
                    let data = self.ci_report(repository, &head, merge, policy).await?;
                    let final_policy = if matches!(policy, Freshness::CachedOnly) { policy } else { Freshness::Revalidate };
                    let final_pr = self.pull_request(repository, number, final_policy).await?;
                    if pr.data["node_id"] != final_pr.data["node_id"]
                        || pr.data["head"]["sha"] != final_pr.data["head"]["sha"]
                        || pr.data["base"]["sha"] != final_pr.data["base"]["sha"]
                        || pr.data["merge_commit_sha"] != final_pr.data["merge_commit_sha"] {
                        continue;
                    }
                    let complete = data.errors.is_empty();
                    let suffix = format!("{}/{repository}/{number}", self.hostname());
                    let mut observations = vec![(format!("metadata://{suffix}"), json!({"conflicts":conflicts(&final_pr.data),"pull_request":final_pr.data}))];
                    if complete {
                        observations.push((format!("ci://{suffix}"), serde_json::to_value(&data).map_err(|e| Error::Invalid(e.to_string()))?));
                    }
                    let cursor = if can_publish() {
                        let cursor = self.observe_many(&observations).await?;
                        if !preserves_ci_health() {
                            self.publish_individual_pr_status(repository, number, &[("ci", source_error_message(&data.errors))]).await?;
                        }
                        complete.then_some(cursor)
                    } else {
                        None
                    };
                    let validations = VALIDATIONS.with(|r| r.borrow().clone());
                    let observed_at_ms = now_ms();
                    let oldest_validation_at_ms = validations.iter().map(|r| r.validated_at_ms).min().unwrap_or(observed_at_ms);
                    return Ok(CiObservation { data, complete, cursor, observed_at_ms, oldest_validation_at_ms, validations });
                }
                Err(Error::Invalid("PR head changed repeatedly while collecting CI".into()))
            }).await
        }))).await
    }
    pub async fn pull_request(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<crate::Response> {
        validate_repository(repository)?;
        if number == 0 {
            return Err(Error::Invalid("pull number must be positive".into()));
        }
        self.get(&format!("repos/{repository}/pulls/{number}"), freshness)
            .await
    }

    /// Enumerate the effective gh user's open PRs without GitHub Search's 1000
    /// result ceiling. Each configured repo is fully paginated through REST.
    pub async fn my_pull_requests(
        &self,
        repository: &str,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        validate_repository(repository)?;
        let user = self.get("user", freshness).await?;
        let login = user.data["login"]
            .as_str()
            .ok_or_else(|| Error::Invalid("GitHub user has no login".into()))?;
        let pulls = self
            .pages(
                &format!("repos/{repository}/pulls?state=open&per_page=100"),
                None,
                freshness,
            )
            .await?;
        Ok(pulls
            .into_iter()
            .filter(|p| {
                p["user"]["login"]
                    .as_str()
                    .is_some_and(|u| u.eq_ignore_ascii_case(login))
            })
            .collect())
    }

    /// All repository PR states, without the GitHub Search result ceiling.
    pub async fn list_pull_requests(
        &self,
        repository: &str,
        state: &str,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        validate_repository(repository)?;
        if !matches!(state, "open" | "closed" | "all") {
            return Err(Error::Invalid(
                "PR state must be open, closed, or all".into(),
            ));
        }
        let pulls=self.pages(&format!("repos/{repository}/pulls?state={state}&sort=updated&direction=desc&per_page=100"),None,freshness).await?;
        self.observe(
            &format!("prs://{}/{repository}/{state}", self.hostname()),
            &json!({"pull_requests":pulls}),
        )
        .await?;
        Ok(pulls)
    }

    pub async fn pr_report(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Report> {
        validate_repository(repository)?;
        let priority = if matches!(freshness, Freshness::CachedOnly) {
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
        } else {
            self.report_priority("pr", repository, number, crate::client::interactive_read())
        };
        crate::client::INTERACTIVE_READ
            .scope(
                priority,
                crate::entity::scope(Box::pin(
                    self.pr_report_inner(repository, number, freshness),
                )),
            )
            .await
    }

    async fn pr_report_inner(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Report> {
        validate_repository(repository)?;
        let lock = self.report_lock(&format!("{}#{number}", repository.to_ascii_lowercase()));
        PUBLICATION_READ_ONLY
            .scope(
                publication_scope(),
                self.individual_read_result(
                    repository,
                    number,
                    "details",
                    tokio::time::timeout(self.report_timeout(), async {
                        let _guard = acquire_report_lock(lock, freshness).await;
                        VALIDATIONS
                            .scope(std::cell::RefCell::new(Vec::new()), async {
                                let mut report =
                                    self.build_report(repository, number, freshness).await?;
                                report.validations =
                                    VALIDATIONS.with(|records| records.borrow().clone());
                                report.oldest_validation_at_ms = report
                                    .validations
                                    .iter()
                                    .map(|v| v.validated_at_ms)
                                    .min()
                                    .unwrap_or(report.observed_at_ms);
                                Ok(report)
                            })
                            .await
                    }),
                ),
            )
            .await
    }

    async fn build_report(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Report> {
        let repository_spelling = self.pr_repository_spelling(repository, number).await?;
        let repository = repository_spelling.as_str();
        // Recheck the PR head after collecting commit-bound sources. Never
        // present old-head checks as a result for a newly pushed head.
        for attempt in 0..2 {
            crate::entity::clear();
            let pr = self
                .pull_request(
                    repository,
                    number,
                    if attempt == 0 || matches!(freshness, Freshness::CachedOnly) {
                        freshness
                    } else {
                        Freshness::Revalidate
                    },
                )
                .await?;
            crate::entity::set(self.pr_owner(repository, number, &pr.data).await?);
            let head = sha(&pr.data, "head")?;
            let merge = pr.data["merge_commit_sha"]
                .as_str()
                .filter(|s| valid_sha(s))
                .map(str::to_owned);
            let base_sha = pr.data["base"]["sha"].clone();
            let ci_observation = self.ci_for_pr(repository, number, freshness).await?;
            VALIDATIONS.with(|records| records.borrow_mut().extend(ci_observation.validations));
            let ci = ci_observation.data;
            if ci.head_sha != head || ci.merge_sha != merge {
                continue;
            }
            let mut errors = Vec::new();
            let prefix = format!("repos/{repository}");
            let comments = collect(
                self.pages(
                    &format!("{prefix}/issues/{number}/comments?per_page=100"),
                    None,
                    freshness,
                )
                .await,
                "comments",
                &mut errors,
            );
            let review_comments = collect(
                self.pages(
                    &format!("{prefix}/pulls/{number}/comments?per_page=100"),
                    None,
                    freshness,
                )
                .await,
                "review_comments",
                &mut errors,
            );
            let reviews = collect(
                self.pages(
                    &format!("{prefix}/pulls/{number}/reviews?per_page=100"),
                    None,
                    freshness,
                )
                .await,
                "reviews",
                &mut errors,
            );
            let timeline = collect(
                self.pages(
                    &format!("{prefix}/issues/{number}/timeline?per_page=100"),
                    None,
                    freshness,
                )
                .await,
                "timeline",
                &mut errors,
            );
            let review_events = collect(
                self.review_events(repository, number, freshness).await,
                "review_events",
                &mut errors,
            );
            let review_threads = collect(
                self.review_threads(repository, number, freshness).await,
                "review_threads",
                &mut errors,
            );
            let final_pr = self
                .pull_request(
                    repository,
                    number,
                    if matches!(freshness, Freshness::CachedOnly) {
                        freshness
                    } else {
                        Freshness::Revalidate
                    },
                )
                .await?;
            if final_pr.data["node_id"] != pr.data["node_id"]
                || final_pr.data["head"]["sha"].as_str() != Some(&head)
                || final_pr.data["base"]["sha"] != base_sha
                || final_pr.data["merge_commit_sha"]
                    .as_str()
                    .filter(|s| valid_sha(s))
                    != merge.as_deref()
            {
                continue;
            }
            let conflicts = conflicts(&final_pr.data).to_owned();
            check_size(
                std::iter::once(&final_pr.data)
                    .chain(&comments)
                    .chain(&review_comments)
                    .chain(&reviews)
                    .chain(&timeline)
                    .chain(&review_events)
                    .chain(&review_threads)
                    .chain(&ci.check_runs)
                    .chain(&ci.commit_statuses)
                    .chain(&ci.workflow_runs)
                    .chain(&ci.jobs),
                self.collection_limit(),
            )?;
            let review_status = review_status(&final_pr.data, &reviews, &review_threads);
            let report = PrReport {
                repository: repository.into(),
                number,
                pull_request: final_pr.data,
                conflicts,
                comments,
                review_comments,
                reviews,
                timeline,
                review_events,
                review_threads,
                review_status,
                ci,
                errors,
            };
            let complete = report.errors.is_empty() && report.ci.errors.is_empty();
            // Incomplete observations are explicit and are not appended as a
            // replacement for a previously complete snapshot.
            let suffix = format!("{}/{repository}/{number}", self.hostname());
            let mut observations = vec![(
                format!("metadata://{suffix}"),
                json!({"pull_request":report.pull_request,"conflicts":report.conflicts}),
            )];
            for (field, data) in [
                ("comments", json!({"comments":report.comments})),
                (
                    "review_comments",
                    json!({"review_comments":report.review_comments}),
                ),
                ("reviews", json!({"reviews":report.reviews})),
                ("timeline", json!({"timeline":report.timeline})),
                (
                    "review_events",
                    json!({"review_events":report.review_events}),
                ),
                (
                    "review_threads",
                    json!({"review_threads":report.review_threads}),
                ),
            ] {
                if !report.errors.iter().any(|e| e.source == field) {
                    observations.push((format!("{field}://{suffix}"), data));
                }
            }
            if !report
                .errors
                .iter()
                .any(|e| matches!(e.source.as_str(), "reviews" | "review_threads"))
            {
                observations.push((
                    format!("review_status://{suffix}"),
                    serde_json::to_value(&report.review_status)
                        .map_err(|e| Error::Invalid(e.to_string()))?,
                ));
            }
            if complete {
                observations.push((
                    format!("pr://{suffix}"),
                    serde_json::to_value(&report).map_err(|e| Error::Invalid(e.to_string()))?,
                ));
            }
            let cursor = if can_publish() {
                let head = self.observe_many(&observations).await?;
                let mut health = vec![("details", source_error_message(&report.errors))];
                if !preserves_ci_health() {
                    health.push(("ci", source_error_message(&report.ci.errors)));
                }
                self.publish_individual_pr_status(repository, number, &health)
                    .await?;
                complete.then_some(head)
            } else {
                None
            };
            return Ok(Report {
                data: report,
                observed_at_ms: now_ms(),
                cursor,
                complete,
                oldest_validation_at_ms: 0,
                validations: Vec::new(),
            });
        }
        Err(Error::Invalid(
            "PR head/base changed repeatedly while collecting status; retry the report".into(),
        ))
    }

    pub async fn ci_report(
        &self,
        repository: &str,
        head: &str,
        merge: Option<&str>,
        freshness: Freshness,
    ) -> Result<CiReport> {
        self.collect_ci_report(repository, head, merge, freshness, true)
            .await
    }

    // Required-check policy depends on checks/statuses, never optional workflow
    // jobs. Do not publish this partial collection as a full CI recovery.
    pub(crate) async fn required_ci_report(
        &self,
        repository: &str,
        head: &str,
        merge: Option<&str>,
        freshness: Freshness,
    ) -> Result<CiReport> {
        self.collect_ci_report(repository, head, merge, freshness, false)
            .await
    }

    async fn collect_ci_report(
        &self,
        repository: &str,
        head: &str,
        merge: Option<&str>,
        freshness: Freshness,
        include_workflows: bool,
    ) -> Result<CiReport> {
        validate_repository(repository)?;
        if !valid_sha(head) || merge.is_some_and(|s| !valid_sha(s)) {
            return Err(Error::Invalid(
                "CI refs must be immutable commit SHAs".into(),
            ));
        }
        let mut errors = Vec::new();
        let (mut checks, mut statuses, mut workflows, mut jobs) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut refs = vec![head];
        if let Some(merge) = merge.filter(|s| *s != head) {
            refs.push(merge);
        }
        for sha in refs {
            let sources = [
                (
                    "check_runs",
                    format!(
                        "repos/{repository}/commits/{sha}/check-runs?filter=latest&per_page=100"
                    ),
                    "check_runs",
                ),
                (
                    "commit_statuses",
                    format!("repos/{repository}/commits/{sha}/status?per_page=100"),
                    "statuses",
                ),
                (
                    "workflow_runs",
                    format!("repos/{repository}/actions/runs?head_sha={sha}&per_page=100"),
                    "workflow_runs",
                ),
            ];
            let sources = if include_workflows {
                &sources[..]
            } else {
                &sources[..2]
            };
            // Batch independent reads of one immutable ref. Requests still use
            // the shared serial scheduler and its quota/cooldown protections.
            let width = if self.status().queue_capacity >= 32 {
                3
            } else {
                1
            };
            for group in sources.chunks(width) {
                let fetch = |index: usize| async move {
                    if let Some((source, path, field)) = group.get(index) {
                        self.ci_source(repository, sha, source, path, field, freshness)
                            .await
                    } else {
                        Ok(Vec::new())
                    }
                };
                let (a, b, c) = tokio::join!(fetch(0), fetch(1), fetch(2));
                for ((source, _, _), result) in group.iter().zip([a, b, c]) {
                    let mut values = collect(result, &format!("{source}:{sha}"), &mut errors);
                    match *source {
                        "check_runs" => checks.extend(values),
                        "commit_statuses" => {
                            for status in &mut values {
                                status["observed_sha"] = json!(sha);
                            }
                            statuses.extend(values);
                        }
                        "workflow_runs" => workflows.extend(values),
                        _ => unreachable!("fixed CI source list"),
                    }
                    check_size(
                        checks.iter().chain(&statuses).chain(&workflows),
                        self.collection_limit(),
                    )?;
                }
            }
        }
        check_size(
            checks.iter().chain(&statuses).chain(&workflows),
            self.collection_limit(),
        )?;
        dedup_id(&mut checks);
        dedup_id(&mut statuses);
        dedup_id(&mut workflows);
        // Only the newest run per workflow + event + SHA contributes. A cancelled
        // previous run must not mask a successful current run; attempts are kept.
        let mut latest = HashMap::<String, Value>::new();
        for run in workflows {
            let key = format!(
                "{}:{}:{}",
                run["workflow_id"], run["event"], run["head_sha"]
            );
            let rank = (
                run["run_number"].as_u64().unwrap_or(0),
                run["run_attempt"].as_u64().unwrap_or(0),
                run["id"].as_u64().unwrap_or(0),
            );
            if latest.get(&key).is_none_or(|old| {
                rank > (
                    old["run_number"].as_u64().unwrap_or(0),
                    old["run_attempt"].as_u64().unwrap_or(0),
                    old["id"].as_u64().unwrap_or(0),
                )
            }) {
                latest.insert(key, run);
            }
        }
        let mut workflows: Vec<Value> = latest.into_values().collect();
        workflows.sort_by_key(|r| r["id"].as_u64().unwrap_or(0));
        for run in &workflows {
            if let (Some(id), Some(attempt)) = (run["id"].as_u64(), run["run_attempt"].as_u64()) {
                jobs.extend(collect(
                    self.workflow_jobs(repository, id, attempt, run, freshness)
                        .await,
                    &format!("jobs:{id}:{attempt}"),
                    &mut errors,
                ));
                check_size(
                    checks
                        .iter()
                        .chain(&statuses)
                        .chain(&workflows)
                        .chain(&jobs),
                    self.collection_limit(),
                )?;
            } else {
                errors.push(SourceError {
                    source: "workflow_runs".into(),
                    message: "workflow run lacks id or attempt".into(),
                });
            }
        }
        check_size(
            checks
                .iter()
                .chain(&statuses)
                .chain(&workflows)
                .chain(&jobs),
            self.collection_limit(),
        )?;
        let summary = summarize(&checks, &statuses, &workflows, &jobs, !errors.is_empty());
        let failures = failed_results(&checks, &statuses, &workflows, &jobs);
        Ok(CiReport {
            head_sha: head.into(),
            merge_sha: merge.map(str::to_owned),
            check_runs: checks,
            commit_statuses: statuses,
            workflow_runs: workflows,
            jobs,
            summary,
            failures,
            errors,
        })
    }

    async fn ci_source(
        &self,
        repository: &str,
        sha: &str,
        source: &str,
        path: &str,
        field: &str,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        let started = tokio::time::Instant::now();
        tracing::info!(repository, sha, source, "CI source refresh started");
        let result = self.pages(path, Some(field), freshness).await;
        match &result {
            Ok(values) => tracing::info!(
                repository,
                sha,
                source,
                items = values.len(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "CI source refresh completed"
            ),
            Err(error) => tracing::warn!(
                repository,
                sha,
                source,
                error_code = error.diagnostic_code(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "CI source refresh failed"
            ),
        }
        result
    }

    async fn workflow_jobs(
        &self,
        repository: &str,
        id: u64,
        attempt: u64,
        run: &Value,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        let path =
            format!("repos/{repository}/actions/runs/{id}/attempts/{attempt}/jobs?per_page=100");
        let finished = run["status"] == "completed";
        let version = json!({"updated_at":run["updated_at"],"completed_at":run["completed_at"],"conclusion":run["conclusion"],"attempt":attempt});
        let key = format!(
            "completed-jobs://{}/{repository}/{id}/{attempt}#{}",
            self.hostname(),
            crate::digest(&version.to_string())
        );
        if finished
            && matches!(freshness, Freshness::MaxAge(_))
            && let Some(cached) = self.derived(&key).await?
            && now_ms().saturating_sub(cached.validated_at_ms) < 86400 * 1000
        {
            return cached.decode();
        }
        // A newly completed parent must validate every page, even if a prior
        // in-progress run happened to have only completed jobs at that moment.
        let policy = if finished && matches!(freshness, Freshness::MaxAge(_)) {
            Freshness::Revalidate
        } else {
            freshness
        };
        let jobs = self
            .ci_source(
                repository,
                run["head_sha"].as_str().unwrap_or("unknown"),
                "jobs",
                &path,
                "jobs",
                policy,
            )
            .await?;
        if finished
            && !jobs.is_empty()
            && jobs.iter().all(|j| j["status"] == "completed")
            && !matches!(freshness, Freshness::CachedOnly)
        {
            self.save_derived(&key, json!(jobs)).await?;
        }
        Ok(jobs)
    }

    async fn review_events(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        let repository_spelling = self.pr_repository_spelling(repository, number).await?;
        let (owner, repo) = repository_spelling
            .split_once('/')
            .expect("validated repository");
        let mut after = Value::Null;
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        let mut bytes = 0usize;
        for _ in 0..1000 {
            let response = self
                .graphql(
                    REVIEW_EVENTS_QUERY,
                    json!({"owner":owner,"repo":repo,"number":number,"after":after}),
                    freshness,
                )
                .await?;
            bytes = bytes.saturating_add(response.data.to_string().len());
            if bytes > self.collection_limit() {
                return Err(Error::Invalid(
                    "review events exceed collection limit".into(),
                ));
            }
            let connection = &response.data["data"]["repository"]["pullRequest"]["timelineItems"];
            events.extend(
                connection["nodes"]
                    .as_array()
                    .ok_or_else(|| Error::Invalid("missing review events".into()))?
                    .iter()
                    .cloned(),
            );
            match connection["pageInfo"]["hasNextPage"].as_bool() {
                Some(false) => return Ok(events),
                Some(true) => {
                    let cursor = connection["pageInfo"]["endCursor"]
                        .as_str()
                        .ok_or_else(|| Error::Invalid("missing review event cursor".into()))?;
                    if !seen.insert(cursor.to_owned()) {
                        return Err(Error::Invalid("review event pagination cycle".into()));
                    }
                    after = json!(cursor);
                }
                None => return Err(Error::Invalid("missing review event page info".into())),
            }
        }
        Err(Error::Invalid(
            "review event pagination exceeds 1000 pages".into(),
        ))
    }

    async fn review_threads(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        let repository_spelling = self.pr_repository_spelling(repository, number).await?;
        let (owner, repo) = repository_spelling
            .split_once('/')
            .expect("validated repository");
        let mut cursor = Value::Null;
        let mut seen = HashSet::new();
        let mut threads = Vec::new();
        let mut bytes = 0usize;
        for _ in 0..1000 {
            let response = self
                .graphql(
                    THREAD_QUERY,
                    json!({"owner":owner,"repo":repo,"number":number,"after":cursor}),
                    freshness,
                )
                .await?;
            bytes = bytes.saturating_add(response.data.to_string().len());
            if bytes > self.collection_limit() {
                return Err(Error::Invalid(
                    "review threads exceed collection byte limit".into(),
                ));
            }
            let connection = &response.data["data"]["repository"]["pullRequest"]["reviewThreads"];
            let nodes = connection["nodes"]
                .as_array()
                .ok_or_else(|| Error::Invalid("missing GraphQL review threads".into()))?;
            // Thread comments are paginated independently, including replies.
            for node in nodes {
                let mut thread = node.clone();
                let mut comments = thread["comments"]["nodes"]
                    .as_array()
                    .cloned()
                    .ok_or_else(|| Error::Invalid("missing thread comments".into()))?;
                let mut info = thread["comments"]["pageInfo"].clone();
                let mut comment_cursors = HashSet::new();
                while info["hasNextPage"].as_bool() == Some(true) {
                    let after = info["endCursor"]
                        .as_str()
                        .ok_or_else(|| Error::Invalid("thread pagination cursor missing".into()))?;
                    if !comment_cursors.insert(after.to_owned()) || comment_cursors.len() > 1000 {
                        return Err(Error::Invalid("thread pagination cycle or limit".into()));
                    }
                    let page = self
                        .graphql(
                            COMMENT_QUERY,
                            json!({"id":thread["id"],"after":after}),
                            freshness,
                        )
                        .await?;
                    bytes = bytes.saturating_add(page.data.to_string().len());
                    if bytes > self.collection_limit() {
                        return Err(Error::Invalid(
                            "thread replies exceed collection byte limit".into(),
                        ));
                    }
                    let conn = &page.data["data"]["node"]["comments"];
                    comments.extend(
                        conn["nodes"]
                            .as_array()
                            .ok_or_else(|| Error::Invalid("missing thread comment page".into()))?
                            .iter()
                            .cloned(),
                    );
                    info = conn["pageInfo"].clone();
                }
                thread["comments"] = json!({"nodes":comments});
                threads.push(thread);
            }
            if connection["pageInfo"]["hasNextPage"].as_bool() == Some(false) {
                return Ok(threads);
            }
            cursor = connection["pageInfo"]["endCursor"].clone();
            if cursor.is_null() || !seen.insert(cursor.to_string()) {
                return Err(Error::Invalid(
                    "review thread pagination cycle or missing cursor".into(),
                ));
            }
        }
        Err(Error::Invalid(
            "review thread pagination exceeds 1000 pages".into(),
        ))
    }
}

fn check_size<'a>(values: impl Iterator<Item = &'a Value>, limit: usize) -> Result<()> {
    let mut bytes = 0usize;
    for value in values {
        bytes = bytes.saturating_add(value.to_string().len());
        if bytes > limit {
            return Err(Error::Invalid(
                "CI results exceed collection byte limit".into(),
            ));
        }
    }
    Ok(())
}

fn conflicts(pr: &Value) -> &'static str {
    match pr["mergeable"].as_bool() {
        Some(true) => "clean",
        Some(false) => "conflicting",
        None => "unknown",
    }
}

fn source_error_message(errors: &[SourceError]) -> Option<String> {
    (!errors.is_empty()).then(|| json!(errors).to_string())
}

fn collect(result: Result<Vec<Value>>, source: &str, errors: &mut Vec<SourceError>) -> Vec<Value> {
    match result {
        Ok(values) => values,
        Err(e) => {
            errors.push(SourceError {
                source: source.into(),
                message: e.to_string(),
            });
            Vec::new()
        }
    }
}
fn sha(pr: &Value, field: &str) -> Result<String> {
    pr[field]["sha"]
        .as_str()
        .filter(|s| valid_sha(s))
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("missing immutable {field} SHA")))
}
fn valid_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|c| c.is_ascii_hexdigit())
}
fn dedup_id(values: &mut Vec<Value>) {
    let mut seen = HashSet::new();
    values.retain(|v| seen.insert(v["id"].to_string()));
    values.sort_by_key(|v| v["id"].as_u64().unwrap_or(0));
}

fn review_status(pr: &Value, reviews: &[Value], threads: &[Value]) -> ReviewStatus {
    let mut latest = std::collections::BTreeMap::<String, &Value>::new();
    for review in reviews {
        if review["state"] == "PENDING" {
            continue;
        }
        if let Some(login) = review["user"]["login"].as_str() {
            let key = login.to_lowercase();
            let rank = (
                review["submitted_at"].as_str().unwrap_or(""),
                review["id"].as_u64().unwrap_or(0),
            );
            if latest.get(&key).is_none_or(|old| {
                rank > (
                    old["submitted_at"].as_str().unwrap_or(""),
                    old["id"].as_u64().unwrap_or(0),
                )
            }) {
                latest.insert(key, review);
            }
        }
    }
    // A later COMMENTED review doesn't discard a still-effective approval or
    // changes-requested review. A dismissed review no longer contributes its
    // own decision; it does not revoke a separate, still-active older review.
    let mut decisions = std::collections::BTreeMap::<String, &Value>::new();
    for review in reviews {
        if !matches!(
            review["state"].as_str(),
            Some("APPROVED" | "CHANGES_REQUESTED")
        ) {
            continue;
        }
        if let Some(login) = review["user"]["login"].as_str() {
            let key = login.to_lowercase();
            let rank = (
                review["submitted_at"].as_str().unwrap_or(""),
                review["id"].as_u64().unwrap_or(0),
            );
            if decisions.get(&key).is_none_or(|old| {
                rank > (
                    old["submitted_at"].as_str().unwrap_or(""),
                    old["id"].as_u64().unwrap_or(0),
                )
            }) {
                decisions.insert(key, review);
            }
        }
    }
    ReviewStatus {
        requested_reviewers: pr["requested_reviewers"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        requested_teams: pr["requested_teams"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        latest_reviews: latest.values().map(|v| (*v).clone()).collect(),
        approved_by: decisions
            .values()
            .filter(|v| v["state"] == "APPROVED")
            .filter_map(|v| v["user"]["login"].as_str().map(str::to_owned))
            .collect(),
        changes_requested_by: decisions
            .values()
            .filter(|v| v["state"] == "CHANGES_REQUESTED")
            .filter_map(|v| v["user"]["login"].as_str().map(str::to_owned))
            .collect(),
        dismissed_reviews: reviews
            .iter()
            .filter(|r| r["state"] == "DISMISSED")
            .cloned()
            .collect(),
        resolved_threads: threads.iter().filter(|t| t["isResolved"] == true).count(),
        unresolved_threads: threads.iter().filter(|t| t["isResolved"] == false).count(),
        outdated_threads: threads.iter().filter(|t| t["isOutdated"] == true).count(),
    }
}
// GitHub's filter=latest is scoped to check suites. A later workflow run
// creates a different suite, so superseded failures can still be returned.
// Preserve raw evidence, but roll up the newest check per SHA/app/name.
fn latest_checks(checks: &[Value]) -> Vec<&Value> {
    let mut latest = HashMap::<(&str, i64, &str), (u64, usize)>::new();
    let mut retained = HashSet::new();
    for (index, check) in checks.iter().enumerate() {
        let (Some(sha), Some(app), Some(name), Some(id)) = (
            check["head_sha"].as_str(),
            check["app"]["id"].as_i64(),
            check["name"].as_str(),
            check["id"].as_u64(),
        ) else {
            // Unidentified evidence cannot safely supersede another result.
            retained.insert(index);
            continue;
        };
        let entry = latest.entry((sha, app, name)).or_insert((id, index));
        if id > entry.0 {
            *entry = (id, index);
        }
    }
    retained.extend(latest.into_values().map(|(_, index)| index));
    checks
        .iter()
        .enumerate()
        .filter_map(|(index, check)| retained.contains(&index).then_some(check))
        .collect()
}

fn failed_results(
    checks: &[Value],
    statuses: &[Value],
    workflows: &[Value],
    jobs: &[Value],
) -> Vec<FailedResult> {
    let mut failures = Vec::new();
    for (kind, values) in [
        ("check", latest_checks(checks)),
        ("commit_status", statuses.iter().collect()),
        ("workflow", workflows.iter().collect()),
        ("job", jobs.iter().collect()),
    ] {
        for value in values {
            let conclusion = if kind == "commit_status" {
                value["state"].as_str()
            } else {
                value["conclusion"].as_str()
            }
            .unwrap_or("");
            if !matches!(
                conclusion,
                "failure"
                    | "error"
                    | "cancelled"
                    | "timed_out"
                    | "action_required"
                    | "stale"
                    | "startup_failure"
            ) {
                continue;
            }
            let url = value["html_url"]
                .as_str()
                .or_else(|| value["details_url"].as_str())
                .or_else(|| value["target_url"].as_str())
                .map(str::to_owned);
            let failed_steps = value["steps"].as_array().map(|steps|steps.iter().filter(|s|matches!(s["conclusion"].as_str(),Some("failure"|"cancelled"|"timed_out"))).map(|step|json!({"name":step["name"],"number":step["number"],"conclusion":step["conclusion"],"url":url})).collect()).unwrap_or_default();
            failures.push(FailedResult {
                kind: kind.into(),
                name: value["name"]
                    .as_str()
                    .or_else(|| value["context"].as_str())
                    .unwrap_or("unnamed")
                    .into(),
                conclusion: conclusion.into(),
                url,
                failed_steps,
            });
        }
    }
    failures
}

fn summarize(
    checks: &[Value],
    statuses: &[Value],
    workflows: &[Value],
    jobs: &[Value],
    incomplete: bool,
) -> CiSummary {
    let mut s = CiSummary {
        state: "unknown".into(),
        successful: 0,
        failed: 0,
        pending: 0,
        skipped: 0,
        unknown: usize::from(incomplete),
    };
    for check in latest_checks(checks)
        .into_iter()
        .chain(workflows)
        .chain(jobs)
    {
        let status = check["status"].as_str().unwrap_or("");
        if matches!(
            status,
            "queued" | "in_progress" | "requested" | "waiting" | "pending"
        ) {
            s.pending += 1;
            continue;
        }
        match check["conclusion"].as_str().unwrap_or("") {
            "success" => s.successful += 1,
            "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure"
            | "stale" => s.failed += 1,
            "skipped" | "neutral" => s.skipped += 1,
            _ => s.unknown += 1,
        }
    }
    // Combined-status endpoints expose the latest state for each context.
    for status in statuses {
        match status["state"].as_str().unwrap_or("") {
            "success" => s.successful += 1,
            "failure" | "error" => s.failed += 1,
            "pending" => s.pending += 1,
            _ => s.unknown += 1,
        }
    }
    s.state = if s.failed > 0 {
        "failure"
    } else if s.pending > 0 {
        "running"
    } else if s.unknown > 0 {
        "unknown"
    } else if s.successful > 0 {
        "success"
    } else if s.skipped > 0 {
        "skipped"
    } else {
        "unknown"
    }
    .into();
    s
}

const THREAD_QUERY: &str = "query Threads($owner:String!,$repo:String!,$number:Int!,$after:String) {
 repository(owner:$owner,name:$repo) { pullRequest(number:$number) { reviewThreads(first:100,after:$after) {
 pageInfo { hasNextPage endCursor } nodes { id isResolved isOutdated path line startLine diffSide startDiffSide
 resolvedBy { login } comments(first:100) { pageInfo { hasNextPage endCursor } nodes {
 id databaseId body createdAt updatedAt url path line originalLine pullRequestReview { databaseId } author { login __typename } replyTo { id databaseId } } } } } } } }";
const COMMENT_QUERY: &str =
    "query Comments($id:ID!,$after:String) { node(id:$id) { ... on PullRequestReviewThread {
 comments(first:100,after:$after) { pageInfo { hasNextPage endCursor } nodes {
 id databaseId body createdAt updatedAt url path line originalLine pullRequestReview { databaseId } author { login __typename } replyTo { id databaseId } } } } } }";

const REVIEW_EVENTS_QUERY: &str = "query ReviewEvents($owner:String!,$repo:String!,$number:Int!,$after:String) {
 repository(owner:$owner,name:$repo) { pullRequest(number:$number) {
 timelineItems(first:100,after:$after,itemTypes:[REVIEW_REQUESTED_EVENT,REVIEW_REQUEST_REMOVED_EVENT]) {
 pageInfo { hasNextPage endCursor } nodes {
 __typename ... on ReviewRequestedEvent { id createdAt requestedReviewer { __typename ... on User { login } ... on Team { slug name } } }
 ... on ReviewRequestRemovedEvent { id createdAt requestedReviewer { __typename ... on User { login } ... on Team { slug name } } }
 } } } } }";

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn monitored_projection_waits_for_publication_while_cached_read_stays_nonblocking() {
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let held = lock.clone().lock_owned().await;
        PUBLICATION_READ_ONLY
            .scope(Arc::default(), async {
                assert!(
                    acquire_report_lock(lock.clone(), Freshness::CachedOnly)
                        .await
                        .is_none()
                );
                assert!(
                    !can_publish(),
                    "caller cached reads must remain read-only on contention"
                );
            })
            .await;
        PRESERVE_CI_HEALTH
            .scope(
                (),
                PUBLICATION_READ_ONLY.scope(Arc::default(), async {
                    let mut pending =
                        std::pin::pin!(acquire_report_lock(lock, Freshness::CachedOnly));
                    std::future::poll_fn(|cx| {
                        let state = std::future::Future::poll(pending.as_mut(), cx);
                        assert!(state.is_pending(), "background publication must wait");
                        std::task::Poll::Ready(())
                    })
                    .await;
                    assert!(can_publish());
                    drop(held);
                    assert!(pending.await.is_some());
                    assert!(can_publish());
                }),
            )
            .await;
    }

    #[test]
    fn superseded_checks_do_not_mask_successful_reruns() {
        let old = json!({"id":1,"name":"tests","app":{"id":9},"head_sha":"aaa","status":"completed","conclusion":"failure"});
        let new = json!({"id":2,"name":"tests","app":{"id":9},"head_sha":"aaa","status":"completed","conclusion":"success"});
        let checks = vec![new.clone(), old.clone()];
        assert_eq!(summarize(&checks, &[], &[], &[], false).state, "success");
        assert!(failed_results(&checks, &[], &[], &[]).is_empty());
        let mut independent = old.clone();
        independent["app"]["id"] = json!(10);
        assert_eq!(
            summarize(
                &[old.clone(), new.clone(), independent],
                &[],
                &[],
                &[],
                false
            )
            .state,
            "failure"
        );
        let mut other_sha = old.clone();
        other_sha["head_sha"] = json!("bbb");
        assert_eq!(
            summarize(&[old.clone(), new.clone(), other_sha], &[], &[], &[], false).state,
            "failure"
        );
        let mut pending = new;
        pending["status"] = json!("in_progress");
        pending["conclusion"] = Value::Null;
        assert_eq!(
            summarize(&[old, pending], &[], &[], &[], false).state,
            "running"
        );
    }
    #[test]
    fn missing_skipped_and_incomplete_are_never_green() {
        assert_eq!(summarize(&[], &[], &[], &[], false).state, "unknown");
        let skipped = json!({"status":"completed","conclusion":"skipped"});
        assert_eq!(summarize(&[skipped], &[], &[], &[], false).state, "skipped");
        let green = json!({"status":"completed","conclusion":"success"});
        assert_eq!(
            summarize(std::slice::from_ref(&green), &[], &[], &[], true).state,
            "unknown"
        );
        assert_eq!(
            summarize(&[green], &[json!({"state":"failure"})], &[], &[], false).state,
            "failure"
        );
        assert_eq!(
            summarize(
                &[json!({"status":"in_progress","conclusion":null})],
                &[],
                &[],
                &[],
                false
            )
            .state,
            "running"
        );
    }
}
