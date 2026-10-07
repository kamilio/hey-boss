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

pub(super) fn fresh(validated_at: u64) -> bool {
    timestamp(validated_at)
        .is_ok_and(|at| at >= crate::issues::worker::now().saturating_sub(120_000))
}

pub(super) fn timestamp(value: u64) -> hey_gh::Result<i64> {
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
    started_at_ms: u64,
}

fn confirmation_fresh(validated_at_ms: u64, started_at_ms: u64, force: bool) -> bool {
    validated_at_ms > 0
        && (crate::issues::worker::now() as u64)
            .checked_sub(validated_at_ms)
            .is_some_and(|age| age < 30_000)
        && (!force || validated_at_ms >= started_at_ms)
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
    let started_at_ms = crate::issues::worker::now() as u64;
    let freshness = fetch_freshness(force);
    let deadline = batch_deadline.min(tokio::time::Instant::now() + Duration::from_secs(60));
    let read_client = client.clone().with_read_deadline(deadline);
    let policy = tokio::time::timeout_at(
        deadline,
        read_client.required_checks_for_pr(repository, number, freshness),
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
        started_at_ms,
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
    let read_client = client.clone().with_read_deadline(deadline);
    let response = tokio::time::timeout_at(
        deadline,
        read_client.pull_request(repository, number, Freshness::Revalidate),
    )
    .await
    .map_err(|_| hey_gh::Error::Deadline)??;
    record_terminal(ctx, url, repository, number, &response)
}

pub(super) fn record_terminal(
    ctx: &Context,
    url: &str,
    repository: &str,
    number: u64,
    response: &hey_gh::Response,
) -> hey_gh::Result<()> {
    let status = super::pr_status(&response.data, repository, number);
    if !fresh(response.validated_at_ms) || !matches!(status, Some("merged" | "closed")) {
        return Err(hey_gh::Error::Invalid(
            "Pull request changed while confirming its closure; refreshing again".into(),
        ));
    }
    record_terminal_status(ctx, url, response, status)
}

// GitHub merges are irreversible. Identity-fenced cached merge evidence remains
// useful after its validation ages; reversible closures still need a fresh read.
pub(super) fn record_cached_merge(
    ctx: &Context,
    url: &str,
    repository: &str,
    number: u64,
    response: &hey_gh::Response,
) -> hey_gh::Result<()> {
    timestamp(response.validated_at_ms)?;
    if !super::merged(&response.data, repository, number) {
        return Err(hey_gh::Error::Invalid(
            "Cached metadata does not confirm this PR merged".into(),
        ));
    }
    record_terminal_status(ctx, url, response, Some("merged"))
}

fn record_terminal_status(
    ctx: &Context,
    url: &str,
    response: &hey_gh::Response,
    status: Option<&str>,
) -> hey_gh::Result<()> {
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
        started_at_ms,
    } = required;
    let _ = batch_deadline;
    let freshness = fetch_freshness(force);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let read_client = client.clone().with_read_deadline(deadline);
    let result = tokio::time::timeout_at(deadline, async {
        let (ci, metadata) = tokio::join!(
            read_client.ci_for_pr(repository, number, freshness),
            async {
                let confirmed = hey_gh::watcher::confirmed_metadata(repository, number, &policy)
                    .filter(|proof| {
                        confirmation_fresh(proof.validated_at_ms, started_at_ms, force)
                            && policy.oldest_validation_at_ms.is_some_and(fresh)
                            && policy
                                .observed_at_ms
                                .is_some_and(|at| proof.validated_at_ms <= at)
                    });
                let (metadata, validated_at_ms, reused) = if let Some(proof) = confirmed {
                    (proof.selectors.clone(), proof.validated_at_ms, true)
                } else {
                    let response = read_client
                        .pull_request(repository, number, freshness)
                        .await?;
                    (response.data, response.validated_at_ms, false)
                };
                if fresh(validated_at_ms)
                    && policy.oldest_validation_at_ms.is_some_and(fresh)
                    && metadata["mergeable"] == false
                {
                    let observation =
                        hey_gh::watcher::observe_metadata(repository, number, &metadata, &policy);
                    if observation.evidence["sources_match"] == true {
                        Store::open(&ctx.path)
                            .map_err(storage)?
                            .record_github_observation(
                                url,
                                &observation,
                                crate::issues::worker::now(),
                            )
                            .map_err(storage)?;
                    }
                }
                Ok::<_, hey_gh::Error>((metadata, validated_at_ms, reused))
            }
        );
        let ci = ci?;
        let mut metadata = metadata?;
        // A slow CI read can outlive the reused proof. Spend only the remaining
        // detail deadline on normal metadata, rather than repeating policy/CI.
        if metadata.2 && !confirmation_fresh(metadata.1, started_at_ms, force) {
            let response = read_client
                .pull_request(repository, number, freshness)
                .await?;
            metadata = (response.data, response.validated_at_ms, false);
        }
        Ok::<_, hey_gh::Error>((ci, metadata))
    })
    .await
    .unwrap_or(Err(hey_gh::Error::Deadline));
    let (ci, (metadata, validated_at_ms, reused)) = match result {
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
    if !fresh(ci.oldest_validation_at_ms)
        || !fresh(validated_at_ms)
        || (reused
            && (!confirmation_fresh(validated_at_ms, started_at_ms, force)
                || !policy.oldest_validation_at_ms.is_some_and(fresh)))
    {
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
    let observation = hey_gh::watcher::observe_ci(repository, number, &metadata, &ci.data, &policy);
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
        if super::pr_status(&metadata, repository, number) == Some("open") {
            store
                .record_open_pr_if_changed(url, timestamp(validated_at_ms)?)
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let read_client = client.clone().with_read_deadline(deadline);
    let result = tokio::time::timeout_at(
        deadline,
        read_client.pr_review_report(repository, number, freshness),
    )
    .await
    .unwrap_or(Err(hey_gh::Error::Deadline));
    let mut store = Store::open(&ctx.path).map_err(storage)?;
    match result {
        Ok(report) if fresh(report.oldest_validation_at_ms) && timestamp(report.observed_at_ms).is_ok() => {
            let observation = hey_gh::watcher::observe_review_report(&report, &policy);
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
