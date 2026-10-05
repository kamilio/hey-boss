use super::RequiredChecksReport;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Prepare,
    Lock,
    Seed,
    Policy,
    Ci,
    Ancestry,
    Confirmation,
    Publication,
}

pub(super) struct Timings {
    id: String,
    target: String,
    started: Instant,
    entered: Instant,
    phase: Phase,
    times: [Duration; 8],
    attempts: u32,
    background: bool,
    outcome: &'static str,
    error_code: &'static str,
    state: String,
    errors: usize,
}

impl Timings {
    pub fn new(repository: &str, number: u64) -> Self {
        let now = Instant::now();
        Self {
            id: format!("{:032x}", fastrand::u128(..)),
            target: crate::digest(&format!(
                "required-checks:{}#{number}",
                repository.to_ascii_lowercase()
            )),
            started: now,
            entered: now,
            phase: Phase::Prepare,
            times: [Duration::ZERO; 8],
            attempts: 0,
            background: crate::client::BACKGROUND_READ.try_with(|_| ()).is_ok(),
            outcome: "interrupted",
            error_code: "none",
            state: "unavailable".into(),
            errors: 0,
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

    pub fn finish(&mut self, result: &crate::Result<RequiredChecksReport>) {
        match result {
            Ok(report) => {
                self.outcome = "returned";
                self.state.clone_from(&report.state);
                self.errors = report.errors.len();
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
        if elapsed < Duration::from_secs(1) && self.outcome == "returned" && self.errors == 0 {
            return;
        }
        self.times[self.phase as usize] += now - self.entered;
        let ms = |phase: Phase| self.times[phase as usize].as_millis() as u64;
        let phase = [
            "prepare",
            "report_lock",
            "seed",
            "policy",
            "ci",
            "ancestry",
            "confirmation",
            "publication",
        ][self.phase as usize];
        // No bodies, URLs, credentials or raw errors. Drop also records caller
        // cancellation, when the report future cannot return its own error.
        tracing::info!(read_id=%self.id, target_key=%self.target,
            outcome=self.outcome, error_code=self.error_code, report_state=%self.state,
            source_errors=self.errors, background=self.background, attempts=self.attempts, phase,
            elapsed_ms=elapsed.as_millis() as u64,
            prepare_ms=ms(Phase::Prepare), lock_ms=ms(Phase::Lock), seed_ms=ms(Phase::Seed),
            policy_ms=ms(Phase::Policy), ci_ms=ms(Phase::Ci), ancestry_ms=ms(Phase::Ancestry),
            confirmation_ms=ms(Phase::Confirmation), publication_ms=ms(Phase::Publication),
            "Required-check read finished");
    }
}
