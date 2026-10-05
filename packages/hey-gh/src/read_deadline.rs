//! Caller-owned total read budgets. Dropping an HTTP caller leaves the daemon's
//! handler and shared scheduler running; no watch or cursor is changed here.
use super::{Command, PrAction, policy, resolve_pr, run_pr};
use hey_gh::{ApiClient, Freshness};
use serde_json::{Value, json};
use std::future::Future;
use tokio::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(super) fn validate(command: Option<&Command>, cursor: Option<&str>) -> Result<()> {
    let single_pr = match command {
        Some(Command::Pr(options)) => {
            options.wait == 0
                && (matches!(
                    options.action,
                    Some(PrAction::View { .. } | PrAction::Checks { .. })
                ) || (options.legacy_repository.is_some() && options.legacy_number.is_some()))
        }
        Some(Command::Ci { .. } | Command::RequiredChecks { .. }) => true,
        _ => false,
    };
    if !single_pr || cursor.is_some() {
        return Err("--timeout applies to single PR view/checks, ci, and required-checks reads; --wait controls cursor long polling".into());
    }
    Ok(())
}

// The fallback is optional evidence, not a prerequisite for a live read.
// Keep both inside the caller's deadline and prefer a ready live response.
async fn with_fallback(
    cached: &mut Option<Value>,
    capture: impl Future<Output = Result<(Value, bool)>>,
    live: impl Future<Output = Result<(Value, bool)>>,
) -> Result<(Value, bool)> {
    tokio::pin!(capture, live);
    tokio::select! {
        biased;
        result = &mut live => result,
        result = &mut capture => {
            if let Ok((value, _)) = result
                && value["available"] != false
            {
                *cached = Some(value);
            }
            live.await
        }
    }
}

pub(super) async fn read(
    api: &ApiClient,
    command: Command,
    repo: Option<String>,
    deadline: Instant,
) -> Result<(Value, bool)> {
    let mut cached = None;
    let mut identity = json!({"repository": repo});
    // One timer includes local git/branch discovery, cache transport, upstream
    // waiting, JSON conversion, and projections. Never start a fallback request
    // after the caller budget is exhausted.
    let result = tokio::time::timeout_at(deadline, async {
        match command {
            Command::Pr(mut options) => {
                if let Some(repository) = &options.legacy_repository {
                    identity = json!({"repository":repository,"number":options.legacy_number});
                } else {
                    let selector = match &options.action {
                        Some(PrAction::View { selector } | PrAction::Checks { selector }) => {
                            selector.clone()
                        }
                        _ => unreachable!("validated single PR"),
                    };
                    identity["selector"] = json!(selector);
                    let (repository, number) = resolve_pr(api, repo, selector).await?;
                    identity = json!({"repository":repository,"number":number});
                    match &mut options.action {
                        Some(PrAction::View { selector } | PrAction::Checks { selector }) => {
                            *selector = Some(number.to_string())
                        }
                        _ => unreachable!("validated single PR"),
                    }
                }
                let selected_repo = if options.legacy_repository.is_some() {
                    None
                } else {
                    identity["repository"].as_str().map(str::to_owned)
                };
                if options.cached_only {
                    return run_pr(api, options, selected_repo, None).await;
                }
                let mut cache_options = options.clone();
                cache_options.refresh = false;
                cache_options.cached_only = true;
                with_fallback(
                    &mut cached,
                    run_pr(api, cache_options, selected_repo.clone(), None),
                    run_pr(api, options, selected_repo, None),
                )
                .await
            }
            Command::Ci {
                repository,
                number,
                refresh,
                cached_only,
            } => {
                identity = json!({"repository":repository,"number":number});
                let capture = async {
                    match api
                        .ci_for_pr(&repository, number, Freshness::CachedOnly)
                        .await
                    {
                        Ok(report) => {
                            let complete = report.complete;
                            Ok((serde_json::to_value(report)?, complete))
                        }
                        Err(hey_gh::Error::CacheMiss) => {
                            Ok((super::unavailable_cached_pr(&repository, number), false))
                        }
                        Err(error) => Err(error.into()),
                    }
                };
                if cached_only {
                    return capture.await;
                }
                with_fallback(&mut cached, capture, async {
                    let report = api
                        .ci_for_pr(&repository, number, policy(refresh, false))
                        .await?;
                    let complete = report.complete;
                    Ok((serde_json::to_value(report)?, complete))
                })
                .await
            }
            Command::RequiredChecks {
                repository,
                number,
                refresh,
                cached_only,
            } => {
                identity = json!({"repository":repository,"number":number});
                // Cached policy reads can wait on refresh locks. Do not reuse a
                // satisfaction result that did not return before this deadline.
                let report = api
                    .required_checks_for_pr(&repository, number, policy(refresh, cached_only))
                    .await?;
                let complete = report.errors.is_empty();
                Ok((serde_json::to_value(report)?, complete))
            }
            _ => unreachable!("validated read command"),
        }
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            let available = cached.is_some();
            let mut value = cached.unwrap_or_else(|| {
                json!({
                    "available": false, "state": "unknown", "validations": [],
                    "oldestValidationAtMs": null, "observedAtMs": null,
                    "repository": identity["repository"], "number": identity["number"],
                    "selector": identity["selector"],
                })
            });
            value["complete"] = json!(false);
            value["available"] = json!(available);
            value["code"] = json!("deadline");
            value["deadlineExceeded"] = json!(true);
            value["pendingSources"] = json!(["upstream_read"]);
            // A fallback is evidence, not an observation-feed replacement. Do not
            // hand a consumer a cursor for a timed-out read.
            value["cursor"] = Value::Null;
            if !value["sourceErrors"].is_object() {
                value["sourceErrors"] = json!({});
            }
            value["sourceErrors"]["deadline"] = json!([{
                "source":"upstream_read", "message":"caller deadline exceeded; upstream evidence remains pending"
            }]);
            Ok((value, false))
        }
    }
}
