//! Caller-owned total read budgets. Dropping an HTTP caller leaves the daemon's
//! handler and shared scheduler running; no watch or cursor is changed here.
use super::{Command, PrAction, policy, resolve_pr, run_pr};
use hey_gh::{ApiClient, Freshness};
use serde_json::{Value, json};
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
                let mut cache_options = options.clone();
                cache_options.refresh = false;
                cache_options.cached_only = true;
                let cached_result = run_pr(api, cache_options, selected_repo.clone(), None).await?;
                if options.cached_only {
                    return Ok(cached_result);
                }
                if cached_result.0["available"] != false {
                    cached = Some(cached_result.0);
                }
                run_pr(api, options, selected_repo, None).await
            }
            Command::Ci {
                repository,
                number,
                refresh,
                cached_only,
            } => {
                identity = json!({"repository":repository,"number":number});
                match api
                    .ci_for_pr(&repository, number, Freshness::CachedOnly)
                    .await
                {
                    Ok(report) => {
                        let complete = report.complete;
                        let value = serde_json::to_value(report)?;
                        if cached_only {
                            return Ok((value, complete));
                        }
                        cached = Some(value);
                    }
                    Err(hey_gh::Error::CacheMiss) => {
                        if cached_only {
                            return Ok((super::unavailable_cached_pr(&repository, number), false));
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
                let report = api
                    .ci_for_pr(&repository, number, policy(refresh, false))
                    .await?;
                let complete = report.complete;
                Ok((serde_json::to_value(report)?, complete))
            }
            Command::RequiredChecks {
                repository,
                number,
                refresh,
                cached_only,
            } => {
                identity = json!({"repository":repository,"number":number});
                // Policy does not expose source validation times and its cached
                // reads can wait on refresh locks. Do not invent policy freshness
                // or reuse an unvalidated satisfaction result on expiry.
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
