//! Keep account hydration moving while another PR waits in the shared queue.
use super::{Refresh, schedule::Work};
use crate::{Client, Error, Freshness, Result};
use serde_json::json;
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::Poll,
    time::Duration,
};
use tokio::time::Instant;

#[derive(Clone, Copy)]
pub(super) struct Cycle<'a> {
    pub client: &'a Client,
    pub freshness: Freshness,
    pub refresh: Refresh,
    pub background: bool,
    pub authoritative_roster: bool,
    pub deadline: Instant,
}

pub(super) struct Completed {
    pub item: Work,
    pub pr_started_at_ms: u64,
    pub result: Result<()>,
    pub cycle_interrupted: bool,
    pub retained_progress: bool,
}

pub(super) type Read<'a> = Pin<Box<dyn Future<Output = Result<Completed>> + Send + 'a>>;

pub(super) async fn next(active: &mut Vec<Read<'_>>) -> Result<Completed> {
    let (index, result) = poll_fn(|cx| {
        for (index, read) in active.iter_mut().enumerate() {
            if let Poll::Ready(result) = read.as_mut().poll(cx) {
                return Poll::Ready((index, result));
            }
        }
        Poll::Pending
    })
    .await;
    drop(active.swap_remove(index));
    result
}

impl Cycle<'_> {
    pub async fn refresh(&self, item: Work, disappeared: bool) -> Result<Completed> {
        let Self {
            client,
            freshness,
            refresh: mode,
            background,
            authoritative_roster,
            deadline,
        } = *self;
        // Discovery can report an edit while the old discussion cache is still
        // young. Validate this pending activity before clearing its queue signal;
        // normal polling resumes ordinary cache reuse after a successful read.
        let freshness = if item.activity_pending && !matches!(freshness, Freshness::CachedOnly) {
            tracing::info!(repository=%item.key.0, number=item.key.1, mode=mode.label(),
                "PR activity validation started");
            Freshness::Revalidate
        } else {
            freshness
        };
        let ci_only = mode == Refresh::Ci;
        let seed_only = mode == Refresh::Discovery;
        let details_only = mode == Refresh::Details;
        let repo = &item.key.0;
        let number = item.key.1;
        let node = &item.node;
        let pr_started_at_ms = crate::now_ms();
        let previously_terminal = if seed_only {
            client
                .stored_pr_snapshot(
                    &format!("metadata://{}/{repo}/{number}", client.hostname()),
                    repo,
                    node["id"].as_str(),
                )
                .await?
                .is_some_and(|v| {
                    v["pull_request"]["state"] == "closed" || v["pull_request"]["merged"] == true
                })
        } else {
            false
        };
        let mut cycle_interrupted = false;
        let refresh = async {
            if seed_only && authoritative_roster && (disappeared || previously_terminal) {
                tokio::time::timeout_at(
                    deadline.min(tokio::time::Instant::now() + Duration::from_secs(5)),
                    async {
                        let response = client
                            .pull_request(
                                repo,
                                number,
                                if matches!(freshness, Freshness::CachedOnly) {
                                    freshness
                                } else {
                                    Freshness::Revalidate
                                },
                            )
                            .await?;
                        let conflicts = match response.data["mergeable"].as_bool() {
                            Some(true) => "clean",
                            Some(false) => "conflicting",
                            None => "unknown",
                        };
                        client
                            .observe(
                                &format!("metadata://{}/{repo}/{number}", client.hostname()),
                                &json!({"pull_request":response.data,"conflicts":conflicts}),
                            )
                            .await?;
                        Ok(())
                    },
                )
                .await
                .unwrap_or(Err(Error::Deadline))
            } else if seed_only {
                Ok(())
            } else if mode == Refresh::Policy {
                tokio::time::timeout_at(deadline, async {
                    let report = client
                        .required_checks_for_pr(repo, number, freshness)
                        .await?;
                    if !report.errors.is_empty() {
                        return Err(Error::Invalid(format!(
                            "incomplete policy: {}",
                            json!(report.errors)
                        )));
                    }
                    Ok(())
                })
                .await
                .unwrap_or_else(|_| {
                    cycle_interrupted = true;
                    Err(Error::Deadline)
                })
            } else if ci_only {
                let result = tokio::time::timeout_at(deadline, async {
                    let report = client.ci_for_pr(repo, number, freshness).await?;
                    if !report.complete {
                        return Err(Error::Invalid(format!(
                            "incomplete CI: {}",
                            json!(report.data.errors)
                        )));
                    }
                    Ok(())
                })
                .await
                .unwrap_or_else(|_| {
                    cycle_interrupted = true;
                    Err(Error::Deadline)
                });
                if !background && result.is_ok() && tokio::time::Instant::now() < deadline {
                    // Policy is best effort in the CI loop. A slow policy read
                    // must not turn already observed CI into a timeout failure.
                    let policy_deadline =
                        deadline.min(tokio::time::Instant::now() + Duration::from_secs(1));
                    if let Err(error) = tokio::time::timeout_at(
                        policy_deadline,
                        client.required_checks_for_pr(repo, number, freshness),
                    )
                    .await
                    .unwrap_or(Err(Error::Deadline))
                    {
                        tracing::warn!(repository=%repo,number,error_code=error.diagnostic_code(),"account required-check refresh failed");
                    }
                }
                result
            } else {
                tokio::time::timeout_at(deadline, async {
                    if details_only {
                        let errors = client.refresh_pr_details(repo, number, freshness).await?;
                        if !errors.is_empty() {
                            return Err(Error::Invalid(format!(
                                "incomplete details: {}",
                                json!(errors)
                            )));
                        }
                        return Ok(());
                    }
                    let report = client.pr_report(repo, number, freshness).await?;
                    if !report.complete {
                        return Err(Error::Invalid(format!(
                            "incomplete PR: {}",
                            json!({"sources":report.data.errors,"ci":report.data.ci.errors})
                        )));
                    }
                    Ok(())
                })
                .await
                .unwrap_or_else(|_| {
                    cycle_interrupted = true;
                    Err(Error::Deadline)
                })
            }
        };
        let refresh = Box::pin(
            crate::client::REQUEST_DEADLINE.scope(background.then_some(deadline), refresh),
        );
        let mut retained_progress = false;
        let result = if background {
            let budget = crate::collection_budget::Budget::new();
            let result = crate::collection_budget::CURRENT
                .scope(budget.clone(), async {
                    tokio::select! {
                        result = refresh => result,
                        _ = budget.exhausted() => Err(Error::Deadline),
                    }
                })
                .await;
            // A finished detail collection can reuse still-fresh sources on
            // the next turn and complete its final metadata check. Partial
            // collections and earlier stalls keep the ordinary rotation.
            // Explicit refreshes cannot reuse these mutable source caches.
            retained_progress = (ci_only && budget.has_retained_pages())
                || (details_only
                    && matches!(freshness, Freshness::MaxAge(age) if !age.is_zero())
                    && matches!(result, Err(Error::Deadline))
                    && tokio::time::Instant::now() >= deadline
                    && budget.has_collected_details());
            result
        } else {
            refresh.await
        };
        // The scheduler may deliver its local deadline just before the
        // outer timer fires, including a coalesced request from the other
        // hydration lane. Both outcomes retain local interruption health.
        if background && matches!(result, Err(Error::Deadline)) {
            cycle_interrupted = true;
        }
        Ok(Completed {
            item,
            pr_started_at_ms,
            result,
            cycle_interrupted,
            retained_progress,
        })
    }
}
