use super::*;

// Only fixed, allowlisted categories may reach callers or persisted state.
// OAuth error bodies can echo tokens, so never retain their text or unknown codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RefreshError {
    Unavailable,
    Expired,
    Reused,
    Revoked,
    Invalid,
    Rejected,
    OutcomeUnknown,
}

impl RefreshError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "codex_oauth_unavailable",
            Self::Expired => "codex_refresh_token_expired",
            Self::Reused => "codex_refresh_token_reused",
            Self::Revoked => "codex_refresh_token_revoked",
            Self::Invalid => "codex_refresh_token_invalid",
            Self::Rejected => "codex_oauth_rejected",
            Self::OutcomeUnknown => "codex_refresh_outcome_unknown",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Unavailable => {
                "Codex OAuth is temporarily unavailable or cooling down; retry shortly"
            }
            Self::Expired => {
                "Codex refresh token expired; sign in again with hey-proxy codex-login for this account"
            }
            Self::Reused => {
                "Codex refresh token was already used; sign in again with hey-proxy codex-login for this account"
            }
            Self::Revoked => {
                "Codex refresh token was revoked; sign in again with hey-proxy codex-login for this account"
            }
            Self::Invalid => {
                "Codex refresh token is invalid; sign in again with hey-proxy codex-login for this account"
            }
            Self::Rejected => {
                "Codex OAuth refresh was rejected; sign in again with hey-proxy codex-login for this account"
            }
            Self::OutcomeUnknown => {
                "Codex OAuth refresh may have completed without saving its replacement token; sign in again with hey-proxy codex-login for this account"
            }
        }
    }
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for RefreshError {}

pub(super) async fn exchange_refresh(
    client: &reqwest::Client,
    endpoint: &str,
    previous: &Tokens,
) -> std::result::Result<Tokens, RefreshError> {
    let mut response = client
        .post(endpoint)
        .timeout(Duration::from_secs(30))
        .header("accept", "application/json")
        .json(&json!({
            "client_id": CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": previous.refresh_token,
            "scope": "openid profile email",
        }))
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() || error.is_builder() {
                RefreshError::Unavailable
            } else {
                RefreshError::OutcomeUnknown
            }
        })?;
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(RefreshError::Unavailable);
    }
    let rejected = matches!(status.as_u16(), 400 | 401 | 403);
    let unreadable = if rejected {
        RefreshError::Rejected
    } else {
        RefreshError::OutcomeUnknown
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unreadable)? {
        if bytes.len() + chunk.len() > MAX_BYTES as usize {
            return Err(unreadable);
        }
        bytes.extend_from_slice(&chunk);
    }
    if status.is_success() {
        return parse_token_response(&bytes, Some(previous))
            .map_err(|_| RefreshError::OutcomeUnknown);
    }
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let code = value["error"]["code"]
        .as_str()
        .or_else(|| value["error"].as_str())
        .or_else(|| value["code"].as_str());
    Err(match code {
        Some("refresh_token_expired") => RefreshError::Expired,
        Some("refresh_token_reused") => RefreshError::Reused,
        Some("refresh_token_invalidated" | "token_revoked") => RefreshError::Revoked,
        Some("invalid_grant" | "invalid_refresh_token") => RefreshError::Invalid,
        Some("invalid_client") => RefreshError::Rejected,
        _ => unreadable,
    })
}
