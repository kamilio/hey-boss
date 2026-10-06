//! Bounded read-only messages for an authenticated companion transport.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TIMEOUT_MS: u64 = 3_600_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub hostname: String,
    pub user_id: u64,
    pub instance: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    pub identity: Identity,
    pub path: String,
    pub query: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub status: u16,
    pub retry_after_seconds: Option<u64>,
    pub body: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Probe { identity: Identity },
    Read { read: Read },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Identity {
        identity: Identity,
    },
    Reply {
        reply: Reply,
    },
    /// Only relay availability failures permit the SDK to try its local daemon.
    Unavailable,
}

impl Reply {
    /// Decode the original local-API response without masking upstream errors
    /// as relay transport failures eligible for fallback.
    pub fn decode<T: serde::de::DeserializeOwned>(self) -> Result<T> {
        if !(100..=599).contains(&self.status) {
            return Err(Error::Invalid("invalid shared response status".into()));
        }
        if !(200..300).contains(&self.status) {
            return Err(crate::api_client::response_error(
                self.status,
                self.retry_after_seconds,
                &self.body,
            ));
        }
        serde_json::from_value(self.body)
            .map_err(|_| Error::Invalid("invalid shared response body".into()))
    }
}

impl Read {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        if self.timeout_ms == 0
            || self.timeout_ms > MAX_TIMEOUT_MS
            || !supported_path(&self.path)
            || self.query.as_ref().is_some_and(|query| {
                query.len() > 8192
                    || !query.is_ascii()
                    || query.bytes().any(|b| b.is_ascii_control() || b == b'#')
                    || query.starts_with('?')
            })
        {
            return Err(Error::Invalid("invalid shared read".into()));
        }
        Ok(())
    }
}

impl Identity {
    pub fn validate(&self) -> Result<()> {
        if self.user_id == 0
            || self.hostname.len() > 253
            || self.hostname.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
            || self.instance.len() != 32
            || !self.instance.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Invalid("invalid shared daemon identity".into()));
        }
        Ok(())
    }
}

/// Paths are already encoded by the SDK. Refuse URL normalization and escaped
/// components instead of allowing them to turn a read into a different route.
pub(crate) fn supported_path(path: &str) -> bool {
    if path.len() > 1024 {
        return false;
    }
    let parts: Vec<_> = path.split('/').collect();
    let repository = |owner: &str, repo: &str| {
        crate::client::validate_repository(&format!("{owner}/{repo}")).is_ok()
    };
    let number = |value: &str| {
        !value.starts_with('0')
            && value.bytes().all(|b| b.is_ascii_digit())
            && value.parse::<u64>().is_ok_and(|n| n > 0)
    };
    match parts.as_slice() {
        // Raw source feeds include local explicit watches. Until their source
        // coverage is negotiated, only the account PR feed can be shared.
        ["", "v1", "pr-status"] => true,
        ["", "v1", "prs" | "repos", owner, repo] => repository(owner, repo),
        ["", "v1", "prs", owner, repo, n] => repository(owner, repo) && number(n),
        [
            "",
            "v1",
            "prs",
            owner,
            repo,
            n,
            "ci" | "metadata" | "required-checks",
        ] => repository(owner, repo) && number(n),
        ["", "v1", "repos", owner, repo, "prs" | "pr-lifecycles"] => repository(owner, repo),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_errors_keep_the_existing_api_error_contract() {
        for error in [
            Error::Deadline,
            Error::CacheMiss,
            Error::CursorExpired,
            Error::QueueFull,
            Error::RateLimited {
                retry_after_seconds: 17,
            },
            Error::GitHub {
                status: 403,
                message: "synthetic denial".into(),
            },
            Error::GraphQL {
                message: "synthetic denial".into(),
                access_denied: true,
            },
            Error::Auth("github.example.com".into()),
            Error::Transport("synthetic disconnect".into()),
        ] {
            let expected = error.to_string();
            let reply = Reply::from(error);
            let wire = serde_json::to_vec(&Response::Reply { reply }).unwrap();
            let Response::Reply { reply } = serde_json::from_slice(&wire).unwrap() else {
                panic!("reply envelope")
            };
            assert_eq!(reply.decode::<Value>().unwrap_err().to_string(), expected);
        }
    }

    fn read(path: &str) -> Read {
        Read {
            identity: Identity {
                hostname: "github.com".into(),
                user_id: 42,
                instance: "a".repeat(32),
            },
            path: path.into(),
            query: Some("cached_only=true".into()),
            timeout_ms: 5000,
        }
    }

    #[test]
    fn shared_reads_allow_only_supported_get_routes() {
        for path in [
            "/v1/pr-status",
            "/v1/prs/acme/demo",
            "/v1/prs/acme/demo/7",
            "/v1/prs/acme/demo/7/ci",
            "/v1/prs/acme/demo/7/metadata",
            "/v1/prs/acme/demo/7/required-checks",
            "/v1/repos/acme/demo",
            "/v1/repos/acme/demo/prs",
            "/v1/repos/acme/demo/pr-lifecycles",
        ] {
            assert!(read(path).validate().is_ok(), "{path}");
        }
        for path in [
            // These contain companion-local source watches. A same-account
            // handshake does not prove that the supervisor covers them.
            "/v1/snapshot",
            "/v1/changes",
            "https://example.com/v1/pr-status",
            "//example.com/v1/pr-status",
            "/v1/identity",
            "/v1/viewer",
            "/v1/status",
            "/v1/watches",
            "/v1/watches/123",
            "/v1/releases/observe",
            "/v1/prs/acme/demo/7/comments",
            "/v1/prs/acme/demo/0",
            "/v1/prs/acme/demo/7/../ci",
            "/v1/prs/acme/demo/%37",
            "/v1/prs/acme/demo/7?refresh=true",
            "/v1/prs/acme/demo/7#anything",
            "/v1/prs/acme%2fdemo/7",
            "/v1/pr-status/",
        ] {
            assert!(read(path).validate().is_err(), "{path}");
        }
    }

    #[test]
    fn shared_reads_require_bounded_messages_and_explicit_identity() {
        let mut request = read("/v1/pr-status");
        for timeout in [0, MAX_TIMEOUT_MS + 1, u64::MAX] {
            request.timeout_ms = timeout;
            assert!(request.validate().is_err());
        }
        request.timeout_ms = MAX_TIMEOUT_MS;
        assert!(request.validate().is_ok());
        request.identity.user_id = 0;
        assert!(request.validate().is_err());
        request.identity.user_id = 42;
        request.identity.instance.clear();
        assert!(request.validate().is_err());
        request.identity.instance = "a".repeat(32);
        for hostname in ["", "https://github.com", "github.com/path", "github.com\n"] {
            request.identity.hostname = hostname.into();
            assert!(request.validate().is_err());
        }
        request.identity.hostname = "github.com".into();
        for query in [
            "x".repeat(8193),
            "cursor=x\nheader=y".into(),
            "cursor=x#fragment".into(),
        ] {
            request.query = Some(query);
            assert!(request.validate().is_err());
        }
    }
}
