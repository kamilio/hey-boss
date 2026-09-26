use crate::{
    ChangePage, Error, Response, Result, Source, Watch, digest, now_ms,
    scheduler::{Inflight, Job, Metrics, Scheduler, Status},
    store::Store,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, watch};
use tracing::instrument::WithSubscriber;
use url::Url;

// Background per-PR budgets also bound newly scheduled work. Otherwise an
// abandoned socket can occupy its lane long after hydration has moved on.
tokio::task_local! { pub(crate) static REQUEST_DEADLINE: Option<tokio::time::Instant>; }
tokio::task_local! { pub(crate) static INTERACTIVE_READ: Arc<AtomicBool>; }
tokio::task_local! { pub(crate) static BACKGROUND_READ: (); }

pub(crate) fn interactive_read() -> bool {
    INTERACTIVE_READ
        .try_with(|priority| priority.load(Ordering::Relaxed))
        .unwrap_or(false)
}

pub(crate) fn foreground_priority() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(true))
}

#[derive(Clone)]
pub struct Config {
    pub gh_program: PathBuf,
    pub hostname: String,
    pub rest_url: Url,
    pub graphql_url: Url,
    pub cache_path: PathBuf,
    pub api_version: String,
    /// Capacity includes active, waiting, and retrying distinct requests.
    pub queue_capacity: usize,
    pub queue_timeout: Duration,
    pub request_timeout: Duration,
    pub report_timeout: Duration,
    pub min_spacing: Duration,
    pub max_attempts: u32,
    pub max_body_bytes: usize,
    pub max_collection_bytes: usize,
    pub change_retention: Duration,
    pub max_change_events: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gh_program: PathBuf::from("gh"),
            hostname: "github.com".into(),
            rest_url: Url::parse("https://api.github.com/").expect("static URL"),
            graphql_url: Url::parse("https://api.github.com/graphql").expect("static URL"),
            cache_path: dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("hey-gh/cache.sqlite"),
            api_version: "2022-11-28".into(),
            queue_capacity: 256,
            queue_timeout: Duration::from_secs(300),
            request_timeout: Duration::from_secs(30),
            report_timeout: Duration::from_secs(120),
            min_spacing: Duration::from_millis(100),
            max_attempts: 3,
            max_body_bytes: 16 * 1024 * 1024,
            max_collection_bytes: 64 * 1024 * 1024,
            change_retention: Duration::from_secs(7 * 86400),
            max_change_events: 10000,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Freshness {
    MaxAge(Duration),
    Revalidate,
    CachedOnly,
}
impl Default for Freshness {
    fn default() -> Self {
        Self::MaxAge(Duration::from_secs(30))
    }
}

#[derive(Clone)]
pub struct Client(Arc<Inner>);
struct Inner {
    config: Config,
    scope: String,
    store: Store,
    queue: mpsc::Sender<Job>,
    permits: Arc<Semaphore>,
    inflight: Inflight,
    read_priorities: Mutex<HashMap<String, Weak<AtomicBool>>>,
    report_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    metrics: Arc<Metrics>,
    changes_notify: tokio::sync::Notify,
}

impl Client {
    // Leave room for one interactive CI batch without expanding total work.
    // Tiny embedded queues keep at least three quarters for ordinary reads.
    fn interactive_reserved_slots(&self) -> usize {
        3.min(self.0.config.queue_capacity / 4)
    }

    fn queue_full(&self) -> Error {
        self.0.metrics.queue_full.fetch_add(1, Ordering::Relaxed);
        Error::QueueFull
    }

    /// Obtain the effective token through gh, without printing or storing it.
    /// gh selects its existing login and respects its token environment overrides.
    pub async fn from_gh(config: Config) -> Result<Self> {
        let output = tokio::time::timeout(
            config.request_timeout,
            tokio::process::Command::new(&config.gh_program)
                .args(["auth", "token", "--hostname", &config.hostname])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| Error::Auth(config.hostname.clone()))?
        .map_err(|_| Error::Auth(config.hostname.clone()))?;
        if !output.status.success() {
            return Err(Error::Auth(config.hostname.clone()));
        }
        let token =
            String::from_utf8(output.stdout).map_err(|_| Error::Auth(config.hostname.clone()))?;
        Self::with_token(config, token.trim().to_owned())
    }

    /// Explicit credentials are useful for GitHub Apps and synthetic tests.
    /// The regular CLI and daemon always use from_gh.
    pub fn with_token(config: Config, token: String) -> Result<Self> {
        if token.is_empty() {
            return Err(Error::Auth(config.hostname.clone()));
        }
        tokio::runtime::Handle::try_current()
            .map_err(|_| Error::Invalid("create the Client inside a Tokio runtime".into()))?;
        if config.queue_capacity == 0
            || config.max_attempts == 0
            || config.queue_timeout.is_zero()
            || config.request_timeout.is_zero()
            || config.report_timeout.is_zero()
            || config.max_body_bytes == 0
            || config.max_collection_bytes == 0
        {
            return Err(Error::Invalid(
                "queue capacity, timeouts, attempts, and body limit must be positive".into(),
            ));
        }
        if config.queue_capacity > 100_000
            || config.max_attempts > 10
            || config.queue_timeout > Duration::from_secs(86400)
            || config.request_timeout > Duration::from_secs(3600)
            || config.report_timeout > Duration::from_secs(3600)
            || config.min_spacing > Duration::from_secs(60)
            || config.max_body_bytes > 64 * 1024 * 1024
            || config.max_collection_bytes > 256 * 1024 * 1024
        {
            return Err(Error::Invalid(
                "configuration exceeds supported queue, retry, timeout, or body bounds".into(),
            ));
        }
        for url in [&config.rest_url, &config.graphql_url] {
            let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
            if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
                return Err(Error::Invalid(
                    "GitHub endpoints must use HTTPS (HTTP is allowed for loopback tests)".into(),
                ));
            }
            if !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(Error::Invalid(
                    "GitHub endpoint URLs cannot contain credentials, queries, or fragments".into(),
                ));
            }
        }
        if !config.rest_url.path().ends_with('/') {
            return Err(Error::Invalid("REST base URL must end with /".into()));
        }
        if config.rest_url.origin() != config.graphql_url.origin() {
            return Err(Error::Invalid(
                "REST and GraphQL must share their GitHub origin".into(),
            ));
        }
        let scope = digest(&format!(
            "{}\0{}\0{}\0{}",
            config.rest_url, config.graphql_url, config.api_version, token
        ));
        if config.change_retention.is_zero()
            || config.change_retention > Duration::from_secs(365 * 86400)
            || !(1..=1_000_000).contains(&config.max_change_events)
        {
            return Err(Error::Invalid("change retention must be positive and <=365 days, and event cap must be 1..1000000".into()));
        }
        let store = Store::open(
            &config.cache_path,
            config.change_retention,
            config.max_change_events,
            config.max_collection_bytes.saturating_mul(4),
        )?;
        let metrics = Arc::new(Metrics::default());
        let inflight = Arc::new(Mutex::new(HashMap::new()));
        let permits = Arc::new(Semaphore::new(config.queue_capacity));
        let (queue, rx) = mpsc::channel(config.queue_capacity);
        let http = reqwest::Client::builder()
            .user_agent(concat!("hey-gh/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        tokio::spawn(
            Scheduler {
                config: config.clone(),
                http,
                token,
                scope: scope.clone(),
                store: store.clone(),
                inflight: inflight.clone(),
                metrics: metrics.clone(),
            }
            .run(rx)
            .with_current_subscriber(),
        );
        Ok(Self(Arc::new(Inner {
            config,
            scope,
            store,
            queue,
            permits,
            inflight,
            read_priorities: Mutex::new(HashMap::new()),
            report_locks: Mutex::new(HashMap::new()),
            metrics,
            changes_notify: tokio::sync::Notify::new(),
        })))
    }

    // A foreground waiter promotes the entire report currently owning its
    // lock, including jobs queued before that waiter arrived. Weak entries
    // disappear after the report and all its shared requests finish.
    pub(crate) fn policy_priority(&self, repository: &str, number: u64) -> Arc<AtomicBool> {
        self.report_priority(
            "policy",
            repository,
            number,
            BACKGROUND_READ.try_with(|_| ()).is_err(),
        )
    }

    pub(crate) fn report_priority(
        &self,
        mode: &str,
        repository: &str,
        number: u64,
        interactive: bool,
    ) -> Arc<AtomicBool> {
        let key = format!("{mode}:{}#{number}", repository.to_ascii_lowercase());
        let mut priorities = self
            .0
            .read_priorities
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        priorities.retain(|_, priority| priority.strong_count() > 0);
        let priority = priorities
            .get(&key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(AtomicBool::new(interactive)));
        if interactive {
            priority.store(true, Ordering::Relaxed);
        }
        priorities.insert(key, Arc::downgrade(&priority));
        priority
    }

    pub async fn get(&self, path: &str, freshness: Freshness) -> Result<Response> {
        let url = self.rest_url(path)?;
        let response = self.request(url.to_string(), None, freshness).await?;
        crate::report::record_validation(url.as_str(), &response);
        Ok(response)
    }

    pub async fn graphql(
        &self,
        query: &str,
        variables: Value,
        freshness: Freshness,
    ) -> Result<Response> {
        // Read-only queries only: retries must never replay mutations.
        let query = query.trim();
        validate_query(query)?;
        let response = self
            .request(
                self.0.config.graphql_url.to_string(),
                Some(serde_json::json!({"query":query,"variables":variables})),
                freshness,
            )
            .await?;
        crate::report::record_validation(self.0.config.graphql_url.as_str(), &response);
        Ok(response)
    }

    async fn request(
        &self,
        url: String,
        body: Option<Value>,
        freshness: Freshness,
    ) -> Result<Response> {
        let repository = if let Some(body) = &body {
            let vars = &body["variables"];
            vars["owner"]
                .as_str()
                .zip(vars["repo"].as_str())
                .map(|(owner, repo)| format!("{owner}/{repo}"))
                .or_else(|| crate::entity::current().map(|owner| owner.repository))
        } else {
            self.request_repository(&url)
        }
        .filter(|repo| validate_repository(repo).is_ok())
        .map(|repo| repo.to_ascii_lowercase());
        for attempt in 0..2 {
            let generation = if let Some(repo) = &repository {
                self.repository_generation(repo).await?
            } else {
                0
            };
            if let Some(owner) = crate::entity::current()
                && repository.as_deref() == Some(owner.repository.as_str())
                && owner.generation != generation
            {
                return Err(if matches!(freshness, Freshness::CachedOnly) {
                    Error::CacheMiss
                } else {
                    crate::entity::changed()
                });
            }
            let (base_key, repository_prefix) = if let Some(body) = &body {
                (format!("{url}#{}", digest(&body.to_string())), None)
            } else {
                self.rest_cache_key(&url)?
            };
            let key = if generation == 0 {
                base_key
            } else {
                format!("{base_key}#repository-generation={generation}")
            };
            let policy = if attempt == 0 || matches!(freshness, Freshness::CachedOnly) {
                freshness
            } else {
                Freshness::Revalidate
            };
            let response = Box::pin(self.request_at(
                url.clone(),
                body.clone(),
                policy,
                key,
                repository_prefix,
            ))
            .await?;
            if let Some(repo) = &repository {
                if body.is_none()
                    && let Some(number) = self.request_pull_number(&url)
                    && let Some(node_id) = response.data["node_id"].as_str()
                {
                    let accepted = if matches!(response.source, Source::Cache) {
                        self.0
                            .store
                            .pr_identity(&self.0.scope, repo, number)
                            .await?
                            .is_none_or(|expected| expected == node_id)
                    } else {
                        self.0
                            .store
                            .accept_rest_identity(
                                &self.0.scope,
                                repo,
                                number,
                                node_id,
                                response.validated_at_ms,
                                &[
                                    format!("pr-status://{}/{repo}/{number}", self.hostname()),
                                    format!("metadata://{}/{repo}/{number}", self.hostname()),
                                ],
                            )
                            .await?
                    };
                    if !accepted {
                        return Err(if matches!(freshness, Freshness::CachedOnly) {
                            Error::CacheMiss
                        } else {
                            crate::entity::changed()
                        });
                    }
                }
                if self.repository_generation(repo).await? != generation {
                    continue;
                }
            }
            return Ok(response);
        }
        Err(if matches!(freshness, Freshness::CachedOnly) {
            Error::CacheMiss
        } else {
            crate::entity::changed()
        })
    }

    fn request_repository(&self, url: &str) -> Option<String> {
        let url = Url::parse(url).ok()?;
        let relative = url.path().strip_prefix(self.0.config.rest_url.path())?;
        let parts: Vec<_> = relative.trim_matches('/').split('/').collect();
        (parts.len() >= 3 && parts[0] == "repos").then(|| format!("{}/{}", parts[1], parts[2]))
    }

    fn rest_cache_key(&self, url: &str) -> Result<(String, Option<String>)> {
        let Some(repository) = self
            .request_repository(url)
            .filter(|r| validate_repository(r).is_ok())
        else {
            return Ok((url.to_owned(), None));
        };
        let original = self
            .0
            .config
            .rest_url
            .join(&format!("repos/{repository}/"))
            .map_err(|e| Error::Invalid(e.to_string()))?
            .to_string();
        let Some(suffix) = url.strip_prefix(&original) else {
            return Ok((url.to_owned(), None));
        };
        let prefix = self
            .0
            .config
            .rest_url
            .join(&format!("repos/{}/", repository.to_ascii_lowercase()))
            .map_err(|e| Error::Invalid(e.to_string()))?
            .to_string();
        Ok((format!("{prefix}{suffix}"), Some(prefix)))
    }

    fn request_pull_number(&self, url: &str) -> Option<u64> {
        let url = Url::parse(url).ok()?;
        let relative = url.path().strip_prefix(self.0.config.rest_url.path())?;
        let parts: Vec<_> = relative.trim_matches('/').split('/').collect();
        if parts.len() == 5 && parts[0] == "repos" && parts[3] == "pulls" {
            parts[4].parse().ok()
        } else {
            None
        }
    }

    async fn request_at(
        &self,
        url: String,
        body: Option<Value>,
        freshness: Freshness,
        key: String,
        repository_prefix: Option<String>,
    ) -> Result<Response> {
        let cached = self
            .0
            .store
            .get_repository_alias(&self.0.scope, &key, repository_prefix.as_deref())
            .await?;
        if let Some(mut response) = cached.clone() {
            let fresh = match freshness {
                Freshness::CachedOnly => true,
                Freshness::Revalidate => false,
                Freshness::MaxAge(age) => {
                    now_ms().saturating_sub(response.validated_at_ms) < age.as_millis() as u64
                }
            };
            if fresh {
                response.source = Source::Cache;
                self.0.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(response);
            }
        }
        if matches!(freshness, Freshness::CachedOnly) {
            return Err(Error::CacheMiss);
        }
        let mut receiver = {
            let mut inflight = self.0.inflight.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((receiver, interactive)) = inflight.get(&key) {
                if interactive_read() {
                    interactive.store(true, Ordering::Relaxed);
                }
                self.0.metrics.coalesced.fetch_add(1, Ordering::Relaxed);
                receiver.clone()
            } else {
                // Coalescing above must remain possible at either admission
                // boundary. Serialize this check with all distinct admissions;
                // scheduler completions can only release more capacity.
                if !interactive_read()
                    && self.0.permits.available_permits() <= self.interactive_reserved_slots()
                {
                    return Err(self.queue_full());
                }
                let permit = self
                    .0
                    .permits
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| self.queue_full())?;
                let (notify, receiver) = watch::channel(None);
                let now = tokio::time::Instant::now();
                let resource = if body.is_some() {
                    "graphql"
                } else if Url::parse(&url).is_ok_and(|u| u.path().contains("/search/")) {
                    "search"
                } else {
                    "core"
                };
                let endpoint = endpoint_class(&url, body.is_some(), &self.0.config.rest_url);
                let interactive = INTERACTIVE_READ
                    .try_with(Arc::clone)
                    .unwrap_or_else(|_| Arc::new(AtomicBool::new(false)));
                let job = Job {
                    interactive: interactive.clone(),
                    detail_lane: matches!(
                        endpoint,
                        "comments" | "review_comments" | "reviews" | "timeline"
                    ),
                    request_id: format!("{:032x}", fastrand::u128(..)),
                    endpoint,
                    queued_at: now,
                    http_status: None,
                    url,
                    key: key.clone(),
                    body,
                    cached,
                    notify,
                    deadline: REQUEST_DEADLINE
                        .try_with(|deadline| *deadline)
                        .ok()
                        .flatten()
                        .map_or(now + self.0.config.queue_timeout, |deadline| {
                            deadline.min(now + self.0.config.queue_timeout)
                        }),
                    ready_at: now,
                    attempts: 0,
                    resource: resource.into(),
                    _permit: permit,
                };
                self.0.queue.try_send(job).map_err(|e| match e {
                    mpsc::error::TrySendError::Closed(_) => Error::Stopped,
                    mpsc::error::TrySendError::Full(_) => self.queue_full(),
                })?;
                inflight.insert(key, (receiver.clone(), interactive));
                receiver
            }
        };
        loop {
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result.map(|r| (*r).clone());
            }
            receiver.changed().await.map_err(|_| Error::Stopped)?;
        }
    }

    fn rest_url(&self, path: &str) -> Result<Url> {
        let base = &self.0.config.rest_url;
        let url = if path.starts_with("https://") || path.starts_with("http://") {
            Url::parse(path)
        } else {
            base.join(path.strip_prefix('/').unwrap_or(path))
        }
        .map_err(|e| Error::Invalid(e.to_string()))?;
        if url.origin() != base.origin()
            || !url.path().starts_with(base.path())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Invalid(
                "request URL must remain inside the configured GitHub REST origin and prefix"
                    .into(),
            ));
        }
        Ok(url)
    }

    /// Follow all pages, retaining per-page cache validators. Refuse cycles and
    /// cross-origin links so pagination cannot forward credentials elsewhere.
    pub async fn pages(
        &self,
        path: &str,
        field: Option<&str>,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        let mut path = path.to_owned();
        let mut seen = std::collections::HashSet::new();
        let mut values = Vec::new();
        let mut bytes = 0usize;
        for _ in 0..1000 {
            if !seen.insert(self.rest_url(&path)?.to_string()) {
                return Err(Error::Invalid("pagination link cycle".into()));
            }
            let response = self.get(&path, freshness).await?;
            bytes = bytes.saturating_add(response.data.to_string().len());
            if bytes > self.0.config.max_collection_bytes {
                return Err(Error::Invalid(
                    "pagination exceeds configured collection byte limit".into(),
                ));
            }
            let page = field
                .map_or(&response.data, |field| &response.data[field])
                .as_array()
                .ok_or_else(|| Error::Invalid("expected a paginated GitHub array".into()))?;
            values.extend(page.iter().cloned());
            if values.len() > 100_000 {
                return Err(Error::Invalid("pagination exceeds 100,000 items".into()));
            }
            let next = response.link.as_deref().and_then(next_link);
            match next {
                Some(next) => path = next,
                None => return Ok(values),
            }
        }
        Err(Error::Invalid("pagination exceeds 1000 pages".into()))
    }

    pub async fn bootstrap(&self) -> Result<crate::SnapshotPage> {
        self.0.store.bootstrap(&self.0.scope).await
    }

    pub(crate) async fn bootstrap_prefix(
        &self,
        prefix: &str,
        fields: Option<&[&str]>,
    ) -> Result<crate::SnapshotPage> {
        self.0
            .store
            .bootstrap_prefix_projected(
                &self.0.scope,
                prefix,
                fields.map(|fields| fields.iter().map(|field| (*field).to_owned()).collect()),
            )
            .await
    }

    pub async fn changes(&self, cursor: Option<&str>, limit: usize) -> Result<ChangePage> {
        self.0.store.changes(&self.0.scope, cursor, limit).await
    }

    pub(crate) async fn validate_change_cursor(&self, cursor: &str) -> Result<()> {
        self.0.store.validate_cursor(&self.0.scope, cursor).await
    }

    pub async fn wait_changes(
        &self,
        cursor: Option<&str>,
        limit: usize,
        wait: Duration,
    ) -> Result<ChangePage> {
        self.wait_changes_prefix(cursor, limit, wait, "", None)
            .await
    }

    pub(crate) async fn wait_changes_prefix(
        &self,
        cursor: Option<&str>,
        limit: usize,
        wait: Duration,
        prefix: &str,
        fields: Option<&[&str]>,
    ) -> Result<ChangePage> {
        if wait > Duration::from_secs(30) {
            return Err(Error::Invalid(
                "long polling is limited to 30 seconds".into(),
            ));
        }
        let deadline = tokio::time::Instant::now() + wait;
        let fields = fields.map(|fields| {
            fields
                .iter()
                .map(|field| (*field).to_owned())
                .collect::<Vec<_>>()
        });
        let mut position = cursor.map(str::to_owned);
        loop {
            let notified = self.0.changes_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let page = self
                .0
                .store
                .changes_prefix_projected(
                    &self.0.scope,
                    position.as_deref(),
                    limit,
                    prefix,
                    fields.clone(),
                )
                .await?;
            if !page.changes.is_empty() || page.has_more || tokio::time::Instant::now() >= deadline
            {
                return Ok(page);
            }
            position = Some(page.next_cursor.clone());
            // A periodic check also observes database writes by another SDK process.
            tokio::select! {
                _ = notified => {},
                _ = tokio::time::sleep_until(deadline.min(tokio::time::Instant::now()+Duration::from_secs(1))) => {},
            }
        }
    }

    pub(crate) async fn observe(&self, resource: &str, data: &Value) -> Result<String> {
        let observations = [(resource.to_owned(), data.clone())];
        let owner = if let Some(owner) = crate::entity::current() {
            Some(owner)
        } else if resource.starts_with("metadata://") {
            let parts: Vec<_> = resource.split_once("://").unwrap().1.split('/').collect();
            if parts.len() == 4 {
                let repo = format!("{}/{}", parts[1], parts[2]);
                Some(
                    self.pr_owner(
                        &repo,
                        parts[3]
                            .parse()
                            .map_err(|_| Error::Invalid("invalid metadata selector".into()))?,
                        &data["pull_request"],
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            None
        };
        let cursor = if let Some(owner) = owner {
            self.0
                .store
                .observe_owned(&self.0.scope, &observations, &owner)
                .await?
        } else {
            self.0.store.observe(&self.0.scope, resource, data).await?
        };
        self.0.changes_notify.notify_waiters();
        Ok(cursor)
    }
    pub(crate) async fn observe_validated_status(
        &self,
        resource: &str,
        data: &Value,
        clock: u64,
        discovery_clock: u64,
        changed: bool,
        owner: &crate::store::PrOwner,
    ) -> Result<String> {
        let cursor = self
            .0
            .store
            .observe_validated_owned(
                &self.0.scope,
                &[(resource.to_owned(), data.clone())],
                &[
                    (resource.to_owned(), clock),
                    (format!("{resource}#discovery"), discovery_clock),
                ],
                owner,
            )
            .await?;
        if changed {
            self.0.changes_notify.notify_waiters();
        }
        Ok(cursor)
    }
    pub(crate) async fn status_validation_clock(&self, resource: &str) -> Result<u64> {
        self.0.store.validation_clock(&self.0.scope, resource).await
    }

    pub(crate) async fn observe_many(&self, observations: &[(String, Value)]) -> Result<String> {
        let cursor = if let Some(owner) = crate::entity::current() {
            self.0
                .store
                .observe_owned(&self.0.scope, observations, &owner)
                .await?
        } else {
            self.0
                .store
                .observe_many(&self.0.scope, observations)
                .await?
        };
        self.0.changes_notify.notify_waiters();
        Ok(cursor)
    }
    pub(crate) async fn derived(&self, key: &str) -> Result<Option<Response>> {
        let (key, prefix) = derived_repository_key(key);
        let key = self.derived_entity_key(&key).await?;
        let response = self
            .0
            .store
            .get_repository_alias(&self.0.scope, &key, prefix.as_deref())
            .await?;
        if let Some(response) = &response {
            crate::report::record_validation(&key, response);
        }
        Ok(response)
    }
    pub(crate) async fn save_derived(&self, key: &str, data: Value) -> Result<()> {
        let (key, _) = derived_repository_key(key);
        let key = self.derived_entity_key(&key).await?;
        let stamp = now_ms();
        let response = Response {
            data,
            fetched_at_ms: stamp,
            validated_at_ms: stamp,
            source: Source::Cache,
            etag: None,
            last_modified: None,
            link: None,
        };
        self.0.store.put(&self.0.scope, &key, &response).await
    }

    async fn derived_entity_key(&self, key: &str) -> Result<String> {
        let repository = if key.starts_with("completed-jobs://") {
            let parts: Vec<_> = key.split_once("://").unwrap().1.split('/').collect();
            (parts.len() >= 3).then(|| format!("{}/{}", parts[1], parts[2]))
        } else if key.starts_with("policy-error://") {
            let parts: Vec<_> = key.split_once("://").unwrap().1.split('/').collect();
            (parts.len() >= 4 && parts[1] == "repos").then(|| format!("{}/{}", parts[2], parts[3]))
        } else {
            None
        };
        if let Some(repo) = repository {
            let generation = self.repository_generation(&repo).await?;
            if let Some(owner) = crate::entity::current()
                && owner.repository.eq_ignore_ascii_case(&repo)
                && owner.generation != generation
            {
                return Err(crate::entity::changed());
            }
            if generation > 0 {
                return Ok(format!("{key}#repository-generation={generation}"));
            }
        }
        Ok(key.to_owned())
    }

    pub(crate) async fn repository_generation(&self, repository: &str) -> Result<u64> {
        self.0
            .store
            .repository_generation(&self.0.scope, repository)
            .await
    }

    pub(crate) async fn expected_pr_node(
        &self,
        repository: &str,
        number: u64,
    ) -> Result<Option<String>> {
        self.0
            .store
            .pr_identity(&self.0.scope, repository, number)
            .await
    }

    pub(crate) async fn pr_owner(
        &self,
        repository: &str,
        number: u64,
        pr: &Value,
    ) -> Result<crate::store::PrOwner> {
        let owner = crate::store::PrOwner {
            repository: repository.to_ascii_lowercase(),
            number,
            node_id: pr["node_id"].as_str().map(str::to_owned),
            generation: self.repository_generation(repository).await?,
        };
        if !self.0.store.owner_is_current(&self.0.scope, &owner).await? {
            return Err(crate::entity::changed());
        }
        Ok(owner)
    }

    pub(crate) async fn current_pr_owner_is_valid(&self) -> Result<bool> {
        if let Some(owner) = crate::entity::current() {
            self.0.store.owner_is_current(&self.0.scope, &owner).await
        } else {
            Ok(true)
        }
    }

    pub(crate) async fn stored_pr_snapshot(
        &self,
        resource: &str,
        repository: &str,
        node_id: Option<&str>,
    ) -> Result<Option<Value>> {
        self.0
            .store
            .snapshot_for_pr(&self.0.scope, resource, repository, node_id)
            .await
    }

    pub(crate) async fn pr_repository_spelling(
        &self,
        repository: &str,
        number: u64,
    ) -> Result<String> {
        let requested = format!("metadata://{}/{repository}/{number}", self.hostname());
        let key = match self
            .0
            .store
            .pr_resource_key(&self.0.scope, &requested)
            .await
        {
            Ok(key) => key,
            Err(error) => {
                // This optional presentation/cache hint must not prevent
                // a real upstream failure from reaching the caller.
                tracing::warn!(
                    error_code = error.diagnostic_code(),
                    "repository spelling hint unavailable; retaining requested spelling"
                );
                requested
            }
        };
        let parts: Vec<_> = key
            .split_once("://")
            .expect("metadata key")
            .1
            .split('/')
            .collect();
        Ok(format!("{}/{}", parts[1], parts[2]))
    }
    pub(crate) async fn discovery_health(&self) -> Result<Option<crate::store::DiscoveryHealth>> {
        self.0
            .store
            .discovery_health(&self.0.scope, crate::dashboard::DISCOVERY_CACHE)
            .await
    }
    pub(crate) async fn begin_discovery(&self, legacy_success_at_ms: Option<u64>) -> Result<u64> {
        self.0
            .store
            .begin_discovery(
                &self.0.scope,
                crate::dashboard::DISCOVERY_CACHE,
                legacy_success_at_ms,
            )
            .await
    }
    pub(crate) async fn finish_discovery(
        &self,
        generation: u64,
        collection: Option<Value>,
        error: Option<String>,
    ) -> Result<()> {
        let stamp = now_ms();
        let collection = collection.map(|data| Response {
            data,
            fetched_at_ms: stamp,
            validated_at_ms: stamp,
            source: Source::Cache,
            etag: None,
            last_modified: None,
            link: None,
        });
        self.0
            .store
            .finish_discovery(
                &self.0.scope,
                crate::dashboard::DISCOVERY_CACHE,
                generation,
                collection,
                error,
            )
            .await
    }
    pub(crate) fn hostname(&self) -> &str {
        &self.0.config.hostname
    }
    pub(crate) fn report_timeout(&self) -> Duration {
        self.0.config.report_timeout
    }
    pub(crate) fn collection_limit(&self) -> usize {
        self.0.config.max_collection_bytes
    }
    pub(crate) fn report_lock(&self, resource: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .0
            .report_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if locks.len() >= 4096 {
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        locks.entry(resource.into()).or_default().clone()
    }
    pub fn status(&self) -> Status {
        Status {
            outstanding_requests: self.0.config.queue_capacity - self.0.permits.available_permits(),
            active_requests: self.0.metrics.active.load(Ordering::Relaxed) as usize,
            max_active_requests: self
                .0
                .config
                .queue_capacity
                .min(crate::scheduler::MAX_ACTIVE_BUCKETS),
            queue_capacity: self.0.config.queue_capacity,
            interactive_reserved_slots: self.interactive_reserved_slots(),
            queue_full_rejections: self.0.metrics.queue_full.load(Ordering::Relaxed),
            cache_hits: self.0.metrics.cache_hits.load(Ordering::Relaxed),
            coalesced_requests: self.0.metrics.coalesced.load(Ordering::Relaxed),
            network_requests: self.0.metrics.network.load(Ordering::Relaxed),
            conditional_requests: self.0.metrics.conditional.load(Ordering::Relaxed),
            not_modified_responses: self.0.metrics.not_modified.load(Ordering::Relaxed),
            rate_limits: self
                .0
                .metrics
                .limits
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }

    /// A pull_number of 0 watches and discovers all of the effective user's open PRs in this repository.
    pub async fn save_watch(
        &self,
        repository: &str,
        pull_number: u64,
        interval_seconds: u64,
    ) -> Result<Watch> {
        validate_repository(repository)?;
        if !(10..=86400).contains(&interval_seconds) {
            return Err(Error::Invalid("interval must be 10..86400 seconds".into()));
        }
        let watch = Watch {
            id: digest(&format!("{repository}#{pull_number}")),
            repository: repository.into(),
            pull_number,
            interval_seconds,
            kind: crate::WatchKind::PullRequests,
            branches: Vec::new(),
            all_branches: false,
        };
        self.0.store.save_watch(&self.0.scope, &watch).await?;
        Ok(watch)
    }
    pub(crate) async fn tracking(&self, id: &str, kind: &str) -> Result<Vec<u64>> {
        self.0.store.tracking(&self.0.scope, id, kind).await
    }
    pub(crate) async fn save_tracking(&self, id: &str, kind: &str, numbers: &[u64]) -> Result<()> {
        self.0
            .store
            .save_tracking(&self.0.scope, id, kind, numbers)
            .await
    }
    pub async fn watches(&self) -> Result<Vec<Watch>> {
        self.0.store.watches(&self.0.scope).await
    }
    pub(crate) async fn stored_snapshot(&self, resource: &str) -> Result<Option<Value>> {
        self.0.store.snapshot(&self.0.scope, resource).await
    }
    pub(crate) async fn persist_watch(&self, watch: &Watch) -> Result<()> {
        self.0.store.save_watch(&self.0.scope, watch).await
    }
    pub async fn delete_watch(&self, id: &str) -> Result<()> {
        self.0.store.delete_watch(&self.0.scope, id).await
    }
}

fn derived_repository_key(key: &str) -> (String, Option<String>) {
    let Some((scheme, path)) = key.split_once("://") else {
        return (key.to_owned(), None);
    };
    let parts: Vec<_> = path.split('/').collect();
    let start = match scheme {
        "completed-jobs" if parts.len() >= 4 => 1,
        "policy-error" if parts.len() >= 5 && parts[1] == "repos" => 2,
        _ => return (key.to_owned(), None),
    };
    let repository = format!("{}/{}", parts[start], parts[start + 1]);
    if validate_repository(&repository).is_err() {
        return (key.to_owned(), None);
    }
    let original = format!("{scheme}://{}{repository}/", parts[..start].join("/") + "/");
    let prefix = format!(
        "{scheme}://{}{}/",
        parts[..start].join("/") + "/",
        repository.to_ascii_lowercase()
    );
    (
        format!(
            "{prefix}{}",
            key.strip_prefix(&original).expect("derived prefix")
        ),
        Some(prefix),
    )
}

pub(crate) fn validate_repository(repository: &str) -> Result<()> {
    let parts: Vec<_> = repository.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|s| {
            s.is_empty()
                || *s == "."
                || *s == ".."
                || !s
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    {
        return Err(Error::Invalid("repository must be OWNER/REPO".into()));
    }
    Ok(())
}

// Only allowlisted labels reach logs. Parse the path independently of query
// values, and support enterprise REST origins with a configured path prefix.
fn endpoint_class(url: &str, graphql: bool, rest_base: &Url) -> &'static str {
    if graphql {
        return "graphql";
    }
    let Ok(url) = Url::parse(url) else {
        return "rest_other";
    };
    let Some(path) = url.path().strip_prefix(rest_base.path()) else {
        return "rest_other";
    };
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    match parts.as_slice() {
        ["user"] => "viewer",
        ["search", ..] => "search",
        ["repos", _, _] => "repository",
        ["repos", _, _, "pulls"] => "pull_requests",
        ["repos", _, _, "pulls", _] => "pull_request",
        ["repos", _, _, "pulls", _, "reviews"] => "reviews",
        ["repos", _, _, "pulls", _, "comments"] => "review_comments",
        ["repos", _, _, "issues", _, "comments"] => "comments",
        ["repos", _, _, "issues", _, "timeline"] => "timeline",
        ["repos", _, _, "commits", _, "check-runs"] => "check_runs",
        ["repos", _, _, "commits", _, "status"] => "commit_statuses",
        ["repos", _, _, "actions", "runs"] => "workflow_runs",
        ["repos", _, _, "actions", "runs", _, "attempts", _, "jobs"] => "workflow_jobs",
        [
            "repos",
            _,
            _,
            "branches",
            _,
            "protection",
            "required_status_checks",
        ] => "branch_protection",
        ["repos", _, _, "rules", "branches", _] => "branch_rules",
        ["repos", _, _, "branches"] => "branches",
        ["repos", _, _, "branches", _] => "branch",
        ["repos", _, _, "commits", _] => "commit",
        ["repos", _, _, "compare", _] => "compare",
        _ => "rest_other",
    }
}

fn next_link(link: &str) -> Option<String> {
    for part in link.split(',') {
        let (target, params) = part.trim().split_once('>')?;
        if params.split(';').any(|p| p.trim() == "rel=\"next\"") {
            return target.strip_prefix('<').map(str::to_owned);
        }
    }
    None
}

fn validate_query(query: &str) -> Result<()> {
    use graphql_parser::query::{Definition, OperationDefinition};
    let document = graphql_parser::parse_query::<String>(query)
        .map_err(|e| Error::Invalid(format!("invalid GraphQL query: {e}")))?;
    let mut operations = 0;
    for definition in document.definitions {
        if let Definition::Operation(operation) = definition {
            operations += 1;
            if !matches!(
                operation,
                OperationDefinition::Query(_) | OperationDefinition::SelectionSet(_)
            ) {
                return Err(Error::Invalid(
                    "only read-only GraphQL queries are supported".into(),
                ));
            }
        }
    }
    if operations != 1 {
        return Err(Error::Invalid(
            "provide exactly one GraphQL read operation".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod priority_tests {
    use super::*;

    #[tokio::test]
    async fn interactive_reserve_is_bounded_and_full_queue_still_coalesces() {
        let dir = tempfile::tempdir().unwrap();
        let release = Arc::new(tokio::sync::Notify::new());
        let router = axum::Router::new().fallback({
            let release = release.clone();
            move || {
                let release = release.clone();
                async move {
                    release.notified().await;
                    axum::Json(serde_json::json!({"ok": true}))
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::with_token(
            Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                queue_capacity: 8,
                min_spacing: Duration::ZERO,
                ..Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let mut tasks = Vec::new();
        for index in 0..8 {
            let c = client.clone();
            tasks.push(tokio::spawn(async move {
                let path = format!("request/{index}");
                let read = c.get(&path, Freshness::Revalidate);
                INTERACTIVE_READ
                    .scope(Arc::new(AtomicBool::new(index >= 6)), read)
                    .await
            }));
            tokio::time::timeout(Duration::from_secs(2), async {
                while client.status().outstanding_requests != index + 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if index == 5 {
                assert!(matches!(
                    client
                        .get("background-overflow", Freshness::Revalidate)
                        .await,
                    Err(Error::QueueFull)
                ));
            }
        }
        assert!(matches!(
            INTERACTIVE_READ
                .scope(
                    foreground_priority(),
                    client.get("foreground-overflow", Freshness::Revalidate)
                )
                .await,
            Err(Error::QueueFull)
        ));
        let status = client.status();
        assert_eq!(status.interactive_reserved_slots, 2);
        assert_eq!(status.outstanding_requests, status.queue_capacity);
        assert_eq!(status.queue_full_rejections, 2);
        tasks[0].abort();
        let c = client.clone();
        let coalesced = tokio::spawn(async move {
            INTERACTIVE_READ
                .scope(
                    foreground_priority(),
                    c.get("request/0", Freshness::Revalidate),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while client.status().coalesced_requests != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(client.status().outstanding_requests, 8);
        assert!(
            client
                .0
                .inflight
                .lock()
                .unwrap()
                .values()
                .any(|(_, priority)| priority.load(Ordering::Relaxed))
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while client.status().outstanding_requests != 0 {
                release.notify_waiters();
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert!(coalesced.await.unwrap().is_ok());
        for task in tasks.into_iter().skip(1) {
            assert!(task.await.unwrap().is_ok());
        }
        assert!(client.get("request/0", Freshness::CachedOnly).await.is_ok());
        server.abort();
    }

    #[tokio::test]
    async fn foreground_waiter_promotes_locked_background_policy_and_releases_registry() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            Config {
                cache_path: dir.path().join("cache.sqlite"),
                ..Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let lock = client.report_lock("required-checks:acme/demo#7");
        let _background_owner = lock.lock().await;
        let background = BACKGROUND_READ
            .scope((), async { client.policy_priority("acme/demo", 7) })
            .await;
        assert!(!background.load(Ordering::Relaxed));
        let foreground = client.policy_priority("ACME/DEMO", 7);
        assert!(Arc::ptr_eq(&background, &foreground));
        assert!(
            background.load(Ordering::Relaxed),
            "promotion must occur before waiting for the report lock"
        );
        drop(background);
        drop(foreground);
        let other = client.policy_priority("acme/other", 8);
        assert!(other.load(Ordering::Relaxed));
        let priorities = client.0.read_priorities.lock().unwrap();
        assert_eq!(
            priorities.len(),
            1,
            "completed report priorities must not accumulate"
        );
        assert!(priorities.contains_key("policy:acme/other#8"));
    }
}
