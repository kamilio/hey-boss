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

mod pacing;
use pacing::Pacing;

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
    /// A queued required read needs this quota; optional callers can use REST.
    OptionalDeferred,
    Active,
    Complete(Result<Arc<Response>>),
}
mod inflight;
pub(crate) use inflight::Inflight;

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
    pub access_failure_epoch: AtomicU64,
    pub active: AtomicU64,
    pub limits: Mutex<BTreeMap<String, RateLimit>>,
}

pub(crate) struct Job {
    pub completion_validation: Arc<AtomicBool>,
    // Once a required caller joins, optional fallback must not defer its job.
    pub required_reader: Arc<AtomicBool>,
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
    // A lower-priority validator borrowed this completion's paced wait. Its
    // charged response keeps the exact job's slot and repays debt afterwards.
    pub protected_pacing: Vec<(u64, Instant)>,
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

    fn rest_quota(&self) -> &'static str {
        if self.installation {
            "installation/core"
        } else {
            "core"
        }
    }

    fn defers_optional(&self, required_quotas: &HashMap<String, bool>) -> bool {
        self.body.is_some()
            && !self.minting
            && !self.required_reader.load(Ordering::Relaxed)
            && required_quotas
                .get(&self.quota())
                .is_some_and(|foreground| *foreground || !self.interactive())
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

// Providers/resources have independent allowances but share dispatch spacing
// and sockets. Completion priority within one must not monopolize the others.
// Details use separate sockets: serving them cannot spend their provider's
// turn at the lifecycle/CI socket while that socket is occupied by a peer.
#[derive(Default)]
struct QuotaOrder(VecDeque<(String, bool)>);

impl QuotaOrder {
    fn choose(&self, quotas: impl Iterator<Item = (String, bool)>) -> Option<(String, bool)> {
        quotas.min_by_key(|quota| self.0.iter().position(|recent| recent == quota))
    }

    fn dispatched(&mut self, quota: (String, bool), capacity: usize) {
        self.0.retain(|recent| recent != &quota);
        if self.0.len() >= capacity {
            self.0.pop_front();
        }
        self.0.push_back(quota);
    }
}

// A conditional validation may borrow a paced turn's wait. If it returns a
// charged response, repay the full interval from the existing future slot,
// rather than replacing that debt with an interval starting at the response.
struct PacingProbe {
    quota: String,
    windows: Vec<(u64, Instant)>,
}

impl PacingProbe {
    fn observation(&self, quota: &str, reset: u64, queued_at: Instant) -> Option<ReservedWindow> {
        (self.quota == quota).then_some(())?;
        let (_, slot) = self.windows.iter().find(|(window, _)| *window == reset)?;
        // A borrowed slot is sustained queued demand, even though it dispatched
        // early. Keep the unreserved header-time anchor; charge_probe repays
        // the captured future slot separately.
        Some(ReservedWindow {
            dispatched_at: Instant::now(),
            waited_for_quota: queued_at < *slot,
            probe: true,
        })
    }
}

struct ProbeTurn {
    quota: String,
    owed: String,
    owns_turn: bool,
    protect_turn: bool,
}

impl ProbeTurn {
    fn protect(&self, debt: &PacingProbe, pending: &mut VecDeque<Job>) {
        if self.protect_turn
            && let Some(owed) = pending.iter_mut().find(|job| job.request_id == self.owed)
        {
            owed.protected_pacing.clone_from(&debt.windows);
        }
    }
}

#[derive(Default)]
struct ProbeBlocks {
    owed: HashMap<String, String>,
    selected: HashMap<String, Vec<(u64, Instant)>>,
}

impl ProbeBlocks {
    fn retain(&mut self, pending: &VecDeque<Job>, budgets: &Budgets, now: Instant) {
        self.owed
            .retain(|_, owed| pending.iter().any(|job| job.request_id == *owed));
        // An early selected turn has already left the queue. Its charged debt
        // must survive completion/cancellation until the borrowed slots pass.
        self.selected.retain(|quota, slots| {
            budgets.for_resource(quota).any(|budget| {
                slots
                    .iter()
                    .any(|(reset, until)| *reset == budget.reset_at_seconds && *until > now)
            })
        });
    }

    fn contains(&self, quota: &str) -> bool {
        self.owed.contains_key(quota) || self.selected.contains_key(quota)
    }

    fn wake(&self, now: Instant) -> Option<Instant> {
        self.selected
            .values()
            .flatten()
            .map(|(_, until)| *until)
            .filter(|until| *until > now)
            .min()
    }

    fn charge(
        &mut self,
        turn: &ProbeTurn,
        debt: &PacingProbe,
        budgets: &mut Budgets,
        pending: &mut VecDeque<Job>,
    ) {
        budgets.charge_probe(debt);
        turn.protect(debt, pending);
        if turn.owns_turn {
            self.selected
                .insert(turn.quota.clone(), budgets.probe(&turn.quota).windows);
        } else {
            self.owed.insert(turn.quota.clone(), turn.owed.clone());
        }
    }
}

struct Reservation {
    ticket: u128,
    resource: String,
    resets: Vec<(u64, bool)>,
    dispatched_at: Instant,
}

#[derive(Clone, Copy)]
struct ReservedWindow {
    dispatched_at: Instant,
    waited_for_quota: bool,
    probe: bool,
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
                probe: false,
            })
    }
}

fn shared_quota(resource: &str) -> bool {
    matches!(resource.rsplit('/').next(), Some("core" | "graphql"))
}

fn core_lane_busy(active: &[Active], prod: bool) -> bool {
    active.iter().filter(|attempt| !attempt.detail_lane).count() >= if prod { 6 } else { 2 }
        || active
            .iter()
            .filter(|attempt| !attempt.detail_lane && attempt.resource == "core")
            .count()
            >= if prod { 5 } else { 1 }
}

fn lane_busy(active: &[Active], job: &Job, prod: bool) -> bool {
    if !job.detail_lane && job.resource == "core" {
        return core_lane_busy(active, prod);
    }
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
    Deadline,
    Headers(std::result::Result<reqwest::Response, reqwest::Error>),
    Stored(Result<Response>),
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

// A short optional reader cannot leave a stalled transport occupying the lane.
// Coalesced callers may extend the shared deadline even after dispatch; keep
// the same transport alive until that current deadline or the HTTP timeout.
async fn before_deadline<T>(job: &Job, future: impl Future<Output = T>) -> Option<T> {
    tokio::pin!(future);
    loop {
        match tokio::time::timeout_at(job.deadline(), &mut future).await {
            Ok(value) => return Some(value),
            Err(_) if job.deadline() > Instant::now() => continue,
            Err(_) => return None,
        }
    }
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
    pacing: Pacing,
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
    fn rest_fallback_has_headroom(&self, job: &Job) -> bool {
        self.for_resource(job.rest_quota())
            .all(|budget| budget.remaining > QUOTA_RESERVE)
    }

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
            for (reset, _) in &probe.windows {
                if let Some(budget) = windows.get_mut(reset) {
                    if budget.remaining > QUOTA_RESERVE {
                        budget.pacing.charge(now, budget.spacing);
                    }
                    budget.next = budget.pacing.deadline();
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
        let mut previous = windows.remove(&reset);
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
        let mut pacing = previous
            .as_mut()
            .map(|b| std::mem::replace(&mut b.pacing, Pacing::new(now)))
            .unwrap_or_else(|| Pacing::new(now));
        let hard_gate = remaining == 0 || (shared_quota(resource) && remaining <= QUOTA_RESERVE);
        if hard_gate || (!unchanged && !reservation.is_some_and(|r| r.probe)) {
            let anchor = reservation
                .filter(|_| !hard_gate)
                .map(|r| r.dispatched_at)
                .unwrap_or(now);
            let deadline = anchor
                .checked_add(spacing)
                .unwrap_or_else(|| quota_deadline(Duration::from_secs(86400)));
            pacing.floor(deadline);
        }
        let next = pacing.deadline();
        windows.insert(
            reset,
            Budget {
                next,
                pacing,
                remaining,
                spacing,
                reset_at_seconds: reset,
                usage,
            },
        );
    }

    fn reserve(&mut self, job: &mut Job) -> Reservation {
        let mut reservation = Reservation {
            ticket: fastrand::u128(..),
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
                    budget.pacing.reserve(
                        reservation.ticket,
                        reservation.dispatched_at,
                        budget.spacing,
                    );
                    budget.next = budget.pacing.deadline();
                }
            }
        }
        // A retry cannot reuse a consumed completion slot.
        job.protected_pacing.clear();
        reservation
    }

    fn settle(&mut self, reservation: &Reservation, free: bool) -> Duration {
        let mut released = Duration::ZERO;
        if let Some(windows) = self.0.get_mut(&reservation.resource) {
            for budget in windows.values_mut() {
                budget.pacing.settle(reservation.ticket, free);
                let next = budget.pacing.deadline();
                released = released.max(budget.next.saturating_duration_since(next));
                budget.next = next;
            }
        }
        released
    }

    fn exhausted(&mut self, resource: &str, reset: u64, wait: Duration) {
        let next = quota_deadline(wait);
        self.0.entry(resource.to_owned()).or_default().insert(
            reset,
            Budget {
                next,
                pacing: Pacing::new(next),
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
    fn persist(
        &self,
        job: Job,
        active: &mut Vec<Active>,
        write: impl Future<Output = Result<Response>> + Send + 'static,
    ) {
        // Storage shares the request's bounded lane and admission permit.
        // Poll it beside sockets so a slow writer cannot prevent independent
        // headers, cooldowns, expiry or dispatch from reaching the scheduler.
        active.push(Active {
            resource: job.resource.clone(),
            detail_lane: job.detail_lane,
            reservation: None,
            probe: None,
            future: Box::pin(async move {
                let started = Instant::now();
                let result = write.await;
                tracing::info!(request_id=%job.request_id,
                    elapsed_ms=started.elapsed().as_millis() as u64,
                    "GitHub response persistence finished");
                (job, Attempt::Stored(result))
            }),
        });
    }

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
        let mut quota_order = QuotaOrder::default();
        let mut blocked_probes = ProbeBlocks::default();
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
            // Another request's borrowed turn stays owned by that exact job.
            // A selected request's early turn stays blocked until its debt is paid.
            blocked_probes.retain(&pending, &budgets, now);
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
            // Background required reads retain their ordinary paced turns,
            // rather than forcing a foreground shortcut into REST fallback.
            let mut required_quotas = HashMap::<String, bool>::new();
            for job in pending
                .iter()
                .filter(|job| job.required_reader.load(Ordering::Relaxed))
            {
                *required_quotas.entry(job.quota()).or_default() |= job.interactive();
            }
            let waits_for_required = |job: &Job| {
                job.defers_optional(&required_quotas) && budgets.rest_fallback_has_headroom(job)
            };
            let rest_socket_busy = core_lane_busy(&active, prod);
            let queued_rest: std::collections::HashSet<_> = pending
                .iter()
                .filter(|job| job.resource == "core")
                .map(Job::rest_quota)
                .collect();
            let defers_optional = |job: &Job| {
                waits_for_required(job)
                    && !rest_socket_busy
                    && !queued_rest.contains(job.rest_quota())
            };
            // Choose each quota's turn before considering pacing. A charged
            // conditional probe must not keep moving an older turn forever.
            // Keep foreground/background and completion/ordinary alternation;
            // ties retain queue order. Busy lanes and retry backoffs do not
            // reserve a turn, so unrelated work can still use its own capacity.
            let mut turns = HashMap::new();
            for (index, job) in pending.iter().enumerate() {
                if job.ready_at > now
                    || waits_for_required(job)
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
                // Selected foreground validators and background GraphQL
                // shortcuts may use their own soft wait. Shortcuts still need
                // congested REST and no required peers below. Charged replies
                // keep borrowing blocked after this job leaves.
                (index != *selected || job.interactive()
                    || (job.body.is_some() && !job.required_reader.load(Ordering::Relaxed)))
                    // GraphQL is always charged. It may only move its own
                    // selected turn forward, never borrow another class's turn.
                    // Optional reads borrow only when fallback is congested,
                    // and never ahead of required peers in this quota.
                    && (job.body.is_none() || (index == *selected
                        && (job.required_reader.load(Ordering::Relaxed)
                            || (!job.defers_optional(&required_quotas)
                                && (rest_socket_busy
                                    || queued_rest.contains(job.rest_quota())
                                    || !budgets.rest_fallback_has_headroom(job))))))
                    && !blocked_probes.contains(&quota)
                    && !probing_quotas.contains(&quota)
                    && ((job.interactive() == turn.interactive()
                        && job.completion_validation.load(Ordering::Relaxed) == turn.completion_validation.load(Ordering::Relaxed))
                        // Foreground validators may use an owed background
                        // turn's pacing wait. The turn and its one-probe debt
                        // remain owned by that exact background request.
                        // Foreground completion and ordinary validators may
                        // borrow each other's waits; a charged reply preserves
                        // the exact owed turn's original slot.
                        || job.interactive())
                    // The class's turn is held by soft pacing, not by a retry,
                    // socket or global spacing.
                    && ready(turn, &budgets, now) > now
                    && budgets.for_resource(&quota).next().is_some()
                    && budgets.for_resource(&quota).all(|budget| pacing_probe_eligible(job, budget))
            };
            // Optional GraphQL selectors have a short REST-fallback budget.
            // Yield to equal/higher priority required reads in the same quota,
            // or fall back when known pacing alone exceeds the shortcut budget.
            // Required coalescers keep their job and its existing quota gates.
            // An exhausted REST quota keeps the GraphQL route eligible. Busy
            // sockets or queued REST work for this provider keep its bounded
            // waiter behind required GraphQL work instead of abandoning it
            // for a congested fallback. Use the effective selected turn so an
            // admitted loan is not rejected using its old pacing deadline.
            for (index, job) in pending
                .iter()
                .enumerate()
                .filter(|(_, job)| job.body.is_some())
            {
                let until = ready_with_probe(job, &budgets, global, can_probe(index, job));
                if defers_optional(job) {
                    if job.notify.send_if_modified(|state| {
                        if !matches!(state, SharedResult::OptionalDeferred) {
                            *state = SharedResult::OptionalDeferred;
                            true
                        } else {
                            false
                        }
                    }) {
                        tracing::info!(request_id=%job.request_id, endpoint=job.endpoint,
                            "Optional GraphQL shortcut yielded to a required read");
                    }
                } else if until > now {
                    job.notify.send_if_modified(|state| {
                        if matches!(state, SharedResult::Queued | SharedResult::OptionalDeferred)
                            || matches!(state, SharedResult::QueuedUntil(previous) if *previous != until)
                        {
                            *state = SharedResult::QueuedUntil(until);
                            true
                        } else {
                            false
                        }
                    });
                } else {
                    job.notify.send_if_modified(|state| {
                        if matches!(
                            state,
                            SharedResult::OptionalDeferred | SharedResult::QueuedUntil(_)
                        ) {
                            *state = SharedResult::Queued;
                            true
                        } else {
                            false
                        }
                    });
                }
            }
            let waiting_for_turn = |index: usize, job: &Job| {
                waits_for_required(job)
                    || (job.ready_at <= now
                        && turns
                            .get(&job.quota())
                            .is_some_and(|(_, selected)| *selected != index)
                        && !can_probe(index, job))
            };
            let next = {
                let eligible = |index: usize, job: &Job| {
                    ready_with_probe(job, &budgets, global, can_probe(index, job)) <= now
                        && !(job.installation && minting)
                        && !lane_busy(&active, job, prod)
                        && !waiting_for_turn(index, job)
                };
                // Rotate only among quotas with a dispatchable request. A
                // paced, throttled or socket-blocked quota cannot hold up a
                // peer. Keep each quota's existing class/turn/probe ordering.
                let quota = quota_order.choose(
                    pending
                        .iter()
                        .enumerate()
                        .filter(|(index, job)| eligible(*index, job))
                        .map(|(_, job)| (job.quota(), job.detail_lane)),
                );
                let eligible = |index: usize, job: &Job| {
                    quota.as_ref().is_some_and(|(quota, detail)| {
                        *quota == job.quota() && *detail == job.detail_lane
                    }) && eligible(index, job)
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
                let selected = pending
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
                    .map(|(index, _)| index);
                let proven_probe = |index: usize, job: &Job| {
                    can_probe(index, job)
                        && budgets
                            .for_resource(&job.quota())
                            .all(|budget| conditional_budget_exempt(job, budget))
                };
                selected.map(|index| {
                    let selected = &pending[index];
                    if !can_probe(index, selected) || proven_probe(index, selected) {
                        return index;
                    }
                    // Spend this class's borrowed window on proven validators
                    // before an unproven probe can charge and close it. Keep
                    // ordinary turns, other quotas/classes, one active probe,
                    // debt repayment, reserves and cooldowns unchanged.
                    let quota = selected.quota();
                    pending
                        .iter()
                        .enumerate()
                        .find(|(candidate, job)| {
                            job.quota() == quota
                                && job.interactive() == selected.interactive()
                                && eligible(*candidate, job)
                                && proven_probe(*candidate, job)
                        })
                        .map_or(index, |(candidate, _)| candidate)
                })
            };
            if active.len() < max_active
                && let Some(index) = next
            {
                let probe = can_probe(index, &pending[index]).then(|| {
                    let quota = pending[index].quota();
                    let owed = pending[turns[&quota].1].request_id.clone();
                    let owns_turn = index == turns[&quota].1;
                    let protect_turn = pending[index].interactive()
                        && pending[turns[&quota].1].interactive()
                        && pending[index].completion_validation.load(Ordering::Relaxed)
                            != pending[turns[&quota].1]
                                .completion_validation
                                .load(Ordering::Relaxed);
                    ProbeTurn {
                        quota,
                        owed,
                        owns_turn,
                        protect_turn,
                    }
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
                let probe = probe.filter(|_| !job.minting);
                // Ordinary work reserves every live window before dispatch.
                // A probe repays its borrowed slot on charged/unknown headers;
                // reserving it here too would charge that interval twice.
                let reservation =
                    (!job.minting && probe.is_none()).then(|| budgets.reserve(&mut job));
                // Borrowing another request's wait cannot advance its turn.
                // A selected validator consumes its own turn even when early.
                let consumes_turn = probe.as_ref().is_none_or(|turn| turn.owns_turn);
                if consumes_turn {
                    let streak = interactive_streaks.entry(job.quota()).or_default();
                    *streak = if job.interactive() {
                        streak.saturating_add(1)
                    } else {
                        0
                    };
                }
                if !job.minting && consumes_turn {
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
                    selected_probe=probe.as_ref().is_some_and(|turn| turn.owns_turn),
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
                .timeout(self.config.request_timeout.min(
                    if job.background_collection() {
                        crate::collection_budget::STALL_LIMIT
                    } else {
                        self.config.request_timeout
                    },
                ));
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
                    future: Box::pin(async move {
                        let outcome = before_deadline(&job, request.send())
                            .await
                            .map_or(Attempt::Deadline, Attempt::Headers);
                        (job, outcome)
                    }),
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
                quota_order.dispatched((quota, attempt.detail_lane), self.config.queue_capacity);
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
                        ready_with_probe(job, &budgets, global, can_probe(index, job))
                            .min(job.deadline())
                    }
                })
                .chain(blocked_probes.wake(now))
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
                Attempt::Deadline => {
                    if let Some(reservation) = &reservation {
                        budgets.settle(reservation, false);
                    }
                    if let Some((debt, turn)) = &probe {
                        blocked_probes.charge(turn, debt, &mut budgets, &mut pending);
                    }
                    if job.minting {
                        minting = false;
                        self.config
                            .installation
                            .as_ref()
                            .unwrap()
                            .failed(Error::Deadline);
                    }
                    self.finish(job, Err(Error::Deadline));
                    continue;
                }
                Attempt::Stored(result) => {
                    self.finish(job, result);
                    continue;
                }
                Attempt::Headers(response) => {
                    let response = match response {
                        Ok(r) => r,
                        Err(e) => {
                            if let Some(reservation) = &reservation {
                                budgets.settle(reservation, false);
                            }
                            if let Some((debt, turn)) = &probe {
                                blocked_probes.charge(turn, debt, &mut budgets, &mut pending);
                            }
                            if job.minting {
                                minting = false;
                            }
                            self.transport_failed(job, e, &mut pending);
                            continue;
                        }
                    };
                    let status = response.status();
                    let unchanged = status == StatusCode::NOT_MODIFIED && job.body.is_none();
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
                            unchanged,
                            reservation
                                .as_ref()
                                .and_then(|r| r.for_window(&job.quota(), reset))
                                .or_else(|| {
                                    probe
                                        .as_ref()?
                                        .0
                                        .observation(&job.quota(), reset, job.queued_at)
                                        .map(|mut observed| {
                                            if job.body.is_none() {
                                                observed.waited_for_quota = false;
                                            }
                                            observed
                                        })
                                }),
                        );
                    }
                    if let Some((debt, turn)) = &probe
                        && !unchanged
                    {
                        blocked_probes.charge(turn, debt, &mut budgets, &mut pending);
                    }
                    if let Some(reservation) = &reservation {
                        let released =
                            budgets.settle(reservation, unchanged && job.cached.is_some());
                        if !released.is_zero() {
                            tracing::info!(request_id=%job.request_id,
                                max_window_refund_ms=released.as_millis() as u64,
                                "Released unused GitHub pacing reservation");
                        }
                    }
                    if status == StatusCode::NOT_MODIFIED && !job.minting {
                        if job.body.is_some() {
                            self.finish(
                                job,
                                Err(Error::Invalid(
                                    "GitHub returned 304 for a nonconditional GraphQL request"
                                        .into(),
                                )),
                            );
                            continue;
                        }
                        if let Some(mut cached) = job.cached.take() {
                            // Validators and pagination metadata may be updated on 304.
                            cached.etag = header(&headers, "etag").or(cached.etag);
                            cached.last_modified =
                                header(&headers, "last-modified").or(cached.last_modified);
                            cached.link = header(&headers, "link").or(cached.link);
                            let (store, scope, key) =
                                (self.store.clone(), self.scope.clone(), job.key.clone());
                            self.persist(job, &mut active, async move {
                                store.revalidated(&scope, &key, cached).await
                            });
                        } else {
                            self.finish(
                                job,
                                Err(Error::Invalid(
                                    "GitHub returned 304 without a cached representation".into(),
                                )),
                            );
                        }
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
                            let outcome =
                                before_deadline(&job, read_body(response, max_body_bytes))
                                    .await
                                    .map_or(Attempt::Deadline, |bytes| Attempt::Body {
                                        status,
                                        headers,
                                        bytes,
                                    });
                            (job, outcome)
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
            let lifecycle_partial =
                if status.is_success() && !job.installation && !graphql_errors.is_empty() {
                    serde_json::from_slice(&bytes).ok().and_then(|data| {
                        crate::client::lifecycle::capture_partial(&job.key, job.body.as_ref(), data)
                    })
                } else {
                    None
                };
            if !graphql_errors.is_empty() && lifecycle_partial.is_none() {
                let partial = if status.is_success()
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
                    Some((key, response))
                } else {
                    None
                };
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
                if let Some((key, response)) = partial {
                    let (store, scope, request_id) = (
                        self.store.clone(),
                        self.scope.clone(),
                        job.request_id.clone(),
                    );
                    self.persist(job, &mut active, async move {
                        match store.put(&scope, &key, &response).await {
                            Ok(()) => tracing::info!(%request_id, "Retained permitted discovery nodes from incomplete response"),
                            Err(error) => tracing::warn!(%request_id, error_code=error.diagnostic_code(), "Partial discovery evidence could not be retained"),
                        }
                        Err(error)
                    });
                } else {
                    self.finish(job, Err(error));
                }
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
            let data = if let Some(data) = lifecycle_partial {
                Ok(data)
            } else if bytes.is_empty() {
                Ok(serde_json::Value::Null)
            } else {
                serde_json::from_slice(&bytes)
                    .map_err(|e| Error::Invalid(format!("invalid GitHub JSON: {e}")))
            };
            match data {
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
                    let (store, scope, key) =
                        (self.store.clone(), self.scope.clone(), job.key.clone());
                    self.persist(job, &mut active, async move {
                        store.put(&scope, &key, &response).await.map(|_| response)
                    });
                }
                Err(e) => self.finish(job, Err(e)),
            };
        }
    }

    fn finish(&self, job: Job, result: Result<Response>) {
        if matches!(
            &result,
            Err(Error::Auth(_)
                | Error::LocalAuth(_)
                | Error::GitHub {
                    status: 401 | 403,
                    ..
                }
                | Error::GraphQL {
                    access_denied: true,
                    ..
                })
        ) {
            // A successful cache payload may remain after a denied refresh.
            // Invalidate optional in-memory retry reuse before waking waiters.
            self.metrics
                .access_failure_epoch
                .fetch_add(1, Ordering::Relaxed);
        }
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
        let key_fingerprint = crate::digest(&job.key);
        tracing::info!(request_id=%job.request_id,endpoint=job.endpoint,resource=%job.resource,attempts=job.attempts+job.auth_attempts,succeeded=result.is_ok(),http_status=job.http_status,source,error_code=result.as_ref().err().map(Error::diagnostic_code),elapsed_ms=job.queued_at.elapsed().as_millis() as u64,
            request_key=%key_fingerprint,
            auth_scope=%if job.installation { self.config.installation.as_ref().unwrap().scope() } else { &self.scope },
            foreground=job.interactive(),
            completion_validation=job.completion_validation.load(Ordering::Relaxed),
            deadline_context,
            "GitHub request finished");
        let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        if result.is_ok() {
            inflight.record_success(key_fingerprint);
        }
        drop(job._permit);
        job.notify
            .send_replace(SharedResult::Complete(result.map(Arc::new)));
        inflight.active.remove(&job.key);
    }
}

fn ready(job: &Job, budgets: &Budgets, global: Instant) -> Instant {
    ready_with_probe(job, budgets, global, false)
}

fn ready_with_probe(job: &Job, budgets: &Budgets, global: Instant, probe: bool) -> Instant {
    let now = Instant::now();
    let stamp = now_ms();
    job.ready_at.max(global).max(
        budgets
            .for_resource(&job.quota())
            .map(|budget| {
                if conditional_budget_exempt(job, budget)
                    || (probe && pacing_probe_eligible(job, budget))
                {
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
                    let slot = if budget.remaining > QUOTA_RESERVE {
                        job.protected_pacing
                            .iter()
                            .find(|(reset, _)| *reset == budget.reset_at_seconds)
                            .map_or(budget.next, |(_, slot)| budget.next.min(*slot))
                    } else {
                        budget.next
                    };
                    slot.min(now.checked_add(until_reset).unwrap_or(budget.next))
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
    // 200 on every poll. Only previously unchanged representations get an
    // ordinary pacing exemption. A changed response revokes it automatically.
    job.body.is_none()
        && budget.remaining > QUOTA_RESERVE
        && job.cached.as_ref().is_some_and(|cached| {
            matches!(cached.source, Source::Revalidated)
                && (cached.etag.is_some() || cached.last_modified.is_some())
        })
}

fn pacing_probe_eligible(job: &Job, budget: &Budget) -> bool {
    // The first foreground REST validation may borrow an older paced turn too.
    // One probe per quota awaits headers; a charged/unknown reply repays the
    // interval and blocks further borrowing until that exact owed job leaves.
    // Foreground GraphQL reads and background shortcuts may use their own
    // selected slot early. Optional readers require congested REST fallback.
    // They always repay it, including transport failures and invalid replies;
    // the selected debt survives completion/cancellation.
    // Neither case grants an ordinary pacing exemption.
    budget.remaining > QUOTA_RESERVE + 1
        && (conditional_budget_exempt(job, budget)
            || (job.interactive()
                && job.resource == "core"
                && job.body.is_none()
                && job
                    .cached
                    .as_ref()
                    .is_some_and(|cached| cached.etag.is_some() || cached.last_modified.is_some()))
            || ((job.interactive() || !job.required_reader.load(Ordering::Relaxed))
                && job.resource == "graphql"
                && job.body.is_some()
                // GraphQL can charge multiple points. Keep the estimated
                // shared charge above the reserve in every live window.
                && budget.remaining.saturating_sub(QUOTA_RESERVE)
                    > budget.usage.share.ceil() as u64))
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
mod persistence_tests;
#[cfg(test)]
mod transport_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_rotation_serves_every_eligible_bucket_and_skips_unavailable_peers() {
        let mut order = QuotaOrder::default();
        let quotas = [
            "core",
            "installation/core",
            "graphql",
            "installation/graphql",
        ];
        for expected in quotas.into_iter().cycle().take(20) {
            let selected = order
                .choose(quotas.into_iter().map(|quota| (quota.to_owned(), false)))
                .unwrap();
            assert_eq!(selected, (expected.to_owned(), false));
            order.dispatched(selected, 256);
        }
        // Eligibility is decided by the real scheduler's quota/socket gates.
        // A bucket absent from that set must not reserve a dispatch turn.
        assert_eq!(
            order.choose([("graphql".into(), false)].into_iter()),
            Some(("graphql".into(), false))
        );
        assert!(order.choose(std::iter::empty()).is_none());
    }

    #[test]
    fn quota_rotation_history_is_bounded_and_repeated_dispatch_moves_only_that_bucket() {
        let mut order = QuotaOrder::default();
        for index in 0..1000 {
            order.dispatched((format!("resource-{index}"), false), 8);
            assert!(order.0.len() <= 8);
        }
        assert_eq!(order.0.front().unwrap().0, "resource-992");
        order.dispatched(("resource-992".into(), false), 8);
        assert_eq!(order.0.front().unwrap().0, "resource-993");
        assert_eq!(order.0.back().unwrap().0, "resource-992");
        assert_eq!(order.0.len(), 8);
    }

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
            required_reader: Arc::new(AtomicBool::new(true)),
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
            protected_pacing: Vec::new(),
            attempts: 0,
            resource: "core".into(),
            _permit: Arc::new(tokio::sync::Semaphore::new(1))
                .try_acquire_owned()
                .unwrap(),
        }
    }

    #[test]
    fn optional_fallback_keeps_personal_installation_and_rest_quota_waits_separate() {
        let mut optional = core_job();
        optional.required_reader.store(false, Ordering::Relaxed);
        optional.resource = "graphql".into();
        optional.body = Some(serde_json::json!({"query":"query { viewer { login } }"}));
        let personal = HashMap::from([("graphql".into(), true)]);
        let installation = HashMap::from([("installation/graphql".into(), true)]);
        let rest = HashMap::from([("core".into(), true)]);
        let background = HashMap::from([("graphql".into(), false)]);
        assert!(optional.defers_optional(&personal));
        assert!(!optional.defers_optional(&installation));
        assert!(!optional.defers_optional(&rest));
        assert!(!optional.defers_optional(&background));
        optional.interactive.store(false, Ordering::Relaxed);
        assert!(optional.defers_optional(&background));
        assert!(optional.defers_optional(&personal));
        optional.report_priority.store(true, Ordering::Relaxed);
        assert!(!optional.defers_optional(&background));
        assert!(optional.defers_optional(&personal));
        optional.installation = true;
        assert!(!optional.defers_optional(&personal));
        assert!(optional.defers_optional(&installation));
        optional.required_reader.store(true, Ordering::Relaxed);
        assert!(!optional.defers_optional(&installation));
    }

    #[test]
    fn optional_rest_fallback_respects_each_live_window_and_its_provider() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        let mut job = core_job();
        assert!(budgets.rest_fallback_has_headroom(&job));
        budgets.observe("installation/core", 0, reset, false, None);
        assert!(budgets.rest_fallback_has_headroom(&job));
        job.installation = true;
        assert!(!budgets.rest_fallback_has_headroom(&job));
        job.installation = false;
        budgets.observe("core", 5000, reset, false, None);
        assert!(budgets.rest_fallback_has_headroom(&job));
        budgets.observe("core", QUOTA_RESERVE, reset + 60, false, None);
        assert!(
            !budgets.rest_fallback_has_headroom(&job),
            "a newer window cannot erase the protected reserve"
        );
        let mut expired = Budgets::default();
        expired.observe("core", 0, now_ms() / 1000 - 2, false, None);
        assert!(
            expired.rest_fallback_has_headroom(&job),
            "expired windows must not suppress fallback"
        );
    }

    fn first_validator() -> Job {
        let mut job = core_job();
        job.cached = Some(Response {
            data: serde_json::json!({}),
            fetched_at_ms: 0,
            validated_at_ms: 0,
            source: Source::Network,
            etag: Some("synthetic".into()),
            last_modified: None,
            link: None,
        });
        job
    }

    #[tokio::test(start_paused = true)]
    async fn first_validator_borrows_only_when_selected_and_keeps_global_and_retry_gates() {
        let now = Instant::now();
        let reset = now_ms() / 1000 + 3600;
        for installation in [false, true] {
            let mut job = first_validator();
            job.installation = installation;
            let mut budgets = Budgets::default();
            budgets.observe(&job.quota(), 5000, reset, false, None);
            let paced = ready(&job, &budgets, now);
            assert!(paced > now);
            assert_eq!(ready_with_probe(&job, &budgets, now, true), now);
            let cooldown = now + Duration::from_secs(120);
            assert_eq!(ready_with_probe(&job, &budgets, cooldown, true), cooldown);
            job.ready_at = cooldown;
            assert_eq!(ready_with_probe(&job, &budgets, now, true), cooldown);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn first_validator_keeps_reserve_and_every_overlapping_window() {
        let now = Instant::now();
        let reset = now_ms() / 1000 + 3600;
        for remaining in [0, QUOTA_RESERVE, QUOTA_RESERVE + 1] {
            let job = first_validator();
            let mut budgets = Budgets::default();
            budgets.observe("core", 5000, reset, false, None);
            budgets.observe("core", remaining, reset + 60, false, None);
            assert_eq!(
                ready_with_probe(&job, &budgets, now, true),
                ready(&job, &budgets, now),
            );
            assert!(!pacing_probe_eligible(
                &job,
                &budgets.0["core"][&(reset + 60)]
            ));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn first_probe_requires_foreground_core_get_and_an_http_validator() {
        let reset = now_ms() / 1000 + 3600;
        let mut budgets = Budgets::default();
        budgets.observe("core", 5000, reset, false, None);
        let budget = &budgets.0["core"][&reset];
        for case in [
            "background",
            "search",
            "graphql",
            "body",
            "cold",
            "no-validator",
        ] {
            let mut job = first_validator();
            match case {
                "background" => job.interactive.store(false, Ordering::Relaxed),
                "search" | "graphql" => job.resource = case.into(),
                "body" => job.body = Some(serde_json::json!({"query":"synthetic"})),
                "cold" => job.cached = None,
                "no-validator" => job.cached.as_mut().unwrap().etag = None,
                _ => unreachable!(),
            }
            assert!(!pacing_probe_eligible(&job, budget), "{case}");
        }
        let mut promoted = first_validator();
        promoted.interactive.store(false, Ordering::Relaxed);
        promoted.report_priority.store(true, Ordering::Relaxed);
        promoted.cached.as_mut().unwrap().etag = None;
        promoted.cached.as_mut().unwrap().last_modified = Some("synthetic-date".into());
        assert!(pacing_probe_eligible(&promoted, budget));
        assert!(!conditional_budget_exempt(&promoted, budget));
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
    async fn free_reservations_release_all_live_windows_only_in_their_auth_quota() {
        for installation in [false, true] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            let mut job = core_job();
            job.installation = installation;
            let quota = job.quota();
            let peer = if installation {
                "core"
            } else {
                "installation/core"
            };
            for window in [reset, reset + 600] {
                budgets.observe(&quota, 5000, window, true, None);
            }
            budgets.observe(peer, 5000, reset, false, None);
            let peer_slot = budgets.for_resource(peer).next().unwrap().next;
            let reserved = budgets.reserve(&mut job);
            tokio::time::advance(Duration::from_millis(100)).await;
            budgets.observe(
                &quota,
                5000,
                reset,
                true,
                reserved.for_window(&quota, reset),
            );
            budgets.settle(&reserved, true);
            assert!(
                budgets
                    .for_resource(&quota)
                    .all(|b| b.next <= Instant::now() && b.pacing.pending() == 0)
            );
            assert_eq!(budgets.for_resource(peer).next().unwrap().next, peer_slot);
            let after = ready(&job, &budgets, Instant::now());
            budgets.settle(&reserved, true);
            assert_eq!(
                ready(&job, &budgets, Instant::now()),
                after,
                "a repeated settlement spent the credit twice"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn free_reservation_releases_only_its_share_of_a_later_charged_probe() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, false, None);
        let turn = budgets.for_resource("core").next().unwrap().next;
        tokio::time::advance(turn - Instant::now()).await;
        let free = budgets.reserve(&mut core_job());
        let credit = budgets.0["core"][&reset].spacing;
        let probe = budgets.probe("core");
        tokio::time::advance(Duration::from_millis(100)).await;
        budgets.observe(
            "core",
            4999,
            reset,
            false,
            probe.observation("core", reset, Instant::now()),
        );
        budgets.charge_probe(&probe);
        let before = budgets.for_resource("core").next().unwrap().next;
        let paid_floor = Instant::now() + budgets.0["core"][&reset].spacing;
        budgets.observe("core", 4999, reset, true, free.for_window("core", reset));
        budgets.settle(&free, true);
        let next = budgets.for_resource("core").next().unwrap().next;
        assert_eq!(next, (before - credit).max(paid_floor));
        assert!(
            next < before && next > Instant::now(),
            "free work must release its interval and retain the paid probe's interval"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn free_reservations_preserve_interleaved_charged_and_unknown_attempts() {
        for charged_first in [false, true] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            budgets.observe("core", 5000, reset, true, None);
            let free = budgets.reserve(&mut core_job());
            let paid = budgets.reserve(&mut core_job());
            let third = budgets.reserve(&mut core_job());
            let before = budgets.0["core"][&reset].next;
            let credit = budgets.0["core"][&reset].spacing;
            if charged_first {
                budgets.observe("core", 5000, reset, false, paid.for_window("core", reset));
                budgets.settle(&paid, false);
            }
            budgets.settle(&free, true);
            assert_eq!(budgets.0["core"][&reset].next, before - credit);
            if !charged_first {
                budgets.observe("core", 5000, reset, false, paid.for_window("core", reset));
                budgets.settle(&paid, false);
            }
            budgets.settle(&third, false);
            assert_eq!(budgets.0["core"][&reset].next, before - credit);
            assert!(budgets.0["core"][&reset].pacing.pending() == 0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn expired_reservation_cannot_refund_a_later_independent_turn() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, true, None);
        let old = budgets.reserve(&mut core_job());
        tokio::time::advance(Duration::from_secs(5)).await;
        let later = budgets.reserve(&mut core_job());
        let another = budgets.reserve(&mut core_job());
        let before = budgets.0["core"][&reset].next;
        budgets.settle(&old, true);
        assert_eq!(budgets.0["core"][&reset].next, before);
        budgets.settle(&later, false);
        budgets.settle(&another, false);
        assert!(budgets.0["core"][&reset].pacing.pending() == 0);
    }

    #[tokio::test(start_paused = true)]
    async fn absorbed_free_credit_cannot_release_later_protected_reservations() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, true, None);
        let old = budgets.reserve(&mut core_job());
        // A larger shared charge replaces the entire earlier reserved wait.
        budgets.observe("core", 500, reset, false, None);
        let paid = budgets.reserve(&mut core_job());
        let before = budgets.0["core"][&reset].next;
        budgets.settle(&old, true);
        assert_eq!(budgets.0["core"][&reset].next, before);
        budgets.settle(&paid, false);
        assert!(budgets.0["core"][&reset].pacing.pending() == 0);
    }

    #[tokio::test(start_paused = true)]
    async fn free_response_preserves_hard_reserves_new_windows_and_cooldowns() {
        for remaining in [0, 50, QUOTA_RESERVE] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            budgets.observe("core", 5000, reset, true, None);
            let free = budgets.reserve(&mut core_job());
            budgets.observe(
                "core",
                remaining,
                reset,
                true,
                free.for_window("core", reset),
            );
            budgets.settle(&free, true);
            assert!(
                ready(&core_job(), &budgets, Instant::now())
                    > Instant::now() + Duration::from_secs(3500)
            );
        }
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, true, None);
        let free = budgets.reserve(&mut core_job());
        budgets.exhausted("core", reset + 60, Duration::from_secs(120));
        budgets.settle(&free, true);
        assert_eq!(
            ready(&core_job(), &budgets, Instant::now()),
            Instant::now() + Duration::from_secs(120)
        );
        assert_eq!(
            ready(
                &core_job(),
                &budgets,
                Instant::now() + Duration::from_secs(300)
            ),
            Instant::now() + Duration::from_secs(300)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reservation_refunds_never_undercut_a_queue_with_free_reads_removed() {
        // Counterfactual lower bound: replay dispatches with every ultimately
        // free read removed. Paid reservations, response floors and probes
        // still cost their complete interval, independent of response order.
        enum Cost {
            Reserve(usize, Instant, Duration),
            Floor(Instant),
            Probe(Instant, Duration),
        }
        fn ideal(start: Instant, free_mask: usize, costs: &[Cost]) -> Instant {
            costs.iter().fold(start, |next, cost| match *cost {
                Cost::Reserve(id, at, spacing) if id >= 3 || free_mask & (1 << id) == 0 => {
                    next.max(at) + spacing
                }
                Cost::Reserve(..) => next,
                Cost::Floor(floor) => next.max(floor),
                Cost::Probe(at, spacing) => next.max(at) + spacing,
            })
        }
        for free_mask in 0..8 {
            for gap in [
                Duration::ZERO,
                Duration::from_millis(100),
                Duration::from_secs(3),
            ] {
                for remaining in [500, 5000] {
                    for order in [
                        [0, 1, 2],
                        [0, 2, 1],
                        [1, 0, 2],
                        [1, 2, 0],
                        [2, 0, 1],
                        [2, 1, 0],
                    ] {
                        let start = Instant::now();
                        let reset = now_ms() / 1000 + 3600;
                        let mut budgets = Budgets::default();
                        budgets.observe("core", 5000, reset, true, None);
                        let mut reservations = Vec::new();
                        let mut costs = Vec::new();
                        for id in 0..3 {
                            tokio::time::advance(gap).await;
                            costs.push(Cost::Reserve(
                                id,
                                Instant::now(),
                                budgets.0["core"][&reset].spacing,
                            ));
                            reservations.push(budgets.reserve(&mut core_job()));
                        }
                        let probe = budgets.probe("core");
                        budgets.observe(
                            "core",
                            remaining,
                            reset,
                            false,
                            probe.observation("core", reset, Instant::now()),
                        );
                        costs.push(Cost::Probe(
                            Instant::now(),
                            budgets.0["core"][&reset].spacing,
                        ));
                        budgets.charge_probe(&probe);
                        for id in order {
                            let free = free_mask & (1 << id) != 0;
                            if !free {
                                budgets.observe(
                                    "core",
                                    remaining,
                                    reset,
                                    false,
                                    reservations[id].for_window("core", reset),
                                );
                                costs.push(Cost::Floor(
                                    reservations[id].dispatched_at
                                        + budgets.0["core"][&reset].spacing,
                                ));
                            }
                            budgets.settle(&reservations[id], free);
                            let ideal = ideal(start, free_mask, &costs);
                            assert!(
                                budgets.0["core"][&reset].next.max(Instant::now())
                                    >= ideal.max(Instant::now()),
                                "refund erased paid work: mask={free_mask}, gap={gap:?}, remaining={remaining}, order={order:?}, completed={id}"
                            );
                            // A refund arriving after this additional turn must
                            // not spend a credit absorbed by an older floor.
                            costs.push(Cost::Reserve(
                                3,
                                Instant::now(),
                                budgets.0["core"][&reset].spacing,
                            ));
                            let later = budgets.reserve(&mut core_job());
                            budgets.settle(&later, false);
                        }
                        assert_eq!(
                            budgets.0["core"][&reset].next.max(Instant::now()),
                            ideal(start, free_mask, &costs).max(Instant::now()),
                            "settled free work retained pacing debt"
                        );
                        assert!(budgets.0["core"][&reset].pacing.pending() == 0);
                    }
                }
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn slow_reserved_response_does_not_charge_pacing_twice() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, true, None);
        let reservation = budgets.reserve(&mut core_job());
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
        let first = budgets.reserve(&mut core_job());
        tokio::time::advance(Duration::from_secs(5)).await;
        budgets.reserve(&mut core_job());
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
        budgets.reserve(&mut core_job());
        let prior = budgets.for_resource("core").next().unwrap().next;
        let probe = budgets.probe("core");
        budgets.observe(
            "core",
            4999,
            reset,
            false,
            probe.observation("core", reset, Instant::now()),
        );
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
        budgets.observe(
            "core",
            4999,
            reset,
            false,
            probe.observation("core", reset, Instant::now()),
        );
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
    async fn selected_probe_debt_survives_departure_and_keeps_each_auth_quota_separate() {
        for quota in [
            "core",
            "installation/core",
            "graphql",
            "installation/graphql",
        ] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            budgets.observe(quota, 5000, reset, false, None);
            let turn = ProbeTurn {
                quota: quota.into(),
                owed: "finished".into(),
                owns_turn: true,
                protect_turn: false,
            };
            let debt = budgets.probe(quota);
            let mut blocks = ProbeBlocks::default();
            let mut pending = VecDeque::new();
            blocks.charge(&turn, &debt, &mut budgets, &mut pending);
            let paid_at = blocks.wake(Instant::now()).unwrap();
            blocks.retain(&pending, &budgets, Instant::now());
            assert!(
                blocks.contains(quota),
                "A completed or canceled probe erased its debt"
            );
            let other_auth = quota
                .strip_prefix("installation/")
                .map(str::to_owned)
                .unwrap_or_else(|| format!("installation/{quota}"));
            assert!(!blocks.contains(&other_auth));
            // Other reservations cannot erase the debt or postpone its own wake.
            let mut later = core_job();
            later.installation = quota.starts_with("installation/");
            later.resource = quota.rsplit('/').next().unwrap().into();
            budgets.reserve(&mut later);
            assert_eq!(blocks.wake(Instant::now()), Some(paid_at));
            tokio::time::advance(paid_at - Instant::now()).await;
            blocks.retain(&pending, &budgets, Instant::now());
            assert!(!blocks.contains(quota));
            assert!(blocks.wake(Instant::now()).is_none());
            assert!(
                ready(&later, &budgets, Instant::now()) > Instant::now(),
                "Debt expiry erased a later reservation"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn selected_probe_keeps_other_live_windows_when_one_slot_expires() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, false, None);
        budgets.observe("core", 1000, reset + 60, false, None);
        let turn = ProbeTurn {
            quota: "core".into(),
            owed: "finished".into(),
            owns_turn: true,
            protect_turn: false,
        };
        let debt = budgets.probe("core");
        let mut blocks = ProbeBlocks::default();
        let mut pending = VecDeque::new();
        blocks.charge(&turn, &debt, &mut budgets, &mut pending);
        let first = blocks.wake(Instant::now()).unwrap();
        tokio::time::advance(first - Instant::now()).await;
        blocks.retain(&pending, &budgets, Instant::now());
        assert!(blocks.contains("core"));
        assert!(blocks.wake(Instant::now()).unwrap() > Instant::now());
        // Pruning an expired quota window also releases its old borrow block.
        budgets.0.get_mut("core").unwrap().remove(&(reset + 60));
        blocks.retain(&pending, &budgets, Instant::now());
        assert!(!blocks.contains("core"));
    }

    #[tokio::test(start_paused = true)]
    async fn protected_completion_keeps_its_slot_and_repays_borrowing_after_dispatch() {
        for charged_headers in [false, true] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            budgets.observe("core", 5000, reset, false, None);
            let debt = budgets.probe("core");
            let slot = debt.windows[0].1;
            let mut pending = VecDeque::from([core_job(), core_job()]);
            pending[1].request_id = "other".into();
            if charged_headers {
                budgets.observe("core", 4999, reset, false, None);
            }
            budgets.charge_probe(&debt);
            ProbeTurn {
                quota: "core".into(),
                owed: "test".into(),
                owns_turn: false,
                protect_turn: true,
            }
            .protect(&debt, &mut pending);
            let now = Instant::now();
            assert_eq!(ready(&pending[0], &budgets, now), slot);
            let paid_slot = budgets.0["core"][&reset].next;
            assert_eq!(ready(&pending[1], &budgets, now), paid_slot);
            assert!(paid_slot > slot);
            tokio::time::advance(slot - now).await;
            let reservation = budgets.reserve(&mut pending[0]);
            let budget = &budgets.0["core"][&reset];
            assert_eq!(budget.next, paid_slot + budget.spacing);
            assert!(
                pending[0].protected_pacing.is_empty(),
                "retry reused its slot"
            );
            budgets.observe(
                "core",
                4998,
                reset,
                false,
                reservation.for_window("core", reset),
            );
            assert!(ready(&pending[0], &budgets, now) > paid_slot);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn protected_completion_retains_hard_gates_and_unreserved_windows() {
        for remaining in [0, 100, 101, 4999] {
            let mut budgets = Budgets::default();
            let reset = now_ms() / 1000 + 3600;
            budgets.observe("core", 5000, reset, false, None);
            let mut job = core_job();
            job.protected_pacing = budgets.probe("core").windows;
            budgets.observe("core", remaining, reset, false, None);
            let now = Instant::now();
            if remaining <= QUOTA_RESERVE {
                assert!(ready(&job, &budgets, now) > now + Duration::from_secs(3000));
            } else {
                assert_eq!(ready(&job, &budgets, now), job.protected_pacing[0].1);
            }
            let delayed = ready(&job, &budgets, now) + Duration::from_secs(10);
            assert_eq!(
                ready(&job, &budgets, delayed),
                delayed,
                "shared backoff bypassed"
            );
            job.ready_at = delayed;
            assert_eq!(
                ready(&job, &budgets, now),
                delayed,
                "retry backoff bypassed"
            );
            job.ready_at = now;
            // A newly observed overlapping window has no reserved slot.
            budgets.observe("core", 101, reset + 600, false, None);
            assert!(ready(&job, &budgets, now) > now + Duration::from_secs(3000));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_a_protected_completion_does_not_cancel_its_probe_debt() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("core", 5000, reset, false, None);
        let debt = budgets.probe("core");
        budgets.charge_probe(&debt);
        let mut pending = VecDeque::from([core_job()]);
        let turn = ProbeTurn {
            quota: "core".into(),
            owed: "test".into(),
            owns_turn: false,
            protect_turn: true,
        };
        turn.protect(&debt, &mut pending);
        pending.clear();
        let mut replacement = core_job();
        replacement.request_id = "replacement".into();
        pending.push_back(replacement);
        turn.protect(&debt, &mut pending);
        assert!(pending[0].protected_pacing.is_empty());
        assert_eq!(
            ready(&pending[0], &budgets, Instant::now()),
            budgets.0["core"][&reset].next
        );
        assert!(budgets.0["core"][&reset].next > debt.windows[0].1);
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
        let exempt = budgets.reserve(&mut probe);
        assert!(exempt.for_window("core", reset).is_none());
        let reserved = budgets.reserve(&mut core_job());
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
            let reservation = budgets.reserve(&mut job);
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
            let reservation = budgets.reserve(&mut waiting);
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
            let reservation = budgets.reserve(&mut resumed);
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

    #[tokio::test(start_paused = true)]
    async fn borrowed_graphql_slot_retains_shared_point_consumption_estimate() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("graphql", 5000, reset, false, None);
        for i in 1..=6 {
            tokio::time::advance(Duration::from_secs(5)).await;
            budgets.observe("graphql", 5000 - i * 60, reset, false, None);
        }
        let queued = Instant::now();
        let loan = budgets.probe("graphql");
        tokio::time::advance(Duration::from_secs(35)).await;
        let observation = loan.observation("graphql", reset, queued).unwrap();
        assert!(observation.waited_for_quota);
        assert!(
            loan.observation("installation/graphql", reset, queued)
                .is_none()
        );
        assert!(loan.observation("graphql", reset + 60, queued).is_none());
        budgets.observe("graphql", 4580, reset, false, Some(observation));
        assert_eq!(
            budgets.for_resource("graphql").next().unwrap().usage.share,
            60.0
        );
    }

    #[tokio::test]
    async fn graphql_borrowing_keeps_room_for_the_estimated_point_charge() {
        let mut budgets = Budgets::default();
        let reset = now_ms() / 1000 + 3600;
        budgets.observe("graphql", 5000, reset, false, None);
        let budget = budgets
            .0
            .get_mut("graphql")
            .unwrap()
            .get_mut(&reset)
            .unwrap();
        budget.usage.share = 20.0;
        let mut job = core_job();
        job.resource = "graphql".into();
        job.body = Some(serde_json::json!({"query":"{ viewer { login } }"}));
        job.interactive.store(true, Ordering::Relaxed);
        for required in [false, true] {
            job.required_reader.store(required, Ordering::Relaxed);
            for remaining in [101, 119, 120, 121] {
                budget.remaining = remaining;
                assert_eq!(pacing_probe_eligible(&job, budget), remaining > 120);
            }
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
