use super::{Duration, Instant, VecDeque, quota_deadline};

// A step maps t to max(t + delay, floor). Fixed adjacent steps compose exactly;
// only in-flight ordinary reservations need individual, removable tickets.
// There are at most two steps per ticket plus one fixed tail.
struct Step {
    ticket: Option<u128>,
    delay: Duration,
    floor: Instant,
}

fn later(at: Instant, delay: Duration) -> Instant {
    at.checked_add(delay)
        .unwrap_or_else(|| quota_deadline(Duration::from_secs(86400)))
}

impl Step {
    fn apply(&self, at: Instant) -> Instant {
        later(at, self.delay).max(self.floor)
    }

    fn then(&mut self, next: Self) {
        self.floor = next.apply(self.floor);
        self.delay = self.delay.saturating_add(next.delay);
    }
}

pub(super) struct Pacing {
    base: Instant,
    steps: VecDeque<Step>,
}

impl Pacing {
    pub(super) fn new(base: Instant) -> Self {
        Self {
            base,
            steps: VecDeque::new(),
        }
    }

    pub(super) fn deadline(&self) -> Instant {
        self.steps.iter().fold(self.base, |at, step| step.apply(at))
    }

    pub(super) fn reserve(&mut self, ticket: u128, at: Instant, delay: Duration) {
        self.steps.push_back(Step {
            ticket: Some(ticket),
            delay,
            floor: later(at, delay),
        });
    }

    pub(super) fn charge(&mut self, at: Instant, delay: Duration) {
        self.fixed(Step {
            ticket: None,
            delay,
            floor: later(at, delay),
        });
    }

    pub(super) fn floor(&mut self, floor: Instant) {
        self.fixed(Step {
            ticket: None,
            delay: Duration::ZERO,
            floor,
        });
    }

    fn fixed(&mut self, step: Step) {
        if let Some(last) = self.steps.back_mut().filter(|last| last.ticket.is_none()) {
            last.then(step);
        } else if self.steps.is_empty() {
            self.base = step.apply(self.base);
        } else {
            self.steps.push_back(step);
        }
    }

    pub(super) fn settle(&mut self, ticket: u128, free: bool) {
        let Some(index) = self
            .steps
            .iter()
            .position(|step| step.ticket == Some(ticket))
        else {
            return;
        };
        if free {
            self.steps.remove(index);
        } else {
            self.steps[index].ticket = None;
        }
        let mut index = 0;
        while index + 1 < self.steps.len() {
            if self.steps[index].ticket.is_none() && self.steps[index + 1].ticket.is_none() {
                let next = self.steps.remove(index + 1).unwrap();
                self.steps[index].then(next);
            } else {
                index += 1;
            }
        }
        while self.steps.front().is_some_and(|step| step.ticket.is_none()) {
            self.base = self.steps.pop_front().unwrap().apply(self.base);
        }
    }

    #[cfg(test)]
    pub(super) fn pending(&self) -> usize {
        let tickets = self
            .steps
            .iter()
            .filter(|step| step.ticket.is_some())
            .count();
        assert!(self.steps.len() <= 2 * tickets + 1);
        tickets
    }
}
