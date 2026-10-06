//! Coordinate active requests with cache lookups that started before a peer
//! completed. History holds fingerprints only, never response payloads.
use super::*;

type Flight = (
    watch::Receiver<SharedResult>,
    Arc<AtomicBool>,
    Arc<Mutex<Instant>>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    Arc<WaitingDeadlines>,
);

pub(crate) type Inflight = Arc<Mutex<Flights>>;

#[derive(Default)]
pub(crate) struct Flights {
    pub active: HashMap<String, Flight>,
    sequence: u64,
    completed: VecDeque<(String, u64)>,
}

impl Flights {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn completed_since(&self, fingerprint: &str, sequence: u64) -> bool {
        self.completed
            .iter()
            .rev()
            .find(|(key, _)| key == fingerprint)
            .is_some_and(|(_, completed)| *completed > sequence)
    }

    pub fn record_success(&mut self, fingerprint: String) {
        self.sequence = self.sequence.wrapping_add(1);
        if self.sequence == 0 {
            self.completed.clear();
        }
        // Eviction can cost a redundant request from a very late caller, but
        // cannot supply stale evidence: a hint only causes another cache read.
        if self.completed.len() == 128 {
            self.completed.pop_front();
        }
        self.completed.push_back((fingerprint, self.sequence));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_bounded_key_specific_and_contains_no_payloads() {
        let mut flights = Flights::default();
        let first = crate::digest("first");
        let before = flights.sequence();
        flights.record_success(first.clone());
        assert!(flights.completed_since(&first, before));
        assert!(!flights.completed_since(&first, flights.sequence()));
        assert!(!flights.completed_since(&crate::digest("other"), before));
        for n in 0..1000 {
            flights.record_success(crate::digest(&format!("key-{n}")));
        }
        assert_eq!(flights.completed.len(), 128);
        assert!(flights.completed.iter().all(|(key, _)| key.len() == 64));
        assert!(!flights.completed_since(&first, before));
        assert!(flights.completed_since(&crate::digest("key-999"), before));
        assert!(flights.active.is_empty());
    }
}
