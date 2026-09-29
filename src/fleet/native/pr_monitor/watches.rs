//! The CI pass publishes before review collection starts.
use super::{ApiClient, Context, Duration, Freshness, Result, Store, schedule, selector};

pub(super) fn poll(
    ctx: &Context,
    runtime: &tokio::runtime::Runtime,
    client: &ApiClient,
) -> Result<()> {
    let tracked: Vec<_> = Store::open(&ctx.path)?
        .github_watch_urls()?
        .into_iter()
        .map(|url| crate::issues::TrackedPullRequest {
            url,
            checked_at: None,
            closed: false,
        })
        .collect();
    let path = ctx.state.join("github-watch-schedule.json");
    let mut schedule: schedule::Schedule = serde_json::from_value(
        ctx.read_json(&path, serde_json::json!({"cooldown_until":0,"entries":{}}))?,
    )?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    for url in schedule.due(&tracked, crate::issues::worker::now()) {
        if ctx.stopped() || tokio::time::Instant::now() >= deadline {
            break;
        }
        let Some((repository, number)) = selector(&url) else {
            continue;
        };
        let result = runtime.block_on(poll_one(ctx, client, &url, &repository, number, deadline));
        match result {
            Ok(()) => schedule.watch_success(&url, crate::issues::worker::now()),
            Err(error) => {
                schedule.failure(&url, crate::issues::worker::now(), &error);
                Store::open(&ctx.path)?.record_github_error(
                    &url,
                    "required_checks",
                    &error.to_string(),
                )?;
            }
        }
        ctx.atomic_json(&path, &serde_json::to_value(&schedule)?)?;
        if schedule.cooldown_until > crate::issues::worker::now() {
            break;
        }
    }
    Ok(())
}

fn fresh(validated_at: u64) -> bool {
    validated_at >= crate::issues::worker::now().saturating_sub(120_000) as u64
}

fn storage(error: crate::issues::Error) -> hey_gh::Error {
    hey_gh::Error::Invalid(format!("Cannot retain GitHub observation: {error}"))
}

async fn poll_one(
    ctx: &Context,
    client: &ApiClient,
    url: &str,
    repository: &str,
    number: u64,
    batch_deadline: tokio::time::Instant,
) -> hey_gh::Result<()> {
    let freshness = Freshness::MaxAge(Duration::from_secs(30));
    let deadline = batch_deadline.min(tokio::time::Instant::now() + Duration::from_secs(20));
    let policy = tokio::time::timeout_at(
        deadline,
        client.required_checks_for_pr(repository, number, freshness),
    )
    .await
    .map_err(|_| hey_gh::Error::Deadline)??;
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
        Store::open(&ctx.path)
            .map_err(storage)?
            .record_github_observation(
                url,
                &hey_gh::watcher::observe_required(&policy),
                observed as i64,
            )
            .map_err(storage)?;
        true
    } else {
        // Older hey-gh daemons do not expose independently validated failure
        // identities; keep the CI-based fallback until their next upgrade.
        false
    };
    let result = tokio::time::timeout_at(
        batch_deadline.min(tokio::time::Instant::now() + Duration::from_secs(20)),
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
    Store::open(&ctx.path)
        .map_err(storage)?
        .record_github_observation(url, &observation, ci.observed_at_ms as i64)
        .map_err(storage)?;
    if observation.evidence["ci_complete"] != true || ctx.stopped() {
        return Ok(());
    }

    // Review failures cannot undo an already published required failure or slow
    // the next CI attempt. The daemon still owns source-specific quota backoff.
    let result = tokio::time::timeout_at(
        batch_deadline.min(tokio::time::Instant::now() + Duration::from_secs(20)),
        client.pr_report(repository, number, freshness),
    )
    .await
    .unwrap_or(Err(hey_gh::Error::Deadline));
    let mut store = Store::open(&ctx.path).map_err(storage)?;
    match result {
        Ok(report) if fresh(report.oldest_validation_at_ms) => store
            .record_github_observation(
                url,
                &hey_gh::watcher::observe(&report, &policy),
                report.observed_at_ms as i64,
            )
            .map_err(storage)?,
        Ok(_) => store
            .record_github_error(
                url,
                "reviews",
                "GitHub review evidence is stale; waiting for fresh validation",
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
