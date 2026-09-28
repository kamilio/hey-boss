use super::{Result, context::send, replica::invalid};
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

const SILENCE: Duration = Duration::from_secs(15);
const STARTUP: Duration = Duration::from_secs(120);

// Start before Context::new: even opening the issue store can wait on its owner.
// Fast startup keeps hello as the first frame for older supervisors.
pub(super) struct Progress {
    phase: Arc<Mutex<&'static str>>,
    done: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Progress {
    pub fn start<W: Write + Send + 'static>(output: Arc<Mutex<W>>) -> Self {
        let phase = Arc::new(Mutex::new("database"));
        let current = phase.clone();
        let (done, wait) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            while matches!(
                wait.recv_timeout(Duration::from_secs(5)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                if send(&mut *output.lock().unwrap(), json!({"kind":"starting","phase":*current.lock().unwrap(),"elapsed_ms":started.elapsed().as_millis() as u64})).is_err() {
                    break;
                }
            }
        });
        Self {
            phase,
            done: Some(done),
            thread: Some(thread),
        }
    }
    pub fn phase(&self, phase: &'static str) {
        *self.phase.lock().unwrap() = phase;
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        drop(self.done.take());
        let _ = self.thread.take().unwrap().join();
    }
}

struct Deadline {
    started: Instant,
    received: Instant,
    phase: Option<String>,
}
impl Deadline {
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            received: now,
            phase: None,
        }
    }
    fn check(&self, now: Instant) -> Result<()> {
        if now.duration_since(self.started) >= STARTUP {
            return Err(invalid(
                &self.describe("Companion startup timed out after 120 seconds"),
            ));
        }
        if now.duration_since(self.received) >= SILENCE {
            return Err(invalid(&self.describe(
                "Companion hello timed out after 15 seconds without protocol progress",
            )));
        }
        Ok(())
    }
    fn describe(&self, reason: &str) -> String {
        match &self.phase {
            Some(phase) => format!("{reason} (last startup phase: {phase})"),
            None => reason.to_owned(),
        }
    }
    fn frame(&mut self, frame: &Value, now: Instant) -> Result<bool> {
        self.check(now)?;
        if frame["version"] != 1 {
            return Err(invalid(
                "Companion has an incompatible fleet protocol version",
            ));
        }
        match frame["kind"].as_str() {
            Some("hello") => Ok(true),
            Some("starting") => {
                let phase = frame["phase"]
                    .as_str()
                    .filter(|phase| !phase.is_empty() && phase.len() <= 80)
                    .ok_or_else(|| invalid("Invalid companion startup progress frame"))?;
                self.phase = Some(phase.to_owned());
                self.received = now;
                Ok(false)
            }
            _ => Err(invalid(
                "Companion has an incompatible fleet protocol: expected hello or startup progress",
            )),
        }
    }
}

pub(super) fn receive(
    input: &mpsc::Receiver<Result<Option<Value>>>,
    stopped: impl Fn() -> bool,
    progress: impl Fn(&Value),
) -> Result<Value> {
    let mut deadline = Deadline::new(Instant::now());
    loop {
        if stopped() {
            return Err(invalid("Companion handshake cancelled"));
        }
        deadline.check(Instant::now())?;
        match input.recv_timeout(Duration::from_millis(250)) {
            Ok(Ok(Some(frame))) => {
                if deadline.frame(&frame, Instant::now())? {
                    return Ok(frame);
                }
                progress(&frame);
            }
            Ok(Ok(None)) => {
                return Err(invalid(&deadline.describe("Companion closed before hello")));
            }
            Ok(Err(error)) => {
                return Err(invalid(
                    &deadline.describe(&format!("Companion hello failed: {error}")),
                ));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(invalid(
                    &deadline.describe("Companion reader exited before hello"),
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_extends_silence_but_never_the_total_startup_deadline() {
        let start = Instant::now();
        let mut deadline = Deadline::new(start);
        for second in (5..120).step_by(5) {
            assert!(
                !deadline
                    .frame(
                        &json!({"version":1,"kind":"starting","phase":"database"}),
                        start + Duration::from_secs(second)
                    )
                    .unwrap()
            );
        }
        assert!(
            deadline
                .check(start + STARTUP)
                .unwrap_err()
                .to_string()
                .contains("120 seconds")
        );
    }
    #[test]
    fn silence_is_a_timeout_with_the_last_phase_not_a_protocol_mismatch() {
        let start = Instant::now();
        let mut deadline = Deadline::new(start);
        deadline
            .frame(
                &json!({"version":1,"kind":"starting","phase":"workers"}),
                start,
            )
            .unwrap();
        let error = deadline.check(start + SILENCE).unwrap_err().to_string();
        assert!(error.contains("15 seconds"));
        assert!(error.contains("workers"));
        assert!(!error.contains("incompatible"));
    }
    #[test]
    fn hello_after_slow_startup_and_legacy_hello_are_accepted() {
        let start = Instant::now();
        for slow in [false, true] {
            let mut deadline = Deadline::new(start);
            let seconds = if slow { 30 } else { 0 };
            for second in (5..=seconds).step_by(5) {
                deadline
                    .frame(
                        &json!({"version":1,"kind":"starting","phase":"capture"}),
                        start + Duration::from_secs(second),
                    )
                    .unwrap();
            }
            assert!(
                deadline
                    .frame(
                        &json!({"version":1,"kind":"hello"}),
                        start + Duration::from_secs(seconds)
                    )
                    .unwrap()
            );
        }
    }
    #[test]
    fn invalid_frames_do_not_extend_startup() {
        let start = Instant::now();
        for frame in [
            json!({"version":2,"kind":"hello"}),
            json!({"version":1,"kind":"ack"}),
            json!({"version":1,"kind":"starting"}),
        ] {
            assert!(Deadline::new(start).frame(&frame, start).is_err());
        }
    }
}
