//! Worker selection uses included quota only; it never configures or executes routing.
use super::{Account, AccountUsage, ExpiringWindow, State, Window, parse_timestamp};
use serde::{Deserialize, Serialize};

pub const WORKER_SCHEMA_VERSION: u32 = 2;
pub const MAX_READING_AGE_SECONDS: u64 = 300;
pub const MAX_WORKER_CANDIDATES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Codex,
    Claude,
    Pi,
}
impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Pi => "pi",
        }
    }
}

/// Ordered independently of route legs. Provider is a named connection alias.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerCandidate {
    pub provider: String,
    pub model: String,
    pub runtime: Runtime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    LowerPreference,
    RuntimeUnavailable,
    NoSubscriptionRoute,
    ReadingUnavailable,
    ReadingExpired,
    UnknownLimit,
    UnknownScope,
    IncludedExhausted,
    ResetNeedsRefresh,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IncludedQuota {
    pub reading_updated_at: u64,
    pub expires_at: u64,
    pub remaining_percent: f64,
    pub windows: Vec<ExpiringWindow>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerEvidence {
    #[serde(flatten)]
    pub candidate: WorkerCandidate,
    pub account: Account,
    pub upstream_model: String,
    pub quota: IncludedQuota,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerEvaluation {
    #[serde(flatten)]
    pub candidate: WorkerCandidate,
    pub reading_updated_at: Option<u64>,
    pub retry_at: u64,
    pub skip_reason: Option<SkipReason>,
    pub evidence: Option<WorkerEvidence>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationStatus {
    Recommended,
    NoRecommendation,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerRecommendation {
    pub schema_version: u32,
    pub config_revision: String,
    pub generated_at: u64,
    /// Inclusive deadline: never assign from this snapshot at or after this time.
    pub expires_at: u64,
    pub recheck_at: u64,
    pub status: RecommendationStatus,
    pub selected: Option<WorkerEvidence>,
    pub candidates: Vec<WorkerEvaluation>,
}

/// All reported applicable limits must be known and positive. Extra spend is never read.
/// Unknown model scopes fail closed; known irrelevant scopes do not block other models.
pub fn included_quota(
    usage: &AccountUsage,
    model: &str,
    now: u64,
) -> Result<IncludedQuota, SkipReason> {
    let reading = &usage.reading;
    if usage.schema_version != super::SCHEMA_VERSION
        || reading.state != State::Ok
        || reading.error.is_some()
    {
        return Err(SkipReason::ReadingUnavailable);
    }
    let updated = reading
        .updated_at
        .filter(|t| *t > 0 && *t <= now)
        .ok_or(SkipReason::ReadingExpired)?;
    let mut expires_at = updated.saturating_add(MAX_READING_AGE_SECONDS);
    if now >= expires_at {
        return Err(SkipReason::ReadingExpired);
    }
    let data = reading.data.as_ref().ok_or(SkipReason::UnknownLimit)?;
    if data.availability_unknown {
        return Err(SkipReason::UnknownLimit);
    }
    let mut windows = Vec::new();
    let mut remaining: f64 = 100.0;
    let mut has_account_limit = false;
    for window in &data.windows {
        if !applies(window, model)? {
            continue;
        }
        has_account_limit |= window.model.is_none()
            && (matches!(window.id.as_str(), "five_hour" | "seven_day")
                || (window.group.is_none()
                    && !matches!(
                        window.id.as_str(),
                        "seven_day_sonnet" | "seven_day_opus" | "seven_day_routines"
                    )));
        let rem = remaining_percent(window).ok_or(SkipReason::UnknownLimit)?;
        if rem <= 0.0 {
            return Err(SkipReason::IncludedExhausted);
        }
        let reset = match window.resets_at.as_deref() {
            None => None,
            Some(value) => Some(parse_timestamp(value).ok_or(SkipReason::UnknownLimit)?),
        };
        if let Some(reset) = reset {
            if reset <= now {
                return Err(SkipReason::ResetNeedsRefresh);
            }
            expires_at = expires_at.min(reset);
        }
        remaining = remaining.min(rem);
        windows.push(ExpiringWindow {
            id: window.id.clone(),
            label: window.label.clone(),
            remaining_percent: Some(rem),
            used_percent: window.used_percent,
            resets_at: window.resets_at.clone(),
            resets_in_seconds: reset.map(|t| t - now),
        });
    }
    if windows.is_empty() || !has_account_limit {
        return Err(SkipReason::UnknownLimit);
    }
    Ok(IncludedQuota {
        reading_updated_at: updated,
        expires_at,
        remaining_percent: remaining,
        windows,
    })
}

pub(super) fn remaining_percent(window: &Window) -> Option<f64> {
    let remaining = window.remaining_percent;
    let used = window.used_percent;
    if remaining.is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v))
        || used.is_some_and(|v| !v.is_finite() || v < 0.0)
    {
        return None;
    }
    match (remaining, used) {
        (Some(r), Some(u)) => Some(r.min((100.0 - u).max(0.0))),
        (Some(r), None) => Some(r),
        (None, Some(u)) => Some((100.0 - u).max(0.0)),
        _ => None,
    }
}

fn applies(window: &Window, model: &str) -> Result<bool, SkipReason> {
    // Version 1 has no model selection: conservatively check every reported scope.
    if model.is_empty() {
        return Ok(true);
    }
    let model = model.to_ascii_lowercase();
    if let Some(scope) = &window.model {
        if scope.is_empty() {
            return Err(SkipReason::UnknownScope);
        }
        return Ok(scope.eq_ignore_ascii_case(&model));
    }
    match window.id.as_str() {
        "seven_day_sonnet" => return Ok(model.starts_with("claude-sonnet-")),
        "seven_day_opus" => return Ok(model.starts_with("claude-opus-")),
        "seven_day_routines" => return Ok(false),
        "codex-spark" | "codex-spark-weekly" => return Ok(model.contains("spark")),
        _ => {}
    }
    match window.group.as_deref() {
        None | Some("account") => Ok(true),
        Some(scope) if scope.starts_with("model:") => Ok(scope[6..] == model),
        _ => Err(SkipReason::UnknownScope),
    }
}
