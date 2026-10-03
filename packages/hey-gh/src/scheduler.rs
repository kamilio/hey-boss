use crate::{Error, Response, Result, Source, client::Config, now_ms, store::Store};
use reqwest::{StatusCode, header::HeaderMap};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    future::{Future, poll_fn},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::Poll,
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{OwnedSemaphorePermit, mpsc, watch},
    time::Instant,
};

pub(crate) const MAX_ACTIVE_BUCKETS: usize = 3;
const QUOTA_RESERVE: u64 = 100;

pub(crate) fn max_active_buckets(config: &Config) -> usize {
    if config.rest_url.host_str() == Some("api.github.com") {
        8
    } else {
        MAX_ACTIVE_BUCKETS
    }
}

pub(crate) type SharedResult = Option<Result<Arc<Response>>>;
pub(crate) type Inflight = Arc<
    Mutex<
        HashMap<
            String,
            (
                watch::Receiver<SharedResult>,
                Arc<AtomicBool>,
                Arc<Mutex<Instant>>,
            ),
        >,
    >,
>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateLimit {
    pub remaining: u64,
    pub reset_at_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub outstanding_requests: usize,
    #[serde(default)]
    pub active_requests: usize,
    #[serde(default)]
    pub max_active_requests: usize,
    pub queue_capacity: usize,
    /// Slots within queue_capacity unavailable to new background requests.
    #[serde(default)]
    pub interactive_reserved_slots: usize,
    /// Distinct admissions rejected, before any network attempt (not completions).
    #[serde(default)]
    pub queue_full_rejections: u64,
    pub cache_hits: u64,
    pub coalesced_requests: u64,
    pub network_requests: u64,
    /// Network attempts carrying an HTTP cache validator, including retries.
    #[serde(default)]
    pub conditional_requests: u64,
    /// HTTP 304 responses observed, including an invalid response without cache.
    #[serde(default)]
    pub not_modified_responses: u64,
    pub rate_limits: BTreeMap<String, RateLimit>,
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub queue_full: AtomicU64,
    pub cache_hits: AtomicU64,
    pub coalesced: AtomicU64,
    pub network: AtomicU64,
    pub conditional: AtomicU64,
    pub not_modified: AtomicU64,
    pub active: AtomicU64,
    pub core_spacing_ms: AtomicU64,
    pub limits: Mutex<BTreeMap<String, RateLimit>>,
}

pub(crate) struct Job {
    // Shared with coalesced readers so interactive use can promote queued work.
    pub interactive: Arc<AtomicBool>,
    pub detail_lane: bool,
    /// Random per-job correlation, independent of credentials and request data.
    pub request_id: String,
    /// Fixed endpoint class; never contains caller-controlled request data.
    pub endpoint: &'static str,
    pub queued_at: Instant,
    pub http_status: Option<u16>,
    pub url: String,
    pub key: String,
    pub body: Option<serde_json::Value>,
    pub cached: Option<Response>,
    pub notify: watch::Sender<SharedResult>,
    pub deadline: Arc<Mutex<Instant>>,
    pub ready_at: Instant,
    pub attempts: u32,
    pub resource: String,
    pub _permit: OwnedSemaphorePermit,
}

// Two ordinary quota buckets and one REST detail lane can make progress.
// Details share core quota, but cannot hold its lifecycle/CI socket. Body reads
// retain their lane; headers reach the scheduler before any body wait so quota
// exhaustion and shared cooldowns take effect immediately.
impl Job {
    pub(crate) fn deadline(&self) -> Instant {
        *self.deadline.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct Active {
    resource: String,
    detail_lane: bool,
    future: Pin<Box<dyn Future<Output = (Job, Attempt)> + Send>>,
}

fn lane_busy(active: &[Active], job: &Job, prod: bool) -> bool {
    if !prod {
        return if job.detail_lane {
            active.iter().any(|attempt| attempt.detail_lane)
        } else {
            active.iter().filter(|attempt| !attempt.detail_lane).count() >= 2
                || active
                    .iter()
                    .any(|attempt| !attempt.detail_lane && attempt.resource == job.resource)
        };
    }
    if job.detail_lane {
        active.iter().filter(|attempt| attempt.detail_lane).count() >= 2
    } else if job.resource == "core" {
        active.iter().filter(|attempt| !attempt.detail_lane).count() >= 6
            || active
                .iter()
                .filter(|attempt| !attempt.detail_lane && attempt.resource == "core")
                .count()
                >= 5
    } else {
        active.iter().filter(|attempt| !attempt.detail_lane).count() >= 6
            || active
                .iter()
                .filter(|attempt| !attempt.detail_lane && attempt.resource == job.resource)
                .count()
                >= 2
    }
}

enum Attempt {
    Headers(std::result::Result<reqwest::Response, reqwest::Error>),
    Body {
        status: StatusCode,
        headers: HeaderMap,
        bytes: Result<Vec<u8>>,
    },
}

async fn next_attempt(active: &mut [Active]) -> (usize, Job, Attempt) {
    poll_fn(|cx| {
        for (index, attempt) in active.iter_mut().enumerate() {
            if let Poll::Ready((job, outcome)) = attempt.future.as_mut().poll(cx) {
                return Poll::Ready((index, job, outcome));
            }
        }
        Poll::Pending
    })
    .await
}

struct Budget {
    next: Instant,
    remaining: u64,
    spacing: Duration,
    reset_at_seconds: u64,
    usage: SharedUsage,
}

#[derive(Clone)]
struct SharedUsage {
    since: Instant,
    remaining: u64,
    charged: u64,
    share: f64,
}

impl SharedUsage {
    fn new(remaining: u64, now: Instant) -> Self {
        Self {
            since: now,
            remaining,
            charged: 0,
            share: 1.0,
        }
    }

    fn observe(&mut self, remaining: u64, charged: bool, now: Instant) {
        self.charged += u64::from(charged);
        let elapsed = now.duration_since(self.since);
        if elapsed >= Duration::from_secs(30) {
            // Response counts are an estimate of this client's charges, not an
            // attribution of all account usage. The header delta includes other
            // daemons and direct CLI traffic using the same shared allowance.
            // Sparse/idle observations cannot estimate sustained local demand.
            // Don't punish the first interactive read after an idle period with
            // an entire interval of unrelated account traffic.
            if self.charged >= 4 && elapsed <= Duration::from_secs(60) {
                self.share = (self.remaining.saturating_sub(remaining) as f64
                    / self.charged as f64)
                    .max(1.0);
            } else {
                self.share = 1.0;
            }
            self.since = now;
            self.remaining = remaining;
            self.charged = 0;
        }
    }

    fn spacing(&self, remaining: u64, seconds: u64) -> Duration {
        // Keep headroom for preflight and other clients outside this daemon.
        // This is the same reserve used by conditional probes. Read deadlines
        // stay unchanged: delayed evidence remains explicitly unavailable.
        let spendable = remaining.saturating_sub(QUOTA_RESERVE);
        if spendable == 0 {
            Duration::from_secs(seconds.saturating_add(1))
        } else {
            Duration::from_secs_f64((seconds as f64 * self.share / spendable as f64).min(86400.0))
        }
    }
}

// GitHub can return overlapping reset windows (including on ordinary REST
// responses). A different reset is not proof that the prior window expired.
#[derive(Default)]
struct Budgets(HashMap<String, BTreeMap<u64, Budget>>);

impl Budgets {
    fn for_resource(&self, resource: &str) -> impl Iterator<Item = &Budget> {
        self.0
            .get(resource)
            .into_iter()
            .flat_map(|windows| windows.values())
    }

    fn observe(&mut self, resource: &str, remaining: u64, reset: u64, unchanged: bool) {
        let now = Instant::now();
        let seconds_now = now_ms() / 1000;
        let windows = self.0.entry(resource.to_owned()).or_default();
        windows.retain(|reset, budget| {
            reset.saturating_add(1) > seconds_now || (*reset == 0 && budget.next > now)
        });
        let previous = windows.remove(&reset);
        // Parallel responses and cached upstream headers may arrive out of order.
        // Only expiry, never a higher header in a live window, restores capacity.
        let remaining = previous
            .as_ref()
            .map_or(remaining, |b| b.remaining.min(remaining));
        let mut usage = SharedUsage::new(remaining, now);
        if let Some(previous) = &previous {
            usage = previous.usage.clone();
            usage.observe(remaining, !unchanged, now);
        }
        let seconds = reset.saturating_sub(seconds_now);
        let spacing = if remaining == 0 {
            Duration::from_secs(seconds.saturating_add(1))
        } else if resource == "core" {
            usage.spacing(remaining, seconds)
        } else {
            Duration::from_secs_f64(seconds as f64 / (remaining as f64 + 1.0))
        };
        let next = if unchanged && remaining > 0 {
            previous.as_ref().map_or(now, |b| b.next)
        } else {
            previous.as_ref().map_or_else(
                || quota_deadline(spacing),
                |b| b.next.max(quota_deadline(spacing)),
            )
        };
        windows.insert(
            reset,
            Budget {
                next,
                remaining,
                spacing,
                reset_at_seconds: reset,
                usage,
            },
        );
    }

    fn reserve(&mut self, job: &Job) {
        if let Some(windows) = self.0.get_mut(&job.resource) {
            for budget in windows.values_mut() {
                if budget.remaining > 0
                    && budget.reset_at_seconds > now_ms() / 1000
                    && !conditional_budget_exempt(job, budget)
                {
                    budget.next = quota_deadline(budget.spacing);
                }
            }
        }
    }

    fn exhausted(&mut self, resource: &str, reset: u64, wait: Duration) {
        self.0.entry(resource.to_owned()).or_default().insert(
            reset,
            Budget {
                next: quota_deadline(wait),
                remaining: 0,
                spacing: wait,
                reset_at_seconds: reset,
                usage: SharedUsage::new(0, Instant::now()),
            },
        );
    }
}

pub(crate) struct Scheduler {
    pub config: Config,
    pub http: reqwest::Client,
    pub token: String,
    pub scope: String,
    pub store: Store,
    pub inflight: Inflight,
    pub metrics: Arc<Metrics>,
}

impl Scheduler {
    pub async fn run(self, mut rx: mpsc::Receiver<Job>) {
        // Random process-local identifier; scope is already an opaque auth hash.
        let instance = format!("{:032x}", fastrand::u128(..));
        let mut pending = VecDeque::<Job>::new();
        let mut interactive_streaks = HashMap::<String, [usize; 2]>::new();
        let mut budgets = Budgets::default();
        let mut routes = HashMap::<String, String>::new();
        let mut global_next = Instant::now();
        let mut secondary_until = Instant::now();
        let mut active = Vec::<Active>::new();
        let prod = self.config.rest_url.host_str() == Some("api.github.com");
        let max_active = self
            .config
            .queue_capacity
            .min(max_active_buckets(&self.config));
        loop {
            self.metrics
                .active
                .store(active.len() as u64, Ordering::Relaxed);
            tokio::task::yield_now().await;
            while let Ok(mut job) = rx.try_recv() {
                if let Some(resource) = routes.get(&job.key) {
                    job.resource.clone_from(resource);
                }
                pending.push_back(job);
            }
            if pending.is_empty() && active.is_empty() {
                match rx.recv().await {
                    Some(mut job) => {
                        if let Some(resource) = routes.get(&job.key) {
                            job.resource.clone_from(resource);
                        }
                        pending.push_back(job);
                    }
                    None => break,
                }
            }
            let now = Instant::now();
            // Expiry is independent of quota availability, including exhausted
            // buckets whose next reset might be an hour away.
            if let Some(index) = pending.iter().position(|j| {
                j.deadline() <= now
                    || ready(j, &budgets, global_next.max(secondary_until)) >= j.deadline()
            }) {
                let job = pending.remove(index).expect("existing queue entry");
                let ready = ready(&job, &budgets, global_next.max(secondary_until));
                let quota_blocked = secondary_until > now
                    || budgets
                        .for_resource(&job.resource)
                        .any(|b| b.next > now && !conditional_budget_exempt(&job, b));
                let error = if quota_blocked {
                    Error::RateLimited {
                        retry_after_seconds: ceil_seconds(ready.saturating_duration_since(now)),
                    }
                } else {
                    Error::Deadline
                };
                self.finish(job, Err(error));
                continue;
            }
            let global = global_next.max(secondary_until);
            let next = {
                let eligible = |job: &Job| {
                    ready(job, &budgets, global) <= now && !lane_busy(&active, job, prod)
                };
                // Prefer interactive policy, but admit an eligible background job
                // after at most three foreground dispatches in the same quota lane.
                // A GraphQL/detail completion cannot reset core's fairness counter.
                // Quotas, lane limits,
                // retries, cooldowns and expiry remain unchanged.
                let preferred = pending.iter().position(|job| {
                    eligible(job)
                        && job.interactive.load(Ordering::Relaxed)
                            == (interactive_streaks
                                .get(&job.resource)
                                .map_or(0, |streaks| streaks[usize::from(job.detail_lane)])
                                < 3)
                });
                preferred.or_else(|| pending.iter().position(eligible))
            };
            if active.len() < max_active
                && let Some(index) = next
            {
                let mut job = pending.remove(index).expect("existing queue entry");
                // Reserve every live window before another socket can dispatch.
                budgets.reserve(&job);
                let streak = interactive_streaks.entry(job.resource.clone()).or_default();
                let streak = &mut streak[usize::from(job.detail_lane)];
                *streak = if job.interactive.load(Ordering::Relaxed) {
                    streak.saturating_add(1)
                } else {
                    0
                };
                job.attempts += 1;
                job.http_status = None;
                tracing::info!(request_id=%job.request_id, attempt=job.attempts,
                    endpoint=job.endpoint, resource=%job.resource,
                    foreground=job.interactive.load(Ordering::Relaxed),
                    conditional=job.body.is_none() && job.cached.as_ref().is_some_and(|c| c.etag.is_some() || c.last_modified.is_some()),
                    auth_scope=%self.scope, %instance, request_key=%crate::digest(&job.key),
                    "GitHub request dispatched");
                self.metrics.network.fetch_add(1, Ordering::Relaxed);
                let mut request = if let Some(body) = &job.body {
                    self.http.post(&job.url).json(body)
                } else {
                    self.http.get(&job.url)
                }
                .bearer_auth(&self.token)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", &self.config.api_version)
                .timeout(
                    self.config
                        .request_timeout
                        .min(job.deadline().saturating_duration_since(now)),
                );
                if let Some(cache) = &job.cached
                    && job.body.is_none()
                {
                    if let Some(etag) = &cache.etag {
                        request = request.header("If-None-Match", etag);
                        self.metrics.conditional.fetch_add(1, Ordering::Relaxed);
                    } else if let Some(last) = &cache.last_modified {
                        request = request.header("If-Modified-Since", last);
                        self.metrics.conditional.fetch_add(1, Ordering::Relaxed);
                    }
                }
                let mut attempt = Active {
                    resource: job.resource.clone(),
                    detail_lane: job.detail_lane,
                    future: Box::pin(async move { (job, Attempt::Headers(request.send().await)) }),
                };
                // Start the socket now, rather than treating an unpolled
                // future as dispatched before a later cooldown is learned.
                let immediate = poll_fn(|cx| {
                    Poll::Ready(match attempt.future.as_mut().poll(cx) {
                        Poll::Ready(outcome) => Some(outcome),
                        Poll::Pending => None,
                    })
                })
                .await;
                if let Some(outcome) = immediate {
                    attempt.future = Box::pin(async move { outcome });
                }
                global_next = Instant::now() + self.config.min_spacing;
                active.push(attempt);
                continue;
            }
            let wake = pending
                .iter()
                .map(|job| {
                    if active.len() >= max_active || lane_busy(&active, job, prod) {
                        // Busy lanes wake on completion; never spin on their old
                        // ready time. Their queued deadlines still expire on time.
                        job.deadline()
                    } else {
                        ready(job, &budgets, global).min(job.deadline())
                    }
                })
                .min();
            let completed = tokio::select! {
                biased;
                completed = next_attempt(&mut active), if !active.is_empty() => Some(completed),
                job = rx.recv(), if !rx.is_closed() => {
                    if let Some(mut job) = job {
                        if let Some(resource) = routes.get(&job.key) { job.resource.clone_from(resource); }
                        pending.push_back(job);
                    }
                    None
                }
                _ = async { if let Some(wake) = wake { tokio::time::sleep_until(wake).await } else { std::future::pending::<()>().await } } => None,
            };
            let Some((index, mut job, outcome)) = completed else {
                continue;
            };
            active.remove(index);
            let (status, headers, bytes) = match outcome {
                Attempt::Headers(response) => {
                    let response = match response {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!(request_id=%job.request_id,resource=%job.resource,attempt=job.attempts,timed_out=e.is_timeout(),"GitHub transport attempt failed");
                            if Instant::now() >= job.deadline() {
                                self.finish(job, Err(Error::Deadline));
                                continue;
                            }
                            if job.attempts < self.config.max_attempts
                                && Instant::now() < job.deadline()
                            {
                                job.ready_at = Instant::now() + transient_backoff(job.attempts);
                                pending.push_back(job);
                            } else {
                                self.finish(
                                    job,
                                    Err(Error::Transport(e.without_url().to_string())),
                                );
                            }
                            continue;
                        }
                    };
                    let status = response.status();
                    job.http_status = Some(status.as_u16());
                    if status == StatusCode::NOT_MODIFIED {
                        self.metrics.not_modified.fetch_add(1, Ordering::Relaxed);
                    }
                    let headers = response.headers().clone();
                    tracing::info!(request_id=%job.request_id, attempt=job.attempts,
                        http_status=status.as_u16(),
                        remaining=number(&headers,"x-ratelimit-remaining"),
                        used=number(&headers,"x-ratelimit-used"),
                        limit=number(&headers,"x-ratelimit-limit"),
                        reset=number(&headers,"x-ratelimit-reset"),
                        "GitHub response headers");
                    if let Some(resource) = header(&headers, "x-ratelimit-resource") {
                        job.resource = resource;
                        if routes.len() >= 4096 {
                            routes.clear();
                        }
                        routes.insert(job.key.clone(), job.resource.clone());
                    }
                    if let (Some(remaining), Some(reset)) = (
                        number(&headers, "x-ratelimit-remaining"),
                        number(&headers, "x-ratelimit-reset"),
                    ) {
                        let limit = RateLimit {
                            remaining,
                            reset_at_seconds: reset,
                        };
                        self.metrics
                            .limits
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(job.resource.clone(), limit);
                        budgets.observe(
                            &job.resource,
                            remaining,
                            reset,
                            status == StatusCode::NOT_MODIFIED,
                        );
                        if job.resource == "core" {
                            self.metrics.core_spacing_ms.store(
                                budgets
                                    .for_resource("core")
                                    .map(|budget| budget.spacing.as_millis().min(30_000) as u64)
                                    .max()
                                    .unwrap_or(0),
                                Ordering::Relaxed,
                            );
                        }
                    }
                    if status == StatusCode::NOT_MODIFIED {
                        let result = if let Some(mut cached) = job.cached.clone() {
                            // Validators and pagination metadata may be updated on 304.
                            cached.etag = header(&headers, "etag").or(cached.etag);
                            cached.last_modified =
                                header(&headers, "last-modified").or(cached.last_modified);
                            cached.link = header(&headers, "link").or(cached.link);
                            self.store.revalidated(&self.scope, &job.key, cached).await
                        } else {
                            Err(Error::Invalid(
                                "GitHub returned 304 without a cached representation".into(),
                            ))
                        };
                        self.finish(job, result);
                        continue;
                    }
                    let retry = retry_after(&headers);
                    let exhausted = number(&headers, "x-ratelimit-remaining") == Some(0);
                    let header_limited = status == StatusCode::TOO_MANY_REQUESTS
                        || (status == StatusCode::FORBIDDEN && (exhausted || retry.is_some()));
                    // Pause traffic as soon as authoritative throttle headers are
                    // available, even if the error body is slow or oversized.
                    if header_limited {
                        let wait = if exhausted {
                            Duration::from_secs(
                                number(&headers, "x-ratelimit-reset").map_or(60, |reset| {
                                    reset.saturating_sub(now_ms() / 1000).saturating_add(1)
                                }),
                            )
                            .max(retry.unwrap_or_default())
                        } else {
                            retry.unwrap_or_else(|| {
                                Duration::from_secs(
                                    60 * 2u64.pow(job.attempts.saturating_sub(1).min(6)),
                                ) + jitter()
                            })
                        };
                        if exhausted {
                            budgets.exhausted(
                                &job.resource,
                                number(&headers, "x-ratelimit-reset").unwrap_or(0),
                                wait,
                            );
                            if let Some(retry) = retry {
                                secondary_until = secondary_until.max(quota_deadline(retry));
                            }
                        } else {
                            secondary_until = secondary_until.max(quota_deadline(wait));
                        }
                    }
                    let max_body_bytes = self.config.max_body_bytes;
                    active.push(Active {
                        resource: job.resource.clone(),
                        detail_lane: job.detail_lane,
                        future: Box::pin(async move {
                            let bytes = read_body(response, max_body_bytes).await;
                            (
                                job,
                                Attempt::Body {
                                    status,
                                    headers,
                                    bytes,
                                },
                            )
                        }),
                    });
                    continue;
                }
                Attempt::Body {
                    status,
                    headers,
                    bytes,
                } => (status, headers, bytes),
            };
            let retry = retry_after(&headers);
            let exhausted = number(&headers, "x-ratelimit-remaining") == Some(0);
            let header_limited = status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN && (exhausted || retry.is_some()));
            let bytes = match bytes {
                Ok(bytes) => bytes,
                Err(_) if header_limited => Vec::new(),
                Err(e) => {
                    self.finish(job, Err(e));
                    continue;
                }
            };
            let message = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
                .unwrap_or_else(|| {
                    status
                        .canonical_reason()
                        .unwrap_or("GitHub error")
                        .to_owned()
                });
            let lower = message.to_ascii_lowercase();
            let graphql_errors = if job.body.is_some() {
                serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|v| v.get("errors").and_then(|e| e.as_array()).cloned())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let graphql_limited = graphql_errors
                .iter()
                .any(|e| e.get("type").and_then(|t| t.as_str()) == Some("RATE_LIMITED"));
            let limited = graphql_limited
                || status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN
                    && (exhausted || retry.is_some() || lower.contains("rate limit")));
            if limited {
                tracing::warn!(request_id=%job.request_id,resource=%job.resource,status=status.as_u16(),attempt=job.attempts,primary_exhausted=exhausted,"GitHub rate limit observed");
                let reset_wait = number(&headers, "x-ratelimit-reset").map(|r| {
                    Duration::from_secs(r.saturating_sub(now_ms() / 1000).saturating_add(1))
                });
                let wait = if exhausted {
                    reset_wait
                        .unwrap_or(Duration::from_secs(60))
                        .max(retry.unwrap_or_default())
                } else {
                    retry.unwrap_or_else(|| {
                        Duration::from_secs(60 * 2u64.pow(job.attempts.saturating_sub(1).min(6)))
                            + jitter()
                    })
                };
                tracing::info!(request_id=%job.request_id,resource=%job.resource,retry_after_seconds=ceil_seconds(wait),"GitHub request cooldown scheduled");
                if exhausted {
                    budgets.exhausted(
                        &job.resource,
                        number(&headers, "x-ratelimit-reset").unwrap_or(0),
                        wait,
                    );
                    // Retry-After always pauses shared traffic, even when
                    // GitHub also reports an exhausted primary bucket.
                    if let Some(retry) = retry {
                        secondary_until = secondary_until.max(quota_deadline(retry));
                    }
                } else {
                    secondary_until = secondary_until.max(quota_deadline(wait));
                }
                if job.attempts >= self.config.max_attempts
                    || quota_deadline(wait) >= job.deadline()
                {
                    self.finish(
                        job,
                        Err(Error::RateLimited {
                            retry_after_seconds: ceil_seconds(wait),
                        }),
                    );
                } else {
                    job.ready_at = quota_deadline(wait);
                    pending.push_back(job);
                }
                continue;
            }
            if status.is_server_error() && job.attempts < self.config.max_attempts {
                tracing::warn!(request_id=%job.request_id,resource=%job.resource,status=status.as_u16(),attempt=job.attempts,"GitHub server error; retry scheduled");
                job.ready_at =
                    quota_deadline(transient_backoff(job.attempts).max(retry.unwrap_or_default()));
                pending.push_back(job);
                continue;
            }
            if !graphql_errors.is_empty() {
                let access_denied = graphql_errors.iter().any(|error| {
                    matches!(
                        error.get("type").and_then(|value| value.as_str()),
                        Some("FORBIDDEN" | "UNAUTHORIZED" | "UNAUTHENTICATED")
                    )
                });
                tracing::warn!(request_id=%job.request_id,resource=%job.resource,status=status.as_u16(),access_denied,error_count=graphql_errors.len(),"GitHub GraphQL operation failed");
                let message = format!(
                    "GitHub GraphQL returned errors: {}",
                    serde_json::Value::Array(graphql_errors)
                );
                let error = if status.is_success() {
                    Error::GraphQL {
                        message,
                        access_denied,
                    }
                } else {
                    Error::GitHub {
                        status: status.as_u16(),
                        message,
                    }
                };
                self.finish(job, Err(error));
                continue;
            }
            if !status.is_success() {
                self.finish(
                    job,
                    Err(Error::GitHub {
                        status: status.as_u16(),
                        message,
                    }),
                );
                continue;
            }
            let data = if bytes.is_empty() {
                Ok(serde_json::Value::Null)
            } else {
                serde_json::from_slice(&bytes)
                    .map_err(|e| Error::Invalid(format!("invalid GitHub JSON: {e}")))
            };
            let result = match data {
                Ok(data) => {
                    let stamp = now_ms();
                    let response = Response {
                        data,
                        fetched_at_ms: stamp,
                        validated_at_ms: stamp,
                        source: Source::Network,
                        etag: header(&headers, "etag"),
                        last_modified: header(&headers, "last-modified"),
                        link: header(&headers, "link"),
                    };
                    self.store
                        .put(&self.scope, &job.key, &response)
                        .await
                        .map(|_| response)
                }
                Err(e) => Err(e),
            };
            self.finish(job, result);
        }
    }

    fn finish(&self, job: Job, result: Result<Response>) {
        let source = match result.as_ref().map(|response| &response.source) {
            Ok(Source::Network) => "network",
            Ok(Source::Revalidated) => "revalidated",
            Ok(Source::Cache) => "cache",
            Err(_) => "error",
        };
        tracing::info!(request_id=%job.request_id,endpoint=job.endpoint,resource=%job.resource,attempts=job.attempts,succeeded=result.is_ok(),http_status=job.http_status,source,error_code=result.as_ref().err().map(Error::diagnostic_code),elapsed_ms=job.queued_at.elapsed().as_millis() as u64,"GitHub request finished");
        let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        drop(job._permit);
        job.notify.send_replace(Some(result.map(Arc::new)));
        inflight.remove(&job.key);
    }
}

fn ready(job: &Job, budgets: &Budgets, global: Instant) -> Instant {
    job.ready_at.max(global).max(
        budgets
            .for_resource(&job.resource)
            .map(|budget| {
                if conditional_budget_exempt(job, budget) {
                    job.ready_at
                } else {
                    budget.next
                }
            })
            .max()
            .unwrap_or(job.ready_at),
    )
}

fn conditional_budget_exempt(job: &Job, budget: &Budget) -> bool {
    // A validator alone does not predict a free response: some endpoints return
    // 200 on every poll. Only previously unchanged representations may probe
    // without pacing. A changed response revokes that exemption automatically.
    job.body.is_none()
        && budget.remaining > QUOTA_RESERVE
        && job.cached.as_ref().is_some_and(|cached| {
            matches!(cached.source, Source::Revalidated)
                && (cached.etag.is_some() || cached.last_modified.is_some())
        })
}

pub(crate) fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_owned)
}
fn number(headers: &HeaderMap, name: &str) -> Option<u64> {
    header(headers, name)?.parse().ok()
}
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = header(headers, "retry-after")?;
    if let Ok(seconds) = value.parse() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(&value)
        .ok()
        .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
}
fn quota_deadline(wait: Duration) -> Instant {
    // An unrepresentable server hint cannot panic the scheduler. No admitted
    // request can wait beyond a day, so all affected jobs still fail closed.
    Instant::now()
        .checked_add(wait)
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400))
}
fn jitter() -> Duration {
    Duration::from_millis(fastrand::u64(0..250))
}
fn transient_backoff(attempt: u32) -> Duration {
    Duration::from_secs(2u64.pow(attempt.saturating_sub(1).min(6))) + jitter()
}
fn ceil_seconds(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() > 0))
}

async fn read_body(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|len| len > max as u64)
    {
        return Err(Error::Invalid(
            "GitHub response exceeds configured body limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::Transport(e.without_url().to_string()))?
    {
        if chunk.len() > max.saturating_sub(bytes.len()) {
            return Err(Error::Invalid(
                "GitHub response exceeds configured body limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn three_consumers_share_a_full_quota_window_with_preflight_headroom() {
        for direct_interval in [None, Some(3000)] {
            let start = Instant::now();
            let mut clients: Vec<_> = (0..3)
                .map(|_| (SharedUsage::new(5000, start), 0u64, 0u64))
                .collect();
            let mut remaining = 5000;
            // Same busy workload, three independent clients, one actual shared
            // quota. This exercises production pacing with a deterministic clock.
            for ms in (0..3_600_000).step_by(10) {
                let now = start + Duration::from_millis(ms);
                if direct_interval.is_some_and(|interval| ms % interval == 0) {
                    remaining -= 1; // Coordinator/CLI reads outside every scheduler.
                }
                for (usage, next, calls) in &mut clients {
                    if ms < *next || remaining == 0 {
                        continue;
                    }
                    remaining -= 1;
                    *calls += 1;
                    usage.observe(remaining, true, now);
                    *next = ms
                        + usage
                            .spacing(remaining, (3_600_000 - ms).div_ceil(1000))
                            .as_millis() as u64;
                }
                // Independent in-flight requests may cross the reserve together.
                assert!(
                    remaining >= 50,
                    "shared allowance depleted at {ms}ms: {remaining}"
                );
            }
            assert!(
                remaining < 300,
                "use available capacity throughout the window"
            );
            assert!(
                clients.iter().all(|(_, _, calls)| *calls > 1000),
                "all consumers retain refresh coverage"
            );
        }
    }

    #[test]
    fn shared_usage_counts_charged_responses_and_ignores_free_validations() {
        let start = Instant::now();
        let mut usage = SharedUsage::new(5000, start);
        for i in 1..=10 {
            let now = start + Duration::from_secs(i * 3);
            usage.observe(5000 - i * 3, false, now - Duration::from_millis(1));
            usage.observe(5000 - i * 3, true, now);
        }
        assert_eq!(usage.share, 3.0);
        assert_eq!(usage.spacing(1000, 900), Duration::from_secs(3));
        assert_eq!(usage.spacing(100, 900), Duration::from_secs(901));
        assert_eq!(usage.spacing(101, u64::MAX), Duration::from_secs(86400));
        usage.observe(1000, true, start + Duration::from_secs(300));
        assert_eq!(
            usage.share, 1.0,
            "an idle consumer can resume interactive work"
        );
    }

    #[test]
    fn quota_windows_keep_the_lowest_remaining_and_expire_independently() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 100, reset, false);
        budgets.observe("core", 5000, reset, true);
        let budget = budgets.for_resource("core").next().unwrap();
        assert_eq!(budget.remaining, 100);
        assert!(budget.spacing > Duration::from_secs(35));
        budgets.observe("core", 5000, reset + 60, true);
        assert_eq!(budgets.for_resource("core").count(), 2);
        // Simulate expiry without sleeping or restarting the scheduler. Only
        // that window's reservation may be forgotten by the next observation.
        let mut expired = budgets.0.get_mut("core").unwrap().remove(&reset).unwrap();
        expired.reset_at_seconds = 1;
        budgets.0.get_mut("core").unwrap().insert(1, expired);
        budgets.observe("core", 4999, reset + 60, false);
        assert_eq!(budgets.for_resource("core").count(), 1);
        assert_eq!(budgets.for_resource("core").next().unwrap().remaining, 4999);
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "17".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(17)));
        let date = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(60));
        headers.insert("retry-after", date.parse().unwrap());
        assert!((58..=60).contains(&retry_after(&headers).unwrap().as_secs()));
        headers.insert("retry-after", "invalid".parse().unwrap());
        assert_eq!(retry_after(&headers), None);
    }
}
