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

pub(crate) type SharedResult = Option<Result<Arc<Response>>>;
pub(crate) type Inflight =
    Arc<Mutex<HashMap<String, (watch::Receiver<SharedResult>, Arc<AtomicBool>)>>>;

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
    pub cache_hits: AtomicU64,
    pub coalesced: AtomicU64,
    pub network: AtomicU64,
    pub conditional: AtomicU64,
    pub not_modified: AtomicU64,
    pub active: AtomicU64,
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
    pub deadline: Instant,
    pub ready_at: Instant,
    pub attempts: u32,
    pub resource: String,
    pub _permit: OwnedSemaphorePermit,
}

// Two ordinary quota buckets and one REST detail lane can make progress.
// Details share core quota, but cannot hold its lifecycle/CI socket. Body reads
// retain their lane; headers reach the scheduler before any body wait so quota
// exhaustion and shared cooldowns take effect immediately.
struct Active {
    resource: String,
    detail_lane: bool,
    future: Pin<Box<dyn Future<Output = (Job, Attempt)> + Send>>,
}

fn lane_busy(active: &[Active], job: &Job) -> bool {
    if job.detail_lane {
        active.iter().any(|attempt| attempt.detail_lane)
    } else {
        active.iter().filter(|attempt| !attempt.detail_lane).count() >= 2
            || active
                .iter()
                .any(|attempt| !attempt.detail_lane && attempt.resource == job.resource)
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
        let mut pending = VecDeque::<Job>::new();
        let mut interactive_streaks = HashMap::<String, [usize; 2]>::new();
        let mut budgets = HashMap::<String, Budget>::new();
        let mut routes = HashMap::<String, String>::new();
        let mut global_next = Instant::now();
        let mut secondary_until = Instant::now();
        let mut active = Vec::<Active>::new();
        let max_active = self.config.queue_capacity.min(MAX_ACTIVE_BUCKETS);
        loop {
            self.metrics
                .active
                .store(active.len() as u64, Ordering::Relaxed);
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
                j.deadline <= now
                    || ready(j, &budgets, global_next.max(secondary_until)) >= j.deadline
            }) {
                let job = pending.remove(index).expect("existing queue entry");
                let ready = ready(&job, &budgets, global_next.max(secondary_until));
                let quota_blocked = secondary_until > now
                    || budgets
                        .get(&job.resource)
                        .is_some_and(|b| b.next > now && !conditional_budget_exempt(&job, b));
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
                let eligible =
                    |job: &Job| ready(job, &budgets, global) <= now && !lane_busy(&active, job);
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
                let streak = interactive_streaks.entry(job.resource.clone()).or_default();
                let streak = &mut streak[usize::from(job.detail_lane)];
                *streak = if job.interactive.load(Ordering::Relaxed) {
                    streak.saturating_add(1)
                } else {
                    0
                };
                job.attempts += 1;
                job.http_status = None;
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
                        .min(job.deadline.saturating_duration_since(now)),
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
                    if active.len() >= max_active || lane_busy(&active, job) {
                        // Busy lanes wake on completion; never spin on their old
                        // ready time. Their queued deadlines still expire on time.
                        job.deadline
                    } else {
                        ready(job, &budgets, global).min(job.deadline)
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
                            if Instant::now() >= job.deadline {
                                self.finish(job, Err(Error::Deadline));
                                continue;
                            }
                            if job.attempts < self.config.max_attempts
                                && Instant::now() < job.deadline
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
                        let seconds = reset.saturating_sub(now_ms() / 1000);
                        let wait = if remaining == 0 {
                            Duration::from_secs(seconds.saturating_add(1))
                        } else {
                            Duration::from_secs_f64(seconds as f64 / (remaining as f64 + 1.0))
                        };
                        // Authenticated REST 304 validations do not consume primary
                        // quota. Preserve pacing debt from the last charged result.
                        let next = if status == StatusCode::NOT_MODIFIED && remaining > 0 {
                            budgets
                                .get(&job.resource)
                                .map_or(Instant::now(), |budget| budget.next)
                        } else {
                            quota_deadline(wait)
                        };
                        budgets.insert(job.resource.clone(), Budget { next, remaining });
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
                            budgets.insert(
                                job.resource.clone(),
                                Budget {
                                    next: quota_deadline(wait),
                                    remaining: 0,
                                },
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
                    budgets.insert(
                        job.resource.clone(),
                        Budget {
                            next: quota_deadline(wait),
                            remaining: 0,
                        },
                    );
                    // Retry-After always pauses shared traffic, even when
                    // GitHub also reports an exhausted primary bucket.
                    if let Some(retry) = retry {
                        secondary_until = secondary_until.max(quota_deadline(retry));
                    }
                } else {
                    secondary_until = secondary_until.max(quota_deadline(wait));
                }
                if job.attempts >= self.config.max_attempts || quota_deadline(wait) >= job.deadline
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

fn ready(job: &Job, budgets: &HashMap<String, Budget>, global: Instant) -> Instant {
    job.ready_at
        .max(global)
        .max(budgets.get(&job.resource).map_or(job.ready_at, |budget| {
            if conditional_budget_exempt(job, budget) {
                job.ready_at
            } else {
                budget.next
            }
        }))
}

fn conditional_budget_exempt(job: &Job, budget: &Budget) -> bool {
    // Conditional requests can still return changed data (200). Keep a reserve
    // before relaxing soft pacing; never bypass exhaustion or shared cooldowns.
    job.body.is_none()
        && budget.remaining > 100
        && job
            .cached
            .as_ref()
            .is_some_and(|cached| cached.etag.is_some() || cached.last_modified.is_some())
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
