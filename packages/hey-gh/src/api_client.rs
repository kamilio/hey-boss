//! SDK transport for the shared daemon. No GitHub token is needed in callers.
use crate::{
    ChangePage, Error, Freshness, Report, Result, SnapshotPage, Status, Watch, api::WatchStatus,
    client::validate_repository,
};
use reqwest::{RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::time::Duration;
use url::Url;

/// PR feed scope and optional row projection. Omitted fields are unknown.
/// Transport projections always retain `complete` and `sourceErrors`.
#[derive(Clone, Copy, Default)]
pub struct PrStatusSelection<'a> {
    pub repository: Option<&'a str>,
    pub cursor: Option<&'a str>,
    pub fields: Option<&'a [&'a str]>,
}

#[derive(Clone)]
pub struct ApiClient {
    base: Url,
    http: reqwest::Client,
}
impl ApiClient {
    pub async fn pr_status(
        &self,
        repository: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
        wait: Duration,
        freshness: Freshness,
    ) -> Result<crate::PrStatusPage> {
        self.pr_status_selected(
            PrStatusSelection {
                repository,
                cursor,
                fields: None,
            },
            limit,
            wait,
            freshness,
        )
        .await
    }

    /// Same cursor/event boundaries as a full read; only PR row fields change.
    pub async fn pr_status_selected(
        &self,
        selection: PrStatusSelection<'_>,
        limit: usize,
        wait: Duration,
        freshness: Freshness,
    ) -> Result<crate::PrStatusPage> {
        let PrStatusSelection {
            repository,
            cursor,
            fields,
        } = selection;
        if let Some(fields) = fields {
            crate::pr_fields::validate(fields)?;
        }
        if let Some(repo) = repository {
            validate_repository(repo)?;
        }
        if !(1..=1000).contains(&limit) || wait > Duration::from_secs(30) {
            return Err(Error::Invalid(
                "limit must be 1..1000 and wait <=30 seconds".into(),
            ));
        }
        let mut request = self
            .http
            .get(self.url("v1/pr-status"))
            .query(&freshness_query(freshness)?)
            .query(&[
                ("limit", limit.to_string()),
                ("wait_seconds", wait.as_secs().to_string()),
            ]);
        if let Some(repo) = repository {
            request = request.query(&[("repository", repo)]);
        }
        if let Some(cursor) = cursor {
            request = request.query(&[("cursor", cursor)]);
        }
        if let Some(fields) = fields {
            request = request.query(&[("fields", fields.join(","))]);
        }
        self.read(request).await
    }

    pub async fn watch_account(&self, interval_seconds: u64) -> Result<Watch> {
        self.read(
            self.http
                .post(self.url("v1/watches"))
                .json(&serde_json::json!({"kind":"account","interval_seconds":interval_seconds})),
        )
        .await
    }
    pub fn new(base: Url) -> Result<Self> {
        if base.scheme() != "http"
            || !matches!(base.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            return Err(Error::Invalid(
                "daemon URL must be an HTTP loopback origin".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self { base, http })
    }
    fn url(&self, path: &str) -> Url {
        self.base.join(path).expect("validated API path")
    }

    fn authorize(&self, request: RequestBuilder) -> Result<RequestBuilder> {
        // Reload per request so a long-lived SDK survives a daemon restart.
        if let Some(token) =
            crate::local_auth::token(self.base.port_or_known_default().expect("HTTP port"))?
        {
            Ok(request.bearer_auth(token))
        } else {
            Ok(request)
        }
    }
    pub async fn pr_report(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Report> {
        validate_repository(repository)?;
        self.read(
            self.http
                .get(self.url(&format!("v1/prs/{repository}/{number}")))
                .query(&freshness_query(freshness)?),
        )
        .await
    }
    pub async fn ci_for_pr(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<crate::CiObservation> {
        validate_repository(repository)?;
        self.read(
            self.http
                .get(self.url(&format!("v1/prs/{repository}/{number}/ci")))
                .query(&freshness_query(freshness)?),
        )
        .await
    }
    pub async fn repository_report(
        &self,
        repository: &str,
        branches: &[String],
        all_branches: bool,
        freshness: Freshness,
    ) -> Result<crate::RepositoryReport> {
        validate_repository(repository)?;
        for branch in branches {
            crate::repository::validate_branch(branch)?;
        }
        self.read(
            self.http
                .get(self.url(&format!("v1/repos/{repository}")))
                .query(&freshness_query(freshness)?)
                .query(&[
                    (
                        "branches",
                        serde_json::to_string(branches)
                            .map_err(|e| Error::Invalid(e.to_string()))?,
                    ),
                    ("all_branches", all_branches.to_string()),
                ]),
        )
        .await
    }
    pub async fn required_checks_for_pr(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<crate::RequiredChecksReport> {
        validate_repository(repository)?;
        self.read(
            self.http
                .get(self.url(&format!("v1/prs/{repository}/{number}/required-checks")))
                .query(&freshness_query(freshness)?),
        )
        .await
    }
    pub async fn watch_repository(
        &self,
        repository: &str,
        branches: Vec<String>,
        all_branches: bool,
        interval_seconds: u64,
    ) -> Result<Watch> {
        validate_repository(repository)?;
        for branch in &branches {
            crate::repository::validate_branch(branch)?;
        }
        self.read(self.http.post(self.url("v1/watches")).json(&serde_json::json!({"kind":"branches","repository":repository,"branches":branches,"all_branches":all_branches,"interval_seconds":interval_seconds}))).await
    }
    pub async fn list_pull_requests(
        &self,
        repository: &str,
        state: &str,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        validate_repository(repository)?;
        if !matches!(state, "open" | "closed" | "all") {
            return Err(Error::Invalid(
                "PR state must be open, closed, or all".into(),
            ));
        }
        self.read(
            self.http
                .get(self.url(&format!("v1/repos/{repository}/prs")))
                .query(&freshness_query(freshness)?)
                .query(&[("state", state)]),
        )
        .await
    }
    pub async fn my_pull_requests(
        &self,
        repository: &str,
        freshness: Freshness,
    ) -> Result<Vec<Value>> {
        validate_repository(repository)?;
        self.read(
            self.http
                .get(self.url(&format!("v1/prs/{repository}")))
                .query(&freshness_query(freshness)?),
        )
        .await
    }
    pub async fn watch(
        &self,
        repository: &str,
        number: Option<u64>,
        interval_seconds: u64,
    ) -> Result<Watch> {
        validate_repository(repository)?;
        self.read(
            self.http
                .post(self.url("v1/watches"))
                .json(&serde_json::json!({
                    "repository":repository,"pull_number":number,"interval_seconds":interval_seconds
                })),
        )
        .await
    }
    pub async fn watches(&self) -> Result<Vec<WatchStatus>> {
        self.read(self.http.get(self.url("v1/watches"))).await
    }
    pub async fn unwatch(&self, id: &str) -> Result<()> {
        if id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::Invalid("invalid watch ID".into()));
        }
        let response = self
            .authorize(self.http.delete(self.url(&format!("v1/watches/{id}"))))?
            .send()
            .await
            .map_err(transport)?;
        if response.status() == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(remote_error(response).await)
        }
    }
    pub async fn status(&self) -> Result<Status> {
        self.read(self.http.get(self.url("v1/status"))).await
    }
    pub async fn bootstrap(&self) -> Result<SnapshotPage> {
        self.read(self.http.get(self.url("v1/snapshot"))).await
    }
    pub async fn changes(
        &self,
        cursor: Option<&str>,
        limit: usize,
        wait: Duration,
    ) -> Result<ChangePage> {
        if !(1..=1000).contains(&limit) || wait > Duration::from_secs(30) {
            return Err(Error::Invalid(
                "limit must be 1..1000 and wait must be <=30 seconds".into(),
            ));
        }
        let mut request = self.http.get(self.url("v1/changes")).query(&[
            ("limit", limit.to_string()),
            ("wait_seconds", wait.as_secs().to_string()),
        ]);
        if let Some(cursor) = cursor {
            request = request.query(&[("cursor", cursor)]);
        }
        self.read(request).await
    }
    async fn read<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        let response = self.authorize(request)?.send().await.map_err(transport)?;
        if !response.status().is_success() {
            return Err(remote_error(response).await);
        }
        response.json().await.map_err(transport)
    }
}

fn freshness_query(freshness: Freshness) -> Result<Vec<(&'static str, String)>> {
    Ok(match freshness {
        Freshness::Revalidate => vec![("refresh", "true".into())],
        Freshness::CachedOnly => vec![("cached_only", "true".into())],
        Freshness::MaxAge(age) => {
            if age > Duration::from_secs(86400) {
                return Err(Error::Invalid("max age must be <=86400 seconds".into()));
            }
            vec![("max_age_seconds", age.as_secs().to_string())]
        }
    })
}
fn transport(e: reqwest::Error) -> Error {
    Error::Transport(e.without_url().to_string())
}
async fn remote_error(response: reqwest::Response) -> Error {
    let status = response.status();
    let retry = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    let message = body["error"]
        .as_str()
        .unwrap_or("local API error")
        .to_owned();
    let upstream_status = body["upstream_status"]
        .as_u64()
        .filter(|value| (100..=599).contains(value) && !(200..300).contains(value));
    match body["code"].as_str().unwrap_or("") {
        "local_auth" => Error::LocalAuth(message),
        "auth" => {
            let hostname = body["auth_hostname"]
                .as_str()
                .or_else(|| {
                    message.strip_prefix(
                        "GitHub authentication unavailable; run gh auth login --hostname ",
                    )
                })
                .filter(|hostname| !hostname.is_empty());
            match hostname {
                Some(hostname) => Error::Auth(hostname.to_owned()),
                None => Error::Invalid(format!(
                    "daemon authentication error omitted its GitHub hostname: {message}"
                )),
            }
        }
        "storage" => Error::Storage(
            body["cause"]
                .as_str()
                .unwrap_or_else(|| {
                    message
                        .strip_prefix("cache storage error: ")
                        .unwrap_or(&message)
                })
                .to_owned(),
        ),
        "transport" => Error::Transport(
            body["cause"]
                .as_str()
                .unwrap_or_else(|| {
                    message
                        .strip_prefix("GitHub transport error: ")
                        .unwrap_or(&message)
                })
                .to_owned(),
        ),
        "stopped" => Error::Stopped,
        "cursor_expired" => Error::CursorExpired,
        "queue_full" => Error::QueueFull,
        "rate_limited" => Error::RateLimited {
            retry_after_seconds: retry.unwrap_or(60),
        },
        "deadline" => Error::Deadline,
        "cache_miss" => Error::CacheMiss,
        "invalid" => Error::Invalid(message),
        "graphql" | "graphql_access_denied" => Error::GraphQL {
            message,
            access_denied: body["code"] == "graphql_access_denied",
        },
        _ => Error::GitHub {
            status: upstream_status.map_or(status.as_u16(), |value| value as u16),
            message: if upstream_status.is_some() {
                body["upstream_message"]
                    .as_str()
                    .unwrap_or(&message)
                    .to_owned()
            } else {
                message
            },
        },
    }
}
