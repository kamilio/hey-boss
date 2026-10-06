//! Attribute slow publications without logging cache keys or holding the writer.
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Transaction,
    Encode,
    Read,
    Write,
    Prune,
    Commit,
    Maintenance,
}

#[derive(Default)]
pub(super) struct Timings {
    entered: Option<(Phase, Instant)>,
    elapsed: [Duration; 7],
    pub payload_bytes: usize,
    pub observations: usize,
}

impl Timings {
    pub fn enter(&mut self, phase: Phase) {
        self.finish();
        self.entered = Some((phase, Instant::now()));
    }

    pub fn finish(&mut self) {
        if let Some((phase, entered)) = self.entered.take() {
            self.elapsed[phase as usize] += entered.elapsed();
        }
    }

    pub fn log(&self, write_id: &str, operation: &'static str) {
        let elapsed: Duration = self.elapsed.iter().sum();
        if elapsed < Duration::from_millis(250) {
            return;
        }
        let ms = |phase: Phase| self.elapsed[phase as usize].as_millis() as u64;
        tracing::info!(
            write_id,
            operation,
            elapsed_ms = elapsed.as_millis() as u64,
            observations = self.observations,
            payload_bytes = self.payload_bytes,
            transaction_ms = ms(Phase::Transaction),
            encode_ms = ms(Phase::Encode),
            read_ms = ms(Phase::Read),
            write_ms = ms(Phase::Write),
            prune_ms = ms(Phase::Prune),
            commit_ms = ms(Phase::Commit),
            maintenance_ms = ms(Phase::Maintenance),
            "Cache observation phases finished"
        );
    }
}
