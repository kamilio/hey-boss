//! Wire records shared by the supervisor and the worker-independent executor.
use super::Run;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Dispatch {
    pub run: Run,
    pub node: String,
    pub generation: i64,
    pub revoke: bool,
}

/// A terminal report is published only after the owned process group stops.
/// Disconnect is deliberately not a terminal observation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Report {
    pub run_id: String,
    pub generation: i64,
    pub state: String,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub reason: Option<String>,
}

impl Report {
    pub fn pending(dispatch: &Dispatch) -> Self {
        Self {
            run_id: dispatch.run.id.clone(),
            generation: dispatch.generation,
            state: "pending".into(),
            session_id: None,
            cwd: None,
            started_at: None,
            finished_at: None,
            reason: None,
        }
    }
}
