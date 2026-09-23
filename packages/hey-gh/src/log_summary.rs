//! Read-only aggregates of retained diagnostics. Never forward arbitrary fields.
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, Read},
    path::Path,
};

const ERROR_CODES: &[&str] = &[
    "incomplete",
    "invalid",
    "auth",
    "local_auth",
    "storage",
    "transport",
    "github_http",
    "graphql_access_denied",
    "graphql",
    "queue_full",
    "deadline",
    "rate_limited",
    "cursor_expired",
    "cache_miss",
    "stopped",
];

type SourceFailureRecords = BTreeMap<String, BTreeMap<String, BTreeMap<String, usize>>>;

#[derive(Default, Serialize)]
pub struct Summary {
    pub requested_start_at_ms: u64,
    pub sampled_at_ms: u64,
    pub atomic: bool,
    pub earliest_retained_at_ms: Option<u64>,
    pub latest_retained_at_ms: Option<u64>,
    pub retention_covers_start: bool,
    pub oversized_files: usize,
    pub archive_gaps: usize,
    pub incomplete_lines: usize,
    pub completed_requests: usize,
    pub successful_requests: usize,
    pub failed_requests: usize,
    /// Includes attempts before the window when a job finishes inside it.
    pub attempts_for_completed_requests: u64,
    pub completions_by_endpoint: BTreeMap<String, usize>,
    /// Completed-job time, including queuing and retries, not HTTP latency.
    pub request_elapsed_ms_by_endpoint: BTreeMap<String, RequestElapsedSummary>,
    pub completions_by_http_status: BTreeMap<String, usize>,
    pub completions_by_source: BTreeMap<String, usize>,
    pub failures_by_code: BTreeMap<String, usize>,
    /// Distinct retained warning records, not requests, attempts or unique PRs.
    /// Exact copies from rotations deduplicate; labels exclude repo/PR contents.
    pub source_refresh_failure_records: SourceFailureRecords,
    pub completed_retries_succeeded: usize,
    pub completed_retries_failed: usize,
    pub retry_jobs_with_warnings: usize,
    /// No retained final in the requested window, not proof the job is active.
    pub retry_jobs_without_final: usize,
    pub rate_limit_observations: usize,
    pub uncorrelated_completion_lines: usize,
    pub account_refresh_cycles: BTreeMap<String, CycleSummary>,
    pub uncorrelated_refresh_cycle_lines: usize,
}

#[derive(Default, Serialize)]
pub struct RequestElapsedSummary {
    pub samples: usize,
    pub unavailable: usize,
    /// Nearest-rank percentiles of retained completed jobs.
    pub p50: Option<u64>,
    pub p95: Option<u64>,
    pub max: Option<u64>,
}

#[derive(Default, Serialize)]
pub struct CycleSummary {
    pub completed_cycles: usize,
    pub attempted: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub local_budget_interruptions: u64,
    /// Repeated deferrals across cycles, not unique PRs.
    pub deferred_across_cycles: u64,
    pub budget_exhausted_cycles: usize,
    pub latest_cycle: Option<Cycle>,
}

#[derive(Serialize)]
pub struct Cycle {
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub total: u64,
    pub attempted: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub interrupted: u64,
    pub deferred: u64,
    pub cycle_budget_exhausted: bool,
}

fn cycle(line: &str, at_ms: u64) -> Option<Cycle> {
    let count = |name| field(line, name)?.parse::<u64>().ok();
    let cycle = Cycle {
        started_at_ms: count("started_at_ms")?,
        finished_at_ms: count("finished_at_ms")?,
        total: count("total")?,
        attempted: count("attempted")?,
        succeeded: count("succeeded")?,
        failed: count("failed")?,
        interrupted: count("interrupted")?,
        deferred: count("deferred")?,
        cycle_budget_exhausted: match field(line, "cycle_budget_exhausted")? {
            "true" => true,
            "false" => false,
            _ => return None,
        },
    };
    (cycle.finished_at_ms >= cycle.started_at_ms
        && cycle.finished_at_ms <= at_ms
        && cycle
            .succeeded
            .checked_add(cycle.failed)?
            .checked_add(cycle.interrupted)?
            == cycle.attempted
        && cycle.attempted.checked_add(cycle.deferred)? == cycle.total
        // A per-PR interruption can leave enough time to visit the whole
        // roster; only deferred work necessarily exhausts the whole cycle.
        && (cycle.deferred == 0 || cycle.cycle_budget_exhausted)
        && (!cycle.cycle_budget_exhausted || cycle.interrupted > 0 || cycle.deferred > 0))
        .then_some(cycle)
}

struct Completion {
    at_ms: u64,
    attempts: u64,
    succeeded: bool,
    endpoint: String,
    http_status: String,
    source: String,
    error_code: String,
    elapsed_ms: Option<u64>,
}

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.split_whitespace().find_map(|token| {
        let (key, value) = token.split_once('=')?;
        (key == name).then(|| value.trim_matches('"'))
    })
}

fn request_id(line: &str) -> Option<&str> {
    field(line, "request_id")
        .filter(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn allowed(value: Option<&str>, choices: &[&str]) -> String {
    value
        .filter(|value| choices.contains(value))
        .unwrap_or("unknown")
        .to_owned()
}

fn source_failure_record(line: &str) -> Option<(&'static str, String, String)> {
    let (mode, source) = if line.contains("PR detail source refresh failed") {
        (
            "details",
            allowed(
                field(line, "source"),
                &[
                    "comments",
                    "review_comments",
                    "reviews",
                    "timeline",
                    "review_events",
                    "review_threads",
                ],
            ),
        )
    } else if line.contains("CI source refresh failed") {
        let source = allowed(
            field(line, "source"),
            &["check_runs", "commit_statuses", "workflow_runs", "jobs"],
        );
        (
            "ci",
            if source == "jobs" {
                "workflow_jobs".into()
            } else {
                source
            },
        )
    } else if line
        .contains("background account discovery incomplete; retaining last good collection")
        || line.contains("account discovery failed; keeping previous roster")
    {
        ("discovery", "open_pull_requests".into())
    } else {
        return None;
    };
    Some((
        mode,
        source,
        allowed(field(line, "error_code"), ERROR_CODES),
    ))
}

fn increment(counts: &mut BTreeMap<String, usize>, key: String) {
    *counts.entry(key).or_default() += 1;
}

pub fn read(directory: &Path, seconds: u64, sampled_at_ms: u64) -> io::Result<Summary> {
    if !(1..=86400).contains(&seconds) {
        return Err(io::Error::other("--since must be 1..86400 seconds"));
    }
    let mut summary = Summary {
        requested_start_at_ms: sampled_at_ms.saturating_sub(seconds * 1000),
        sampled_at_ms,
        ..Summary::default()
    };
    let mut completions = BTreeMap::<String, Completion>::new();
    let mut retries = BTreeSet::new();
    let mut cycles = BTreeMap::<(String, u64, u64), Cycle>::new();
    let mut source_failure_lines = BTreeSet::new();
    let mut archives_found = BTreeSet::new();
    for archive in (0..=4).rev() {
        let name = if archive == 0 {
            "hey-gh.log".to_owned()
        } else {
            format!("hey-gh.log.{archive}")
        };
        let file = match File::open(directory.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        archives_found.insert(archive);
        const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
        summary.oversized_files += usize::from(file.metadata()?.len() > MAX_FILE_BYTES);
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES).read_to_end(&mut bytes)?;
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            if !part.ends_with(b"\n") {
                summary.incomplete_lines += 1;
                continue;
            }
            let line = String::from_utf8_lossy(part);
            let Some(at_ms) = line
                .split_whitespace()
                .next()
                .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                .and_then(|stamp| u64::try_from(stamp.timestamp_millis()).ok())
            else {
                continue;
            };
            if at_ms > sampled_at_ms {
                continue;
            }
            summary.earliest_retained_at_ms = Some(
                summary
                    .earliest_retained_at_ms
                    .map_or(at_ms, |old| old.min(at_ms)),
            );
            summary.latest_retained_at_ms = Some(
                summary
                    .latest_retained_at_ms
                    .map_or(at_ms, |old| old.max(at_ms)),
            );
            if at_ms < summary.requested_start_at_ms {
                continue;
            }
            if let Some((mode, source, error_code)) = source_failure_record(&line)
                && source_failure_lines.insert(line.to_string())
            {
                let records = summary
                    .source_refresh_failure_records
                    .entry(mode.to_owned())
                    .or_default()
                    .entry(source)
                    .or_default();
                increment(records, error_code);
            }
            if line.contains("account refresh cycle finished") {
                if field(&line, "seed_only") == Some("true") {
                    continue;
                }
                let mode = field(&line, "mode").filter(|mode| matches!(*mode, "ci" | "details"));
                if field(&line, "seed_only") == Some("false")
                    && let Some((mode, cycle)) = mode.zip(cycle(&line, at_ms))
                {
                    // Rotation can expose the same terminal record twice.
                    cycles.insert(
                        (mode.to_owned(), cycle.started_at_ms, cycle.finished_at_ms),
                        cycle,
                    );
                } else {
                    summary.uncorrelated_refresh_cycle_lines += 1;
                }
                continue;
            }
            if line.contains("GitHub rate limit observed") {
                summary.rate_limit_observations += 1;
            }
            if (line.contains("retry scheduled") || line.contains("transport attempt failed"))
                && let Some(id) = request_id(&line)
            {
                retries.insert(id.to_owned());
            }
            if !line.contains("GitHub request finished") {
                continue;
            }
            let Some((id, succeeded, attempts)) = request_id(&line).and_then(|id| {
                let succeeded = match field(&line, "succeeded")? {
                    "true" => true,
                    "false" => false,
                    _ => return None,
                };
                Some((
                    id,
                    succeeded,
                    field(&line, "attempts")?.parse::<u64>().ok()?,
                ))
            }) else {
                summary.uncorrelated_completion_lines += 1;
                continue;
            };
            let http_status = field(&line, "http_status")
                .and_then(|status| status.parse::<u16>().ok())
                .filter(|status| (100..=599).contains(status))
                .map_or_else(|| "unavailable".to_owned(), |status| status.to_string());
            let completion = Completion {
                at_ms,
                attempts,
                succeeded,
                http_status,
                elapsed_ms: field(&line, "elapsed_ms").and_then(|ms| ms.parse::<u64>().ok()),
                endpoint: allowed(
                    field(&line, "endpoint"),
                    &[
                        "viewer",
                        "graphql",
                        "search",
                        "repository",
                        "pull_requests",
                        "pull_request",
                        "reviews",
                        "review_comments",
                        "comments",
                        "timeline",
                        "check_runs",
                        "commit_statuses",
                        "workflow_runs",
                        "workflow_jobs",
                        "branch_protection",
                        "branch_rules",
                        "branches",
                        "branch",
                        "commit",
                        "compare",
                        "rest_other",
                    ],
                ),
                source: allowed(
                    field(&line, "source"),
                    &["network", "cache", "revalidated", "error"],
                ),
                error_code: allowed(field(&line, "error_code"), ERROR_CODES),
            };
            // Rotation during a read can expose the same final twice. Never
            // double-count it, or correlate a retry to another request's success.
            if completions.get(id).is_none_or(|old| old.at_ms <= at_ms) {
                completions.insert(id.to_owned(), completion);
            }
        }
    }
    if let Some(highest) = archives_found.last() {
        summary.archive_gaps = (0..=*highest)
            .filter(|archive| !archives_found.contains(archive))
            .count();
    }
    summary.retention_covers_start = summary.oversized_files == 0
        && summary.archive_gaps == 0
        && summary
            .earliest_retained_at_ms
            .is_some_and(|at| at <= summary.requested_start_at_ms);
    for ((mode, _, _), cycle) in cycles {
        let aggregate = summary.account_refresh_cycles.entry(mode).or_default();
        aggregate.completed_cycles += 1;
        aggregate.attempted = aggregate.attempted.saturating_add(cycle.attempted);
        aggregate.succeeded = aggregate.succeeded.saturating_add(cycle.succeeded);
        aggregate.failed = aggregate.failed.saturating_add(cycle.failed);
        aggregate.local_budget_interruptions = aggregate
            .local_budget_interruptions
            .saturating_add(cycle.interrupted);
        aggregate.deferred_across_cycles = aggregate
            .deferred_across_cycles
            .saturating_add(cycle.deferred);
        aggregate.budget_exhausted_cycles += usize::from(cycle.cycle_budget_exhausted);
        if aggregate.latest_cycle.as_ref().is_none_or(|latest| {
            (latest.finished_at_ms, latest.started_at_ms)
                < (cycle.finished_at_ms, cycle.started_at_ms)
        }) {
            aggregate.latest_cycle = Some(cycle);
        }
    }
    summary.retry_jobs_with_warnings = retries.len();
    summary.retry_jobs_without_final = retries
        .iter()
        .filter(|id| !completions.contains_key(*id))
        .count();
    summary.completed_requests = completions.len();
    let mut elapsed = BTreeMap::<String, Vec<u64>>::new();
    for completion in completions.into_values() {
        summary.attempts_for_completed_requests = summary
            .attempts_for_completed_requests
            .saturating_add(completion.attempts);
        let timing = summary
            .request_elapsed_ms_by_endpoint
            .entry(completion.endpoint.clone())
            .or_default();
        if let Some(ms) = completion.elapsed_ms {
            elapsed
                .entry(completion.endpoint.clone())
                .or_default()
                .push(ms);
            timing.samples += 1;
        } else {
            timing.unavailable += 1;
        }
        increment(&mut summary.completions_by_endpoint, completion.endpoint);
        increment(
            &mut summary.completions_by_http_status,
            completion.http_status,
        );
        increment(&mut summary.completions_by_source, completion.source);
        if completion.succeeded {
            summary.successful_requests += 1;
            summary.completed_retries_succeeded += usize::from(completion.attempts > 1);
        } else {
            summary.failed_requests += 1;
            summary.completed_retries_failed += usize::from(completion.attempts > 1);
            increment(&mut summary.failures_by_code, completion.error_code);
        }
    }
    for (endpoint, mut samples) in elapsed {
        samples.sort_unstable();
        let timing = summary
            .request_elapsed_ms_by_endpoint
            .get_mut(&endpoint)
            .expect("completion endpoint initialized");
        // Logs are bounded to 10 MiB, so these products cannot overflow.
        timing.p50 = Some(samples[(samples.len() * 50).div_ceil(100) - 1]);
        timing.p95 = Some(samples[(samples.len() * 95).div_ceil(100) - 1]);
        timing.max = samples.last().copied();
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const NOW: u64 = 1789971900000; // 2026-09-21T06:25:00Z

    #[test]
    fn timing_percentiles_use_nearest_rank_and_latest_deduplicated_final() {
        let root = tempfile::tempdir().unwrap();
        let rows: String = (1..=20)
            .map(|id| format!("2026-09-21T06:24:00Z INFO hey_gh: GitHub request finished request_id={id:032x} endpoint=check_runs attempts=1 succeeded=true elapsed_ms={id}\n"))
            .collect();
        fs::write(root.path().join("hey-gh.log.1"), "2026-09-21T06:23:59Z INFO hey_gh: GitHub request finished request_id=00000000000000000000000000000014 endpoint=check_runs attempts=1 succeeded=true elapsed_ms=999\n").unwrap();
        fs::write(root.path().join("hey-gh.log"), rows).unwrap();
        let summary = read(root.path(), 120, NOW).unwrap();
        let timing = &summary.request_elapsed_ms_by_endpoint["check_runs"];
        assert_eq!((timing.samples, timing.unavailable), (20, 0));
        assert_eq!(
            (timing.p50, timing.p95, timing.max),
            (Some(10), Some(19), Some(20))
        );
    }

    #[test]
    fn elapsed_times_include_failed_jobs_deduplicate_and_leave_unknown_times_explicit() {
        let root = tempfile::tempdir().unwrap();
        let row = |id, endpoint, elapsed, succeeded| {
            format!(
                "2026-09-21T06:24:00Z INFO hey_gh: GitHub request finished request_id={id:032x} endpoint={endpoint} attempts=1 succeeded={succeeded} {elapsed}\n"
            )
        };
        let first = row(1, "check_runs", "elapsed_ms=0", "true");
        fs::write(root.path().join("hey-gh.log.1"), &first).unwrap();
        fs::write(
            root.path().join("hey-gh.log"),
            format!(
                "{first}{}{}{}{}{}{}{}",
                row(2, "check_runs", "elapsed_ms=10", "true"),
                row(3, "check_runs", "elapsed_ms=900", "false"),
                row(4, "check_runs", "", "true"),
                row(5, "check_runs", "elapsed_ms=-1", "true"),
                row(6, "check_runs", "elapsed_ms=18446744073709551616", "true"),
                row(7, "graphql", "elapsed_ms=42", "true"),
                row(
                    8,
                    "private-token-and-comment",
                    "elapsed_ms=private-token-and-comment",
                    "false"
                ),
            ),
        )
        .unwrap();
        let summary = read(root.path(), 120, NOW).unwrap();
        let checks = &summary.request_elapsed_ms_by_endpoint["check_runs"];
        assert_eq!((checks.samples, checks.unavailable), (3, 3));
        assert_eq!(
            (checks.p50, checks.p95, checks.max),
            (Some(10), Some(900), Some(900))
        );
        assert_eq!(
            summary.completions_by_endpoint["check_runs"],
            checks.samples + checks.unavailable
        );
        assert_eq!(
            summary.request_elapsed_ms_by_endpoint["graphql"].p95,
            Some(42)
        );
        let unknown = &summary.request_elapsed_ms_by_endpoint["unknown"];
        assert_eq!(
            (
                unknown.samples,
                unknown.unavailable,
                unknown.p50,
                unknown.p95,
                unknown.max
            ),
            (0, 1, None, None, None)
        );
        assert!(
            !serde_json::to_string(&summary)
                .unwrap()
                .contains("private-token-and-comment")
        );
    }

    #[test]
    fn rotated_finals_are_deduplicated_and_retries_keep_their_own_failure() {
        let root = tempfile::tempdir().unwrap();
        let failed = "2026-09-21T06:24:14Z INFO hey_gh::scheduler: GitHub request finished request_id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa endpoint=graphql attempts=2 succeeded=false http_status=200 source=error error_code=graphql_access_denied\n";
        fs::write(root.path().join("hey-gh.log.1"), format!("2026-09-21T06:20:00Z INFO hey_gh: daemon starting\n2026-09-21T06:24:05Z WARN hey_gh: GitHub server error; retry scheduled request_id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa status=504 attempt=1\n{failed}")).unwrap();
        fs::write(root.path().join("hey-gh.log"), format!("{failed}2026-09-21T06:24:20Z INFO hey_gh: GitHub request finished request_id=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb endpoint=check_runs attempts=1 succeeded=true http_status=304 source=revalidated\n2026-09-21T06:24:30Z WARN hey_gh: transport attempt failed request_id=cccccccccccccccccccccccccccccccc\n")).unwrap();
        let summary = read(root.path(), 120, NOW).unwrap();
        assert_eq!(summary.completed_requests, 2);
        assert_eq!(summary.attempts_for_completed_requests, 3);
        assert_eq!(summary.successful_requests, 1);
        assert_eq!(summary.failed_requests, 1);
        assert_eq!(summary.completed_retries_succeeded, 0);
        assert_eq!(summary.completed_retries_failed, 1);
        assert_eq!(summary.retry_jobs_with_warnings, 2);
        assert_eq!(summary.retry_jobs_without_final, 1);
        assert!(summary.retention_covers_start);
        assert_eq!(summary.failures_by_code["graphql_access_denied"], 1);
        assert_eq!(summary.completions_by_http_status["200"], 1);
        assert_eq!(summary.completions_by_http_status["304"], 1);
    }

    #[test]
    fn window_partial_legacy_and_unknown_fields_never_forward_private_values() {
        let root = tempfile::tempdir().unwrap();
        let private = "private-token-and-comment";
        fs::write(root.path().join("hey-gh.log"), format!("2026-09-21T06:24:00Z INFO hey_gh: GitHub request finished request_id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa attempts=1 succeeded=false endpoint={private} source={private} error_code={private} http_status={private}\n2026-09-21T06:24:01Z INFO hey_gh: GitHub request finished attempts=1 succeeded=false\n2026-09-21T06:26:00Z WARN hey_gh: GitHub rate limit observed\n2026-09-21T06:24:02Z INFO hey_gh: GitHub request finished request_id=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb attempts=1 succeeded=true")).unwrap();
        let summary = read(root.path(), 120, NOW).unwrap();
        assert_eq!(summary.completed_requests, 1);
        assert_eq!(summary.uncorrelated_completion_lines, 1);
        assert_eq!(summary.incomplete_lines, 1);
        assert_eq!(summary.rate_limit_observations, 0);
        assert!(!summary.retention_covers_start);
        assert_eq!(summary.completions_by_endpoint["unknown"], 1);
        assert_eq!(summary.completions_by_http_status["unavailable"], 1);
        assert!(!serde_json::to_string(&summary).unwrap().contains(private));
        assert!(!summary.atomic);
    }

    #[test]
    fn cycle_summaries_distinguish_local_budget_interruptions_and_deduplicate_rotations() {
        let root = tempfile::tempdir().unwrap();
        let cycle = "2026-09-21T06:24:00Z INFO hey_gh: account refresh cycle finished mode=ci seed_only=false started_at_ms=1789971720000 finished_at_ms=1789971840000 total=122 attempted=5 succeeded=4 failed=0 interrupted=1 deferred=117 cycle_budget_exhausted=true repository=private-repo\n";
        fs::write(
            root.path().join("hey-gh.log.1"),
            format!("2026-09-21T06:20:00Z INFO hey_gh: daemon starting\n{cycle}"),
        )
        .unwrap();
        fs::write(root.path().join("hey-gh.log"), format!("{cycle}2026-09-21T06:24:30Z INFO hey_gh: account refresh cycle finished mode=details seed_only=false started_at_ms=1789971750000 finished_at_ms=1789971870000 total=122 attempted=4 succeeded=2 failed=1 interrupted=1 deferred=118 cycle_budget_exhausted=true\n")).unwrap();
        let summary = serde_json::to_value(read(root.path(), 120, NOW).unwrap()).unwrap();
        let ci = &summary["account_refresh_cycles"]["ci"];
        assert_eq!(ci["completed_cycles"], 1);
        assert_eq!(ci["succeeded"], 4);
        assert_eq!(ci["failed"], 0);
        assert_eq!(ci["local_budget_interruptions"], 1);
        assert_eq!(ci["deferred_across_cycles"], 117);
        assert_eq!(ci["latest_cycle"]["finished_at_ms"], 1789971840000u64);
        assert_eq!(summary["account_refresh_cycles"]["details"]["failed"], 1);
        assert_eq!(
            summary["completed_requests"], 0,
            "cycle progress is not network traffic"
        );
        assert!(!summary.to_string().contains("private-repo"));
    }

    #[test]
    fn per_pr_interruptions_can_complete_a_cycle_without_exhausting_it() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("hey-gh.log"), "2026-09-21T06:24:00Z INFO hey_gh: account refresh cycle finished mode=ci seed_only=false started_at_ms=1789971830000 finished_at_ms=1789971840000 total=2 attempted=2 succeeded=1 failed=0 interrupted=1 deferred=0 cycle_budget_exhausted=false\n").unwrap();
        let summary = read(root.path(), 120, NOW).unwrap();
        let ci = &summary.account_refresh_cycles["ci"];
        assert_eq!(ci.completed_cycles, 1);
        assert_eq!(ci.local_budget_interruptions, 1);
        assert_eq!(ci.budget_exhausted_cycles, 0);
        assert_eq!(summary.uncorrelated_refresh_cycle_lines, 0);
    }

    #[test]
    fn cycle_summaries_ignore_seed_legacy_invalid_and_private_modes_and_use_finish_order() {
        let root = tempfile::tempdir().unwrap();
        let valid = "total=122 attempted=5 succeeded=4 failed=0 interrupted=1 deferred=117 cycle_budget_exhausted=true";
        fs::write(root.path().join("hey-gh.log"), format!(
            "2026-09-21T06:24:40Z INFO hey_gh: account refresh cycle finished mode=ci seed_only=false started_at_ms=1789971600000 finished_at_ms=1789971870000 {valid}\n\
             2026-09-21T06:24:00Z INFO hey_gh: account refresh cycle finished mode=ci seed_only=false started_at_ms=1789971780000 finished_at_ms=1789971840000 {valid}\n\
             2026-09-21T06:24:41Z INFO hey_gh: account refresh cycle finished mode=ci seed_only=true started_at_ms=1789971600000 finished_at_ms=1789971870000 {valid}\n\
             2026-09-21T06:24:42Z INFO hey_gh: account refresh cycle finished mode=private-token-and-comment seed_only=false started_at_ms=1789971600000 finished_at_ms=1789971870000 {valid}\n\
             2026-09-21T06:24:43Z INFO hey_gh: account refresh cycle finished mode=details seed_only=false\n\
             2026-09-21T06:24:44Z INFO hey_gh: account refresh cycle finished mode=details seed_only=false started_at_ms=1789971600000 finished_at_ms=1789971870000 total=122 attempted=5 succeeded=5 failed=0 interrupted=1 deferred=117 cycle_budget_exhausted=true\n\
             2026-09-21T06:24:45Z INFO hey_gh: account refresh cycle finished mode=details seed_only=false started_at_ms=1789971600000 finished_at_ms=1789972000000 {valid}\n\
             2026-09-21T06:20:00Z INFO hey_gh: account refresh cycle finished mode=ci seed_only=false started_at_ms=1789971000000 finished_at_ms=1789971600000 {valid}\n\
             2026-09-21T06:26:00Z INFO hey_gh: account refresh cycle finished mode=ci seed_only=false started_at_ms=1789971000000 finished_at_ms=1789971960000 {valid}\n"
        )).unwrap();
        let summary = read(root.path(), 120, NOW).unwrap();
        let ci = &summary.account_refresh_cycles["ci"];
        assert_eq!(ci.completed_cycles, 2);
        assert_eq!(ci.succeeded, 8);
        assert_eq!(ci.local_budget_interruptions, 2);
        assert_eq!(ci.deferred_across_cycles, 234);
        assert_eq!(
            ci.latest_cycle.as_ref().unwrap().finished_at_ms,
            1789971870000
        );
        assert_eq!(summary.account_refresh_cycles.len(), 1);
        assert_eq!(summary.uncorrelated_refresh_cycle_lines, 4);
        assert!(
            !serde_json::to_string(&summary)
                .unwrap()
                .contains("private-token-and-comment")
        );
    }

    #[test]
    fn source_failure_records_locate_denied_collections_without_leaking_private_fields() {
        let root = tempfile::tempdir().unwrap();
        let denied = "2026-09-21T06:24:00Z WARN hey_gh: PR detail source refresh failed source=review_threads error_code=graphql_access_denied repository=private-repo number=private-number body=private-comment\n";
        fs::write(root.path().join("hey-gh.log.1"), denied).unwrap();
        fs::write(root.path().join("hey-gh.log"), format!(
            "{denied}2026-09-21T06:24:01Z WARN hey_gh: PR detail source refresh failed source=review_events error_code=graphql_access_denied\n\
             2026-09-21T06:24:02Z WARN hey_gh: CI source refresh failed source=jobs error_code=deadline\n\
             2026-09-21T06:24:03Z WARN hey_gh: background account discovery incomplete; retaining last good collection error_code=graphql_access_denied\n\
             2026-09-21T06:24:04Z WARN hey_gh: account discovery failed; keeping previous roster error_code=graphql_access_denied\n\
             2026-09-21T06:24:05Z WARN hey_gh: PR detail source refresh failed source=private-token error_code=private-error\n\
             2026-09-21T06:24:06Z INFO hey_gh: PR detail source refresh completed source=comments\n\
             2026-09-21T06:20:00Z WARN hey_gh: PR detail source refresh failed source=reviews error_code=transport\n\
             2026-09-21T06:26:00Z WARN hey_gh: CI source refresh failed source=workflow_runs error_code=transport\n\
             2026-09-21T06:24:07Z WARN hey_gh: PR detail source refresh failed source=comments error_code=deadline"
        )).unwrap();
        let summary = serde_json::to_value(read(root.path(), 120, NOW).unwrap()).unwrap();
        let sources = &summary["source_refresh_failure_records"];
        assert_eq!(
            sources["details"]["review_threads"]["graphql_access_denied"],
            1
        );
        assert_eq!(
            sources["details"]["review_events"]["graphql_access_denied"],
            1
        );
        assert_eq!(sources["ci"]["workflow_jobs"]["deadline"], 1);
        assert_eq!(
            sources["discovery"]["open_pull_requests"]["graphql_access_denied"],
            2
        );
        assert_eq!(sources["details"]["unknown"]["unknown"], 1);
        assert!(sources["details"]["reviews"].is_null());
        assert!(sources["details"]["comments"].is_null());
        assert!(sources["ci"]["workflow_runs"].is_null());
        assert_eq!(
            summary["completed_requests"], 0,
            "collection failure records are not request completions"
        );
        assert_eq!(summary["failed_requests"], 0);
        for private in [
            "private-repo",
            "private-number",
            "private-comment",
            "private-token",
            "private-error",
        ] {
            assert!(!summary.to_string().contains(private));
        }
    }

    #[test]
    fn missing_and_oversized_logs_cannot_claim_window_coverage() {
        let root = tempfile::tempdir().unwrap();
        let summary = read(root.path(), 900, NOW).unwrap();
        assert_eq!(summary.completed_requests, 0);
        assert_eq!(summary.earliest_retained_at_ms, None);
        assert!(!summary.retention_covers_start);
        assert!(read(root.path(), 0, NOW).is_err());
        assert!(read(root.path(), 86401, NOW).is_err());
        fs::write(
            root.path().join("hey-gh.log.2"),
            "2026-09-21T06:00:00Z INFO hey_gh: daemon starting\n",
        )
        .unwrap();
        let summary = read(root.path(), 900, NOW).unwrap();
        assert_eq!(summary.archive_gaps, 2);
        assert!(!summary.retention_covers_start);
        fs::write(
            root.path().join("hey-gh.log"),
            "2026-09-21T06:00:00Z INFO hey_gh: daemon starting\n",
        )
        .unwrap();
        File::options()
            .write(true)
            .open(root.path().join("hey-gh.log"))
            .unwrap()
            .set_len(3 * 1024 * 1024)
            .unwrap();
        let summary = read(root.path(), 900, NOW).unwrap();
        assert_eq!(summary.oversized_files, 1);
        assert!(!summary.retention_covers_start);
    }
}
