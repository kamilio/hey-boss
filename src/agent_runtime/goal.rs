//! Serializable goal continuation, with the same behavior for every provider.
//! The embedding application persists snapshots and decides how to present them.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedGoal {
    objective: String,
    status: GoalStatus,
    session: Option<SessionRef>,
    turn: Option<String>,
    turns_completed: u64,
    summary: Option<String>,
    last_usage: Value,
    #[serde(skip)]
    owner_pid: Option<u32>,
}
impl ManagedGoal {
    pub fn new(objective: &str) -> io::Result<Self> {
        Self::validate_objective(objective)?;
        Ok(Self {
            objective: objective.into(),
            status: GoalStatus::Paused,
            session: None,
            turn: None,
            turns_completed: 0,
            summary: None,
            last_usage: Value::Null,
            owner_pid: None,
        })
    }
    pub fn objective(&self) -> &str {
        &self.objective
    }
    pub fn status(&self) -> GoalStatus {
        self.status
    }
    pub fn turns_completed(&self) -> u64 {
        self.turns_completed
    }
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }
    pub fn session(&self) -> Option<&SessionRef> {
        self.session.as_ref()
    }
    pub fn last_usage(&self) -> &Value {
        &self.last_usage
    }
    fn validate_objective(objective: &str) -> io::Result<()> {
        if objective.trim().is_empty() || objective.len() > 28000 {
            Err(io::Error::other(
                "Goal objective must contain 1–28000 bytes",
            ))
        } else {
            Ok(())
        }
    }
    fn check_session(&self, agent: &AgentSession) -> io::Result<()> {
        if self.owner_pid.is_some_and(|pid| pid != agent.pid()) {
            return Err(io::Error::other(
                "Goal is attached to another owned process",
            ));
        }
        if let Some(saved) = &self.session {
            let actual = agent.session.as_ref().or(agent.expected_session.as_ref());
            if !actual.is_some_and(|actual| {
                saved.provider == actual.provider
                    && saved.id == actual.id
                    && (saved.path.is_none() || saved.path == actual.path)
            }) {
                return Err(io::Error::other(
                    "Goal belongs to a different saved agent session",
                ));
            }
        }
        Ok(())
    }
    /// Begin a new goal or explicitly re-enable a paused/blocked goal. A complete
    /// goal is immutable; an active goal cannot be accidentally started twice.
    pub fn start(&mut self, agent: &mut AgentSession) -> io::Result<String> {
        Self::validate_objective(&self.objective)?;
        if self.status == GoalStatus::Complete
            || (self.status == GoalStatus::Active && self.turn.is_some())
        {
            return Err(io::Error::other("Goal is complete or already running"));
        }
        agent.inspect()?;
        self.check_session(agent)?;
        let prompt = format!(
            "{}\n\nContinue pursuing this goal until all requested work is implemented and verified. Preserve its full scope. If a turn ends before completion, continue working in this conversation. Return a JSON object with status (completed or blocked) and a nonempty summary only after verifying completion or identifying an actual blocker. Never report completed merely because this turn is ending.",
            self.objective
        );
        let turn = agent.prompt(&prompt, None)?;
        self.status = GoalStatus::Active;
        self.owner_pid = Some(agent.pid());
        self.turn = Some(turn.clone());
        self.session = agent
            .session
            .clone()
            .or_else(|| agent.expected_session.clone());
        self.summary = None;
        Ok(turn)
    }
    /// Call after explicitly launching the exact saved session following a crash
    /// or process stop. This discards the previous process's turn guard, retaining
    /// the objective, session identity and completed-turn accounting.
    pub fn resume(&mut self, agent: &mut AgentSession) -> io::Result<String> {
        agent.inspect()?;
        let actual = agent.session.as_ref().or(agent.expected_session.as_ref());
        if self.session.is_none() || actual != self.session.as_ref() {
            return Err(io::Error::other(
                "Goal resume requires its exact saved session",
            ));
        }
        if agent.turn.is_some() || self.status == GoalStatus::Complete {
            return Err(io::Error::other(
                "Cannot resume a complete goal or interrupt an active agent implicitly",
            ));
        }
        self.turn = None;
        self.owner_pid = None;
        self.status = GoalStatus::Paused;
        self.start(agent)
    }
    pub fn pause(&mut self, agent: &mut AgentSession) -> io::Result<()> {
        self.check_session(agent)?;
        if self.status == GoalStatus::Complete {
            return Err(io::Error::other("Completed goal cannot be paused"));
        }
        self.status = GoalStatus::Paused;
        self.turn = None;
        // Set intent first: a late completion cannot automatically re-enable it.
        agent.interrupt()
    }
    /// Feed events in order. A valid completion report ends the goal; ordinary
    /// turn completion continues the same session, without native goal support.
    /// Errors/interruption block it, never manufacture success or silent retries.
    pub fn observe(&mut self, agent: &mut AgentSession, event: &Event) -> io::Result<()> {
        Self::validate_objective(&self.objective)?;
        self.check_session(agent)?;
        match event {
            Event::Session(reference) => {
                if self.session.as_ref().is_some_and(|saved| {
                    saved.provider != reference.provider || saved.id != reference.id
                }) {
                    self.status = GoalStatus::Blocked;
                    return Err(io::Error::other("Goal received another agent session"));
                }
                self.session = Some(reference.clone());
            }
            Event::TurnCompleted {
                id,
                next_turn,
                status,
                output,
                structured_output,
                usage,
            } if self.status == GoalStatus::Active && self.turn.as_deref() == Some(id) => {
                self.turn = None;
                self.turns_completed = self.turns_completed.saturating_add(1);
                self.last_usage = usage.clone();
                if *status != TurnStatus::Completed {
                    self.status = GoalStatus::Blocked;
                    self.summary = Some(output.clone());
                    return Ok(());
                }
                // Claude steering creates another client turn. An earlier
                // report cannot finish the goal while queued instructions run.
                if let Some(turn) = next_turn {
                    self.turn = Some(turn.clone());
                    return Ok(());
                }
                let report = structured_output
                    .clone()
                    .or_else(|| serde_json::from_str::<Value>(output.trim()).ok())
                    .filter(|v| {
                        matches!(v["status"].as_str(), Some("completed" | "blocked"))
                            && v["summary"].as_str().is_some_and(|s| !s.trim().is_empty())
                    });
                if let Some(report) = report {
                    self.status = if report["status"] == "completed" {
                        GoalStatus::Complete
                    } else {
                        GoalStatus::Blocked
                    };
                    self.summary = report["summary"].as_str().map(str::to_owned);
                } else {
                    self.check_session(agent)?;
                    let prompt = format!(
                        "Continue pursuing the same saved goal, preserving its full scope:\n\n{}\n\nImplement and verify any remaining requirements. Return the required JSON status and summary only after verification or identifying an actual blocker.",
                        self.objective
                    );
                    match agent.prompt(&prompt, None) {
                        Ok(turn) => self.turn = Some(turn),
                        Err(error) => {
                            self.status = GoalStatus::Blocked;
                            return Err(error);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
