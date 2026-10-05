//! Background roster scans retain healthy page progress through quota pacing.
use crate::{Error, Result};
use std::{future::Future, time::Duration};
use tokio::{sync::watch, time::Instant};

tokio::task_local! { static PROGRESS: watch::Sender<Instant>; }

pub(super) async fn background<T>(read: impl Future<Output = T>) -> T {
    let (progress, _) = watch::channel(Instant::now());
    PROGRESS.scope(progress, read).await
}

pub(super) fn page_validated() {
    let _ = PROGRESS.try_with(|progress| progress.send_replace(Instant::now()));
}

pub(super) async fn scan<T>(
    mut deadline: Instant,
    page_timeout: Duration,
    read: impl Future<Output = Result<T>>,
) -> Result<T> {
    let Ok(mut progress) = PROGRESS.try_with(|progress| progress.subscribe()) else {
        return tokio::time::timeout_at(deadline, read)
            .await
            .unwrap_or(Err(Error::Deadline));
    };
    // Only the dedicated background scanner gets a rolling page budget.
    // Lock acquisition, interactive callers and individual scheduler requests
    // keep their deadlines. The scan still validates every page, its starting
    // cohort, source clocks, the page cap and the shared collection byte limit.
    // A slow or queued page cannot renew this deadline without valid evidence.
    tokio::pin!(read);
    loop {
        tokio::select! {
            biased;
            result = &mut read => return result,
            changed = progress.changed() => {
                if changed.is_err() { return Err(Error::Stopped); }
                deadline = *progress.borrow_and_update() + page_timeout;
            },
            _ = tokio::time::sleep_until(deadline) => return Err(Error::Deadline),
        }
    }
}
