//! Prioritize required policy across PRs before filling spare slots with details.
use super::*;
use std::collections::VecDeque;

struct Task {
    url: String,
    repository: String,
    number: u64,
    required: Option<RequiredEvidence>,
}

pub(super) async fn poll(
    ctx: &Context,
    client: &ApiClient,
    due: Vec<String>,
    schedule: &mut schedule::Schedule,
    path: &std::path::Path,
) -> Result<()> {
    let mut required: VecDeque<_> = due
        .into_iter()
        .filter_map(|url| {
            let (repository, number) = selector(&url)?;
            Some(Task {
                url,
                repository,
                number,
                required: None,
            })
        })
        .collect();
    let mut details = VecDeque::new();
    let mut running = tokio::task::JoinSet::new();
    let mut policy_streak = 0_usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    loop {
        while running.len() < 4
            && !ctx.stopped()
            && tokio::time::Instant::now() < deadline
            && schedule.cooldown_until <= crate::issues::worker::now()
        {
            // Required checks get three of every four admissions while both
            // lanes have work. Slow policies must not starve known completions.
            let next = if !details.is_empty() && (required.is_empty() || policy_streak >= 3) {
                details.pop_front()
            } else {
                required.pop_front()
            };
            let Some(task) = next else {
                break;
            };
            policy_streak = if task.required.is_some() {
                0
            } else {
                policy_streak.saturating_add(1)
            };
            let ctx = ctx.clone();
            let client = client.clone();
            running.spawn(async move {
                let source = if task.required.is_some() {
                    "ci"
                } else {
                    "required_checks"
                };
                let result = match task.required {
                    Some(evidence) => poll_details(
                        &ctx,
                        &client,
                        &task.url,
                        &task.repository,
                        task.number,
                        deadline,
                        evidence,
                    )
                    .await
                    .map(|_| None),
                    None => {
                        poll_required(
                            &ctx,
                            &client,
                            &task.url,
                            &task.repository,
                            task.number,
                            deadline,
                        )
                        .await
                    }
                };
                (task.url, task.repository, task.number, source, result)
            });
        }
        let Some(result) = running.join_next().await else {
            break;
        };
        let (url, repository, number, source, result) = result?;
        match result {
            Ok(evidence) => {
                // Checkpoint policy progress even when this batch has no time
                // left for details. The next cycle can resume from cached policy.
                schedule.watch_success(&url, crate::issues::worker::now());
                if let Some(evidence) = evidence {
                    details.push_back(Task {
                        url,
                        repository,
                        number,
                        required: Some(evidence),
                    });
                }
            }
            Err(error) => {
                schedule.failure(&url, crate::issues::worker::now(), &error);
                Store::open(&ctx.path)?.record_github_error(&url, source, &error.to_string())?;
            }
        }
        ctx.atomic_json(path, &serde_json::to_value(&schedule)?)?;
    }
    Ok(())
}
