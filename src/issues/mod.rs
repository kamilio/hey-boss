//! Durable project issues. SQLite transactions arbitrate ownership; Markdown is text.
mod discovery;
use store::chief;
pub(crate) mod blockers;
mod fleet;
mod global_settings;
pub mod identity;
pub mod planning;
pub(crate) mod provenance;
pub mod remote;
mod store;
pub mod web;
pub mod worker;
mod worker_approvals;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub use store::Store;

pub const BODY_LIMIT: usize = 1024 * 1024;
pub const WIRE_LIMIT: usize = 16 * 1024 * 1024;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum PrPurpose {
    #[default]
    Unspecified,
    Fix,
    Prerequisite,
    SupportingEvidence,
}
impl PrPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Fix => "fix",
            Self::Prerequisite => "prerequisite",
            Self::SupportingEvidence => "supporting-evidence",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_input", message)
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new("conflict", message)
    }
    pub fn exit_code(&self) -> i32 {
        match self.code.as_str() {
            "invalid_input" | "identity_unavailable" => 2,
            "not_found" => 3,
            "conflict" | "fleet_reserved" | "fleet_allocation_missing" => 4,
            _ => 1,
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::new("io_error", value.to_string())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        let code = match &value {
            rusqlite::Error::SqliteFailure(error, _)
                if matches!(
                    error.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                "database_busy"
            }
            _ => "database_error",
        };
        Self::new(code, value.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::invalid(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub id: String,
    pub kind: String,
    pub session_id: Option<String>,
    pub machine: String,
    pub host: String,
    pub pid: Option<u32>,
    pub process_start: Option<String>,
    pub cwd: PathBuf,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<Invocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_run: Option<CreationRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreationRun {
    pub id: String,
    pub project_id: String,
    pub number: i64,
    pub title: Option<String>,
    pub started_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub offset: u64,
    pub call_id: Option<String>,
}

/// A guarded triage entry. The owner guard must be present, including null.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchEdit {
    pub number: i64,
    pub if_version: i64,
    #[serde(deserialize_with = "required_assignee")]
    pub expected_assignee: Option<String>,
    #[serde(default)]
    pub add_labels: Vec<String>,
    #[serde(default)]
    pub remove_labels: Vec<String>,
    #[serde(default)]
    pub assignment: BatchAssignment,
}
fn required_assignee<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchAssignment {
    #[default]
    Keep,
    Unassign,
    Boss,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Attachment {
        operation: crate::attachments::Operation,
    },
    Batch {
        edits: Vec<BatchEdit>,
        #[serde(default)]
        dry_run: bool,
    },
    Artifact {
        operation: crate::artifacts::Operation,
    },
    Mindmap {
        operation: crate::mindmap::Operation,
    },
    Projects {
        #[serde(default)]
        include_hidden: bool,
    },
    HideProject,
    RestoreProject,
    Workers {
        worker_id: Option<String>,
    },
    ConfigureWorker {
        worker_id: Option<String>,
        config: worker::Settings,
        if_version: Option<i64>,
    },
    ControlWorker {
        worker_id: String,
        command: String,
        run_id: Option<String>,
    },
    PreviewWorker {
        config: worker::Settings,
        number: Option<i64>,
        /// Unsaved project permission, used only by the read-only settings preview.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_allowed: Option<bool>,
        /// Read-only task preview; omitted for ordinary worker candidate previews.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_kind: Option<String>,
    },
    GlobalSettings,
    ConfigureGlobal {
        boss_name: String,
        if_version: Option<i64>,
    },
    ProjectSettings,
    ConfigureProject {
        prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chief_enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chief_prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        boss_name: Option<String>,
        prs_enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_overrides: Option<worker::PromptOverrides>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drafts_enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan_template: Option<String>,
        if_version: Option<i64>,
    },
    PullRequests {
        number: i64,
    },
    AddPullRequest {
        number: i64,
        url: String,
        #[serde(default)]
        purpose: PrPurpose,
    },
    ClassifyPullRequest {
        number: i64,
        url: String,
        purpose: PrPurpose,
    },
    RemovePullRequest {
        number: i64,
        url: String,
    },
    WorkerStatus,
    WorkerPreview {
        config: worker::ProjectConfig,
        number: Option<i64>,
    },
    WorkerRun {
        run_id: String,
    },
    WorkerConfigure {
        config: worker::ProjectConfig,
        if_version: Option<i64>,
    },
    WorkerPool {
        concurrency: u32,
    },
    WorkerControl {
        run_id: Option<String>,
        command: String,
    },
    Whoami,
    List {
        state: String,
        mine: bool,
        unassigned: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assignee: Option<String>,
        labels: Vec<String>,
        search: Option<String>,
        limit: u32,
        offset: u32,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all: bool,
    },
    Transfer {
        number: i64,
        destination: String,
        if_version: i64,
    },
    Move {
        number: i64,
        before: Option<i64>,
        after: Option<i64>,
        if_order_version: Option<i64>,
    },
    View {
        number: i64,
    },
    Allocation {
        number: i64,
        machine: String,
    },
    Subtasks {
        number: i64,
        #[serde(default)]
        include_deleted: bool,
    },
    CreateSubtask {
        number: i64,
        title: String,
        body: String,
        labels: Vec<String>,
        #[serde(default)]
        at_top: bool,
        if_version: Option<i64>,
    },
    AddSubtask {
        number: i64,
        child: i64,
        if_version: Option<i64>,
        if_child_version: Option<i64>,
    },
    RemoveSubtask {
        number: i64,
        child: i64,
        if_version: Option<i64>,
        if_child_version: Option<i64>,
    },
    History {
        number: i64,
        limit: u32,
        offset: u32,
    },
    Comments {
        number: i64,
        limit: u32,
        offset: u32,
        #[serde(default)]
        sort: CommentSort,
    },
    Create {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        draft: bool,
        title: String,
        body: String,
        labels: Vec<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        at_top: bool,
    },
    Edit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        draft: Option<bool>,
        number: i64,
        title: Option<String>,
        body: Option<String>,
        add_labels: Vec<String>,
        remove_labels: Vec<String>,
        if_version: Option<i64>,
    },
    SetYolo {
        number: i64,
        enabled: bool,
        if_version: i64,
    },
    BindPlan {
        number: i64,
        plan: planning::Plan,
        if_version: i64,
    },
    ReadPlan {
        plan: planning::Plan,
    },
    Undraft {
        number: i64,
    },
    Claim {
        number: i64,
        force: bool,
    },
    AssignBoss {
        number: i64,
        force: bool,
    },
    Unassign {
        number: i64,
        force: bool,
    },
    Comment {
        number: i64,
        body: String,
    },
    Status {
        number: i64,
        level: StatusLevel,
        comment: String,
    },
    StatusHistory {
        number: i64,
        limit: u32,
        offset: u32,
        #[serde(default)]
        before: Option<i64>,
    },
    StatusView {
        number: i64,
    },
    ResolveComment {
        number: i64,
        comment_id: i64,
        resolved: bool,
    },
    Close {
        number: i64,
        comment: Option<String>,
        force: bool,
    },
    SetBlockers {
        number: i64,
        blockers: Vec<i64>,
        #[serde(default)]
        if_version: Option<i64>,
        #[serde(default)]
        force: bool,
    },
    Block {
        number: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blockers: Option<Vec<i64>>,
        comment: Option<String>,
        force: bool,
    },
    Reopen {
        number: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        if_version: Option<i64>,
    },
    Delete {
        number: i64,
        force: bool,
    },
    Restore {
        number: i64,
    },
}
impl Operation {
    pub fn writes(&self) -> bool {
        if let Self::Attachment { operation } = self {
            return operation.writes();
        }
        if let Self::Artifact { operation } = self {
            return operation.writes();
        }
        if let Self::Mindmap { operation } = self {
            return operation.writes();
        }
        !matches!(
            self,
            Self::Projects { .. }
                | Self::Workers { .. }
                | Self::PreviewWorker { .. }
                | Self::GlobalSettings
                | Self::ProjectSettings
                | Self::PullRequests { .. }
                | Self::WorkerStatus
                | Self::WorkerPreview { .. }
                | Self::WorkerRun { .. }
                | Self::Whoami
                | Self::List { .. }
                | Self::ReadPlan { .. }
                | Self::View { .. }
                | Self::Allocation { .. }
                | Self::Subtasks { .. }
                | Self::History { .. }
                | Self::Comments { .. }
                | Self::StatusHistory { .. }
                | Self::StatusView { .. }
                | Self::Batch { dry_run: true, .. }
        )
    }
    pub fn needs_actor(&self) -> bool {
        self.writes() || matches!(self, Self::Whoami | Self::List { mine: true, .. })
    }
    pub fn number(&self) -> Option<i64> {
        match self {
            Self::Attachment { .. }
            | Self::Artifact { .. }
            | Self::Batch { .. }
            | Self::Mindmap { .. }
            | Self::Projects { .. }
            | Self::HideProject
            | Self::RestoreProject
            | Self::Workers { .. }
            | Self::ConfigureWorker { .. }
            | Self::ControlWorker { .. }
            | Self::PreviewWorker { .. }
            | Self::GlobalSettings
            | Self::ConfigureGlobal { .. }
            | Self::ProjectSettings
            | Self::ConfigureProject { .. }
            | Self::WorkerStatus
            | Self::WorkerPreview { .. }
            | Self::WorkerRun { .. }
            | Self::WorkerConfigure { .. }
            | Self::WorkerPool { .. }
            | Self::WorkerControl { .. }
            | Self::Whoami
            | Self::List { .. }
            | Self::ReadPlan { .. }
            | Self::Create { .. } => None,
            Self::PullRequests { number }
            | Self::AddPullRequest { number, .. }
            | Self::ClassifyPullRequest { number, .. }
            | Self::RemovePullRequest { number, .. }
            | Self::Move { number, .. }
            | Self::Transfer { number, .. }
            | Self::View { number }
            | Self::Allocation { number, .. }
            | Self::Subtasks { number, .. }
            | Self::CreateSubtask { number, .. }
            | Self::AddSubtask { number, .. }
            | Self::RemoveSubtask { number, .. }
            | Self::History { number, .. }
            | Self::Comments { number, .. }
            | Self::Edit { number, .. }
            | Self::SetYolo { number, .. }
            | Self::BindPlan { number, .. }
            | Self::Undraft { number }
            | Self::Claim { number, .. }
            | Self::AssignBoss { number, .. }
            | Self::Unassign { number, .. }
            | Self::Comment { number, .. }
            | Self::Status { number, .. }
            | Self::StatusHistory { number, .. }
            | Self::StatusView { number }
            | Self::ResolveComment { number, .. }
            | Self::Close { number, .. }
            | Self::Block { number, .. }
            | Self::SetBlockers { number, .. }
            | Self::Reopen { number, .. }
            | Self::Delete { number, .. }
            | Self::Restore { number } => Some(*number),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum CommentSort {
    #[default]
    Newest,
    Oldest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum StatusLevel {
    Green,
    Orange,
    Red,
}
impl StatusLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Orange => "orange",
            Self::Red => "red",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub project: Project,
    pub project_override: Option<String>,
    pub actor: Option<Actor>,
    pub operation: Operation,
    pub request_id: Option<String>,
}

pub fn identifier(value: &str, name: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(Error::invalid(format!(
            "{name} must be nonblank, at most {max} bytes, and contain no control characters"
        )));
    }
    Ok(())
}

pub fn database_path() -> Result<PathBuf> {
    database_path_for_installation(&std::env::current_exe()?)
}

/// Resolve a staged upgrade's database using the installed CLI's state pointer.
/// Environment overrides follow the same precedence as ordinary commands.
pub fn database_path_for_installation(exe: &std::path::Path) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HEY_BOSS_ISSUE_DB").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(Error::invalid("HEY_BOSS_ISSUE_DB must be absolute"));
        }
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("HEY_BOSS_STATE_DIR").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(Error::invalid("HEY_BOSS_STATE_DIR must be absolute"));
        }
        return Ok(path.join("issues.db"));
    }
    // Linux current_exe() names the unlinked inode after CLI replacement.
    // Explicit database overrides must work while that worker drains.
    let exe = exe.canonicalize()?;
    match std::fs::read_to_string(exe.with_file_name("hey-boss.state")) {
        Ok(value) => {
            let path = PathBuf::from(value.trim());
            if !path.is_absolute() {
                return Err(Error::invalid(
                    "hey-boss.state must contain an absolute state directory",
                ));
            }
            return Ok(path.join("issues.db"));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let home = std::env::var_os("HOME")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Error::invalid("HOME is unavailable"))?;
    let root = PathBuf::from(home);
    if !root.is_absolute() {
        return Err(Error::invalid("HOME must be absolute"));
    }
    #[cfg(target_os = "macos")]
    let root = root.join("Library/Application Support/hey-boss");
    #[cfg(not(target_os = "macos"))]
    let root = match std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(Error::invalid("XDG_DATA_HOME must be absolute"));
            }
            path.join("hey-boss")
        }
        None => root.join(".local/share/hey-boss"),
    };
    Ok(root.join("issues.db"))
}
