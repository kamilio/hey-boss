use super::*;
use futures_util::{StreamExt, stream};
use hey_proxy::usage::{
    RecommendationStatus, Runtime, SkipReason, WORKER_SCHEMA_VERSION, WorkerEvaluation,
    WorkerEvidence, WorkerRecommendation, included_quota,
};
use std::collections::BTreeMap;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::proxy) struct Query {
    runtimes: Option<String>,
}

pub(crate) async fn recommend_workers(
    State(service): State<Arc<Service>>,
    axum::extract::Query(query): axum::extract::Query<Query>,
) -> Response {
    let runtimes: Vec<Runtime> = match query.runtimes {
        None => vec![Runtime::Codex, Runtime::Claude, Runtime::Pi],
        Some(s) if s.is_empty() => vec![],
        Some(s) if s.len() <= 64 => {
            let parsed: Result<Vec<Runtime>, _> = s
                .split(',')
                .map(|r| serde_json::from_value(json!(r)))
                .collect();
            match parsed {
                Ok(r) => r,
                Err(_) => {
                    return failure(
                        StatusCode::BAD_REQUEST,
                        "invalid_capabilities",
                        "Unknown worker runtime",
                    );
                }
            }
        }
        _ => {
            return failure(
                StatusCode::BAD_REQUEST,
                "invalid_capabilities",
                "Too many worker runtimes",
            );
        }
    };
    let proxy = service.snapshot();
    if proxy.config.mode == Mode::Client {
        let path = format!(
            "/usage/v2/recommend?runtimes={}",
            runtimes
                .iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
        return relay::<WorkerRecommendation>(&proxy, &path).await;
    }
    reply(evaluate_workers(&proxy, &runtimes).await)
}

pub(crate) async fn evaluate_workers(proxy: &Proxy, runtimes: &[Runtime]) -> WorkerRecommendation {
    // Deduplicate named accounts and bound concurrent quota reads. No inference requests.
    let accounts: BTreeMap<_, _> = proxy
        .config
        .worker_candidates
        .iter()
        .filter(|c| runtimes.contains(&c.runtime))
        .filter_map(|c| {
            proxy
                .config
                .worker_target(c)
                .map(|leg| (leg.provider, leg.implementation.to_owned()))
        })
        .collect();
    let mut readings = BTreeMap::new();
    let mut pending = stream::iter(accounts)
        .map(|(id, implementation)| async move {
            let usage = account_usage(proxy, &implementation, &id).await.ok();
            (id, usage)
        })
        .buffer_unordered(8);
    let _ = tokio::time::timeout(Duration::from_secs(25), async {
        while let Some((id, usage)) = pending.next().await {
            if let Some(usage) = usage {
                readings.insert(id, usage);
            }
        }
    })
    .await;
    from_readings(
        &proxy.config,
        runtimes,
        &readings,
        crate::claude_auth::now(),
    )
}

pub(crate) fn from_readings(
    config: &Arc<Config>,
    runtimes: &[Runtime],
    readings: &BTreeMap<String, AccountUsage>,
    now: u64,
) -> WorkerRecommendation {
    let mut candidates: Vec<_> = config
        .worker_candidates
        .iter()
        .map(|candidate| {
            let usage = readings.get(&candidate.provider);
            let result = (|| {
                if !runtimes.contains(&candidate.runtime) {
                    return Err(SkipReason::RuntimeUnavailable);
                }
                let leg = config
                    .worker_target(candidate)
                    .ok_or(SkipReason::NoSubscriptionRoute)?;
                let usage = usage.ok_or(SkipReason::ReadingUnavailable)?;
                if usage.account.id != candidate.provider
                    || usage.account.provider != leg.implementation
                {
                    return Err(SkipReason::ReadingUnavailable);
                }
                let quota = included_quota(usage, &leg.upstream_model, now)?;
                Ok(WorkerEvidence {
                    candidate: candidate.clone(),
                    account: usage.account.clone(),
                    upstream_model: leg.upstream_model,
                    quota,
                })
            })();
            let retry_at = now.saturating_add(
                usage
                    .and_then(|u| u.reading.retry_after_seconds)
                    .unwrap_or(60)
                    .clamp(1, 300),
            );
            WorkerEvaluation {
                candidate: candidate.clone(),
                reading_updated_at: usage.and_then(|u| u.reading.updated_at),
                retry_at,
                skip_reason: result.as_ref().err().copied(),
                evidence: result.ok(),
            }
        })
        .collect();
    let selected = candidates.iter().find_map(|c| c.evidence.clone());
    for candidate in &mut candidates {
        if candidate.evidence.is_some() && candidate.evidence.as_ref() != selected.as_ref() {
            candidate.evidence = None;
            candidate.skip_reason = Some(SkipReason::LowerPreference);
        }
    }
    let expires_at = selected.as_ref().map_or(now, |s| s.quota.expires_at);
    let recheck_at = candidates
        .iter()
        .map(|c| {
            c.retry_at
                .min(c.evidence.as_ref().map_or(u64::MAX, |e| e.quota.expires_at))
        })
        .min()
        .unwrap_or(now.saturating_add(60));
    WorkerRecommendation {
        schema_version: WORKER_SCHEMA_VERSION,
        config_revision: config.revision.clone(),
        generated_at: now,
        expires_at,
        recheck_at,
        status: if selected.is_some() {
            RecommendationStatus::Recommended
        } else {
            RecommendationStatus::NoRecommendation
        },
        selected,
        candidates,
    }
}

#[cfg(test)]
mod tests;
