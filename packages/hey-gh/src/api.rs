//! Loopback HTTP API and persistent PR monitoring.
use crate::{
    ChangePage, Client, Error, Freshness, Report, Result, Status, Watch, WatchKind, now_ms,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::Mutex, task::JoinHandle};

#[derive(Clone)]
pub struct Api(Arc<ApiInner>);
struct ApiInner {
    client: Client,
    monitors: Mutex<BTreeMap<String, Monitor>>,
}

#[derive(Clone)]
struct Access {
    token: Option<Arc<str>>,
}
impl Drop for ApiInner {
    fn drop(&mut self) {
        for monitor in self.monitors.get_mut().values() {
            monitor.task.abort();
        }
    }
}
struct Monitor {
    state: Arc<Mutex<WatchStatus>>,
    task: JoinHandle<()>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchStatus {
    #[serde(flatten)]
    pub watch: Watch,
    pub last_poll_at_ms: Option<u64>,
    pub last_success_at_ms: Option<u64>,
    pub last_error: Option<String>,
    pub ci_last_poll_at_ms: Option<u64>,
    pub ci_last_success_at_ms: Option<u64>,
    pub ci_last_error: Option<String>,
    /// Latest completed account detail/combined hydration cycle, not readiness.
    #[serde(default)]
    pub last_cycle: Option<crate::AccountRefreshCycle>,
    /// Latest completed account CI hydration cycle, independent of details.
    #[serde(default)]
    pub ci_last_cycle: Option<crate::AccountRefreshCycle>,
    /// Account discovery runs independently of source hydration.
    pub discovery_last_poll_at_ms: Option<u64>,
    pub discovery_last_success_at_ms: Option<u64>,
    pub discovery_last_error: Option<String>,
    /// An equal-or-faster account watch owns upstream polling for this PR.
    pub covered_by_account: bool,
}

impl Api {
    pub async fn new(client: Client) -> Result<Self> {
        let api = Self(Arc::new(ApiInner {
            client,
            monitors: Mutex::new(BTreeMap::new()),
        }));
        for watch in api.0.client.watches().await? {
            api.start_monitor(watch).await?;
        }
        Ok(api)
    }

    pub fn router(&self) -> Router {
        self.router_with_auth(None)
    }

    /// The executable always supplies an automatically registered local token.
    /// Embedded, synthetic servers can opt out via router().
    pub fn router_with_auth(&self, token: Option<String>) -> Router {
        Router::new()
            .route("/v1/status", get(status))
            .route("/v1/pr-status", get(pr_status))
            .route("/v1/prs/{owner}/{repo}", get(my_prs))
            .route("/v1/prs/{owner}/{repo}/{number}", get(pr))
            .route("/v1/prs/{owner}/{repo}/{number}/ci", get(ci))
            .route(
                "/v1/prs/{owner}/{repo}/{number}/required-checks",
                get(required_checks),
            )
            .route("/v1/repos/{owner}/{repo}", get(repository))
            .route("/v1/repos/{owner}/{repo}/prs", get(repository_prs))
            .route("/v1/changes", get(changes))
            .route("/v1/snapshot", get(snapshot))
            .route("/v1/watches", get(watches).post(add_watch))
            .route("/v1/watches/{id}", delete(remove_watch))
            .layer(middleware::from_fn_with_state(
                Access {
                    token: token.map(Arc::from),
                },
                local_requests,
            ))
            .with_state(self.clone())
    }

    async fn start_monitor(&self, watch: Watch) -> Result<()> {
        let discovery = if watch.kind == WatchKind::Account {
            self.0.client.discovery_health().await?
        } else {
            None
        };
        let mut monitors = self.0.monitors.lock().await;
        if let Some(old) = monitors.remove(&watch.id) {
            old.task.abort();
        }
        let state = Arc::new(Mutex::new(WatchStatus {
            watch: watch.clone(),
            last_poll_at_ms: None,
            last_success_at_ms: None,
            last_error: None,
            ci_last_poll_at_ms: None,
            ci_last_success_at_ms: None,
            ci_last_error: None,
            last_cycle: None,
            ci_last_cycle: None,
            discovery_last_poll_at_ms: discovery.as_ref().and_then(|health| health.last_poll_at_ms),
            discovery_last_success_at_ms: discovery
                .as_ref()
                .and_then(|health| health.last_success_at_ms),
            discovery_last_error: discovery.and_then(|health| health.last_error),
            covered_by_account: false,
        }));
        let (client, task_state, task_watch) =
            (self.0.client.clone(), state.clone(), watch.clone());
        let task = tokio::spawn(async move {
            if task_watch.kind == WatchKind::Branches {
                monitor_loop(client, task_watch, task_state, false).await;
                return;
            }
            if task_watch.kind == WatchKind::Account {
                tokio::join!(biased;
                    account_discovery_loop(client.clone(), task_watch.clone(), task_state.clone()),
                    monitor_loop(client.clone(),task_watch.clone(),task_state.clone(),true),
                    monitor_loop(client,task_watch,task_state,false)
                );
                return;
            }
            tokio::join!(biased;
                monitor_loop(client.clone(),task_watch.clone(),task_state.clone(),true),
                monitor_loop(client,task_watch,task_state,false)
            );
        });
        monitors.insert(watch.id.clone(), Monitor { state, task });
        Ok(())
    }

    pub async fn stop(&self) {
        let mut monitors = self.0.monitors.lock().await;
        for (_, monitor) in std::mem::take(&mut *monitors) {
            monitor.task.abort();
        }
    }

    pub async fn watch_repository(
        &self,
        repository: &str,
        branches: Vec<String>,
        all_branches: bool,
        interval_seconds: u64,
    ) -> Result<Watch> {
        let watch = self
            .0
            .client
            .save_repository_watch(repository, branches, all_branches, interval_seconds)
            .await?;
        self.start_monitor(watch.clone()).await?;
        Ok(watch)
    }

    pub async fn watch(
        &self,
        repository: &str,
        number: u64,
        interval_seconds: u64,
    ) -> Result<Watch> {
        let watch = self
            .0
            .client
            .save_watch(repository, number, interval_seconds)
            .await?;
        self.start_monitor(watch.clone()).await?;
        Ok(watch)
    }

    pub async fn watch_account(&self, interval_seconds: u64) -> Result<Watch> {
        let watch = self.0.client.save_account_watch(interval_seconds).await?;
        self.start_monitor(watch.clone()).await?;
        Ok(watch)
    }

    async fn ensure_account_watch(&self) -> Result<()> {
        let id = crate::digest("account-open-prs");
        if !self.0.monitors.lock().await.contains_key(&id) {
            self.watch_account(60).await?;
        }
        Ok(())
    }
}

async fn account_discovery_loop(client: Client, watch: Watch, state: Arc<Mutex<WatchStatus>>) {
    let mut interval = tokio::time::interval(Duration::from_secs(watch.interval_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        state.lock().await.discovery_last_poll_at_ms = Some(now_ms());
        let result = client
            .all_my_open_pull_requests(Freshness::MaxAge(Duration::from_secs(30)))
            .await;
        let health = client.discovery_health().await;
        {
            let mut status = state.lock().await;
            match health {
                Ok(Some(health)) => {
                    status.discovery_last_success_at_ms = health.last_success_at_ms;
                    status.discovery_last_error = result
                        .as_ref()
                        .err()
                        .map(ToString::to_string)
                        .or(health.last_error);
                }
                Ok(None) => {
                    status.discovery_last_error = result.as_ref().err().map(ToString::to_string);
                    if result.is_ok() {
                        status.discovery_last_success_at_ms = Some(now_ms());
                    }
                }
                Err(error) => status.discovery_last_error = Some(error.to_string()),
            }
        }
        if let Err(error) = result {
            tracing::warn!(
                error_code = error.diagnostic_code(),
                "background account discovery incomplete; retaining last good collection"
            );
            if matches!(
                error,
                Error::GraphQL {
                    access_denied: true,
                    ..
                }
            ) {
                match client
                    .recover_accessible_account_prs(Duration::from_secs(30), false)
                    .await
                {
                    Ok(added) if added > 0 => {
                        if let Err(error) = client.prepare_pr_status(Freshness::CachedOnly).await {
                            tracing::warn!(
                                error_code = error.diagnostic_code(),
                                "REST discovery projection incomplete; retaining known feed rows"
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        error_code = error.diagnostic_code(),
                        "REST account discovery unavailable; retaining known feed rows"
                    ),
                }
            }
        } else {
            // Publish newly discovered pending rows immediately. Waiting for
            // serial detail hydration can otherwise hide new PRs for hours.
            // The completed collection is reused; this does not hydrate CI or
            // comments and preserves independent source failures.
            match client
                .prepare_pr_status(Freshness::MaxAge(Duration::from_secs(30)))
                .await
            {
                Ok(errors) if errors.is_empty() => {}
                Ok(_) => tracing::warn!(
                    "account discovery projection incomplete; inspect PR source errors"
                ),
                Err(error) => tracing::warn!(
                    error_code = error.diagnostic_code(),
                    "account discovery projection failed; retaining prior feed rows"
                ),
            }
        }
    }
}

async fn monitor_loop(client: Client, watch: Watch, state: Arc<Mutex<WatchStatus>>, ci_only: bool) {
    let mut interval = tokio::time::interval(Duration::from_secs(watch.interval_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut tracked = HashSet::new();
    loop {
        interval.tick().await;
        let coverage = account_watch_resource(&client, &watch)
            .await
            .unwrap_or(None);
        let covered = coverage.is_some();
        {
            let mut status = state.lock().await;
            if covered != status.covered_by_account {
                tracing::info!(watch_id=%watch.id,covered_by_account=covered,"watch polling ownership changed");
                status.covered_by_account = covered;
            }
            if ci_only {
                status.ci_last_poll_at_ms = Some(now_ms());
            } else {
                status.last_poll_at_ms = Some(now_ms());
            }
        }
        let result = if let Some(resource) = coverage {
            covered_watch_status(&client, &watch, &resource, ci_only).await
        } else {
            poll_watch(&client, &watch, &mut tracked, ci_only).await
        };
        let mut status = state.lock().await;
        let error = result.as_ref().err().map(ToString::to_string);
        if error.is_some() {
            tracing::warn!(watch_id=%watch.id,ci_only,error_code=result.as_ref().err().map(Error::diagnostic_code),"PR monitor refresh incomplete; inspect watches for source details");
        }
        if ci_only {
            status.ci_last_error = error;
            if result.is_ok() {
                status.ci_last_success_at_ms = Some(now_ms());
            }
        } else {
            status.last_error = error;
            if result.is_ok() {
                status.last_success_at_ms = Some(now_ms());
            }
        }
    }
}

async fn account_watch_resource(client: &Client, watch: &Watch) -> Result<Option<String>> {
    if watch.kind != WatchKind::PullRequests || watch.pull_number == 0 {
        return Ok(None);
    }
    if !client.watches().await?.iter().any(|account| {
        account.kind == WatchKind::Account && account.interval_seconds <= watch.interval_seconds
    }) {
        return Ok(None);
    }
    Ok(client
        .stored_snapshot(&client.roster_resource())
        .await?
        .and_then(|roster| roster.as_array().cloned())
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|node| {
                    node["number"] == watch.pull_number
                        && node["repository"]["nameWithOwner"]
                            .as_str()
                            .is_some_and(|repo| repo.eq_ignore_ascii_case(&watch.repository))
                })
                .map(|node| {
                    format!(
                        "pr-status://{}/{}/{}",
                        client.hostname(),
                        node["repository"]["nameWithOwner"]
                            .as_str()
                            .expect("matched repository"),
                        watch.pull_number
                    )
                })
        }))
}

async fn covered_watch_status(
    client: &Client,
    watch: &Watch,
    resource: &str,
    ci_only: bool,
) -> Result<()> {
    let snapshot = client
        .stored_snapshot(resource)
        .await?
        .unwrap_or(Value::Null);
    let row = &snapshot["pullRequest"];
    let available = if ci_only {
        !row["ci"].is_null() && row["sourceErrors"]["ci"].is_null()
    } else {
        row["complete"] == true
    };
    let validated_at = client
        .account_pr_validated_at(&watch.repository, watch.pull_number, ci_only)
        .await?;
    let max_age_ms = watch
        .interval_seconds
        .saturating_mul(2)
        .max(60)
        .saturating_mul(1000);
    if available && validated_at.is_some_and(|at| now_ms().saturating_sub(at) <= max_age_ms) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "account monitor {} evidence is stale or pending (last validation: {}); inspect the account watch backlog and PR source errors",
            if ci_only { "CI/lifecycle" } else { "detail" },
            validated_at.map_or_else(|| "unknown".into(), |at| at.to_string())
        )))
    }
}

async fn poll_watch(
    client: &Client,
    watch: &Watch,
    tracked: &mut HashSet<u64>,
    ci_only: bool,
) -> Result<()> {
    if watch.kind == WatchKind::Account {
        let freshness = Freshness::MaxAge(Duration::from_secs(30));
        let errors = if ci_only {
            client.hydrate_pr_status_ci(freshness).await?
        } else {
            client.hydrate_pr_status_details(freshness).await?
        };
        return if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(errors.join("; ")))
        };
    }
    if watch.kind == WatchKind::Branches {
        let report = client
            .repository_report(
                &watch.repository,
                &watch.branches,
                watch.all_branches,
                Freshness::Revalidate,
            )
            .await?;
        let pending_key = format!("branch-ci-pending://{}", watch.id);
        let mut pending: std::collections::BTreeSet<String> =
            match client.derived(&pending_key).await? {
                Some(response) => response.decode()?,
                None => Default::default(),
            };
        pending.extend(report.changed_branches);
        client.save_derived(&pending_key, json!(pending)).await?;
        client
            .refresh_affected_prs(&watch.repository, &pending.into_iter().collect::<Vec<_>>())
            .await?;
        client.save_derived(&pending_key, json!([])).await?;
        if report.errors.is_empty() {
            return Ok(());
        }
        return Err(Error::Invalid(
            report
                .errors
                .iter()
                .map(|e| format!("{}: {}", e.source, e.message))
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    if watch.pull_number != 0 {
        return refresh_pr(client, &watch.repository, watch.pull_number, ci_only).await;
    }
    let resource = format!("prs://{}/{}/mine", client.hostname(), watch.repository);
    let kind = if ci_only { "ci" } else { "full" };
    if tracked.is_empty() {
        tracked.extend(client.tracking(&watch.id, kind).await?);
    }
    let policy = if ci_only {
        Freshness::Revalidate
    } else {
        Freshness::MaxAge(Duration::from_secs(5))
    };
    let pulls = client.my_pull_requests(&watch.repository, policy).await?;
    let current: HashSet<u64> = pulls.iter().filter_map(|p| p["number"].as_u64()).collect();
    let all: HashSet<u64> = tracked.union(&current).copied().collect();
    let mut failed = Vec::new();
    let mut retry_closed = HashSet::new();
    for number in all {
        match refresh_pr(client, &watch.repository, number, ci_only).await {
            Ok(()) => {}
            Err(e) => {
                failed.push(format!("PR {number}: {e}"));
                retry_closed.insert(number);
            }
        }
    }
    *tracked = current.union(&retry_closed).copied().collect();
    let numbers: Vec<_> = tracked.iter().copied().collect();
    client.save_tracking(&watch.id, kind, &numbers).await?;
    if !ci_only {
        client
            .observe(&resource, &json!({"pull_requests":pulls}))
            .await?;
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid(failed.join("; ")))
    }
}

async fn refresh_pr(client: &Client, repository: &str, number: u64, ci_only: bool) -> Result<()> {
    if ci_only {
        let report = client
            .ci_for_pr(
                repository,
                number,
                Freshness::MaxAge(Duration::from_secs(30)),
            )
            .await?;
        if report.complete {
            match crate::client::BACKGROUND_READ
                .scope(
                    (),
                    client.required_checks_for_pr(
                        repository,
                        number,
                        Freshness::MaxAge(Duration::from_secs(30)),
                    ),
                )
                .await
            {
                Ok(policy) if policy.errors.is_empty() => {}
                Ok(_) => tracing::warn!(
                    repository,
                    number,
                    "required-check policy unavailable; observed CI remains independent"
                ),
                Err(error) => {
                    tracing::warn!(
                        repository,
                        number,
                        error_code = error.diagnostic_code(),
                        "required-check policy refresh failed; observed CI remains independent"
                    )
                }
            }
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "incomplete CI report: {}",
                report
                    .data
                    .errors
                    .iter()
                    .map(|e| format!("{}: {}", e.source, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            )))
        }
    } else {
        let errors = client
            .refresh_monitored_pr_details(
                repository,
                number,
                Freshness::MaxAge(Duration::from_secs(30)),
            )
            .await?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "incomplete PR details: {}",
                errors
                    .iter()
                    .map(|e| format!("{}: {}", e.source, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            )))
        }
    }
}

#[derive(Default, Deserialize)]
struct ReadQuery {
    refresh: Option<bool>,
    cached_only: Option<bool>,
    max_age_seconds: Option<u64>,
}
impl ReadQuery {
    fn freshness(&self) -> Result<Freshness> {
        if self.refresh == Some(true) && self.cached_only == Some(true) {
            return Err(Error::Invalid(
                "refresh and cached_only are mutually exclusive".into(),
            ));
        }
        if self.cached_only == Some(true) {
            return Ok(Freshness::CachedOnly);
        }
        if self.refresh == Some(true) {
            return Ok(Freshness::Revalidate);
        }
        let age = self.max_age_seconds.unwrap_or(30);
        if age > 86400 {
            return Err(Error::Invalid(
                "max_age_seconds must be at most 86400".into(),
            ));
        }
        Ok(Freshness::MaxAge(Duration::from_secs(age)))
    }
}

async fn pr(
    State(api): State<Api>,
    Path((owner, repo, number)): Path<(String, String, u64)>,
    Query(query): Query<ReadQuery>,
) -> ApiResult<Json<Report>> {
    Ok(Json(
        crate::client::INTERACTIVE_READ
            .scope(
                crate::client::foreground_priority(),
                api.0
                    .client
                    .pr_report(&format!("{owner}/{repo}"), number, query.freshness()?),
            )
            .await?,
    ))
}
async fn ci(
    State(api): State<Api>,
    Path((owner, repo, number)): Path<(String, String, u64)>,
    Query(query): Query<ReadQuery>,
) -> ApiResult<Json<crate::CiObservation>> {
    Ok(Json(
        crate::client::INTERACTIVE_READ
            .scope(
                crate::client::foreground_priority(),
                api.0
                    .client
                    .ci_for_pr(&format!("{owner}/{repo}"), number, query.freshness()?),
            )
            .await?,
    ))
}
#[derive(Default, Deserialize)]
struct RepositoryQuery {
    refresh: Option<bool>,
    cached_only: Option<bool>,
    max_age_seconds: Option<u64>,
    #[serde(default)]
    branches: String,
    #[serde(default)]
    all_branches: bool,
}
async fn repository(
    State(api): State<Api>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<RepositoryQuery>,
) -> ApiResult<Json<crate::RepositoryReport>> {
    let branches = if query.branches.is_empty() {
        Vec::new()
    } else {
        if query.branches.starts_with('[') {
            serde_json::from_str(&query.branches)
                .map_err(|_| Error::Invalid("branches must be a JSON string array".into()))?
        } else {
            query.branches.split(',').map(str::to_owned).collect()
        }
    };
    Ok(Json(
        api.0
            .client
            .repository_report(
                &format!("{owner}/{repo}"),
                &branches,
                query.all_branches,
                ReadQuery {
                    refresh: query.refresh,
                    cached_only: query.cached_only,
                    max_age_seconds: query.max_age_seconds,
                }
                .freshness()?,
            )
            .await?,
    ))
}
async fn required_checks(
    State(api): State<Api>,
    Path((owner, repo, number)): Path<(String, String, u64)>,
    Query(query): Query<ReadQuery>,
) -> ApiResult<Json<crate::RequiredChecksReport>> {
    Ok(Json(
        api.0
            .client
            .required_checks_for_pr(&format!("{owner}/{repo}"), number, query.freshness()?)
            .await?,
    ))
}
#[derive(Deserialize)]
struct PrListQuery {
    state: Option<String>,
    refresh: Option<bool>,
    cached_only: Option<bool>,
    max_age_seconds: Option<u64>,
}
async fn repository_prs(
    State(api): State<Api>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<PrListQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let freshness = ReadQuery {
        refresh: query.refresh,
        cached_only: query.cached_only,
        max_age_seconds: query.max_age_seconds,
    }
    .freshness()?;
    Ok(Json(
        api.0
            .client
            .list_pull_requests(
                &format!("{owner}/{repo}"),
                query.state.as_deref().unwrap_or("open"),
                freshness,
            )
            .await?,
    ))
}
async fn my_prs(
    State(api): State<Api>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<ReadQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        api.0
            .client
            .my_pull_requests(&format!("{owner}/{repo}"), query.freshness()?)
            .await?,
    ))
}
async fn snapshot(State(api): State<Api>) -> ApiResult<Json<crate::SnapshotPage>> {
    Ok(Json(api.0.client.bootstrap().await?))
}

#[derive(Deserialize)]
struct PrStatusQuery {
    repository: Option<String>,
    fields: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
    wait_seconds: Option<u64>,
    refresh: Option<bool>,
    cached_only: Option<bool>,
    max_age_seconds: Option<u64>,
}
async fn pr_status(
    State(api): State<Api>,
    Query(query): Query<PrStatusQuery>,
) -> ApiResult<Json<crate::PrStatusPage>> {
    // Validate selection/cursor before any upstream work or watch registration.
    let fields = query
        .fields
        .as_deref()
        .map(|fields| fields.split(',').collect::<Vec<_>>());
    if let Some(fields) = &fields {
        crate::pr_fields::validate(fields)?;
    }
    let limit = query.limit.unwrap_or(1000);
    if !(1..=1000).contains(&limit) {
        return Err(Error::Invalid("limit must be 1..1000".into()).into());
    }
    let wait = Duration::from_secs(query.wait_seconds.unwrap_or(0));
    if wait > Duration::from_secs(30) {
        return Err(Error::Invalid("wait must be at most 30 seconds".into()).into());
    }
    let freshness = ReadQuery {
        refresh: query.refresh,
        cached_only: query.cached_only,
        max_age_seconds: query.max_age_seconds,
    }
    .freshness()?;
    if let Some(cursor) = &query.cursor {
        // Validate cursor scope/expiry before registering or refreshing. A
        // bootstrap has no cursor to validate and must not decode its entire
        // snapshot twice merely for this preflight.
        api.0
            .client
            .validate_pr_status_cursor(query.repository.as_deref(), cursor)
            .await?;
    } else if let Some(repository) = &query.repository {
        crate::client::validate_repository(repository)?;
    }
    let errors = if matches!(freshness, Freshness::CachedOnly) {
        if api
            .0
            .client
            .stored_snapshot(&api.0.client.roster_resource())
            .await?
            .is_none()
        {
            return Err(Error::CacheMiss.into());
        }
        vec![]
    } else {
        let errors = if query.refresh == Some(true) {
            api.0.client.refresh_pr_status(freshness, false).await?
        } else if query.cursor.is_some() {
            // Normal incremental reads consume observed changes. The durable
            // watch owns upstream polling, independent of consumer frequency.
            vec![]
        } else {
            api.0.client.prepare_pr_status(freshness).await?
        };
        api.ensure_account_watch().await?;
        errors
    };
    let mut page = api
        .0
        .client
        .pr_status_page_projected(
            query.repository.as_deref(),
            query.cursor.as_deref(),
            limit,
            wait,
            fields.as_deref(),
        )
        .await?;
    // Feed reads consume observations, but a known discovery failure still
    // means new/terminal PRs may be missing. Expose it in the envelope without
    // inventing PR activity or advancing the observation cursor.
    let discovery_state = api
        .0
        .monitors
        .lock()
        .await
        .get(&crate::digest("account-open-prs"))
        .map(|monitor| Arc::clone(&monitor.state));
    if let Some(state) = discovery_state
        && let Some(error) = &state.lock().await.discovery_last_error
    {
        page.record_discovery_error(error);
    }
    page.complete &= errors.is_empty()
        && page
            .changes
            .iter()
            .all(|c| c.pull_request["complete"] == true);
    for error in errors {
        if !page.errors.contains(&error) {
            page.errors.push(error);
        }
    }
    if let Some(fields) = fields {
        for row in page.pull_requests.iter_mut().chain(
            page.changes
                .iter_mut()
                .map(|change| &mut change.pull_request),
        ) {
            crate::pr_fields::project(row, &fields);
        }
    }
    Ok(Json(page))
}

async fn status(State(api): State<Api>) -> Json<Status> {
    Json(api.0.client.status())
}

#[derive(Deserialize)]
struct ChangesQuery {
    cursor: Option<String>,
    limit: Option<usize>,
    wait_seconds: Option<u64>,
}
async fn changes(
    State(api): State<Api>,
    Query(query): Query<ChangesQuery>,
) -> ApiResult<Json<ChangePage>> {
    Ok(Json(
        api.0
            .client
            .wait_changes(
                query.cursor.as_deref(),
                query.limit.unwrap_or(100),
                Duration::from_secs(query.wait_seconds.unwrap_or(0)),
            )
            .await?,
    ))
}

#[derive(Deserialize)]
struct WatchInput {
    #[serde(default)]
    kind: WatchKind,
    #[serde(default)]
    branches: Vec<String>,
    #[serde(default)]
    all_branches: bool,
    #[serde(default)]
    repository: String,
    pull_number: Option<u64>,
    interval_seconds: Option<u64>,
}
async fn add_watch(
    State(api): State<Api>,
    Json(input): Json<WatchInput>,
) -> ApiResult<Json<Watch>> {
    if input.kind == WatchKind::Account {
        if !input.repository.is_empty()
            || input.pull_number.is_some_and(|n| n != 0)
            || !input.branches.is_empty()
            || input.all_branches
        {
            return Err(Error::Invalid(
                "account watches cannot specify repository/PR/branch options".into(),
            )
            .into());
        }
        return Ok(Json(
            api.watch_account(input.interval_seconds.unwrap_or(60))
                .await?,
        ));
    }
    if input.kind == WatchKind::Branches {
        if input.pull_number.is_some_and(|n| n != 0) {
            return Err(Error::Invalid("branch watches cannot specify a PR number".into()).into());
        }
        return Ok(Json(
            api.watch_repository(
                &input.repository,
                input.branches,
                input.all_branches,
                input.interval_seconds.unwrap_or(60),
            )
            .await?,
        ));
    }
    if !input.branches.is_empty() || input.all_branches {
        return Err(Error::Invalid("branch options require kind=branches".into()).into());
    }
    Ok(Json(
        api.watch(
            &input.repository,
            input.pull_number.unwrap_or(0),
            input.interval_seconds.unwrap_or(60),
        )
        .await?,
    ))
}
async fn watches(State(api): State<Api>) -> ApiResult<Json<Vec<WatchStatus>>> {
    let mut statuses = Vec::new();
    {
        let monitors = api.0.monitors.lock().await;
        for monitor in monitors.values() {
            statuses.push(monitor.state.lock().await.clone());
        }
    }
    if statuses
        .iter()
        .any(|status| status.watch.kind == WatchKind::Account)
    {
        let health = api.0.client.discovery_health().await?;
        let last_cycle = api.0.client.account_refresh_cycle(false).await?;
        let ci_last_cycle = api.0.client.account_refresh_cycle(true).await?;
        for status in statuses
            .iter_mut()
            .filter(|status| status.watch.kind == WatchKind::Account)
        {
            status.last_cycle = last_cycle.clone();
            status.ci_last_cycle = ci_last_cycle.clone();
            let Some(health) = &health else { continue };
            status.discovery_last_success_at_ms = health.last_success_at_ms;
            if health.last_error.is_some()
                || health
                    .last_success_at_ms
                    .zip(status.discovery_last_poll_at_ms)
                    .is_some_and(|(success, poll)| success >= poll)
            {
                status.discovery_last_error = health.last_error.clone();
            }
        }
    }
    Ok(Json(statuses))
}
async fn remove_watch(State(api): State<Api>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let mut monitors = api.0.monitors.lock().await;
    api.0.client.delete_watch(&id).await?;
    if let Some(monitor) = monitors.remove(&id) {
        monitor.task.abort();
    }
    Ok(StatusCode::NO_CONTENT)
}

// Reject browser origins and non-loopback Host headers (including DNS rebinding).
// The daemon accepts loopback bindings only; GitHub credentials never cross it.
async fn local_requests(
    State(access): State<Access>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let local_host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|host| url::Url::parse(&format!("http://{host}/")).ok())
        .is_some_and(|u| matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")));
    if request.headers().contains_key(header::ORIGIN)
        || request.headers().contains_key("sec-fetch-site")
        || !local_host
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"only local, non-browser API requests are supported"})),
        )
            .into_response();
    }
    if let Some(expected) = access.token {
        use subtle::ConstantTimeEq;
        let supplied = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .unwrap_or("");
        if !bool::from(expected.as_bytes().ct_eq(supplied.as_bytes())) {
            return (StatusCode::UNAUTHORIZED,Json(json!({"error":"local API authentication required; use hey-gh or ApiClient","code":"local_auth"}))).into_response();
        }
    }
    next.run(request).await
}

type ApiResult<T> = std::result::Result<T, ApiError>;
struct ApiError(Error);
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, retry) = match &self.0 {
            Error::Invalid(_) => (StatusCode::BAD_REQUEST, None),
            Error::LocalAuth(_) => (StatusCode::UNAUTHORIZED, None),
            Error::CursorExpired => (StatusCode::GONE, None),
            Error::CacheMiss => (StatusCode::NOT_FOUND, None),
            Error::QueueFull => (StatusCode::SERVICE_UNAVAILABLE, Some(1)),
            Error::RateLimited {
                retry_after_seconds,
            } => (StatusCode::SERVICE_UNAVAILABLE, Some(*retry_after_seconds)),
            Error::Deadline => (StatusCode::GATEWAY_TIMEOUT, None),
            Error::GitHub { status, .. } if *status == 404 => (StatusCode::NOT_FOUND, None),
            Error::GitHub { .. } | Error::GraphQL { .. } | Error::Transport(_) => {
                (StatusCode::BAD_GATEWAY, None)
            }
            _ => (StatusCode::INTERNAL_SERVER_ERROR, None),
        };
        let code = match &self.0 {
            Error::CursorExpired => "cursor_expired",
            Error::QueueFull => "queue_full",
            Error::RateLimited { .. } => "rate_limited",
            Error::Deadline => "deadline",
            Error::CacheMiss => "cache_miss",
            Error::Invalid(_) => "invalid",
            Error::LocalAuth(_) => "local_auth",
            Error::Auth(_) => "auth",
            Error::Storage(_) => "storage",
            Error::Stopped => "stopped",
            Error::Transport(_) => "transport",
            Error::GraphQL {
                access_denied: true,
                ..
            } => "graphql_access_denied",
            Error::GraphQL { .. } => "graphql",
            _ => "upstream",
        };
        let mut envelope = json!({"error":self.0.to_string(),"code":code});
        match &self.0 {
            Error::Storage(cause) | Error::Transport(cause) => {
                envelope["cause"] = json!(cause);
            }
            Error::Auth(hostname) => {
                envelope["auth_hostname"] = json!(hostname);
            }
            _ => {}
        }
        if let Error::GitHub { status, message } = &self.0 {
            // The gateway status describes the local API result. Preserve the
            // actual upstream failure independently for SDK diagnostics.
            envelope["upstream_status"] = json!(status);
            envelope["upstream_message"] = json!(message);
        }
        if matches!(self.0, Error::CacheMiss) {
            envelope["complete"] = json!(false);
            envelope["available"] = json!(false);
            envelope["oldest_validation_at_ms"] = Value::Null;
            envelope["validations"] = json!([]);
        }
        let mut response = (status, Json(envelope)).into_response();
        if let Some(retry) = retry {
            response.headers_mut().insert(
                header::RETRY_AFTER,
                retry.to_string().parse().expect("numeric header"),
            );
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn covered_watches_require_recent_lane_validation_not_cached_completeness() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            crate::Config {
                cache_path: dir.path().join("cache.sqlite"),
                ..crate::Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let watch = client.save_watch("ACME/DEMO", 7, 60).await.unwrap();
        let resource = "pr-status://github.com/acme/demo/7";
        client
            .observe(
                resource,
                &json!({"pullRequest":{"complete":true,"ci":{},"sourceErrors":{}}}),
            )
            .await
            .unwrap();
        let before = client.bootstrap().await.unwrap().cursor;
        for ci_only in [true, false] {
            let mode = if ci_only { "ci" } else { "details" };
            let key = format!("account-status-validated:{mode}:acme/demo/7");
            // Unknown legacy evidence and old successful hydration are stale.
            assert!(
                covered_watch_status(&client, &watch, resource, ci_only)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("stale or pending")
            );
            client
                .save_derived(&key, json!(now_ms() - 180_000))
                .await
                .unwrap();
            assert!(
                covered_watch_status(&client, &watch, resource, ci_only)
                    .await
                    .is_err()
            );
            client.save_derived(&key, json!(now_ms())).await.unwrap();
            assert!(
                covered_watch_status(&client, &watch, resource, ci_only)
                    .await
                    .is_ok()
            );
        }
        // Read-only diagnostics must not create evidence or move feed cursors.
        assert_eq!(client.bootstrap().await.unwrap().cursor, before);
        assert_eq!(client.status().network_requests, 0);
    }

    #[tokio::test]
    async fn daemon_error_categories_and_causes_survive_the_sdk_boundary() {
        let source = Arc::new(Mutex::new(Error::Invalid("invalid selector".into())));
        let handler_source = source.clone();
        let router = Router::new().route(
            "/v1/status",
            get(move || {
                let source = handler_source.clone();
                async move { ApiError(source.lock().await.clone()) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sdk = crate::ApiClient::new(
            format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
        )
        .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        for error in [
            Error::Invalid("invalid selector".into()),
            Error::Auth("github.example.com".into()),
            Error::LocalAuth("local credential rejected".into()),
            Error::Storage("SQLite unavailable".into()),
            Error::Transport("operation timed out".into()),
            Error::GitHub {
                status: 503,
                message: "upstream unavailable".into(),
            },
            Error::GraphQL {
                message: "operation denied".into(),
                access_denied: true,
            },
            Error::QueueFull,
            Error::Deadline,
            Error::RateLimited {
                retry_after_seconds: 7,
            },
            Error::CursorExpired,
            Error::CacheMiss,
            Error::Stopped,
        ] {
            *source.lock().await = error.clone();
            let remote = sdk.status().await.unwrap_err();
            assert_eq!(
                std::mem::discriminant(&remote),
                std::mem::discriminant(&error),
                "{error}"
            );
            assert_eq!(remote.to_string(), error.to_string());
        }
        server.abort();
    }

    #[tokio::test]
    async fn graphql_operation_errors_retain_their_codes_without_inventing_an_http_failure() {
        for access_denied in [false, true] {
            let message = "GitHub GraphQL returned errors: operation diagnostic";
            let response = ApiError(Error::GraphQL {
                message: message.into(),
                access_denied,
            })
            .into_response();
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            assert!(response.headers().get(header::RETRY_AFTER).is_none());
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error"], message);
            assert!(body["upstream_status"].is_null());
            assert_eq!(
                body["code"],
                if access_denied {
                    "graphql_access_denied"
                } else {
                    "graphql"
                }
            );
        }
    }
}
