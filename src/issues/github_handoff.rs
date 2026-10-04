//! A handoff acknowledges supplied evidence, never whatever a later fetch finds.
use super::*;

pub(super) fn acknowledge(
    db: &Connection,
    project: &str,
    number: i64,
    evidence: &[ReviewedGithubEvidence],
) -> Result<()> {
    let links: BTreeSet<String> = registry::pull_requests(db, project, number)?
        .iter()
        .filter(|pr| pr["status"] != "closed" && pr["status"] != "merged")
        .filter_map(|pr| pr["url"].as_str())
        .filter(|url| hey_gh::watcher::pull_request_selector(url).is_some())
        .map(|url| url.trim_end_matches('/').to_owned())
        .collect();
    let (_, status) = saved(db, project, number)?;
    let mut reviewed = BTreeSet::new();
    for snapshot in evidence {
        let report = &snapshot.report;
        let policy = &snapshot.policy;
        let url = format!(
            "https://github.com/{}/pull/{}",
            report.data.repository, report.data.number
        );
        let observation = hey_gh::watcher::observe(report, policy);
        if !links.contains(&url) || !reviewed.insert(url.clone()) {
            return Err(Error::invalid(
                "Reviewed evidence must contain each attached open GitHub PR exactly once",
            ));
        }
        // Explicit review requires fully fetched, matching sources, not settled
        // CI. Keep the observation's actual signals: no synthetic completion.
        if !report.complete
            || !report.data.errors.is_empty()
            || !report.data.ci.errors.is_empty()
            || !policy.errors.is_empty()
            || policy.state == "unknown"
            || observation.evidence["sources_match"] != true
            || report.data.pull_request["state"] != "open"
            || report.data.pull_request["head"]["sha"] != observation.head
            || report.data.pull_request["base"]["sha"].as_str().is_none()
            || report.data.pull_request["base"]["sha"].as_str() != policy.pr_base_sha.as_deref()
            || policy.base_sha.as_deref().is_none_or(str::is_empty)
            || policy.pull_request_state.as_deref() != Some("open")
        {
            return Err(Error::conflict(
                "Reviewed evidence is incomplete or its PR, head, base, CI and policy do not match; handoff preserved",
            ));
        }
        let signals = signal_keys(&observation);
        let validated_after = policy
            .oldest_validation_at_ms
            .map(|at| at.min(report.oldest_validation_at_ms))
            .filter(|at| *at > 0);
        for (stored_url, current) in status["prs"].as_object().into_iter().flatten() {
            if stored_url.trim_end_matches('/') != url {
                continue;
            }
            // Fresh validation of every reviewed source supersedes an older
            // watcher snapshot. Read time alone is not freshness evidence.
            if current["checked_at"]
                .as_u64()
                .zip(validated_after)
                .is_some_and(|(seen, fresh)| seen < fresh)
            {
                continue;
            }
            // A newer observation may have arrived before this transaction.
            // Reject instead of acknowledging an event absent from the review.
            if current["head"]
                .as_str()
                .is_some_and(|head| head != observation.head)
                || current["signals"].as_array().is_some_and(|known| {
                    known
                        .iter()
                        .any(|s| s.as_str().is_none_or(|s| !signals.contains(&s)))
                })
                || (status["event"].is_string() && current.get("signals").is_none())
            {
                return Err(Error::conflict(
                    "GitHub evidence changed since the reviewed snapshot; inspect current evidence before handing off. Ownership and reservations preserved",
                ));
            }
        }
        db.execute("INSERT OR IGNORE INTO issue_github_signals SELECT ?1,?2,?3,?4,value FROM json_each(?5)", params![project,number,url,observation.head,serde_json::to_string(&signals)?])?;
    }
    if reviewed != links || reviewed.is_empty() {
        return Err(Error::invalid(
            "Reviewed evidence must cover every attached open GitHub PR",
        ));
    }
    Ok(())
}
