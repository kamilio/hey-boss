//! Bound collection stalls without charging deliberate scheduler queue waits.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::{sync::Notify, time::Instant};

pub(crate) const STALL_LIMIT: Duration = Duration::from_secs(5);
tokio::task_local! { pub(crate) static CURRENT: Arc<Budget>; }

pub(crate) struct Budget {
    state: Mutex<State>,
    changed: Notify,
    retained_pages: AtomicBool,
    details_collected: AtomicBool,
}

// Only newly validated terminal job pages can advance an interrupted
// collection on its next turn. Cache hits and mutable sources cannot renew it.
pub(crate) fn retained_completed_page() {
    let _ = CURRENT.try_with(|budget| budget.retained_pages.store(true, Ordering::Relaxed));
}

pub(crate) fn completed_details() {
    let _ = CURRENT.try_with(|budget| budget.details_collected.store(true, Ordering::Relaxed));
}

struct State {
    remaining: Duration,
    updated: Instant,
    queued: usize,
    active: usize,
}

impl State {
    fn charge(&mut self) {
        let now = Instant::now();
        if self.queued == 0 || self.active > 0 {
            self.remaining = self.remaining.saturating_sub(now - self.updated);
        }
        self.updated = now;
    }
}

impl Budget {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                remaining: STALL_LIMIT,
                updated: Instant::now(),
                queued: 0,
                active: 0,
            }),
            changed: Notify::new(),
            retained_pages: AtomicBool::new(false),
            details_collected: AtomicBool::new(false),
        })
    }

    pub(crate) fn has_retained_pages(&self) -> bool {
        self.retained_pages.load(Ordering::Relaxed)
    }

    pub(crate) fn has_collected_details(&self) -> bool {
        self.details_collected.load(Ordering::Relaxed)
    }

    pub(crate) async fn exhausted(&self) {
        loop {
            let changed = self.changed.notified();
            // Register before examining state so a dispatch cannot lose its wake.
            tokio::pin!(changed);
            changed.as_mut().enable();
            let wait = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.charge();
                if state.remaining.is_zero() {
                    return;
                }
                (state.queued == 0 || state.active > 0).then_some(state.remaining)
            };
            tokio::select! {
                _ = changed => {},
                _ = async {
                    match wait {
                        Some(wait) => tokio::time::sleep(wait).await,
                        None => std::future::pending().await,
                    }
                } => {},
            }
        }
    }

    fn change(&self, previous: Option<bool>, next: Option<bool>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.charge();
        if let Some(queued) = previous {
            if queued {
                state.queued -= 1;
            } else {
                state.active -= 1;
            }
        }
        if let Some(queued) = next {
            if queued {
                state.queued += 1;
            } else {
                state.active += 1;
            }
        }
        drop(state);
        self.changed.notify_waiters();
    }
}

/// Each coalesced caller owns its own guard. A queued sibling cannot conceal
/// a stalled active request; concurrent network time is charged only once.
pub(crate) struct Wait {
    budget: Arc<Budget>,
    queued: bool,
}

impl Wait {
    pub(crate) fn current(queued: bool) -> Option<Self> {
        CURRENT
            .try_with(|budget| {
                budget.change(None, Some(queued));
                Self {
                    budget: budget.clone(),
                    queued,
                }
            })
            .ok()
    }

    pub(crate) fn update(&mut self, queued: bool) {
        if self.queued != queued {
            self.budget.change(Some(self.queued), Some(queued));
            self.queued = queued;
        }
    }

    pub(crate) fn completed(&self) {
        let mut state = self.budget.state.lock().unwrap_or_else(|e| e.into_inner());
        state.remaining = STALL_LIMIT;
        state.updated = Instant::now();
        drop(state);
        self.budget.changed.notify_waiters();
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        self.budget.change(Some(self.queued), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn successful_sources_renew_progress_but_the_next_stall_still_expires() {
        let budget = Budget::new();
        CURRENT
            .scope(budget.clone(), async {
                let wait = Wait::current(false).unwrap();
                for _ in 0..3 {
                    tokio::select! {
                        _ = budget.exhausted() => panic!("healthy collection was interrupted"),
                        _ = tokio::time::sleep(Duration::from_secs(3)) => wait.completed(),
                    }
                }
                let started = Instant::now();
                budget.exhausted().await;
                assert_eq!(Instant::now() - started, STALL_LIMIT);
            })
            .await;
    }

    #[tokio::test(start_paused = true)]
    async fn queued_time_is_free_but_active_siblings_and_dropped_waiters_are_charged() {
        let budget = Budget::new();
        CURRENT
            .scope(budget.clone(), async {
                let mut first = Wait::current(true).unwrap();
                let second = Wait::current(true).unwrap();
                tokio::time::advance(Duration::from_secs(40)).await;
                first.update(false);
                tokio::time::advance(Duration::from_secs(3)).await;
                drop(first);
                tokio::time::advance(Duration::from_secs(40)).await;
                drop(second);
                let started = Instant::now();
                budget.exhausted().await;
                assert_eq!(Instant::now() - started, Duration::from_secs(2));
            })
            .await;
    }
}
