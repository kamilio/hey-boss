//! The CI pass publishes before review collection starts.
use super::{ApiClient, Context, Duration, Freshness, Result, Store, schedule, selector};
mod queue;

#[cfg(test)]
pub(super) fn poll(
    ctx: &Context,
    runtime: &tokio::runtime::Runtime,
    client: &ApiClient,
) -> Result<()> {
    runtime.block_on(poll_once(ctx, client))
}

pub(super) async fn poll_once(ctx: &Context, client: &ApiClient) -> Result<()> {
    let mut store = Store::open(&ctx.path)?;
    let mut actor = ctx.actor()?;
    actor.id = "human:pr-monitor".into();
    store.close_merged_pull_requests(&actor)?;
    store.reconcile_github_assignments(&actor)?;
    let tracked: Vec<_> = store
        .github_watch_urls()?
        .into_iter()
        .map(|url| crate::issues::TrackedPullRequest {
            url,
            checked_at: None,
            closed: false,
            backfill: false,
        })
        .collect();
    let requested = store.requested_github_fetches()?;
    drop(store);
    let path = ctx.state.join("github-watch-schedule.json");
    let mut schedule: schedule::Schedule = serde_json::from_value(
        ctx.read_json(&path, serde_json::json!({"cooldown_until":0,"entries":{}}))?,
    )?;
    let due = schedule.watch_due(&tracked, &requested, crate::issues::worker::now());
    if schedule.cooldown_until > crate::issues::worker::now() {
        Store::open(&ctx.path)?.github_fetch_cooldown(schedule.cooldown_until)?;
    }
    queue::poll(ctx, client, due, &mut schedule, &path).await
}

fn fetch_freshness(force: bool) -> Freshness {
    if force {
        Freshness::Revalidate
    } else {
        Freshness::MaxAge(Duration::from_secs(30))
    }
}

fn fresh(validated_at: u64) -> bool {
    timestamp(validated_at)
        .is_ok_and(|at| at >= crate::issues::worker::now().saturating_sub(120_000))
}

fn timestamp(value: u64) -> hey_gh::Result<i64> {
    i64::try_from(value)
        .ok()
        .filter(|at| *at > 0 && *at <= crate::issues::worker::now().saturating_add(30_000))
        .ok_or_else(|| {
            hey_gh::Error::Invalid(
                "GitHub observation timestamp is invalid or in the future".into(),
            )
        })
}

fn storage(error: crate::issues::Error) -> hey_gh::Error {
    hey_gh::Error::Invalid(format!("Cannot retain GitHub observation: {error}"))
}

struct RequiredEvidence {
    policy: hey_gh::RequiredChecksReport,
    published: bool,
    force: bool,
}

async fn poll_required(
    ctx: &Context,
    client: &ApiClient,
    url: &str,
    repository: &str,
    number: u64,
    batch_deadline: tokio::time::Instant,
    force: bool,
) -> hey_gh::Result<Option<RequiredEvidence>> {
    let freshness = fetch_freshness(force);
    let deadline = batch_deadline.min(tokio::time::Instant::now() + Duration::from_secs(60));
    let policy = tokio::time::timeout_at(
        deadline,
        client.required_checks_for_pr(repository, number, freshness),
    )
    .await
    .unwrap_or(Err(hey_gh::Error::Deadline));
    let policy = match policy {
        Ok(policy) => policy,
        Err(error) => {
            // Do not turn a policy failure into readiness. Only independently
            // confirmed terminal metadata can finish this watch.
            return match confirm_terminal(ctx, client, url, repository, number, deadline).await {
                Ok(()) => Ok(None),
                Err(_) => Err(error),
            };
        }
    };
    if !policy.repository.eq_ignore_ascii_case(repository)
        || policy.pull_number != number
        || policy.head_sha.is_empty()
    {
        return Err(hey_gh::Error::Invalid(
            "Required-check evidence does not identify the requested pull request".into(),
        ));
    }
    let published_required = if let (Some(observed), Some(validated)) =
        (policy.observed_at_ms, policy.oldest_validation_at_ms)
    {
        if !fresh(validated) {
            return Err(hey_gh::Error::Invalid(
                "GitHub required-check evidence is stale; waiting for fresh validation".into(),
            ));
        }
        let observed = timestamp(observed)?;
        let mut store = Store::open(&ctx.path).map_err(storage)?;
        if policy.pull_request_state.as_deref() == Some("open") {
            store
                .record_open_pr_if_changed(url, timestamp(validated)?)
                .map_err(storage)?;
        }
        store
            .record_github_observation(url, &hey_gh::watcher::observe_required(&policy), observed)
            .map_err(storage)?;
        true
    } else {
        // Older hey-gh daemons do not expose independently validated failure
        // identities; keep the CI-based fallback until their next upgrade.
        false
    };
    if policy.pull_request_state.as_deref() == Some("closed") {
        confirm_terminal(ctx, client, url, repository, number, batch_deadline).await?;
        return Ok(None);
    }
    Ok(Some(RequiredEvidence {
        policy,
        published: published_required,
        force,
    }))
}

// Lifecycle is independent of check policy: a merged native stack may no
// longer contain enough metadata to evaluate its former required checks.
async fn confirm_terminal(
    ctx: &Context,
    client: &ApiClient,
    url: &str,
    repository: &str,
    number: u64,
    deadline: tokio::time::Instant,
) -> hey_gh::Result<()> {
    let response = tokio::time::timeout_at(
        deadline,
        client.pull_request(repository, number, Freshness::Revalidate),
    )
    .await
    .map_err(|_| hey_gh::Error::Deadline)??;
    let status = super::pr_status(&response.data, repository, number);
    if !fresh(response.validated_at_ms) || !matches!(status, Some("merged" | "closed")) {
        return Err(hey_gh::Error::Invalid(
            "Pull request changed while confirming its closure; refreshing again".into(),
        ));
    }
    let mut store = Store::open(&ctx.path).map_err(storage)?;
    store
        .record_pr_status(url, status, timestamp(response.validated_at_ms)?, None)
        .map_err(storage)?;
    if status == Some("merged") {
        if let Some(id) = response.data["user"]["id"].as_i64() {
            store.record_pr_author(url, id).map_err(storage)?;
        }
        store
            .record_pr_merge_details(
                url,
                response.data["title"].as_str().unwrap_or(""),
                response.data["merged_at"].as_str(),
                timestamp(response.validated_at_ms)?,
            )
            .map_err(storage)?;
    }
    let mut actor = ctx
        .actor()
        .map_err(|error| hey_gh::Error::Invalid(error.to_string()))?;
    actor.id = "human:pr-monitor".into();
    store.close_merged_pull_requests(&actor).map_err(storage)?;
    store
        .reconcile_github_assignments(&actor)
        .map_err(storage)?;
    Ok(())
}

async fn poll_details(
    ctx: &Context,
    client: &ApiClient,
    url: &str,
    repository: &str,
    number: u64,
    batch_deadline: tokio::time::Instant,
    required: RequiredEvidence,
) -> hey_gh::Result<()> {
    let RequiredEvidence {
        policy,
        published: published_required,
        force,
    } = required;
    let _ = batch_deadline;
    let freshness = fetch_freshness(force);
    let result = tokio::time::timeout_at(
        tokio::time::Instant::now() + Duration::from_secs(60),
        async {
            let (ci, metadata) = tokio::join!(
                client.ci_for_pr(repository, number, freshness),
                client.pull_request(repository, number, freshness)
            );
            Ok::<_, hey_gh::Error>((ci?, metadata?))
        },
    )
    .await
    .unwrap_or(Err(hey_gh::Error::Deadline));
    let (ci, metadata) = match result {
        Ok(result) => result,
        Err(error) if published_required => {
            Store::open(&ctx.path)
                .map_err(storage)?
                .record_github_error(url, "ci", &error.to_string())
                .map_err(storage)?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if !fresh(ci.oldest_validation_at_ms) || !fresh(metadata.validated_at_ms) {
        let error = hey_gh::Error::Invalid(
            "GitHub CI evidence is stale; waiting for fresh validation".into(),
        );
        if published_required {
            Store::open(&ctx.path)
                .map_err(storage)?
                .record_github_error(url, "ci", &error.to_string())
                .map_err(storage)?;
            return Ok(());
        }
        return Err(error);
    }
    let observation =
        hey_gh::watcher::observe_ci(repository, number, &metadata.data, &ci.data, &policy);
    if observation.evidence["sources_match"] == false {
        Store::open(&ctx.path)
            .map_err(storage)?
            .record_github_error(
                url,
                "ci",
                "Pull request changed during CI collection; refreshing again. Last validated status retained.",
            )
            .map_err(storage)?;
        return Ok(());
    }
    {
        let mut store = Store::open(&ctx.path).map_err(storage)?;
        if super::pr_status(&metadata.data, repository, number) == Some("open") {
            store
                .record_open_pr_if_changed(url, timestamp(metadata.validated_at_ms)?)
                .map_err(storage)?;
        }
        store
            .record_github_observation(url, &observation, timestamp(ci.observed_at_ms)?)
            .map_err(storage)?;
    }
    if observation.evidence["ci_settled"] != true || ctx.stopped() {
        return Ok(());
    }

    // Review failures cannot undo an already published required failure or slow
    // the next CI attempt. The daemon still owns source-specific quota backoff.
    let result = tokio::time::timeout_at(
        tokio::time::Instant::now() + Duration::from_secs(60),
        client.pr_report(repository, number, freshness),
    )
    .await
    .unwrap_or(Err(hey_gh::Error::Deadline));
    let mut store = Store::open(&ctx.path).map_err(storage)?;
    match result {
        Ok(report) if fresh(report.oldest_validation_at_ms) && timestamp(report.observed_at_ms).is_ok() => {
            let observation = hey_gh::watcher::observe(&report, &policy);
            if observation.evidence["sources_match"] == false {
                store.record_github_error(url, "reviews", "Pull request changed during review collection; refreshing again. Last validated status retained.").map_err(storage)?;
            } else {
                store.record_github_observation(url, &observation, timestamp(report.observed_at_ms)?).map_err(storage)?;
            }
        }
        Ok(_) => store
            .record_github_error(
                url,
                "reviews",
                "GitHub review evidence has stale or invalid timestamps; waiting for fresh validation",
            )
            .map_err(storage)?,
        Err(error) => store
            .record_github_error(url, "reviews", &error.to_string())
            .map_err(storage)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests;
