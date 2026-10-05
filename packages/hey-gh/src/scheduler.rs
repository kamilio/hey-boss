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

#[derive(Clone)]
pub(crate) enum SharedResult {
    Queued,
    /// Known pacing/backoff lower bound; not a dispatch promise or new deadline.
    QueuedUntil(Instant),
    Active,
    Complete(Result<Arc<Response>>),
}
pub(crate) type Inflight = Arc<
    Mutex<
        HashMap<
            String,
            (
                watch::Receiver<SharedResult>,
                Arc<AtomicBool>,
                Arc<Mutex<Instant>>,
                Arc<AtomicBool>,
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
    pub limits: Mutex<BTreeMap<String, RateLimit>>,
}

pub(crate) struct Job {
    pub completion_validation: Arc<AtomicBool>,
    pub installation: bool,
    pub minting: bool,
    pub auth_attempts: u32,
    pub auth_generation: u64,
    // Shared with coalesced readers so interactive use can promote queued work.
    pub interactive: Arc<AtomicBool>,
    // Report-lock waiters promote the owner's entire collection independently
    // of callers joining just one of its shared requests.
    pub report_priority: Arc<AtomicBool>,
    pub detail_lane: bool,
    pub collection_slice: bool,
    /// Random per-job correlation, independent of credentials and request data.
    pub request_id: String,
    /// Fixed endpoint class; never contains caller-controlled request data.
    pub endpoint: &'static str,
    pub queued_at: Instant,
    pub http_status: Option<u16>,
    // Headers and body belong to one throttle observation, even for slow bodies.
    pub secondary_retry_at: Option<Instant>,
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
    fn interactive(&self) -> bool {
        self.interactive.load(Ordering::Relaxed) || self.report_priority.load(Ordering::Relaxed)
    }

    fn background_collection(&self) -> bool {
        self.collection_slice && !self.interactive()
    }

    fn quota(&self) -> String {
        if self.installation {
            format!("installation/{}", self.resource)
        } else {
            self.resource.clone()
        }
    }
    pub(crate) fn deadline(&self) -> Instant {
        *self.deadline.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct Active {
    resource: String,
    detail_lane: bool,
    reservation: Option<Reservation>,
    probe: Option<ProbeTurn>,
    future: Pin<Box<dyn Future<Output = (Job, Attempt)> + Send>>,
}

// A known unchanged validator may borrow a paced turn's wait. If it returns a
// charged response, repay the full interval from the existing future slot,
// rather than replacing that debt with an interval starting at the response.
struct PacingProbe {
    quota: String,
    windows: Vec<(u64, Instant)>,
}

struct ProbeTurn {
    quota: String,
    owed: String,
}

struct Reservation {
    resource: String,
    resets: Vec<(u64, bool)>,
    dispatched_at: Instant,
}

#[derive(Clone, Copy)]
struct ReservedWindow {
    dispatched_at: Instant,
    waited_for_quota: bool,
}

impl Reservation {
    fn for_window(&self, resource: &str, reset: u64) -> Option<ReservedWindow> {
        if self.resource != resource {
            return None;
        }
        self.resets
            .iter()
            .find(|(window, _)| *window == reset)
            .map(|(_, waited)| ReservedWindow {
                dispatched_at: self.dispatched_at,
                waited_for_quota: *waited,
            })
    }
}

fn shared_quota(resource: &str) -> bool {
    matches!(resource.rsplit('/').next(), Some("core" | "graphql"))
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
        bytes: std::result::Result<Vec<u8>, BodyError>,
    },
}

enum BodyError {
    Transport(reqwest::Error),
    TooLarge,
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

// A scan can abandon a throttled job and later create a different one. Keep
// escalation with the shared cooldown, rather than that job's retry counter.
struct SecondaryBackoff {
    until: Instant,
    episodes: u32,
}

impl SecondaryBackoff {
    fn new() -> Self {
        Self {
            until: Instant::now(),
            episodes: 0,
        }
    }

    fn extend(&mut self, wait: Duration) {
        self.until = self.until.max(quota_deadline(wait));
    }

    fn observe(&mut self, retry: Option<Duration>) -> Instant {
        let now = Instant::now();
        if now < self.until {
            // In-flight siblings can report the same episode. Honor a longer
            // server hint without multiplying backoff for each response.
            if let Some(wait) = retry {
                self.extend(wait);
            }
            return self.until;
        }
        if now.duration_since(self.until) >= Duration::from_secs(15 * 60) {
            self.episodes = 0;
        }
        let fallback =
            Duration::from_secs((60 * 2u64.pow(self.episodes.min(4))).min(15 * 60)) + jitter();
        let wait = if self.episodes == 0 {
            retry.unwrap_or(fallback)
        } else {
            retry.unwrap_or_default().max(fallback)
        };
        self.episodes = self.episodes.saturating_add(1);
        self.extend(wait);
        self.until
    }
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

    fn observe(&mut self, remaining: u64, charged: bool, now: Instant, waited_for_quota: bool) {
        self.charged += u64::from(charged);
        let elapsed = now.duration_since(self.since);
        if elapsed >= Duration::from_secs(30) {
            // Response counts are an estimate of this client's charges, not an
            // attribution of all account usage. The header delta includes other
            // daemons and direct CLI traffic using the same shared allowance.
            // Sparse/idle observations cannot estimate sustained local demand.
            // Don't punish the first interactive read after an idle period with
            // an entire interval of unrelated account traffic.
            // A request already queued before its quota slot was available is
            // sustained demand, even if pacing allows fewer than four replies
            // per minute. Resetting that sample to one restarts a burst that
            // exhausts shared or multi-point GraphQL budgets early.
            if self.charged > 0
                && (waited_for_quota || (self.charged >= 4 && elapsed <= Duration::from_secs(60)))
            {
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
    fn probe(&self, quota: &str) -> PacingProbe {
        PacingProbe {
            quota: quota.to_owned(),
            windows: self
                .for_resource(quota)
                .map(|budget| (budget.reset_at_seconds, budget.next))
                .collect(),
        }
    }

    fn charge_probe(&mut self, probe: &PacingProbe) {
        let now = Instant::now();
        if let Some(windows) = self.0.get_mut(&probe.quota) {
            for (reset, prior_slot) in &probe.windows {
                if let Some(budget) = windows.get_mut(reset) {
                    let debt = now
                        .max(*prior_slot)
                        .checked_add(budget.spacing)
                        .unwrap_or_else(|| quota_deadline(Duration::from_secs(86400)));
                    budget.next = budget.next.max(debt);
                }
            }
        }
    }

    fn for_resource(&self, resource: &str) -> impl Iterator<Item = &Budget> {
        let seconds_now = now_ms() / 1000;
        self.0
            .get(resource)
            .into_iter()
            .flat_map(|windows| windows.values())
            // Queued work must recover at reset even when no new response can
            // arrive to prune the window in observe(). Unknown reset times
            // retain their explicit cooldown; shared Retry-After is separate.
            .filter(move |budget| {
                budget.reset_at_seconds == 0
                    || budget.reset_at_seconds.saturating_add(1) > seconds_now
            })
    }

    fn observe(
        &mut self,
        resource: &str,
        remaining: u64,
        reset: u64,
        unchanged: bool,
        reservation: Option<ReservedWindow>,
    ) {
        let now = Instant::now();
        let seconds_now = now_ms() / 1000;
        let windows = self.0.entry(resource.to_owned()).or_default();
        windows.retain(|reset, budget| {
            reset.saturating_add(1) > seconds_now || (*reset == 0 && budget.next > now)
        });
        // Every live window paces the entire resource in reserve(), even when
        // this response reports another window. Use that same population for
        // the local charge estimate; a rare window must not multiply the wait
        // on all requests by counting only its own responses. Free validations
        // and other resources do not contribute, and each header delta stays
        // scoped to its window.
        if !unchanged {
            for (other_reset, budget) in windows.iter_mut() {
                if *other_reset != reset {
                    budget.usage.charged += 1;
                }
            }
        }
        let previous = windows.remove(&reset);
        // Parallel responses and cached upstream headers may arrive out of order.
        // Only expiry, never a higher header in a live window, restores capacity.
        let remaining = previous
            .as_ref()
            .map_or(remaining, |b| b.remaining.min(remaining));
        let mut usage = SharedUsage::new(remaining, now);
        if let Some(previous) = &previous {
            usage = previous.usage.clone();
            usage.observe(
                remaining,
                !unchanged,
                now,
                reservation.is_some_and(|r| r.waited_for_quota),
            );
        }
        let seconds = reset.saturating_sub(seconds_now);
        let spacing = if remaining == 0 {
            Duration::from_secs(seconds.saturating_add(1))
        } else if shared_quota(resource) {
            usage.spacing(remaining, seconds)
        } else {
            Duration::from_secs_f64(seconds as f64 / (remaining as f64 + 1.0))
        };
        let next = if unchanged && remaining > 0 {
            previous.as_ref().map_or(now, |b| b.next)
        } else {
            // A charged dispatch already reserved this window's pacing slot.
            // Revise its spacing from dispatch, without charging header latency
            // again or erasing later reservations. Unreserved probes/new windows
            // still start pacing at observation. Exhaustion always waits to reset.
            let anchor = reservation
                .filter(|_| remaining > 0 && (!shared_quota(resource) || remaining > QUOTA_RESERVE))
                .map(|r| r.dispatched_at)
                .unwrap_or(now);
            let deadline = anchor
                .checked_add(spacing)
                .unwrap_or_else(|| quota_deadline(Duration::from_secs(86400)));
            previous.as_ref().map_or(deadline, |b| b.next.max(deadline))
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

    fn reserve(&mut self, job: &Job) -> Reservation {
        let mut reservation = Reservation {
            resource: job.quota(),
            resets: Vec::new(),
            dispatched_at: Instant::now(),
        };
        if let Some(windows) = self.0.get_mut(&job.quota()) {
            for budget in windows.values_mut() {
                if budget.remaining > 0
                    && budget.reset_at_seconds > now_ms() / 1000
                    && !conditional_budget_exempt(job, budget)
                {
                    reservation
                        .resets
                        .push((budget.reset_at_seconds, job.queued_at < budget.next));
                    budget.next = quota_deadline(budget.spacing);
                }
            }
        }
        reservation
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
    pub changed: Arc<tokio::sync::Notify>,
}

impl Scheduler {
    fn transport_failed(&self, mut job: Job, error: reqwest::Error, pending: &mut VecDeque<Job>) {
        if job.minting {
            let error = if error.is_timeout() {
                Error::Deadline
            } else {
                Error::Transport("GitHub App token exchange failed".into())
            };
            self.config
                .installation
                .as_ref()
                .unwrap()
                .failed(error.clone());
            self.finish(job, Err(error));
            return;
        }
        tracing::warn!(request_id=%job.request_id,resource=%job.resource,attempt=job.attempts,
            timed_out=error.is_timeout(),response_body=job.http_status.is_some(),
            "GitHub transport attempt failed");
        if Instant::now() >= job.deadline()
            || self.abandoned_request(&job)
            || (error.is_timeout() && job.background_collection())
        {
            self.finish(job, Err(Error::Deadline));
            return;
        }
        // Only replay reads. A known permanent HTTP failure is not made
        // retryable by a broken body; quota/cooldown headers still gate every
        // queued attempt. Partial bytes and their validators are discarded.
        let retryable_status = job
            .http_status
            .is_none_or(|status| (200..300).contains(&status) || (500..600).contains(&status));
        if retryable_status && job.attempts < self.config.max_attempts {
            job.ready_at = job
                .ready_at
                .max(Instant::now() + transient_backoff(job.attempts));
            job.notify.send_replace(SharedResult::Queued);
            pending.push_back(job);
        } else {
            self.finish(job, Err(Error::Transport(error.without_url().to_string())));
        }
    }

    fn abandoned_request(&self, job: &Job) -> bool {
        // Admission inserts the registry receiver while holding this lock.
        // Do not mistake a newly sent job for one whose last caller left.
        // HTTP caller cancellation keeps the daemon handler warming its cache;
        // once that handler's report expires, unobserved queued work is waste.
        // Active responses still finish, and coalesced callers retain their turn.
        let _inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        job.notify.receiver_count() <= 1
    }

    pub async fn run(self, mut rx: mpsc::Receiver<Job>) {
        // Random process-local identifier; scope is already an opaque auth hash.
        let instance = format!("{:032x}", fastrand::u128(..));
        let mut pending = VecDeque::<Job>::new();
        let mut interactive_streaks = HashMap::<String, usize>::new();
        let mut completion_yields = std::collections::HashSet::<(String, bool)>::new();
        let mut blocked_probes = HashMap::<String, String>::new();
        let mut budgets = Budgets::default();
        let mut routes = HashMap::<String, String>::new();
        let mut global_next = Instant::now();
        let mut secondary = SecondaryBackoff::new();
        let mut minting = false;
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
            // Only the exact borrowed turn can renew this allowance. A stream
            // of completion checks must not keep lending the same older read's
            // slot to changed probes. Cancellation/expiry also releases it.
            blocked_probes.retain(|_, owed| pending.iter().any(|job| job.request_id == *owed));
            let quota_blocked = |job: &Job| {
                secondary.until > now
                    || budgets.for_resource(&job.quota()).any(|budget| {
                        budget.next > now
                            && (budget.remaining == 0
                                || (shared_quota(&job.resource)
                                    && budget.remaining <= QUOTA_RESERVE))
                    })
            };
            // Expiry is independent of quota availability, including exhausted
            // buckets whose next reset might be an hour away.
            // Ordinary pacing is not a throttle. Keep that work queued until
            // its actual deadline: a coalescing reader may extend it, and early
            // rejection can otherwise fail an entire account's remaining rows
            // in the last pacing interval of a background cycle.
            // Collection work also waits out hard quota/cooldowns within that
            // deadline, keeping untouched PRs queued instead of sweeping the
            // roster with local failures. Other callers retain prompt feedback.
            if let Some(index) = pending.iter().position(|j| {
                self.abandoned_request(j)
                    || j.deadline() <= now
                    || (!j.background_collection()
                        && quota_blocked(j)
                        && ready(j, &budgets, global_next.max(secondary.until)) >= j.deadline())
            }) {
                let job = pending.remove(index).expect("existing queue entry");
                let ready = ready(&job, &budgets, global_next.max(secondary.until));
                let error = if quota_blocked(&job)
                    && !job.background_collection()
                    && !self.abandoned_request(&job)
                {
                    Error::RateLimited {
                        retry_after_seconds: ceil_seconds(ready.saturating_duration_since(now)),
                    }
                } else {
                    Error::Deadline
                };
                self.finish(job, Err(error));
                continue;
            }
            let global = global_next.max(secondary.until);
            // Optional GraphQL selectors have a short REST-fallback budget.
            // Tell waiters when known pacing alone exceeds that budget instead
            // of making them wait out a shortcut that cannot dispatch in time.
            // Ordinary/coalesced callers still wait; no quota or turn changes.
            for job in pending.iter().filter(|job| job.body.is_some()) {
                let until = ready(job, &budgets, global);
                if until > now {
                    job.notify.send_if_modified(|state| {
                        if matches!(state, SharedResult::Queued)
                            || matches!(state, SharedResult::QueuedUntil(previous) if *previous != until)
                        {
                            *state = SharedResult::QueuedUntil(until);
                            true
                        } else {
                            false
                        }
                    });
                }
            }
            // Choose each quota's turn before considering pacing. A charged
            // conditional probe must not keep moving an older turn forever.
            // Keep foreground/background and completion/ordinary alternation;
            // ties retain queue order. Busy lanes and retry backoffs do not
            // reserve a turn, so unrelated work can still use its own capacity.
            let mut turns = HashMap::new();
            for (index, job) in pending.iter().enumerate() {
                if job.ready_at > now
                    || (job.installation && minting)
                    || lane_busy(&active, job, prod)
                {
                    continue;
                }
                let interactive = job.interactive();
                let quota = job.quota();
                let completing = job.completion_validation.load(Ordering::Relaxed);
                let priority = (
                    interactive != (interactive_streaks.get(&quota).copied().unwrap_or(0) < 3),
                    completing == completion_yields.contains(&(quota.clone(), interactive)),
                );
                let turn = turns.entry(quota).or_insert((priority, index));
                if priority < turn.0 {
                    *turn = (priority, index);
                }
            }
            let probing_quotas: std::collections::HashSet<_> = active
                .iter()
                .filter_map(|attempt| attempt.probe.as_ref().map(|probe| probe.quota.clone()))
                .collect();
            let can_probe = |index: usize, job: &Job| {
                let quota = job.quota();
                let Some((_, selected)) = turns.get(&quota) else {
                    return false;
                };
                let turn = &pending[*selected];
                index != *selected
                    && !blocked_probes.contains_key(&quota)
                    && !probing_quotas.contains(&quota)
                    && ((job.interactive() == turn.interactive()
                        && job.completion_validation.load(Ordering::Relaxed) == turn.completion_validation.load(Ordering::Relaxed))
                        // Foreground validators may use an owed background
                        // turn's pacing wait. The turn and its one-probe debt
                        // remain owned by that exact background request.
                        || (job.interactive()
                            && !turn.interactive()))
                    // The class's turn is held by soft pacing, not by a retry,
                    // socket or global spacing.
                    && ready(turn, &budgets, now) > now
                    && budgets.for_resource(&quota).next().is_some()
                    && budgets.for_resource(&quota).all(|budget| {
                        conditional_budget_exempt(job, budget)
                            // Leave a charged slot for the turn being borrowed.
                            && budget.remaining > QUOTA_RESERVE + 1
                    })
            };
            let waiting_for_turn = |index: usize, job: &Job| {
                job.ready_at <= now
                    && turns
                        .get(&job.quota())
                        .is_some_and(|(_, selected)| *selected != index)
                    && !can_probe(index, job)
            };
            let next = {
                let eligible = |index: usize, job: &Job| {
                    ready(job, &budgets, global) <= now
                        && !(job.installation && minting)
                        && !lane_busy(&active, job, prod)
                        && !waiting_for_turn(index, job)
                };
                // Prefer interactive policy, but owe the background a turn
                // after three foreground turns in the same quota. Validators
                // can borrow its pacing wait; one changed probe closes that
                // allowance and repays its spacing before the owed job runs.
                // CI and details spend the same core allowance despite using
                // separate socket lanes. GraphQL keeps its own counter.
                // Quotas, lane limits,
                // retries, cooldowns and expiry remain unchanged.
                let preferred = |index: usize, job: &Job| {
                    eligible(index, job)
                        && job.interactive()
                            == (interactive_streaks.get(&job.quota()).copied().unwrap_or(0) < 3)
                };
                let completing = |job: &Job| job.completion_validation.load(Ordering::Relaxed);
                pending
                    .iter()
                    .enumerate()
                    .find(|(index, job)| preferred(*index, job) && completing(job))
                    .or_else(|| {
                        pending
                            .iter()
                            .enumerate()
                            .find(|(index, job)| preferred(*index, job))
                    })
                    .or_else(|| {
                        pending
                            .iter()
                            .enumerate()
                            .find(|(index, job)| eligible(*index, job) && completing(job))
                    })
                    .or_else(|| {
                        pending
                            .iter()
                            .enumerate()
                            .find(|(index, job)| eligible(*index, job))
                    })
                    .map(|(index, _)| index)
            };
            if active.len() < max_active
                && let Some(index) = next
            {
                let probe = can_probe(index, &pending[index]).then(|| {
                    let quota = pending[index].quota();
                    let owed = pending[turns[&quota].1].request_id.clone();
                    ProbeTurn { quota, owed }
                });
                let mut job = pending.remove(index).expect("existing queue entry");
                let token = if job.installation {
                    match self
                        .config
                        .installation
                        .as_ref()
                        .expect("configured installation")
                        .token()
                    {
                        Ok(Some((token, generation))) => {
                            job.auth_generation = generation;
                            token
                        }
                        Ok(None) => {
                            job.minting = true;
                            String::new()
                        }
                        Err(error) => {
                            self.finish(job, Err(error));
                            continue;
                        }
                    }
                } else {
                    self.token.clone()
                };
                // Reserve every live window before another socket can dispatch.
                let reservation = (!job.minting).then(|| budgets.reserve(&job));
                let probe = probe.filter(|_| !job.minting);
                // A borrowed wait is not a new scheduling turn. Advancing the
                // priority counters here can replace its owed request and
                // renew speculative borrowing before the debt is repaid.
                if probe.is_none() {
                    let streak = interactive_streaks.entry(job.quota()).or_default();
                    *streak = if job.interactive() {
                        streak.saturating_add(1)
                    } else {
                        0
                    };
                }
                if !job.minting && probe.is_none() {
                    let class = (job.quota(), job.interactive());
                    if job.completion_validation.load(Ordering::Relaxed) {
                        completion_yields.insert(class);
                    } else {
                        completion_yields.remove(&class);
                    }
                }
                if job.minting {
                    job.auth_attempts += 1;
                    minting = true;
                } else {
                    job.attempts += 1;
                }
                job.http_status = None;
                job.secondary_retry_at = None;
                job.notify.send_replace(SharedResult::Active);
                let quota = job.quota();
                let pacing = budgets
                    .for_resource(&quota)
                    .filter(|_| !job.minting)
                    .max_by_key(|budget| budget.spacing);
                tracing::info!(request_id=%job.request_id, attempt=job.attempts + job.auth_attempts,
                    endpoint=if job.minting { "app_token" } else { job.endpoint }, resource=if job.minting { "app_auth" } else { job.resource.as_str() },
                    foreground=job.interactive(),
                    completion_validation=job.completion_validation.load(Ordering::Relaxed),
                    pacing_probe=probe.is_some(),
                    conditional=!job.minting && job.body.is_none() && job.cached.as_ref().is_some_and(|c| c.etag.is_some() || c.last_modified.is_some()),
                    auth_scope=%if job.installation { self.config.installation.as_ref().unwrap().scope() } else { &self.scope }, %instance, request_key=%crate::digest(&job.key),
                    // Total job age; on the first attempt this is queue time.
                    elapsed_ms=job.queued_at.elapsed().as_millis() as u64,
                    pacing_ms=pacing.map(|budget| budget.spacing.as_millis() as u64),
                    pacing_reset=pacing.map(|budget| budget.reset_at_seconds),
                    pacing_share=pacing.map(|budget| budget.usage.share),
                    "GitHub request dispatched");
                self.metrics.network.fetch_add(1, Ordering::Relaxed);
                let mut request = if job.minting {
                    match self
                        .config
                        .installation
                        .as_ref()
                        .unwrap()
                        .request(&self.http, &self.config.rest_url)
                    {
                        Ok(request) => request,
                        Err(error) => {
                            minting = false;
                            self.config
                                .installation
                                .as_ref()
                                .unwrap()
                                .failed(error.clone());
                            self.finish(job, Err(error));
                            continue;
                        }
                    }
                } else if let Some(body) = &job.body {
                    self.http.post(&job.url).json(body).bearer_auth(&token)
                } else {
                    self.http.get(&job.url).bearer_auth(&token)
                }
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", &self.config.api_version)
                .timeout(
                    self.config
                        .request_timeout
                        .min(if job.background_collection() {
                            crate::collection_budget::STALL_LIMIT
                        } else {
                            self.config.request_timeout
                        })
                        .min(job.deadline().saturating_duration_since(now)),
                );
                if let Some(cache) = &job.cached
                    && job.body.is_none()
                    && !job.minting
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
                    reservation,
                    probe,
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
                .enumerate()
                .map(|(index, job)| {
                    if active.len() >= max_active
                        || (job.installation && minting)
                        || lane_busy(&active, job, prod)
                        || waiting_for_turn(index, job)
                    {
                        // A completion or the background timer wakes held work;
                        // never spin on its old ready time. Deadlines still apply.
                        job.deadline()
                    } else {
                        ready(job, &budgets, global).min(job.deadline())
                    }
                })
                .min();
            let completed = tokio::select! {
                biased;
                completed = next_attempt(&mut active), if !active.is_empty() => Some(completed),
                _ = self.changed.notified() => None,
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
            let Active {
                reservation, probe, ..
            } = active.remove(index);
            // Include normal reservations made while this probe was in flight,
            // before its own response headers update the observed quota debt.
            let probe = probe.map(|turn| (budgets.probe(&turn.quota), turn));
            let (status, headers, bytes) = match outcome {
                Attempt::Headers(response) => {
                    let response = match response {
                        Ok(r) => r,
                        Err(e) => {
                            if let Some((debt, turn)) = &probe {
                                budgets.charge_probe(debt);
                                blocked_probes.insert(turn.quota.clone(), turn.owed.clone());
                            }
                            if job.minting {
                                minting = false;
                            }
                            self.transport_failed(job, e, &mut pending);
                            continue;
                        }
                    };
                    let status = response.status();
                    job.http_status = Some(status.as_u16());
                    if status == StatusCode::NOT_MODIFIED {
                        self.metrics.not_modified.fetch_add(1, Ordering::Relaxed);
                    }
                    let headers = response.headers().clone();
                    tracing::info!(request_id=%job.request_id, attempt=job.attempts + job.auth_attempts,
                        http_status=status.as_u16(),
                        http_version=?response.version(),
                        remaining=number(&headers,"x-ratelimit-remaining"),
                        used=number(&headers,"x-ratelimit-used"),
                        limit=number(&headers,"x-ratelimit-limit"),
                        reset=number(&headers,"x-ratelimit-reset"),
                        "GitHub response headers");
                    if let Some(resource) = header(&headers, "x-ratelimit-resource")
                        && !job.minting
                    {
                        job.resource = resource;
                        if routes.len() >= 4096 {
                            routes.clear();
                        }
                        routes.insert(job.key.clone(), job.resource.clone());
                    }
                    if let (Some(remaining), Some(reset)) = (
                        number(&headers, "x-ratelimit-remaining"),
                        number(&headers, "x-ratelimit-reset"),
                    ) && !job.minting
                    {
                        let limit = RateLimit {
                            remaining,
                            reset_at_seconds: reset,
                        };
                        self.metrics
                            .limits
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(job.quota(), limit);
                        budgets.observe(
                            &job.quota(),
                            remaining,
                            reset,
                            status == StatusCode::NOT_MODIFIED,
                            reservation
                                .as_ref()
                                .and_then(|r| r.for_window(&job.quota(), reset)),
                        );
                    }
                    if let Some((debt, turn)) = &probe
                        && status != StatusCode::NOT_MODIFIED
                    {
                        budgets.charge_probe(debt);
                        blocked_probes.insert(turn.quota.clone(), turn.owed.clone());
                    }
                    if status == StatusCode::NOT_MODIFIED && !job.minting {
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
                        if exhausted {
                            let wait = Duration::from_secs(
                                number(&headers, "x-ratelimit-reset").map_or(60, |reset| {
                                    reset.saturating_sub(now_ms() / 1000).saturating_add(1)
                                }),
                            )
                            .max(retry.unwrap_or_default());
                            if job.minting {
                                secondary.extend(wait);
                            } else {
                                budgets.exhausted(
                                    &job.quota(),
                                    number(&headers, "x-ratelimit-reset").unwrap_or(0),
                                    wait,
                                );
                            }
                            if let Some(retry) = retry {
                                secondary.extend(retry);
                            }
                        } else {
                            job.secondary_retry_at = Some(secondary.observe(retry));
                        }
                    }
                    let max_body_bytes = if job.minting {
                        64 * 1024
                    } else {
                        self.config.max_body_bytes
                    };
                    active.push(Active {
                        resource: job.resource.clone(),
                        detail_lane: job.detail_lane,
                        reservation: None,
                        probe: None,
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
                Err(BodyError::Transport(error)) => {
                    if job.minting {
                        minting = false;
                    }
                    if let Some(wait) = retry {
                        job.ready_at = job.ready_at.max(quota_deadline(wait));
                    }
                    self.transport_failed(job, error, &mut pending);
                    continue;
                }
                Err(BodyError::TooLarge) => {
                    let error =
                        Error::Invalid("GitHub response exceeds configured body limit".into());
                    if job.minting {
                        minting = false;
                        self.config
                            .installation
                            .as_ref()
                            .unwrap()
                            .failed(error.clone());
                    }
                    self.finish(job, Err(error));
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
            if job.minting {
                minting = false;
                let app = self.config.installation.as_ref().unwrap();
                let limited = header_limited
                    || (status == StatusCode::FORBIDDEN && lower.contains("rate limit"));
                let result = if limited {
                    let wait = if exhausted {
                        let wait =
                            retry
                                .unwrap_or(Duration::from_secs(60))
                                .max(Duration::from_secs(
                                    number(&headers, "x-ratelimit-reset")
                                        .unwrap_or(0)
                                        .saturating_sub(now_ms() / 1000)
                                        .saturating_add(1),
                                ));
                        secondary.extend(wait);
                        wait
                    } else {
                        job.secondary_retry_at
                            .unwrap_or_else(|| secondary.observe(retry))
                            .max(secondary.until)
                            .saturating_duration_since(Instant::now())
                    };
                    Err(Error::RateLimited {
                        retry_after_seconds: ceil_seconds(wait),
                    })
                } else if status == StatusCode::CREATED {
                    app.accept(&bytes)
                } else {
                    Err(Error::Invalid(format!(
                        "GitHub App token exchange failed (HTTP {})",
                        status.as_u16()
                    )))
                };
                match result {
                    Ok(()) => {
                        job.minting = false;
                        job.notify.send_replace(SharedResult::Queued);
                        pending.push_front(job);
                    }
                    Err(error) => {
                        app.failed(error.clone());
                        self.finish(job, Err(error));
                    }
                }
                continue;
            }
            if status == StatusCode::UNAUTHORIZED
                && job.installation
                && job.auth_attempts < 2
                && job.attempts < self.config.max_attempts
            {
                self.config
                    .installation
                    .as_ref()
                    .unwrap()
                    .invalidate(job.auth_generation);
                job.notify.send_replace(SharedResult::Queued);
                pending.push_back(job);
                continue;
            }
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
                    job.secondary_retry_at
                        .unwrap_or_else(|| secondary.observe(retry))
                        .max(secondary.until)
                        .saturating_duration_since(Instant::now())
                };
                tracing::info!(request_id=%job.request_id,resource=%job.resource,retry_after_seconds=ceil_seconds(wait),"GitHub request cooldown scheduled");
                if exhausted {
                    budgets.exhausted(
                        &job.quota(),
                        number(&headers, "x-ratelimit-reset").unwrap_or(0),
                        wait,
                    );
                    // Retry-After always pauses shared traffic, even when
                    // GitHub also reports an exhausted primary bucket.
                    if let Some(retry) = retry {
                        secondary.extend(retry);
                    }
                }
                if job.attempts >= self.config.max_attempts
                    || (!job.background_collection() && quota_deadline(wait) >= job.deadline())
                {
                    self.finish(
                        job,
                        Err(Error::RateLimited {
                            retry_after_seconds: ceil_seconds(wait),
                        }),
                    );
                } else {
                    job.ready_at = quota_deadline(wait);
                    job.notify.send_replace(SharedResult::Queued);
                    pending.push_back(job);
                }
                continue;
            }
            if status.is_server_error() && job.attempts < self.config.max_attempts {
                tracing::warn!(request_id=%job.request_id,resource=%job.resource,status=status.as_u16(),attempt=job.attempts,"GitHub server error; retry scheduled");
                job.ready_at =
                    quota_deadline(transient_backoff(job.attempts).max(retry.unwrap_or_default()));
                job.notify.send_replace(SharedResult::Queued);
                pending.push_back(job);
                continue;
            }
            if !graphql_errors.is_empty() {
                if status.is_success()
                    && !job.installation
                    && let Ok(data) = serde_json::from_slice(&bytes)
                    && let Some(data) = crate::dashboard::partial::capture(job.body.as_ref(), data)
                {
                    let stamp = now_ms();
                    let response = Response {
                        data,
                        fetched_at_ms: stamp,
                        validated_at_ms: stamp,
                        source: Source::Network,
                        etag: None,
                        last_modified: None,
                        link: None,
                    };
                    let key = crate::client::tagged_cache_key(
                        &job.key,
                        crate::dashboard::partial::CACHE_TAG,
                    );
                    match self.store.put(&self.scope, &key, &response).await {
                        Ok(()) => {
                            tracing::info!(request_id=%job.request_id, "Retained permitted discovery nodes from incomplete response")
                        }
                        Err(error) => {
                            tracing::warn!(request_id=%job.request_id,error_code=error.diagnostic_code(), "Partial discovery evidence could not be retained")
                        }
                    }
                }
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
        // A collection can drop its last waiter before the shared request's
        // deadline. Keep that observation distinct from a waiting caller's
        // expiry; neither the sticky foreground flag nor elapsed time proves it.
        let deadline_context = matches!(&result, Err(Error::Deadline)).then(|| {
            match (
                job.deadline() <= Instant::now(),
                self.abandoned_request(&job),
            ) {
                (true, false) => "expired_with_waiters",
                (true, true) => "expired_unobserved",
                (false, true) => "unobserved_before_expiry",
                (false, false) => "observed_before_expiry",
            }
        });
        let source = match result.as_ref().map(|response| &response.source) {
            Ok(Source::Network) => "network",
            Ok(Source::Revalidated) => "revalidated",
            Ok(Source::Cache) => "cache",
            Err(_) => "error",
        };
        tracing::info!(request_id=%job.request_id,endpoint=job.endpoint,resource=%job.resource,attempts=job.attempts+job.auth_attempts,succeeded=result.is_ok(),http_status=job.http_status,source,error_code=result.as_ref().err().map(Error::diagnostic_code),elapsed_ms=job.queued_at.elapsed().as_millis() as u64,
            request_key=%crate::digest(&job.key),
            auth_scope=%if job.installation { self.config.installation.as_ref().unwrap().scope() } else { &self.scope },
            foreground=job.interactive(),
            completion_validation=job.completion_validation.load(Ordering::Relaxed),
            deadline_context,
            "GitHub request finished");
        let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        drop(job._permit);
        job.notify
            .send_replace(SharedResult::Complete(result.map(Arc::new)));
        inflight.remove(&job.key);
    }
}

fn ready(job: &Job, budgets: &Budgets, global: Instant) -> Instant {
    let now = Instant::now();
    let stamp = now_ms();
    job.ready_at.max(global).max(
        budgets
            .for_resource(&job.quota())
            .map(|budget| {
                if conditional_budget_exempt(job, budget) {
                    job.ready_at
                } else if budget.reset_at_seconds != 0 {
                    // Wake at known expiry even if no response or new job can
                    // wake the scheduler. Retry-After still gates via global.
                    let until_reset = Duration::from_millis(
                        budget
                            .reset_at_seconds
                            .saturating_add(1)
                            .saturating_mul(1000)
                            .saturating_sub(stamp),
                    );
                    budget
                        .next
                        .min(now.checked_add(until_reset).unwrap_or(budget.next))
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

async fn read_body(
    mut response: reqwest::Response,
    max: usize,
) -> std::result::Result<Vec<u8>, BodyError> {
    if response
        .content_length()
        .is_some_and(|len| len > max as u64)
    {
        return Err(BodyError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Transport)? {
        if chunk.len() > max.saturating_sub(bytes.len()) {
            return Err(BodyError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod transport_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn secondary_episodes_share_backoff_and_recover_after_quiet() {
        let mut backoff = SecondaryBackoff::new();
        let first = backoff.observe(None);
        assert!((60..=61).contains(&ceil_seconds(first - Instant::now())));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            backoff.observe(None),
            first,
            "an in-flight sibling is the same episode"
        );
        let extended = backoff.observe(Some(Duration::from_secs(180)));
        assert_eq!(extended - Instant::now(), Duration::from_secs(180));
        tokio::time::advance(extended - Instant::now()).await;
        let second = backoff.observe(Some(Duration::from_secs(1)));
        assert!((120..=121).contains(&ceil_seconds(second - Instant::now())));
        tokio::time::advance(second - Instant::now() + Duration::from_secs(15 * 60)).await;
        let recovered = backoff.observe(None);
        assert!((60..=61).contains(&ceil_seconds(recovered - Instant::now())));
    }

    #[tokio::test(start_paused = true)]
    async fn primary_retry_hints_do_not_escalate_secondary_backoff() {
        let mut backoff = SecondaryBackoff::new();
        backoff.extend(Duration::from_secs(180));
        tokio::time::advance(Duration::from_secs(180)).await;
        let first_secondary = backoff.observe(None);
        assert!((60..=61).contains(&ceil_seconds(first_secondary - Instant::now())));
    }

    #[tokio::test(start_paused = true)]
    async fn secondary_backoff_stays_bounded_and_honors_longer_server_hints() {
        let mut backoff = SecondaryBackoff::new();
        for episode in 0..10 {
            let until = backoff.observe(None);
            let seconds = ceil_seconds(until - Instant::now());
            let expected = (60 * 2u64.pow(episode.min(4))).min(15 * 60);
            assert!((expected..=expected + 1).contains(&seconds));
            tokio::time::advance(until - Instant::now()).await;
        }
        let until = backoff.observe(Some(Duration::from_secs(7200)));
        assert_eq!(until - Instant::now(), Duration::from_secs(7200));
    }

    fn core_job() -> Job {
        Job {
            completion_validation: Arc::new(AtomicBool::new(false)),
            installation: false,
            minting: false,
            auth_attempts: 0,
            auth_generation: 0,
            interactive: Arc::new(AtomicBool::new(true)),
            report_priority: Arc::new(AtomicBool::new(false)),
            detail_lane: false,
            collection_slice: false,
            request_id: "test".into(),
            endpoint: "pull_request",
            queued_at: Instant::now(),
            http_status: None,
            secondary_retry_at: None,
            url: "https://api.github.com/test".into(),
            key: "test".into(),
            body: None,
            cached: None,
            notify: watch::channel(SharedResult::Queued).0,
            deadline: Arc::new(Mutex::new(Instant::now() + Duration::from_secs(60))),
            ready_at: Instant::now(),
            attempts: 0,
            resource: "core".into(),
            _permit: Arc::new(tokio::sync::Semaphore::new(1))
                .try_acquire_owned()
                .unwrap(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn expired_quota_releases_queued_work_without_another_response() {
        let now = Instant::now();
        let job = core_job();
        let mut budgets = Budgets::default();
        // A pacing reservation can outlive the wall-clock reset. No response
        // can retire that window while all queued requests wait behind it.
        budgets.exhausted("core", now_ms() / 1000 - 2, Duration::from_secs(600));
        assert_eq!(ready(&job, &budgets, now), now);
    }

    #[tokio::test(start_paused = true)]
    async fn quota_wait_wakes_at_reset_without_new_queue_activity() {
        let now = Instant::now();
        let job = core_job();
        let mut budgets = Budgets::default();
        budgets.exhausted("core", now_ms() / 1000 + 10, Duration::from_secs(600));
        let wake = ready(&job, &budgets, now);
        assert!(wake >= now + Duration::from_secs(10));
        assert!(wake <= now + Duration::from_secs(11));
    }

    #[tokio::test(start_paused = true)]
    async fn quota_expiry_preserves_other_windows_and_shared_cooldowns() {
        let now = Instant::now();
        let job = core_job();
        let mut budgets = Budgets::default();
        budgets.exhausted("core", now_ms() / 1000 - 2, Duration::from_secs(600));
        budgets.exhausted("core", now_ms() / 1000 + 3600, Duration::from_secs(30));
        assert_eq!(ready(&job, &budgets, now), now + Duration::from_secs(30));
        assert_eq!(
            ready(&job, &budgets, now + Duration::from_secs(90)),
            now + Duration::from_secs(90)
        );
        budgets.exhausted("core", 0, Duration::from_secs(120));
        assert_eq!(ready(&job, &budgets, now), now + Duration::from_secs(120));
    }

    #[tokio::test(start_paused = true)]
    async fn slow_reserved_response_does_not_charge_pacing_twice() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, true, None);
        let reservation = budgets.reserve(&core_job());
        tokio::time::advance(Duration::from_secs(5)).await;
        budgets.observe(
            "core",
            4999,
            reset,
            false,
            reservation.for_window("core", reset),
        );
        assert!(
            budgets.for_resource("core").next().unwrap().next <= Instant::now(),
            "the reserved pacing interval already elapsed while awaiting headers"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn late_headers_preserve_later_reservations_and_revise_spacing() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, true, None);
        let first = budgets.reserve(&core_job());
        tokio::time::advance(Duration::from_secs(5)).await;
        budgets.reserve(&core_job());
        let later = budgets.for_resource("core").next().unwrap().next;
        budgets.observe("core", 4998, reset, false, first.for_window("core", reset));
        assert_eq!(budgets.for_resource("core").next().unwrap().next, later);

        // A lower remaining header can increase spacing beyond the later debt.
        budgets.observe("core", 200, reset, false, first.for_window("core", reset));
        let budget = budgets.for_resource("core").next().unwrap();
        assert_eq!(budget.next, first.dispatched_at + budget.spacing);
        assert!(budget.next > later);
    }

    #[tokio::test(start_paused = true)]
    async fn changed_pacing_probe_repays_a_full_slot_after_later_reservations() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, false, None);
        // A normal turn may dispatch while a slow speculative probe is in
        // flight. Its future slot must survive the probe's charged headers.
        tokio::time::advance(Duration::from_secs(2)).await;
        budgets.reserve(&core_job());
        let prior = budgets.for_resource("core").next().unwrap().next;
        let probe = budgets.probe("core");
        budgets.observe("core", 4999, reset, false, None);
        budgets.charge_probe(&probe);
        let budget = budgets.for_resource("core").next().unwrap();
        assert_eq!(budget.next, prior + budget.spacing);
    }

    #[tokio::test(start_paused = true)]
    async fn pacing_probe_debt_preserves_overlapping_windows_and_separate_auth_quotas() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        for window in [reset, reset + 600] {
            budgets.observe("core", 5000, window, false, None);
        }
        budgets.observe("installation/core", 15000, reset, false, None);
        let app_slot = budgets
            .for_resource("installation/core")
            .next()
            .unwrap()
            .next;
        let probe = budgets.probe("core");
        budgets.observe("core", 4999, reset, false, None);
        budgets.charge_probe(&probe);
        for (window, prior) in probe.windows {
            let budget = &budgets.0["core"][&window];
            assert_eq!(budget.next, prior + budget.spacing);
        }
        assert_eq!(
            budgets
                .for_resource("installation/core")
                .next()
                .unwrap()
                .next,
            app_slot
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unconfirmed_pacing_probe_conservatively_repays_debt_after_transport_failure() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, false, None);
        let probe = budgets.probe("core");
        let prior = probe.windows[0].1;
        // No headers establish whether GitHub charged the failed attempt.
        budgets.charge_probe(&probe);
        let budget = budgets.for_resource("core").next().unwrap();
        assert_eq!(budget.next, prior + budget.spacing);
    }

    #[tokio::test(start_paused = true)]
    async fn changed_exempt_probe_and_unreserved_windows_still_pay_pacing() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, true, None);
        let mut probe = core_job();
        probe.cached = Some(Response {
            data: serde_json::json!({}),
            fetched_at_ms: 0,
            validated_at_ms: 0,
            source: Source::Revalidated,
            etag: Some("synthetic".into()),
            last_modified: None,
            link: None,
        });
        let exempt = budgets.reserve(&probe);
        assert!(exempt.for_window("core", reset).is_none());
        let reserved = budgets.reserve(&core_job());
        tokio::time::advance(Duration::from_secs(5)).await;
        for (resource, window, reservation) in [
            ("core", reset, &exempt),
            ("core", reset + 60, &reserved),
            ("search", reset, &reserved),
        ] {
            let stamp = reservation.for_window(resource, window);
            assert!(stamp.is_none());
            budgets.observe(resource, 4999, window, false, stamp);
            let budget = &budgets.0[resource][&window];
            assert_eq!(budget.next, Instant::now() + budget.spacing);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn reserved_response_cannot_shorten_exhaustion_or_shared_headroom() {
        for (resource, remaining) in [
            ("core", 0),
            ("core", 100),
            ("graphql", 0),
            ("graphql", 100),
            ("installation/graphql", 100),
            ("search", 0),
        ] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            budgets.observe(resource, 5000, reset, true, None);
            let mut job = core_job();
            job.resource = resource.into();
            let reservation = budgets.reserve(&job);
            tokio::time::advance(Duration::from_secs(5)).await;
            budgets.observe(
                resource,
                remaining,
                reset,
                false,
                reservation.for_window(resource, reset),
            );
            let budget = &budgets.0[resource][&reset];
            assert_eq!(budget.next, Instant::now() + budget.spacing);
            assert!(budget.spacing >= Duration::from_secs(3600));
        }
    }

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
                    usage.observe(remaining, true, now, false);
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

    #[tokio::test(start_paused = true)]
    async fn graphql_pacing_learns_shared_point_consumption() {
        for resource in ["graphql", "installation/graphql"] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            budgets.observe(resource, 5000, reset, false, None);
            for i in 1..=6 {
                tokio::time::advance(Duration::from_secs(5)).await;
                // Three consumers, each executing a 20-point batched query.
                budgets.observe(resource, 5000 - i * 60, reset, false, None);
            }
            let budget = budgets.for_resource(resource).next().unwrap();
            assert_eq!(budget.usage.share, 60.0);
            assert!(
                budget.spacing >= Duration::from_secs(45),
                "{resource}: {:?}",
                budget.spacing
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn quota_wait_is_not_mistaken_for_idle_demand() {
        for resource in ["core", "graphql", "installation/graphql"] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            budgets.observe(resource, 5000, reset, false, None);
            for i in 1..=6 {
                tokio::time::advance(Duration::from_secs(5)).await;
                budgets.observe(resource, 5000 - i * 60, reset, false, None);
            }
            let mut waiting = core_job();
            waiting.resource = resource.into();
            tokio::time::advance(Duration::from_secs(90)).await;
            let reservation = budgets.reserve(&waiting);
            budgets.observe(
                resource,
                4580,
                reset,
                false,
                reservation.for_window(resource, reset),
            );
            assert_eq!(
                budgets.for_resource(resource).next().unwrap().usage.share,
                60.0,
                "{resource}"
            );

            // A newly queued read after real inactivity must not inherit the
            // account's unrelated traffic from the entire idle interval.
            tokio::time::advance(Duration::from_secs(300)).await;
            let mut resumed = core_job();
            resumed.resource = resource.into();
            let reservation = budgets.reserve(&resumed);
            budgets.observe(
                resource,
                4000,
                reset,
                false,
                reservation.for_window(resource, reset),
            );
            assert_eq!(
                budgets.for_resource(resource).next().unwrap().usage.share,
                1.0,
                "{resource}"
            );
        }
    }

    #[test]
    fn shared_graphql_workloads_keep_capacity_throughout_the_hour() {
        for scenario in ["one-point", "twenty-point", "mixed", "cost-change"] {
            for external_interval in [None, Some(3000)] {
                let start = Instant::now();
                let mut clients: Vec<_> = (0..3)
                    .map(|_| (SharedUsage::new(5000, start), 0u64, 0u64, 0u64))
                    .collect();
                let mut remaining = 5000u64;
                for ms in (0u64..3_600_000).step_by(10) {
                    if external_interval.is_some_and(|interval| ms % interval == 0) {
                        remaining = remaining.checked_sub(1).expect("external quota exhausted");
                    }
                    for (index, (usage, next, calls, last)) in clients.iter_mut().enumerate() {
                        if ms < *next {
                            continue;
                        }
                        let cost = match scenario {
                            "one-point" => 1,
                            "twenty-point" => 20,
                            "mixed" => {
                                if index == 0 {
                                    20
                                } else {
                                    1
                                }
                            }
                            "cost-change" => {
                                if ms < 1_800_000 {
                                    20
                                } else {
                                    1
                                }
                            }
                            _ => unreachable!(),
                        };
                        remaining = remaining.checked_sub(cost).expect("local quota exhausted");
                        *calls += 1;
                        *last = ms;
                        // These consumers always have work queued before their
                        // next slot. The reservation regression above separately
                        // proves that real idle reads do not set this flag.
                        usage.observe(remaining, true, start + Duration::from_millis(ms), true);
                        *next = ms
                            + usage
                                .spacing(remaining, (3_600_000 - ms).div_ceil(1000))
                                .as_millis() as u64;
                    }
                    assert!(
                        remaining >= 40,
                        "{scenario} {external_interval:?}: depleted at {ms}: {remaining}"
                    );
                }
                assert!(remaining < 300, "{scenario}: unused capacity {remaining}");
                assert!(
                    clients.iter().all(|(_, _, _, last)| *last > 3_300_000),
                    "{scenario} {external_interval:?}: early stall {:?}",
                    clients
                        .iter()
                        .map(|(_, _, calls, last)| (*calls, *last))
                        .collect::<Vec<_>>()
                );
                assert!(
                    clients.iter().all(|(_, _, calls, _)| *calls >= 45),
                    "{scenario}: starved consumer"
                );
            }
        }
    }

    #[test]
    fn shared_usage_counts_charged_responses_and_ignores_free_validations() {
        let start = Instant::now();
        let mut usage = SharedUsage::new(5000, start);
        for i in 1..=10 {
            let now = start + Duration::from_secs(i * 3);
            usage.observe(5000 - i * 3, false, now - Duration::from_millis(1), false);
            usage.observe(5000 - i * 3, true, now, false);
        }
        assert_eq!(usage.share, 3.0);
        assert_eq!(usage.spacing(1000, 900), Duration::from_secs(3));
        assert_eq!(usage.spacing(100, 900), Duration::from_secs(901));
        assert_eq!(usage.spacing(101, u64::MAX), Duration::from_secs(86400));
        usage.observe(1000, true, start + Duration::from_secs(300), false);
        assert_eq!(
            usage.share, 1.0,
            "an idle consumer can resume interactive work"
        );
    }

    #[test]
    fn quota_windows_keep_the_lowest_remaining_and_expire_independently() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 100, reset, false, None);
        budgets.observe("core", 5000, reset, true, None);
        let budget = budgets.for_resource("core").next().unwrap();
        assert_eq!(budget.remaining, 100);
        assert!(budget.spacing > Duration::from_secs(35));
        budgets.observe("core", 5000, reset + 60, true, None);
        assert_eq!(budgets.for_resource("core").count(), 2);
        // Simulate expiry without sleeping or restarting the scheduler. Only
        // that window's reservation may be forgotten by the next observation.
        let mut expired = budgets.0.get_mut("core").unwrap().remove(&reset).unwrap();
        expired.reset_at_seconds = 1;
        budgets.0.get_mut("core").unwrap().insert(1, expired);
        budgets.observe("core", 4999, reset + 60, false, None);
        assert_eq!(budgets.for_resource("core").count(), 1);
        assert_eq!(budgets.for_resource("core").next().unwrap().remaining, 4999);
    }

    #[tokio::test(start_paused = true)]
    async fn overlapping_windows_estimate_the_whole_paced_resource() {
        for other_charged_resource in ["core", "search"] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            budgets.observe("core", 4000, reset, true, None);
            budgets.observe(other_charged_resource, 5000, reset + 60, true, None);
            for i in 1..=20 {
                tokio::time::advance(Duration::from_millis(1500)).await;
                budgets.observe(other_charged_resource, 5000 - i, reset + 60, false, None);
                // Free validations must not dilute the charged-call estimate.
                budgets.observe("core", 5000 - i, reset + 60, true, None);
                if i % 5 == 0 {
                    budgets.observe("core", 4000 - i - 3, reset, false, None);
                }
            }
            let budget = &budgets.0["core"][&reset];
            let expected_share = if other_charged_resource == "core" {
                1.0 // 23 allowance units / 24 local core responses, floored at 1.
            } else {
                5.75 // Search has separate pacing: only four core responses.
            };
            assert_eq!(budget.usage.share, expected_share);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn mixed_window_traffic_uses_capacity_without_depleting_either_allowance() {
        for direct_interval in [None, Some(3000)] {
            let reset = now_ms() / 1000 + 3600;
            let mut clients: Vec<_> = (0..3)
                .map(|_| {
                    let mut budgets = Budgets::default();
                    for window in 0..2 {
                        budgets.observe("core", 5000, reset + window, true, None);
                    }
                    (budgets, 0u64, 0u64)
                })
                .collect();
            let mut remaining = [5000u64; 2];
            let mut direct_calls = 0;
            let mut advanced_ms = 0;
            for ms in (0u64..3_600_000).step_by(10) {
                let direct_due = direct_interval.is_some_and(|interval| ms % interval == 0);
                if !direct_due && clients.iter().all(|(_, next, _)| ms < *next) {
                    continue;
                }
                tokio::time::advance(Duration::from_millis(ms - advanced_ms)).await;
                advanced_ms = ms;
                if direct_due {
                    remaining[usize::from(direct_calls % 4 == 0)] -= 1;
                    direct_calls += 1;
                }
                for (budgets, next, calls) in &mut clients {
                    if ms < *next {
                        continue;
                    }
                    let window = usize::from(*calls % 4 == 0);
                    remaining[window] -= 1;
                    *calls += 1;
                    budgets.observe(
                        "core",
                        remaining[window],
                        reset + window as u64,
                        false,
                        None,
                    );
                    // Supply simulated wall-clock time; Tokio's paused clock
                    // advances the production estimator's observation intervals.
                    let spacing = budgets
                        .for_resource("core")
                        .map(|b| {
                            b.usage
                                .spacing(b.remaining, (3_600_000 - ms).div_ceil(1000))
                        })
                        .max()
                        .unwrap();
                    *next = ms + spacing.as_millis() as u64;
                }
                assert!(remaining.iter().all(|left| *left >= 50));
            }
            assert!(remaining[0] < 300, "unused primary capacity: {remaining:?}");
            assert!(clients.iter().all(|(_, _, calls)| *calls > 1400));
        }
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
