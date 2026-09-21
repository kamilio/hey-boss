//! Owned agent sessions, independent of issue pickup and task routing.
//! Codex uses app-server JSON-RPC, Claude uses the SDK control stream, and Pi
//! uses RPC mode. All controls apply only to the child started by this client.
mod goal;
mod protocol;
use crate::agent_process::Process;
pub use goal::{GoalStatus, ManagedGoal};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Claude,
    Pi,
}
impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Pi => "pi",
        }
    }
    pub fn capabilities(self) -> Capabilities {
        Capabilities {
            native_goal: self == Self::Codex,
            structured_output: self != Self::Pi,
            tool_approvals: self != Self::Pi,
            steering: match self {
                Self::Codex => SteeringDelivery::Direct,
                Self::Claude => SteeringDelivery::NextTurn,
                Self::Pi => SteeringDelivery::BeforeNextModelCall,
            },
        }
    }
    pub fn binary(self) -> io::Result<PathBuf> {
        let key = format!("HEY_BOSS_{}", self.name().to_uppercase());
        if let Some(path) = std::env::var_os(&key) {
            let path = PathBuf::from(path);
            if !path.is_absolute() || !executable(&path) {
                return Err(io::Error::other(format!(
                    "{key} must point to an executable absolute {} path",
                    self.name()
                )));
            }
            return Ok(path);
        }
        let mut candidates: Vec<_> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|p| p.join(self.name()))
                .collect();
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            candidates.extend([
                home.join(".local/bin").join(self.name()),
                home.join(".cargo/bin").join(self.name()),
            ]);
            if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
                let mut versions: Vec<_> = versions.flatten().map(|p| p.path()).collect();
                versions.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
                candidates.extend(
                    versions
                        .into_iter()
                        .rev()
                        .map(|p| p.join("bin").join(self.name())),
                );
            }
        }
        candidates.extend([
            PathBuf::from("/opt/homebrew/bin").join(self.name()),
            PathBuf::from("/usr/local/bin").join(self.name()),
        ]);
        if self == Self::Codex {
            candidates.push("/Applications/Codex.app/Contents/Resources/codex".into());
        }
        candidates
            .into_iter()
            .find(|p| executable(p))
            .ok_or_else(|| {
                io::Error::other(format!(
                    "{} CLI was not found. Install it or set {key} to its absolute path.",
                    self.name()
                ))
            })
    }
}
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SteeringDelivery {
    Direct,
    BeforeNextModelCall,
    NextTurn,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub native_goal: bool,
    pub structured_output: bool,
    pub tool_approvals: bool,
    pub steering: SteeringDelivery,
}
/// Persist the whole reference. Pi resumes an exact JSONL file, not the most
/// recent session; provider identity prevents accidental cross-agent resumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub provider: Provider,
    pub id: String,
    pub path: Option<PathBuf>,
}
pub struct Launch {
    pub provider: Provider,
    pub binary: Option<PathBuf>,
    pub cwd: PathBuf,
    pub resume: Option<SessionRef>,
    pub env: BTreeMap<OsString, OsString>,
    /// Claude requires its schema before the process starts. Codex uses this
    /// as the default per-turn schema. Pi rejects a native output schema.
    pub output_schema: Option<Value>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Session(SessionRef),
    TurnStarted {
        id: String,
    },
    TextDelta {
        text: String,
    },
    Message {
        role: String,
        text: String,
    },
    ToolStarted {
        id: String,
        name: String,
        input: Value,
    },
    ToolCompleted {
        id: String,
        output: Value,
        failed: bool,
    },
    Approval {
        id: String,
        kind: String,
        payload: Value,
    },
    Input {
        id: String,
        payload: Value,
    },
    RequestCancelled {
        id: String,
    },
    TurnCompleted {
        id: String,
        status: TurnStatus,
        output: String,
        structured_output: Option<Value>,
        usage: Value,
    },
    Goal(Value),
    Other(Value),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub provider: Provider,
    pub session: Option<SessionRef>,
    pub turn: Option<String>,
    pub pending_requests: Vec<String>,
    pub capabilities: Capabilities,
    pub stopped: bool,
    pub outcome_uncertain: bool,
    /// Pi may request extension input before acknowledging prompt preflight.
    #[serde(default)]
    pub awaiting_prompt_ack: bool,
}

pub struct AgentSession {
    provider: Provider,
    process: Process,
    session: Option<SessionRef>,
    expected_session: Option<SessionRef>,
    turn: Option<String>,
    sequence: u64,
    pending: VecDeque<Value>,
    events: VecDeque<Event>,
    requests: BTreeMap<String, Value>,
    output: String,
    status: TurnStatus,
    interrupted: bool,
    stopped: bool,
    uncertain: bool,
    output_schema: Option<Value>,
    queued_turns: VecDeque<(String, String)>,
    prompt_ack: Option<(String, Instant)>,
    tasks: BTreeSet<String>,
    refreshing_pi: bool,
}
impl AgentSession {
    pub fn launch(launch: Launch) -> io::Result<Self> {
        if launch.output_schema.is_some() && !launch.provider.capabilities().structured_output {
            return Err(io::Error::other("Provider has no native output schema"));
        }
        if let Some(saved) = &launch.resume {
            if saved.provider != launch.provider || !valid_id(&saved.id) {
                return Err(io::Error::other(
                    "Invalid or cross-provider session reference",
                ));
            }
            if launch.provider == Provider::Pi {
                let file = saved
                    .path
                    .as_ref()
                    .filter(|p| p.is_absolute() && p.is_file())
                    .ok_or_else(|| {
                        io::Error::other("Pi resume requires the exact saved session file")
                    })?;
                let reader = std::io::BufReader::new(std::fs::File::open(file)?);
                use std::io::{BufRead, Read};
                let mut line = Vec::new();
                reader.take(65537).read_until(b'\n', &mut line)?;
                let header: Value = serde_json::from_slice(&line)?;
                if header["type"] != "session" || header["id"].as_str() != Some(&saved.id) {
                    return Err(io::Error::other(
                        "Pi session file does not match the saved session ID",
                    ));
                }
            }
        }
        let binary = match launch.binary {
            Some(path) if path.is_absolute() && executable(&path) => path,
            Some(_) => {
                return Err(io::Error::other(
                    "Agent binary must be an executable absolute path",
                ));
            }
            None => launch.provider.binary()?,
        };
        let mut command = protocol::command(launch.provider, &binary, launch.resume.as_ref());
        if launch.provider == Provider::Claude
            && let Some(schema) = &launch.output_schema
        {
            command
                .arg("--json-schema")
                .arg(serde_json::to_string(schema)?);
        }
        let mut paths = vec![
            binary.parent().unwrap().to_owned(),
            std::env::current_exe()?.parent().unwrap().to_owned(),
        ];
        paths.extend(std::env::split_paths(
            &launch
                .env
                .get(std::ffi::OsStr::new("PATH"))
                .cloned()
                .or_else(|| std::env::var_os("PATH"))
                .unwrap_or_default(),
        ));
        for key in [
            "HEY_BOSS_AGENT_ID",
            "CODEX_THREAD_ID",
            "CODEX_SESSION_ID",
            "CLAUDE_SESSION_ID",
            "PI_SESSION_ID",
            "CLAUDECODE",
        ] {
            command.env_remove(key);
        }
        // Discard inherited caller identity, then honor explicit owned-session
        // context supplied by the embedding application.
        command.current_dir(launch.cwd).envs(launch.env).env(
            "PATH",
            std::env::join_paths(paths).map_err(io::Error::other)?,
        );
        let process = Process::spawn(&mut command)?;
        let mut client = Self {
            provider: launch.provider,
            process,
            session: None,
            expected_session: launch.resume,
            turn: None,
            sequence: 0,
            pending: VecDeque::new(),
            events: VecDeque::new(),
            requests: BTreeMap::new(),
            output: String::new(),
            status: TurnStatus::Completed,
            interrupted: false,
            stopped: false,
            uncertain: false,
            output_schema: launch.output_schema,
            queued_turns: VecDeque::new(),
            prompt_ack: None,
            tasks: BTreeSet::new(),
            refreshing_pi: false,
        };
        match client.provider {
            Provider::Codex => {
                client.rpc("initialize", json!({"clientInfo":{"name":"hey_boss","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))?;
                client.send(&json!({"method":"initialized","params":{}}))?;
                let (method, params) = match &client.expected_session {
                    Some(s) => ("thread/resume", json!({"threadId":s.id})),
                    None => ("thread/start", json!({"ephemeral":false})),
                };
                let result = client.rpc(method, crate::codex_permissions::thread(params))?;
                client.attach(required(&result["thread"], "id")?, None)?;
            }
            Provider::Claude => {
                client.rpc("initialize", json!({"hooks":null}))?;
            }
            Provider::Pi => {
                client.refresh_pi()?;
            }
        }
        Ok(client)
    }
    pub fn pid(&self) -> u32 {
        self.process.pid()
    }
    pub fn capabilities(&self) -> Capabilities {
        self.provider.capabilities()
    }
    pub fn inspect(&mut self) -> io::Result<State> {
        // Consume already-arrived notifications before checking a turn guard.
        self.normalize_pending()?;
        while !self.stopped {
            match self.process.receive(Duration::ZERO) {
                Ok(Some(value)) => {
                    self.normalize(value)?;
                    // A normalizer may perform an RPC that buffers earlier
                    // notifications. Preserve their order before reading more.
                    self.normalize_pending()?;
                }
                Ok(None) => break,
                Err(error) => {
                    self.uncertain = true;
                    return Err(error);
                }
            }
        }
        self.normalize_pending()?;
        self.check_prompt_ack()?;
        if !self.stopped && self.provider == Provider::Pi {
            self.refresh_pi()?;
            self.normalize_pending()?;
        }
        Ok(self.state())
    }
    /// Last observed state, including a saved session after a disconnect.
    pub fn state(&self) -> State {
        State {
            provider: self.provider,
            session: self.session.clone(),
            turn: self.turn.clone(),
            pending_requests: self.requests.keys().cloned().collect(),
            capabilities: self.capabilities(),
            stopped: self.stopped,
            outcome_uncertain: self.uncertain,
            awaiting_prompt_ack: self.prompt_ack.is_some(),
        }
    }
    /// Return the owned turn guard. Pi can request preflight input before its
    /// acknowledgment: drain Input events while state.awaiting_prompt_ack is
    /// true. A late rejection completes the turn as Failed, never successful.
    pub fn prompt(&mut self, text: &str, schema: Option<&Value>) -> io::Result<String> {
        self.ready()?;
        validate_text(text)?;
        self.inspect()?;
        if self.turn.is_some() {
            return Err(io::Error::other(
                "Agent already has an active turn; use steer",
            ));
        }
        if schema.is_some() && !self.capabilities().structured_output {
            return Err(io::Error::other(
                "This provider has no native structured output; include the format in the prompt",
            ));
        }
        if schema.is_some()
            && self.provider == Provider::Claude
            && schema != self.output_schema.as_ref()
        {
            return Err(io::Error::other(
                "Claude output schema is set at launch, not per turn",
            ));
        }
        let schema = schema.cloned().or_else(|| self.output_schema.clone());
        self.output.clear();
        self.status = TurnStatus::Completed;
        self.interrupted = false;
        self.sequence += 1;
        let mut turn = format!("hey-boss-turn-{}", self.sequence);
        match self.provider {
            Provider::Codex => {
                let session = self.session.as_ref().unwrap();
                let mut params =
                    json!({"threadId":session.id,"input":[{"type":"text","text":text}]});
                if let Some(schema) = schema {
                    params["outputSchema"] = schema;
                }
                let result = self.rpc("turn/start", params)?;
                turn = required(&result["turn"], "id").inspect_err(|_| self.uncertain = true)?;
            }
            Provider::Claude => {
                self.send(&json!({"type":"user","session_id":self.session.as_ref().map(|s|s.id.as_str()).unwrap_or(""),"message":{"role":"user","content":text},"parent_tool_use_id":null}))?;
            }
            Provider::Pi => {
                self.rpc("prompt", json!({"message":text}))?;
            }
        }
        self.turn = Some(turn.clone());
        self.events
            .push_back(Event::TurnStarted { id: turn.clone() });
        Ok(turn)
    }
    pub fn steer(&mut self, expected_turn: &str, text: &str) -> io::Result<SteeringDelivery> {
        self.ready()?;
        validate_text(text)?;
        self.inspect()?;
        if self.turn.as_deref() != Some(expected_turn) {
            return Err(io::Error::other(
                "Active turn changed; refresh controls before sending",
            ));
        }
        if self.prompt_ack.is_some() {
            return Err(io::Error::other(
                "Prompt is waiting for provider acknowledgment",
            ));
        }
        match self.provider {
            Provider::Codex => {
                let result = self.rpc("turn/steer", json!({"threadId":self.session.as_ref().unwrap().id,"expectedTurnId":expected_turn,"input":[{"type":"text","text":text}]}))?;
                if result["turnId"].as_str() != Some(expected_turn) {
                    self.uncertain = true;
                    return Err(io::Error::other("Unexpected steering acknowledgement"));
                }
            }
            Provider::Claude => {
                if self.queued_turns.len() >= 64 {
                    return Err(io::Error::other("Agent steering queue exceeded limit"));
                }
                // SDK streaming input can be folded into the current tool loop
                // without a second result. Own the queue so NextTurn has a
                // deterministic guard and exactly one completion per prompt.
                self.sequence += 1;
                self.queued_turns
                    .push_back((format!("hey-boss-turn-{}", self.sequence), text.into()));
            }
            Provider::Pi => {
                self.rpc("steer", json!({"message":text}))?;
            }
        }
        Ok(self.capabilities().steering)
    }
    pub fn interrupt(&mut self) -> io::Result<()> {
        self.ready()?;
        self.inspect()?;
        if self.turn.is_none() {
            return Ok(());
        }
        self.interrupted = true;
        let result = self.interrupt_active();
        if result.is_err() && !self.uncertain {
            // An explicit rejection did not interrupt the provider. Preserve
            // its eventual completion rather than rewriting it as canceled.
            self.interrupted = false;
        }
        result
    }
    fn interrupt_active(&mut self) -> io::Result<()> {
        match self.provider {
            Provider::Codex => {
                self.rpc(
                    "turn/interrupt",
                    json!({"threadId":self.session.as_ref().unwrap().id,"turnId":self.turn}),
                )?;
                self.terminate_codex_tools()
                    .inspect_err(|_| self.uncertain = true)?;
            }
            Provider::Claude => {
                if self.queued_turns.is_empty() {
                    self.rpc("interrupt", json!({}))?;
                } else {
                    // Claude has no clear_queue operation. Stop this owned
                    // process to guarantee queued turns cannot restart work.
                    self.stop()?;
                }
            }
            Provider::Pi => {
                if self.prompt_ack.is_some() || !self.requests.is_empty() {
                    // Extension dialogs without an abort signal can hold
                    // preflight/tool hooks indefinitely. Stop the owned group
                    // rather than answering input or claiming native abort.
                    return self.stop();
                }
                // Pi abort otherwise continues queued steering/follow-up prompts.
                self.rpc("clear_queue", json!({}))?;
                self.rpc("abort", json!({}))?;
            }
        }
        Ok(())
    }
    fn codex_terminals(&mut self) -> io::Result<BTreeSet<String>> {
        let mut processes = BTreeSet::new();
        let mut cursors = BTreeSet::new();
        let mut cursor = Value::Null;
        loop {
            let result = self.rpc(
                "thread/backgroundTerminals/list",
                json!({"threadId":self.session.as_ref().unwrap().id,"limit":100,"cursor":cursor}),
            )?;
            let terminals = result["data"]
                .as_array()
                .ok_or_else(|| io::Error::other("Codex omitted its native tool list"))?;
            for terminal in terminals {
                processes.insert(required(terminal, "processId")?);
            }
            if result["nextCursor"].is_null() {
                return Ok(processes);
            }
            let next = required(&result, "nextCursor")?;
            if cursors.len() >= 128 || !cursors.insert(next.clone()) {
                return Err(io::Error::other(
                    "Codex returned invalid native tool pagination",
                ));
            }
            cursor = json!(next);
        }
    }
    fn terminate_codex_tools(&mut self) -> io::Result<()> {
        // Turn interruption alone can leave unified-exec shells running. These
        // handles belong to our saved thread, not guessed operating-system PIDs.
        for process in self.codex_terminals()? {
            let result = self.rpc(
                "thread/backgroundTerminals/terminate",
                json!({"threadId":self.session.as_ref().unwrap().id,"processId":process}),
            )?;
            match result["terminated"].as_bool() {
                Some(true) => {}
                Some(false) if !self.codex_terminals()?.contains(&process) => {}
                _ => {
                    return Err(io::Error::other(
                        "Codex did not confirm native tool termination",
                    ));
                }
            }
        }
        Ok(())
    }
    /// Only command/file Codex requests and Claude can_use_tool requests accept
    /// boolean decisions. Other input is returned to the caller explicitly.
    pub fn decide(&mut self, id: &str, allow: bool) -> io::Result<()> {
        self.ready()?;
        self.inspect()?;
        let value = self
            .requests
            .get(id)
            .ok_or_else(|| io::Error::other("No owned pending approval with that ID"))?;
        let reply = protocol::decision(self.provider, value, allow)?;
        self.send(&reply)?;
        self.requests.remove(id);
        Ok(())
    }
    pub fn respond_input(&mut self, id: &str, answer: Option<&str>) -> io::Result<()> {
        self.ready()?;
        self.inspect()?;
        let value = self
            .requests
            .get(id)
            .ok_or_else(|| io::Error::other("No owned pending input with that ID"))?;
        let reply = protocol::input_response(self.provider, value, answer)?;
        self.send(&reply)?;
        self.requests.remove(id);
        if let Some((_, deadline)) = &mut self.prompt_ack {
            *deadline = Instant::now() + Duration::from_secs(45);
        }
        Ok(())
    }
    pub fn goal(&mut self) -> io::Result<Value> {
        self.ready()?;
        self.native_goal()?;
        let result = self.rpc(
            "thread/goal/get",
            json!({"threadId":self.session.as_ref().unwrap().id}),
        )?;
        Ok(result["goal"].clone())
    }
    pub fn set_goal(&mut self, objective: Option<&str>, status: &str) -> io::Result<Value> {
        self.ready()?;
        self.native_goal()?;
        if !matches!(status, "active" | "paused" | "blocked" | "complete") {
            return Err(io::Error::other("Invalid goal status"));
        }
        if let Some(text) = objective {
            validate_text(text)?;
        }
        let mut params = json!({"threadId":self.session.as_ref().unwrap().id,"status":status});
        if let Some(text) = objective {
            params["objective"] = json!(text);
        }
        let result = self.rpc("thread/goal/set", params)?;
        if result["goal"]["status"] != status {
            self.uncertain = true;
            return Err(io::Error::other("Goal change was not acknowledged"));
        }
        Ok(result["goal"].clone())
    }
    fn native_goal(&self) -> io::Result<()> {
        if self.capabilities().native_goal {
            Ok(())
        } else {
            Err(io::Error::other(
                "Provider does not have native goals; the caller must manage continuation",
            ))
        }
    }
    pub fn receive(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        self.normalize_pending()?;
        self.check_prompt_ack()?;
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        if self.stopped {
            return Ok(None);
        }
        match self.process.receive(timeout) {
            Ok(Some(value)) => self.normalize(value)?,
            Ok(None) => {}
            Err(error) => {
                self.uncertain = true;
                return Err(error);
            }
        }
        Ok(self.events.pop_front())
    }
    pub fn stop(&mut self) -> io::Result<()> {
        self.process.stop()?;
        self.stopped = true;
        self.pending.clear();
        self.queued_turns.clear();
        self.prompt_ack = None;
        self.tasks.clear();
        self.finish(
            TurnStatus::Interrupted,
            self.output.clone(),
            None,
            Value::Null,
        )?;
        self.requests.clear();
        Ok(())
    }
    fn ready(&self) -> io::Result<()> {
        if self.stopped || self.uncertain {
            Err(io::Error::other(
                "Agent stopped or action outcome uncertain; inspect state before retrying",
            ))
        } else {
            Ok(())
        }
    }
    fn attach(&mut self, id: String, path: Option<PathBuf>) -> io::Result<()> {
        if !valid_id(&id)
            || self
                .expected_session
                .as_ref()
                .is_some_and(|s| s.id != id || (self.provider == Provider::Pi && s.path != path))
            || self.session.as_ref().is_some_and(|s| s.id != id)
        {
            self.uncertain = true;
            return Err(io::Error::other(
                "Agent returned a different or invalid saved session",
            ));
        }
        let reference = SessionRef {
            provider: self.provider,
            id,
            path,
        };
        if self.session.as_ref() != Some(&reference) {
            self.events.push_back(Event::Session(reference.clone()));
        }
        self.session = Some(reference);
        Ok(())
    }
    fn refresh_pi(&mut self) -> io::Result<()> {
        self.refreshing_pi = true;
        let result = self.rpc("get_state", json!({}));
        self.refreshing_pi = false;
        let state = result?;
        let id = required(&state, "sessionId").inspect_err(|_| self.uncertain = true)?;
        self.attach(id, state["sessionFile"].as_str().map(PathBuf::from))
    }
    fn send(&mut self, value: &Value) -> io::Result<()> {
        self.process.send(value).inspect_err(|_| {
            self.uncertain = true;
        })
    }
    fn rpc(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.sequence += 1;
        let id = format!("hey-boss-{}", self.sequence);
        self.send(&protocol::request(self.provider, &id, method, params))?;
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if Instant::now() >= deadline {
                self.uncertain = true;
                return Err(io::Error::other(format!(
                    "Agent timed out acknowledging {method}; inspect state before retrying"
                )));
            }
            let value = match self.process.receive(Duration::from_millis(100)) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(error) => {
                    self.uncertain = true;
                    return Err(error);
                }
            };
            if let Some(result) = protocol::response(self.provider, &id, &value) {
                return match result {
                    protocol::Response::Accepted(result) => Ok(result),
                    protocol::Response::Rejected(error) => Err(error),
                    protocol::Response::Invalid(error) => {
                        self.uncertain = true;
                        Err(error)
                    }
                };
            }
            if self.refreshing_pi {
                // State reads can race large output bursts. Normalize in order
                // without buffering a second copy or recursively refreshing.
                self.normalize_pending()?;
                self.normalize(value)?;
                continue;
            }
            let preflight_input = self.provider == Provider::Pi
                && method == "prompt"
                && value["type"] == "extension_ui_request"
                && matches!(
                    value["method"].as_str(),
                    Some("select" | "confirm" | "input" | "editor")
                );
            self.queue(value)?;
            if preflight_input {
                self.prompt_ack = Some((id, Instant::now() + Duration::from_secs(45)));
                return Ok(Value::Null);
            }
        }
    }
    fn queue(&mut self, value: Value) -> io::Result<()> {
        if self.pending.len() >= 128 {
            self.uncertain = true;
            return Err(io::Error::other(
                "Too many agent events before acknowledgement",
            ));
        }
        self.pending.push_back(value);
        Ok(())
    }
    fn normalize_pending(&mut self) -> io::Result<()> {
        while let Some(value) = self.pending.pop_front() {
            self.normalize(value)?;
        }
        Ok(())
    }
    fn check_prompt_ack(&mut self) -> io::Result<()> {
        if self
            .prompt_ack
            .as_ref()
            .is_some_and(|(_, deadline)| Instant::now() >= *deadline)
            && self.requests.is_empty()
        {
            self.uncertain = true;
            return Err(io::Error::other(
                "Agent timed out acknowledging prompt after extension input; inspect state before retrying",
            ));
        }
        Ok(())
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn validate_text(text: &str) -> io::Result<()> {
    if text.trim().is_empty() || text.len() > 32000 {
        Err(io::Error::other("Instruction must contain 1–32000 bytes"))
    } else {
        Ok(())
    }
}
fn required(value: &Value, key: &str) -> io::Result<String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other(format!("Agent response missing {key}")))
}
