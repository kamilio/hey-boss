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
    pub fn admitted_at(self, at: Instant) -> Self {
        // The cycle stops admitting work at its deadline. An admitted background
        // PR owns a full collection budget, even when its turn starts late.
        // Capturing admission before checkpoint I/O bounds the complete cycle
        // to two report budgets, with at most its existing width left to drain.
        let mut deadline = if self.background && self.refresh != Refresh::Discovery {
            at + self.client.report_timeout()
        } else {
            self.deadline
        };
        if let Some(caller) = crate::client::REQUEST_DEADLINE
            .try_with(|v| *v)
            .ok()
            .flatten()
        {
            deadline = deadline.min(caller);
        }
        if let Ok(caller) = crate::client::READ_DEADLINE.try_with(|v| *v) {
            deadline = deadline.min(caller);
        }
        Self { deadline, ..self }
    }

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
                let deadline = deadline.min(Instant::now() + Duration::from_secs(5));
                tokio::time::timeout_at(
                    deadline,
                    // Stop the shared transport when this lifecycle read expires,
                    // unless another live caller extends the same request.
                    crate::client::REQUEST_DEADLINE.scope(Some(deadline), async {
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
                    }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[tokio::test(start_paused = true)]
    async fn admission_preserves_each_callers_deadline_and_the_collection_bound() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            crate::Config {
                cache_path: dir.path().join("cache.sqlite"),
                report_timeout: Duration::from_secs(10),
                ..Default::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let start = Instant::now();
        let admitted = start + Duration::from_secs(9);
        tokio::time::advance(Duration::from_secs(9)).await;
        for refresh in [
            Refresh::Ci,
            Refresh::Details,
            Refresh::Policy,
            Refresh::Combined,
            Refresh::Discovery,
        ] {
            for background in [false, true] {
                let cycle = Cycle {
                    client: &client,
                    freshness: Freshness::Revalidate,
                    refresh,
                    background,
                    authoritative_roster: true,
                    deadline: start + Duration::from_secs(10),
                };
                let expected = if background && refresh != Refresh::Discovery {
                    start + Duration::from_secs(19)
                } else {
                    cycle.deadline
                };
                // Checkpoint delays count against the admitted PR's allowance.
                tokio::time::advance(Duration::from_millis(100)).await;
                assert_eq!(cycle.admitted_at(admitted).deadline, expected);
                for (read, request) in [(12, 11), (11, 12), (8, 30), (30, 8), (30, 30)] {
                    let read = start + Duration::from_secs(read);
                    let request = start + Duration::from_secs(request);
                    let deadline = crate::client::READ_DEADLINE
                        .scope(
                            read,
                            crate::client::REQUEST_DEADLINE.scope(Some(request), async {
                                cycle.admitted_at(admitted).deadline
                            }),
                        )
                        .await;
                    assert_eq!(deadline, expected.min(read).min(request));
                }
            }
        }
    }

    #[tokio::test]
    async fn expired_discovery_metadata_releases_the_transport_lane() {
        for cycle_budget in [Duration::from_millis(200), Duration::from_secs(30)] {
            discovery_deadline(cycle_budget, false).await;
        }
    }

    #[tokio::test]
    async fn discovery_expiry_preserves_a_coalesced_metadata_reader() {
        discovery_deadline(Duration::from_millis(200), true).await;
    }

    async fn discovery_deadline(cycle_budget: Duration, shared: bool) {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let router = axum::Router::new().fallback({
            let entered = entered.clone();
            let release = release.clone();
            move |uri: axum::http::Uri| {
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    if uri.path() == "/repos/acme/demo/pulls/7" {
                        entered.notify_one();
                        release.notified().await;
                    }
                    axum::Json(json!({"ok":true}))
                }
            }
        });
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            crate::Config {
                cache_path: dir.path().join("cache.sqlite"),
                rest_url: origin.parse().unwrap(),
                graphql_url: format!("{origin}graphql").parse().unwrap(),
                min_spacing: Duration::ZERO,
                ..Default::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let nodes = BTreeMap::from([(("acme/demo".into(), 7), json!({"id":"PR_7"}))]);
        let item = super::super::schedule::Schedule::baseline(&nodes, None, false)
            .order(nodes)
            .pop()
            .unwrap();
        let refresh = {
            let client = client.clone();
            tokio::spawn(async move {
                crate::client::BACKGROUND_READ
                    .scope(
                        (),
                        Cycle {
                            client: &client,
                            freshness: Freshness::Revalidate,
                            refresh: Refresh::Discovery,
                            background: false,
                            authoritative_roster: true,
                            deadline: Instant::now() + cycle_budget,
                        }
                        .refresh(item, true),
                    )
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        let reader = shared.then(|| {
            let client = client.clone();
            tokio::spawn(async move {
                client
                    .pull_request("acme/demo", 7, Freshness::Revalidate)
                    .await
            })
        });
        if shared {
            tokio::time::timeout(Duration::from_secs(1), async {
                while client.status().coalesced_requests == 0 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
        }
        let completed = tokio::time::timeout(Duration::from_secs(6), refresh)
            .await
            .expect("discovery exceeded its five-second lifecycle cap")
            .unwrap()
            .unwrap();
        assert!(matches!(completed.result, Err(Error::Deadline)));
        assert!(
            client
                .stored_snapshot("metadata://github.com/acme/demo/7")
                .await
                .unwrap()
                .is_none(),
            "expired metadata must not publish lifecycle evidence"
        );
        if let Some(reader) = reader {
            assert!(
                !reader.is_finished(),
                "discovery expiry cancelled a live shared reader"
            );
            release.notify_one();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), reader)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .data["ok"],
                true
            );
        }
        let neighbor = tokio::time::timeout(
            Duration::from_millis(500),
            client.get("neighbor", Freshness::Revalidate),
        )
        .await;
        server.abort();
        assert!(
            neighbor.is_ok(),
            "expired discovery read retained the only core transport lane"
        );
        assert_eq!(neighbor.unwrap().unwrap().data["ok"], true);
    }
}
