use super::*;
use hey_gh::{PrLifecycleBatch, PrLifecycleState};
use std::{
    collections::{BTreeSet, VecDeque},
    path::Path,
};

#[derive(Clone)]
struct Link {
    url: String,
    number: u64,
}
struct Batch {
    repository: String,
    links: Vec<Link>,
}

pub(super) async fn poll(
    ctx: &Context,
    client: &ApiClient,
    prs: &[crate::issues::TrackedPullRequest],
    schedule: &mut schedule::Schedule,
    path: &Path,
) -> Result<()> {
    let now = crate::issues::worker::now();
    let urls = schedule.lifecycle_due(prs, now);
    let mut batches: VecDeque<Batch> = VecDeque::new();
    for url in urls {
        if let Some((repository, number)) = selector(&url)
            && number <= i32::MAX as u64
        {
            let repository = repository.to_ascii_lowercase();
            let link = Link { url, number };
            // Preserve the oldest due link's admission order across repositories.
            // A large repository must not reorder an older peer behind its sweep.
            if let Some(batch) = batches.iter_mut().find(|batch| {
                batch.repository == repository
                    && batch.links.len() < hey_gh::PR_LIFECYCLE_BATCH_LIMIT
            }) {
                batch.links.push(link);
            } else {
                batches.push_back(Batch {
                    repository,
                    links: vec![link],
                });
            }
        } else {
            let error = hey_gh::Error::Invalid("Unsupported PR URL".into());
            schedule.failure(&url, now, &error);
            Store::open(&ctx.path)?.record_pr_status(&url, None, now, Some(&error.to_string()))?;
            ctx.atomic_json(path, &serde_json::to_value(&schedule)?)?;
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    let mut active = tokio::task::JoinSet::new();
    let mut stop = false;
    while !batches.is_empty() || !active.is_empty() {
        while !stop && !ctx.stopped() && active.len() < 2 && tokio::time::Instant::now() < deadline
        {
            let Some(batch) = batches.pop_front() else {
                break;
            };
            let now = crate::issues::worker::now();
            for link in &batch.links {
                schedule.lifecycle_started(&link.url, now);
            }
            // Persist admission before waiting, so a cancelled sweep resumes
            // with unattempted links rather than replaying its first batch.
            ctx.atomic_json(path, &serde_json::to_value(&schedule)?)?;
            let read_deadline = deadline.min(tokio::time::Instant::now() + Duration::from_secs(20));
            let client = client.clone().with_read_deadline(read_deadline);
            active.spawn(async move {
                let numbers = batch
                    .links
                    .iter()
                    .map(|link| link.number)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let result = tokio::time::timeout_at(
                    read_deadline,
                    client.pr_lifecycles(
                        &batch.repository,
                        &numbers,
                        Freshness::MaxAge(Duration::from_secs(30)),
                    ),
                )
                .await
                .unwrap_or(Err(hey_gh::Error::Deadline));
                (batch, result)
            });
        }
        let Some(completed) = active.join_next().await else {
            break;
        };
        let (batch, result) = completed?;
        let result = result.and_then(|report| validate(&batch, report));
        match result {
            Ok(report) => {
                let repairs = apply(ctx, &batch, &report, schedule)?;
                ctx.atomic_json(path, &serde_json::to_value(&schedule)?)?;
                // Identity replacement is exceptional. Let authoritative REST
                // establish it through its existing generation/retirement fence.
                for link in repairs {
                    if ctx.stopped() || tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    let repair_client = client.clone().with_read_deadline(deadline);
                    match super::repair_identity(&repair_client, &batch.repository, link.number)
                        .await
                    {
                        Ok(response) if watches::fresh(response.validated_at_ms) => {
                            let status =
                                super::pr_status(&response.data, &batch.repository, link.number);
                            match status {
                                Some("merged" | "closed") => {
                                    watches::record_terminal(
                                        ctx,
                                        &link.url,
                                        &batch.repository,
                                        link.number,
                                        &response,
                                    )?;
                                    schedule.lifecycle_success(
                                        &link.url,
                                        crate::issues::worker::now(),
                                        response.validated_at_ms as i64,
                                        status == Some("closed"),
                                    );
                                }
                                Some("open") => {
                                    Store::open(&ctx.path)?.record_open_pr_if_changed(
                                        &link.url,
                                        response.validated_at_ms as i64,
                                    )?;
                                    schedule.lifecycle_success(
                                        &link.url,
                                        crate::issues::worker::now(),
                                        response.validated_at_ms as i64,
                                        false,
                                    );
                                }
                                _ => schedule.failure(
                                    &link.url,
                                    crate::issues::worker::now(),
                                    &hey_gh::Error::Invalid(
                                        "Incomplete lifecycle identity repair".into(),
                                    ),
                                ),
                            }
                        }
                        Ok(_) => schedule.failure(
                            &link.url,
                            crate::issues::worker::now(),
                            &hey_gh::Error::Invalid("Stale lifecycle identity repair".into()),
                        ),
                        Err(error) => {
                            stop |= global(&error);
                            schedule.failure(&link.url, crate::issues::worker::now(), &error);
                        }
                    }
                    ctx.atomic_json(path, &serde_json::to_value(&schedule)?)?;
                    if stop {
                        break;
                    }
                }
            }
            Err(error) => {
                stop |= global(&error);
                // During rolling installation an older daemon has no batch
                // route. A route-level 404 must not impose repository denial backoff.
                let error = if matches!(error, hey_gh::Error::GitHub { status: 404, .. }) {
                    hey_gh::Error::Invalid(
                        "Lifecycle API unavailable; retrying after daemon upgrade".into(),
                    )
                } else {
                    error
                };
                let mut store = Store::open(&ctx.path)?;
                for link in &batch.links {
                    schedule.failure(&link.url, crate::issues::worker::now(), &error);
                    store.record_pr_status(
                        &link.url,
                        None,
                        crate::issues::worker::now(),
                        Some(&error.to_string()),
                    )?;
                }
                ctx.atomic_json(path, &serde_json::to_value(&schedule)?)?;
                eprintln!("PR monitor: lifecycle batch {}: {error}", batch.repository);
            }
        }
    }
    Ok(())
}

fn global(error: &hey_gh::Error) -> bool {
    matches!(
        error,
        hey_gh::Error::GitHub { status: 401, .. }
            | hey_gh::Error::Auth(_)
            | hey_gh::Error::RateLimited { .. }
            | hey_gh::Error::Transport(_)
            | hey_gh::Error::LocalAuth(_)
            | hey_gh::Error::QueueFull
            | hey_gh::Error::Deadline
    )
}
fn validate(batch: &Batch, report: PrLifecycleBatch) -> hey_gh::Result<PrLifecycleBatch> {
    let expected: BTreeSet<_> = batch.links.iter().map(|link| link.number).collect();
    let numbers: Vec<_> = report
        .pull_requests
        .iter()
        .map(|pr| pr.number)
        .chain(report.errors.iter().map(|error| error.number))
        .collect();
    if !report.repository.eq_ignore_ascii_case(&batch.repository)
        || !watches::fresh(report.validated_at_ms)
        || report.validated_at_ms <= crate::issues::worker::now().saturating_sub(60_000) as u64
        || report.complete != report.errors.is_empty()
        || numbers.len() != expected.len()
        || numbers.iter().copied().collect::<BTreeSet<_>>() != expected
        || report.pull_requests.iter().any(|pr| {
            pr.node_id.is_empty()
                || pr.author_id.is_some_and(|id| id <= 0)
                || match pr.state {
                    PrLifecycleState::Merged => pr.merged_at_ms().is_none(),
                    _ => pr.merged_at.is_some(),
                }
        })
    {
        return Err(hey_gh::Error::Invalid(
            "Incomplete, stale, or mismatched lifecycle batch".into(),
        ));
    }
    Ok(report)
}
fn apply(
    ctx: &Context,
    batch: &Batch,
    report: &PrLifecycleBatch,
    schedule: &mut schedule::Schedule,
) -> Result<Vec<Link>> {
    let mut store = Store::open(&ctx.path)?;
    let checked_at = watches::timestamp(report.validated_at_ms)?;
    for pr in &report.pull_requests {
        for link in batch.links.iter().filter(|link| link.number == pr.number) {
            if let Some(id) = pr.author_id {
                store.record_pr_author(&link.url, id)?;
            }
            match pr.state {
                PrLifecycleState::Open => store.record_open_pr_if_changed(&link.url, checked_at)?,
                PrLifecycleState::Closed => {
                    store.record_pr_status(&link.url, Some("closed"), checked_at, None)?
                }
                PrLifecycleState::Merged => {
                    store.record_pr_status(&link.url, Some("merged"), checked_at, None)?;
                    store.record_pr_merge_details(
                        &link.url,
                        &pr.title,
                        pr.merged_at.as_deref(),
                        checked_at,
                    )?;
                }
            }
            schedule.lifecycle_success(
                &link.url,
                crate::issues::worker::now(),
                checked_at,
                pr.state == PrLifecycleState::Closed,
            );
        }
    }
    let mut repairs = Vec::new();
    for error in &report.errors {
        for link in batch
            .links
            .iter()
            .filter(|link| link.number == error.number)
        {
            let failure = match error.code.as_str() {
                "identity_changed" => {
                    repairs.push(link.clone());
                    continue;
                }
                "access_denied" => hey_gh::Error::GitHub {
                    status: 403,
                    message: "Lifecycle metadata unavailable".into(),
                },
                "not_found" => hey_gh::Error::GitHub {
                    status: 404,
                    message: "Lifecycle metadata unavailable".into(),
                },
                _ => hey_gh::Error::Invalid("Incomplete lifecycle metadata".into()),
            };
            schedule.failure(&link.url, crate::issues::worker::now(), &failure);
            store.record_pr_status(&link.url, None, checked_at, Some(&failure.to_string()))?;
        }
    }
    let mut actor = ctx.actor()?;
    actor.id = "human:pr-monitor".into();
    store.close_merged_pull_requests(&actor)?;
    store.reconcile_github_assignments(&actor)?;
    Ok(repairs)
}
