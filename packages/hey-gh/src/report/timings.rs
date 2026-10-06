//! Separate local report waits from individual GitHub request latency.
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Prepare,
    Lock,
    Seed,
    Collection,
    Confirmation,
    Assembly,
    Publication,
    StatusPublication,
}

pub(super) struct Timings {
    id: String,
    target: String,
    kind: &'static str,
    started: Instant,
    entered: Instant,
    phase: Phase,
    times: [Duration; 8],
    attempts: u32,
    background: bool,
    cached_only: bool,
    outcome: &'static str,
    error_code: &'static str,
    complete: bool,
}

impl Timings {
    pub fn new(
        kind: &'static str,
        repository: &str,
        number: u64,
        freshness: crate::Freshness,
    ) -> Self {
        let now = Instant::now();
        Self {
            id: format!("{:032x}", fastrand::u128(..)),
            target: crate::digest(&format!("{}#{number}", repository.to_ascii_lowercase())),
            kind,
            started: now,
            entered: now,
            phase: Phase::Prepare,
            times: [Duration::ZERO; 8],
            attempts: 0,
            background: crate::client::BACKGROUND_READ.try_with(|_| ()).is_ok(),
            cached_only: matches!(freshness, crate::Freshness::CachedOnly),
            outcome: "interrupted",
            error_code: "none",
            complete: false,
        }
    }

    pub fn enter(&mut self, phase: Phase) {
        let now = Instant::now();
        self.times[self.phase as usize] += now - self.entered;
        self.entered = now;
        self.phase = phase;
        if matches!(phase, Phase::Seed) {
            self.attempts += 1;
        }
    }

    pub fn finish(&mut self, result: Result<bool, &crate::Error>) {
        match result {
            Ok(complete) => {
                self.outcome = "returned";
                self.complete = complete;
            }
            Err(error) => {
                self.outcome = "error";
                self.error_code = error.diagnostic_code();
            }
        }
    }
}

impl Drop for Timings {
    fn drop(&mut self) {
        let now = Instant::now();
        let elapsed = now - self.started;
        if elapsed < Duration::from_secs(1) && self.complete {
            return;
        }
        self.times[self.phase as usize] += now - self.entered;
        let ms = |phase: Phase| self.times[phase as usize].as_millis() as u64;
        let phase = [
            "prepare",
            "report_lock",
            "seed",
            "collection",
            "confirmation",
            "assembly",
            "publication",
            "status_publication",
        ][self.phase as usize];
        // Cancellation also emits its final phase. Hash the target and never
        // include response bodies, raw errors, URLs, or credentials.
        tracing::info!(read_id=%self.id, target_key=%self.target, kind=self.kind,
            outcome=self.outcome, error_code=self.error_code, complete=self.complete,
            background=self.background, cached_only=self.cached_only, attempts=self.attempts, phase,
            elapsed_ms=elapsed.as_millis() as u64,
            prepare_ms=ms(Phase::Prepare), lock_ms=ms(Phase::Lock), seed_ms=ms(Phase::Seed),
            collection_ms=ms(Phase::Collection), confirmation_ms=ms(Phase::Confirmation),
            assembly_ms=ms(Phase::Assembly), observation_ms=ms(Phase::Publication),
            status_publication_ms=ms(Phase::StatusPublication),
            "PR evidence read finished");
    }
}
