//! Persistent, conditional GitHub REST reads and a durable observed-change feed.
pub mod api;
mod api_client;
mod client;
mod dashboard;
mod discovery_fallback;
mod entity;
pub mod local_auth;
mod policy;
mod pr_fields;
mod report;
mod repository;
mod scheduler;
mod store;

pub use api_client::{ApiClient, PrStatusSelection};
pub use client::{Client, Config, Freshness};
pub use dashboard::{
    AccountDiscoveryHealth, AccountRefreshCycle, PrStatusChange, PrStatusCoverage, PrStatusPage,
};
pub use policy::{RequiredCheck, RequiredChecksReport};
pub use pr_fields::PR_STATUS_FIELDS;
pub use report::{
    CiObservation, CiReport, CiSummary, FailedResult, PrReport, Report, ResourceValidation,
    ReviewStatus, SourceError,
};
pub use repository::{BranchReport, BranchTransition, RepositoryReport};
pub use scheduler::{RateLimit, Status};
pub use store::{Change, ChangePage, Snapshot, SnapshotPage, Watch, WatchKind};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("GitHub authentication unavailable; run gh auth login --hostname {0}")]
    Auth(String),
    #[error("{0}")]
    LocalAuth(String),
    #[error("cache storage error: {0}")]
    Storage(String),
    #[error("GitHub transport error: {0}")]
    Transport(String),
    #[error("GitHub returned HTTP {status}: {message}")]
    GitHub { status: u16, message: String },
    /// Operation errors returned in a successful GraphQL HTTP response.
    #[error("{message}")]
    GraphQL {
        message: String,
        access_denied: bool,
    },
    #[error("request queue is full")]
    QueueFull,
    #[error("request deadline exceeded")]
    Deadline,
    #[error("GitHub rate limited this request; retry after {retry_after_seconds} seconds")]
    RateLimited { retry_after_seconds: u64 },
    #[error("change cursor expired; bootstrap again with /v1/snapshot")]
    CursorExpired,
    #[error("no cached response available")]
    CacheMiss,
    #[error("request scheduler stopped")]
    Stopped,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Safe diagnostics: never persist upstream response bodies or credentials.
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::Invalid(message) if message.starts_with("incomplete") => "incomplete",
            Self::Invalid(_) => "invalid",
            Self::Auth(_) => "auth",
            Self::LocalAuth(_) => "local_auth",
            Self::Storage(_) => "storage",
            Self::Transport(_) => "transport",
            Self::GitHub { .. } => "github_http",
            Self::GraphQL {
                access_denied: true,
                ..
            } => "graphql_access_denied",
            Self::GraphQL { .. } => "graphql",
            Self::QueueFull => "queue_full",
            Self::Deadline => "deadline",
            Self::RateLimited { .. } => "rate_limited",
            Self::CursorExpired => "cursor_expired",
            Self::CacheMiss => "cache_miss",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Cache,
    Network,
    Revalidated,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub data: Value,
    pub fetched_at_ms: u64,
    pub validated_at_ms: u64,
    pub source: Source,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Pagination is deliberately explicit; each page has its own validators.
    pub link: Option<String>,
}

impl Response {
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.data.clone()).map_err(|e| Error::Invalid(e.to_string()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub state: String,
    pub title: String,
    pub updated_at: String,
    pub head: GitRef,
    #[serde(default)]
    pub merged: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitRef {
    pub sha: String,
    #[serde(rename = "ref")]
    pub name: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CiStatus {
    pub sha: String,
    pub check_runs: Vec<Value>,
    pub commit_status: Value,
    pub validated_at_ms: u64,
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn digest(s: &str) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(s.as_bytes()))
}
