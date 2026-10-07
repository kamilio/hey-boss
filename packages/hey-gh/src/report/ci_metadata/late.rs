//! Consume newly validated selectors while REST sources are still queued.
use super::{CachedList, CommitList, commit_summaries};
use crate::{Client, Freshness, ResourceValidation, Result, report::VALIDATIONS};
use serde_json::Value;
use std::{cell::RefCell, future::Future};
use tokio::sync::watch;

tokio::task_local! { static METADATA_READY: watch::Receiver<bool>; }

pub(super) fn metadata_pending() -> bool {
    METADATA_READY
        .try_with(|ready| !*ready.borrow())
        .unwrap_or(false)
}

async fn staged<T>(read: impl Future<Output = T>) -> (T, Vec<ResourceValidation>) {
    VALIDATIONS
        .scope(RefCell::new(Vec::new()), async {
            let result = read.await;
            (result, VALIDATIONS.with(|records| records.take()))
        })
        .await
}

fn consume<T>((result, validations): (T, Vec<ResourceValidation>)) -> T {
    let _ = VALIDATIONS.try_with(|records| records.borrow_mut().extend(validations));
    result
}

async fn ready(receiver: &mut Option<watch::Receiver<bool>>) {
    if let Some(receiver) = receiver {
        drop(receiver.wait_for(|done| *done).await);
    } else {
        std::future::pending::<()>().await;
    }
}

impl Client {
    pub(in crate::report) async fn collect_with_ci_metadata<V, C>(
        &self,
        metadata: impl Future<Output = V>,
        collection: impl Future<Output = C>,
    ) -> (V, C) {
        let (ready, receiver) = watch::channel(false);
        self.collect_with_pending_validation(
            async {
                let result = metadata.await;
                ready.send_replace(true);
                result
            },
            METADATA_READY.scope(receiver, collection),
        )
        .await
    }

    pub(in crate::report) async fn commit_list_from_metadata(
        &self,
        repository: &str,
        sha: &str,
        rest_path: &str,
        list: CommitList,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        // Keep the pagination future off the surrounding PR/report stack.
        let read = Box::pin(self.load_commit_list(repository, sha, rest_path, list, freshness));
        let mut metadata = METADATA_READY.try_with(Clone::clone).ok().filter(|ready| {
            matches!(freshness, Freshness::MaxAge(age) if !age.is_zero()) && !*ready.borrow()
        });
        let mut summary = commit_summaries::ready()
            .filter(|_| matches!(freshness, Freshness::MaxAge(age) if !age.is_zero()));
        if metadata.is_none() && summary.is_none() {
            return read.await;
        }
        // REST starts normally while metadata is pending. Once selectors are
        // ready, reuse their evidence; closed PRs can also use their collection's
        // bounded, shared commit-summary fallback. Never restart the REST read.
        let read = staged(read);
        tokio::pin!(read);
        let late = Box::pin(async {
            loop {
                tokio::select! {
                    _ = ready(&mut metadata) => metadata = None,
                    _ = ready(&mut summary) => summary = None,
                }
                let result =
                    staged(self.reusable_commit_list(repository, sha, rest_path, list, freshness))
                        .await;
                if matches!(&result.0, Ok(CachedList::Ready(_)))
                    || result
                        .0
                        .as_ref()
                        .is_err_and(commit_summaries::required_error)
                    || (metadata.is_none() && summary.is_none())
                {
                    return result;
                }
            }
        });
        tokio::select! {
            // Prefer a completed direct read, including its explicit errors.
            biased;
            result = &mut read => consume(result),
            (late, validations) = late => {
                if let Err(error) = &late
                    && commit_summaries::required_error(error)
                {
                    consume((Err(error.clone()), validations))
                } else if let Ok(CachedList::Ready(values)) = late {
                    // Dropping this receiver abandons only our REST wait.
                    // Coalesced readers and active responses still finish.
                    // Only consumed proof clocks reach the report.
                    tracing::info!(source=list.label(), "CI list reused newly validated metadata");
                    consume((Ok(values), validations))
                } else {
                    // Missing, changed or unusable optional proof retains the
                    // original read, its errors, and its original deadline.
                    consume(read.await)
                }
            }
        }
    }
}
