//! Route background diagnostics through the dashboard while it owns the terminal.
use std::sync::Mutex;

#[derive(Default)]
struct State {
    captures: usize,
    latest: Option<String>,
}

impl State {
    fn record(&mut self, message: String) -> Option<String> {
        if self.captures == 0 {
            Some(message)
        } else {
            self.latest = Some(message.chars().take(2000).collect());
            None
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State {
    captures: 0,
    latest: None,
});

/// Write to stderr, or the dashboard footer while an interactive runtime is active.
pub fn report(message: std::fmt::Arguments<'_>) {
    if let Some(message) = STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .record(message.to_string())
    {
        eprintln!("{message}");
    }
}

pub(crate) struct Capture;
impl Capture {
    pub(crate) fn start() -> Self {
        STATE.lock().unwrap_or_else(|e| e.into_inner()).captures += 1;
        Self
    }
    pub(crate) fn latest(&self) -> Option<String> {
        STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .latest
            .clone()
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.captures -= 1;
        if state.captures == 0 {
            state.latest = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dashboard_diagnostics_never_escape_to_stderr_and_remain_bounded() {
        let mut state = State {
            captures: 1,
            latest: None,
        };
        assert!(
            state
                .record("Worker recovery: FOREIGN KEY constraint failed".into())
                .is_none()
        );
        assert_eq!(
            state.latest.as_deref(),
            Some("Worker recovery: FOREIGN KEY constraint failed")
        );
        for _ in 0..1000 {
            assert!(state.record("x".repeat(10_000)).is_none());
        }
        assert_eq!(state.latest.as_ref().unwrap().len(), 2000);
        state.captures = 0;
        assert_eq!(
            state.record("Headless diagnostic".into()).as_deref(),
            Some("Headless diagnostic")
        );
    }
}
