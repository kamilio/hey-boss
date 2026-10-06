//! Scheduling follows the earliest live caller; transport keeps the shared lifetime.
use super::*;

#[derive(Default)]
pub(crate) struct WaitingDeadlines(Mutex<BTreeMap<Instant, usize>>);

impl WaitingDeadlines {
    pub(crate) fn earliest(&self) -> Option<Instant> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .next()
            .copied()
    }

    pub(crate) fn register(self: &Arc<Self>, at: Instant) -> WaitingDeadline {
        *self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(at)
            .or_default() += 1;
        WaitingDeadline {
            deadlines: self.clone(),
            at,
        }
    }
}

pub(crate) struct WaitingDeadline {
    deadlines: Arc<WaitingDeadlines>,
    at: Instant,
}

impl WaitingDeadline {
    fn remove(&self, deadlines: &mut BTreeMap<Instant, usize>) {
        if let Some(count) = deadlines.get_mut(&self.at) {
            *count -= 1;
            if *count == 0 {
                deadlines.remove(&self.at);
            }
        }
    }

    pub(crate) fn extend(&mut self, at: Instant) {
        if at > self.at {
            let mut deadlines = self.deadlines.0.lock().unwrap_or_else(|e| e.into_inner());
            self.remove(&mut deadlines);
            *deadlines.entry(at).or_default() += 1;
            self.at = at;
        }
    }
}

impl Drop for WaitingDeadline {
    fn drop(&mut self) {
        self.remove(&mut self.deadlines.0.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn deadlines_track_live_duplicates_extensions_and_cancellation() {
        let deadlines = Arc::new(WaitingDeadlines::default());
        let early = Instant::now() + Duration::from_secs(2);
        let later = early + Duration::from_secs(5);
        let mut first = deadlines.register(early);
        let second = deadlines.register(early);
        let third = deadlines.register(later);
        first.extend(later + Duration::from_secs(1));
        assert_eq!(deadlines.earliest(), Some(early));
        drop(second);
        assert_eq!(deadlines.earliest(), Some(later));
        drop(first);
        assert_eq!(deadlines.earliest(), Some(later));
        drop(third);
        assert_eq!(deadlines.earliest(), None);
        assert!(deadlines.0.lock().unwrap().is_empty());
    }
}
