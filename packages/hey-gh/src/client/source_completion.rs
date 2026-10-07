//! Only live source groups share a report's final completion turn. Requests
//! coalesced with another reader use the same queue flag; no work is duplicated.
use std::{
    future::Future,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

struct Group {
    completing: Arc<AtomicBool>,
    pending: Mutex<Vec<Weak<PendingRequest>>>,
    parent: Option<Arc<Group>>,
}

pub(super) struct PendingRequest(Arc<AtomicBool>);

tokio::task_local! { static SOURCE: Arc<Group>; }

pub(super) fn track(priority: &Arc<AtomicBool>) -> Option<Vec<Arc<PendingRequest>>> {
    SOURCE
        .try_with(|group| {
            // Nested CI groups still belong to a full report's source group.
            // Keep one guard per ancestor so either collection can promote
            // the same shared request without retaining cancelled groups.
            let mut requests = Vec::new();
            let mut current = Some(group.as_ref());
            while let Some(group) = current {
                let mut pending = group.pending.lock().unwrap_or_else(|e| e.into_inner());
                if group.completing.load(Ordering::Acquire) {
                    priority.store(true, Ordering::Relaxed);
                } else {
                    pending.retain(|request| request.strong_count() > 0);
                    let request = pending
                        .iter()
                        .filter_map(Weak::upgrade)
                        .find(|request| Arc::ptr_eq(&request.0, priority))
                        .unwrap_or_else(|| {
                            let request = Arc::new(PendingRequest(priority.clone()));
                            pending.push(Arc::downgrade(&request));
                            request
                        });
                    requests.push(request);
                }
                current = group.parent.as_deref();
            }
            requests
        })
        .ok()
        .filter(|requests| !requests.is_empty())
}

pub(crate) struct SourceCompletion {
    completing: Arc<AtomicBool>,
    groups: Mutex<Vec<Weak<Group>>>,
    changed: Arc<Notify>,
}

impl SourceCompletion {
    pub(super) fn new(changed: Arc<Notify>) -> Self {
        Self {
            completing: Arc::new(AtomicBool::new(false)),
            groups: Mutex::new(Vec::new()),
            changed,
        }
    }

    pub(crate) async fn collect<T>(&self, read: impl Future<Output = T>) -> T {
        let group = Arc::new(Group {
            completing: self.completing.clone(),
            pending: Mutex::new(Vec::new()),
            parent: SOURCE.try_with(Arc::clone).ok(),
        });
        {
            let mut groups = self.groups.lock().unwrap_or_else(|e| e.into_inner());
            groups.retain(|group| group.strong_count() > 0);
            groups.push(Arc::downgrade(&group));
        }
        // Completion or cancellation drops the scope and its group. A request
        // still retained by an unrelated consumer cannot inherit its priority.
        SOURCE.scope(group, read).await
    }

    pub(crate) fn promote(&self) {
        self.completing.store(true, Ordering::Release);
        for group in self
            .groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(Weak::upgrade)
        {
            for request in group
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
                .filter_map(|flag| flag.upgrade())
            {
                request.0.store(true, Ordering::Relaxed);
            }
        }
        self.changed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn poll_pending(future: std::pin::Pin<&mut impl Future>) {
        let mut future = future;
        std::future::poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await
    }

    #[tokio::test]
    async fn only_live_groups_promote_all_pending_and_later_requests() {
        let tail = SourceCompletion::new(Arc::new(Notify::new()));
        let completed = Arc::new(AtomicBool::new(false));
        tail.collect(async {
            let _read = track(&completed);
        })
        .await;
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut cancelled_read = Box::pin(tail.collect(async {
            let _read = track(&cancelled);
            std::future::pending::<()>().await;
        }));
        poll_pending(cancelled_read.as_mut()).await;
        drop(cancelled_read);
        let first = Arc::new(AtomicBool::new(false));
        let parallel = Arc::new(AtomicBool::new(false));
        let next_page = Arc::new(AtomicBool::new(false));
        let abandoned = Arc::new(AtomicBool::new(false));
        let next = Notify::new();
        let mut live = Box::pin(tail.collect(async {
            let _first_read = track(&first);
            let _parallel_read = track(&parallel);
            drop(track(&abandoned));
            next.notified().await;
            track(&next_page);
        }));
        poll_pending(live.as_mut()).await;
        assert!(!first.load(Ordering::Relaxed));
        tail.promote();
        assert!(first.load(Ordering::Relaxed));
        assert!(parallel.load(Ordering::Relaxed));
        assert!(!completed.load(Ordering::Relaxed));
        assert!(!cancelled.load(Ordering::Relaxed));
        assert!(!abandoned.load(Ordering::Relaxed));
        next.notify_one();
        live.await;
        assert!(next_page.load(Ordering::Relaxed));
        let late_group = Arc::new(AtomicBool::new(false));
        tail.collect(async {
            track(&late_group);
        })
        .await;
        assert!(late_group.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn registration_retains_only_distinct_live_requests() {
        let tail = SourceCompletion::new(Arc::new(Notify::new()));
        tail.collect(async {
            let shared = Arc::new(AtomicBool::new(false));
            let _read = track(&shared);
            for _ in 0..1000 {
                track(&Arc::new(AtomicBool::new(false)));
                track(&shared);
                track(&shared);
            }
            SOURCE.with(|group| assert_eq!(group.pending.lock().unwrap().len(), 1));
        })
        .await;
    }

    #[tokio::test]
    async fn nested_groups_keep_parent_completion_and_release_cancelled_tracking() {
        for cancel in [false, true] {
            let parent = SourceCompletion::new(Arc::new(Notify::new()));
            let child = SourceCompletion::new(Arc::new(Notify::new()));
            let flag = Arc::new(AtomicBool::new(false));
            let mut read = Box::pin(parent.collect(child.collect(async {
                let _request = track(&flag);
                for _ in 0..1000 {
                    track(&flag);
                }
                SOURCE.with(|group| {
                    assert_eq!(group.pending.lock().unwrap().len(), 1);
                    assert_eq!(
                        group.parent.as_ref().unwrap().pending.lock().unwrap().len(),
                        1
                    );
                });
                std::future::pending::<()>().await;
            })));
            poll_pending(read.as_mut()).await;
            if cancel {
                drop(read);
            }
            parent.promote();
            assert_eq!(
                flag.load(Ordering::Relaxed),
                !cancel,
                "nested tracking must preserve parent promotion and cancellation"
            );
            let later = Arc::new(AtomicBool::new(false));
            parent
                .collect(child.collect(async {
                    track(&later);
                }))
                .await;
            assert!(
                later.load(Ordering::Relaxed),
                "later children inherit a completing parent"
            );
        }
    }
}
