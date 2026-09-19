//! Worker dashboard components. Rendering never performs IO or controls workers.
pub mod backend;
pub mod runtime;
pub mod ui;

use serde_json::Value;

/// Strip terminal controls, including ANSI and bidi overrides, from queue text.
pub fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(8192)
        .collect()
}

#[derive(Default)]
pub struct Dashboard {
    pub snapshot: Value,
    pub owned_worker: bool,
    pub worker_id: Option<String>,
    pub run_id: Option<String>,
    pub sessions_focused: bool,
    pub history: bool,
    pub help: bool,
    pub pending: bool,
    pub error: Option<String>,
    pub confirmation: Option<Confirmation>,
    pub detail_scroll: u16,
    /// Current wall-clock milliseconds, supplied by the application for live timers.
    pub now_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
    pub worker_id: String,
    pub name: String,
    pub stop: bool,
}

impl Dashboard {
    pub fn workers(&self) -> Vec<&Value> {
        self.snapshot["workers"]
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    }

    pub fn runs(&self) -> Vec<&Value> {
        // Never display another worker's sessions while its refresh is in flight.
        if self.snapshot["worker_id"].as_str() != self.worker_id.as_deref() {
            return vec![];
        }
        self.snapshot["runs"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|r| r["finished_at"].is_null() || self.history)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn apply(&mut self, snapshot: Value) {
        self.snapshot = snapshot;
        let workers = self.workers();
        if !workers
            .iter()
            .any(|w| w["id"].as_str() == self.worker_id.as_deref())
        {
            self.worker_id = workers
                .first()
                .and_then(|w| w["id"].as_str())
                .map(str::to_owned);
        }
        self.normalize_run();
        self.error = None;
    }

    pub fn normalize_run(&mut self) {
        let runs = self.runs();
        if !runs
            .iter()
            .any(|r| r["id"].as_str() == self.run_id.as_deref())
        {
            self.run_id = runs
                .first()
                .and_then(|r| r["id"].as_str())
                .map(str::to_owned);
            self.detail_scroll = 0;
        }
    }

    pub fn navigate(&mut self, delta: isize) {
        let ids: Vec<String> = if self.sessions_focused {
            self.runs()
        } else {
            self.workers()
        }
        .iter()
        .filter_map(|v| v["id"].as_str().map(str::to_owned))
        .collect();
        let selected = if self.sessions_focused {
            &mut self.run_id
        } else {
            &mut self.worker_id
        };
        if ids.is_empty() {
            return;
        }
        let at = ids
            .iter()
            .position(|id| Some(id) == selected.as_ref())
            .unwrap_or(0);
        let next = at.saturating_add_signed(delta).min(ids.len() - 1);
        *selected = Some(ids[next].clone());
        self.detail_scroll = 0;
        if !self.sessions_focused {
            self.normalize_run();
        }
    }

    pub fn confirm(&mut self, stop: bool) {
        if self.pending || self.error.is_some() {
            return;
        }
        if let Some(w) = self
            .workers()
            .into_iter()
            .find(|w| w["id"].as_str() == self.worker_id.as_deref())
        {
            self.confirmation = Some(Confirmation {
                worker_id: w["id"].as_str().unwrap_or_default().into(),
                name: text(&w["config"]["name"]),
                stop,
            });
        }
    }
}
