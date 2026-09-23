//! Node.js bindings for the daemon SDK. Callers share its queue and gh login.
use hey_gh::{ApiClient, Error, Freshness};
use napi::Result;
use napi_derive::napi;
use serde::Serialize;
use serde_json::{Value, json};
use std::time::Duration;

fn js_error(error: Error) -> napi::Error {
    let code = match &error {
        Error::Invalid(_) => "invalid",
        Error::Auth(_) => "auth",
        Error::LocalAuth(_) => "local_auth",
        Error::Storage(_) => "storage",
        Error::Transport(_) => "transport",
        Error::GitHub { .. } => "github",
        Error::GraphQL {
            access_denied: true,
            ..
        } => "graphql_access_denied",
        Error::GraphQL { .. } => "graphql",
        Error::QueueFull => "queue_full",
        Error::Deadline => "deadline",
        Error::RateLimited { .. } => "rate_limited",
        Error::CursorExpired => "cursor_expired",
        Error::CacheMiss => "cache_miss",
        Error::Stopped => "stopped",
    };
    let retry = match &error {
        Error::RateLimited {
            retry_after_seconds,
        } => Some(*retry_after_seconds),
        _ => None,
    };
    let status = match &error {
        Error::GitHub { status, .. } => Some(*status),
        _ => None,
    };
    napi::Error::new(
        napi::Status::GenericFailure,
        format!(
            "HEY_GH_ERROR:{}",
            json!({"code":code,"message":error.to_string(),"retryAfterSeconds":retry,"httpStatus":status})
        ),
    )
}
fn invalid(message: impl Into<String>) -> napi::Error {
    js_error(Error::Invalid(message.into()))
}
fn integer(value: f64, name: &str, min: u64, max: u64) -> Result<u64> {
    if !value.is_finite() || value.fract() != 0.0 || value < min as f64 || value > max as f64 {
        return Err(invalid(format!(
            "{name} must be an integer in {min}..{max}"
        )));
    }
    Ok(value as u64)
}
fn pull_number(number: f64) -> Result<u64> {
    integer(number, "pull number", 1, 9_007_199_254_740_991)
}
fn json_value<T: Serialize>(value: T) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| js_error(Error::Invalid(e.to_string())))
}

#[napi(object)]
#[derive(Default)]
pub struct ReadOptions {
    pub refresh: Option<bool>,
    pub cached_only: Option<bool>,
    pub max_age_seconds: Option<f64>,
}
impl ReadOptions {
    fn freshness(options: Option<Self>) -> Result<Freshness> {
        let options = options.unwrap_or_default();
        if options.refresh == Some(true) && options.cached_only == Some(true) {
            return Err(invalid("refresh and cachedOnly are mutually exclusive"));
        }
        // Validate even ignored options, preventing silently accepted bad input.
        let age = integer(
            options.max_age_seconds.unwrap_or(30.0),
            "maxAgeSeconds",
            0,
            86400,
        )?;
        Ok(if options.refresh == Some(true) {
            Freshness::Revalidate
        } else if options.cached_only == Some(true) {
            Freshness::CachedOnly
        } else {
            Freshness::MaxAge(Duration::from_secs(age))
        })
    }
}
#[napi(object)]
#[derive(Default)]
pub struct RepositoryOptions {
    pub branches: Option<Vec<String>>,
    pub all_branches: Option<bool>,
    pub read: Option<ReadOptions>,
}
#[napi(object)]
#[derive(Default)]
pub struct WatchOptions {
    pub pull_number: Option<f64>,
    pub interval_seconds: Option<f64>,
}
#[napi(object)]
#[derive(Default)]
pub struct RepositoryWatchOptions {
    pub branches: Option<Vec<String>>,
    pub all_branches: Option<bool>,
    pub interval_seconds: Option<f64>,
}
#[napi(object)]
#[derive(Default)]
pub struct ChangesOptions {
    pub cursor: Option<String>,
    pub limit: Option<f64>,
    pub wait_seconds: Option<f64>,
}

#[napi(object)]
#[derive(Default)]
pub struct PrStatusOptions {
    pub repository: Option<String>,
    pub fields: Option<Vec<String>>,
    pub cursor: Option<String>,
    pub limit: Option<f64>,
    pub wait_seconds: Option<f64>,
    pub read: Option<ReadOptions>,
}

#[napi]
pub struct NativeApiClient {
    inner: ApiClient,
}
#[napi]
impl NativeApiClient {
    #[napi]
    pub async fn pr_status(&self, options: Option<PrStatusOptions>) -> Result<Value> {
        let options = options.unwrap_or_default();
        let limit = integer(options.limit.unwrap_or(1000.0), "limit", 1, 1000)? as usize;
        let wait = integer(options.wait_seconds.unwrap_or(0.0), "waitSeconds", 0, 30)?;
        let fields = options
            .fields
            .as_ref()
            .map(|fields| fields.iter().map(String::as_str).collect::<Vec<_>>());
        json_value(
            self.inner
                .pr_status_selected(
                    hey_gh::PrStatusSelection {
                        repository: options.repository.as_deref(),
                        cursor: options.cursor.as_deref(),
                        fields: fields.as_deref(),
                    },
                    limit,
                    Duration::from_secs(wait),
                    ReadOptions::freshness(options.read)?,
                )
                .await
                .map_err(js_error)?,
        )
    }

    #[napi]
    pub async fn watch_account(&self, interval_seconds: Option<f64>) -> Result<Value> {
        let interval = integer(
            interval_seconds.unwrap_or(60.0),
            "intervalSeconds",
            10,
            86400,
        )?;
        json_value(self.inner.watch_account(interval).await.map_err(js_error)?)
    }
    #[napi(constructor)]
    pub fn new(server: Option<String>) -> Result<Self> {
        let url = server
            .unwrap_or_else(|| "http://127.0.0.1:8787/".into())
            .parse()
            .map_err(|_| invalid("invalid daemon URL"))?;
        Ok(Self {
            inner: ApiClient::new(url).map_err(js_error)?,
        })
    }
    #[napi]
    pub async fn pr_report(
        &self,
        repository: String,
        number: f64,
        options: Option<ReadOptions>,
    ) -> Result<Value> {
        json_value(
            self.inner
                .pr_report(
                    &repository,
                    pull_number(number)?,
                    ReadOptions::freshness(options)?,
                )
                .await
                .map_err(js_error)?,
        )
    }
    #[napi]
    pub async fn ci_for_pr(
        &self,
        repository: String,
        number: f64,
        options: Option<ReadOptions>,
    ) -> Result<Value> {
        json_value(
            self.inner
                .ci_for_pr(
                    &repository,
                    pull_number(number)?,
                    ReadOptions::freshness(options)?,
                )
                .await
                .map_err(js_error)?,
        )
    }
    #[napi]
    pub async fn required_checks_for_pr(
        &self,
        repository: String,
        number: f64,
        options: Option<ReadOptions>,
    ) -> Result<Value> {
        json_value(
            self.inner
                .required_checks_for_pr(
                    &repository,
                    pull_number(number)?,
                    ReadOptions::freshness(options)?,
                )
                .await
                .map_err(js_error)?,
        )
    }
    #[napi]
    pub async fn repository_report(
        &self,
        repository: String,
        options: Option<RepositoryOptions>,
    ) -> Result<Value> {
        let options = options.unwrap_or_default();
        json_value(
            self.inner
                .repository_report(
                    &repository,
                    &options.branches.unwrap_or_default(),
                    options.all_branches.unwrap_or(false),
                    ReadOptions::freshness(options.read)?,
                )
                .await
                .map_err(js_error)?,
        )
    }
    #[napi]
    pub async fn my_pull_requests(
        &self,
        repository: String,
        options: Option<ReadOptions>,
    ) -> Result<Value> {
        json_value(
            self.inner
                .my_pull_requests(&repository, ReadOptions::freshness(options)?)
                .await
                .map_err(js_error)?,
        )
    }
    #[napi]
    pub async fn list_pull_requests(
        &self,
        repository: String,
        state: Option<String>,
        options: Option<ReadOptions>,
    ) -> Result<Value> {
        json_value(
            self.inner
                .list_pull_requests(
                    &repository,
                    &state.unwrap_or_else(|| "open".into()),
                    ReadOptions::freshness(options)?,
                )
                .await
                .map_err(js_error)?,
        )
    }
    #[napi]
    pub async fn watch(&self, repository: String, options: Option<WatchOptions>) -> Result<Value> {
        let options = options.unwrap_or_default();
        let number = options
            .pull_number
            .map(|n| integer(n, "pullNumber", 0, 9_007_199_254_740_991))
            .transpose()?;
        let interval = integer(
            options.interval_seconds.unwrap_or(60.0),
            "intervalSeconds",
            10,
            86400,
        )?;
        json_value(
            self.inner
                .watch(&repository, number, interval)
                .await
                .map_err(js_error)?,
        )
    }
    #[napi]
    pub async fn watch_repository(
        &self,
        repository: String,
        options: Option<RepositoryWatchOptions>,
    ) -> Result<Value> {
        let options = options.unwrap_or_default();
        let interval = integer(
            options.interval_seconds.unwrap_or(60.0),
            "intervalSeconds",
            10,
            86400,
        )?;
        json_value(
            self.inner
                .watch_repository(
                    &repository,
                    options.branches.unwrap_or_default(),
                    options.all_branches.unwrap_or(false),
                    interval,
                )
                .await
                .map_err(js_error)?,
        )
    }
    #[napi]
    pub async fn watches(&self) -> Result<Value> {
        json_value(self.inner.watches().await.map_err(js_error)?)
    }
    #[napi]
    pub async fn unwatch(&self, id: String) -> Result<()> {
        self.inner.unwatch(&id).await.map_err(js_error)
    }
    #[napi]
    pub async fn status(&self) -> Result<Value> {
        json_value(self.inner.status().await.map_err(js_error)?)
    }
    #[napi]
    pub async fn bootstrap(&self) -> Result<Value> {
        json_value(self.inner.bootstrap().await.map_err(js_error)?)
    }
    #[napi]
    pub async fn changes(&self, options: Option<ChangesOptions>) -> Result<Value> {
        let options = options.unwrap_or_default();
        let limit = integer(options.limit.unwrap_or(100.0), "limit", 1, 1000)?;
        let wait = integer(options.wait_seconds.unwrap_or(0.0), "waitSeconds", 0, 30)?;
        json_value(
            self.inner
                .changes(
                    options.cursor.as_deref(),
                    limit as usize,
                    Duration::from_secs(wait),
                )
                .await
                .map_err(js_error)?,
        )
    }
}
