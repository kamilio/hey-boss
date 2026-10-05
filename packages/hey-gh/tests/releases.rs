use axum::{
    Json, Router,
    extract::{OriginalUri, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use hey_gh::{
    Client, Config, Freshness,
    release::{Project, Request},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
const D: &str = "dddddddddddddddddddddddddddddddddddddddd";
const E: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
fn now() -> chrono::DateTime<chrono::Utc> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    chrono::DateTime::from_timestamp(seconds as i64, 0).unwrap()
}
#[derive(Clone, Default)]
struct Mock {
    mode: String,
    calls: Arc<Mutex<Vec<String>>>,
}
struct Harness {
    client: Client,
    mock: Mock,
    _dir: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Harness {
    async fn new(mode: &str) -> Self {
        let mock = Mock {
            mode: mode.into(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new().fallback(handler).with_state(mock.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = Client::with_token(
            Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache"),
                min_spacing: Duration::ZERO,
                max_attempts: 1,
                report_timeout: if mode.starts_with("partial_") {
                    Duration::from_secs(2)
                } else {
                    Config::default().report_timeout
                },
                max_collection_bytes: if mode == "paged_branch_budget" {
                    1024
                } else {
                    Config::default().max_collection_bytes
                },
                ..Config::default()
            },
            "synthetic".into(),
        )
        .unwrap();
        Self {
            client,
            mock,
            _dir: dir,
            task,
        }
    }
    async fn report(&self, targets: &[&str]) -> hey_gh::release::Batch {
        self.client
            .release_report(
                &Request {
                    project: project(),
                    targets: targets.iter().map(|s| s.to_string()).collect(),
                },
                Freshness::MaxAge(Duration::from_secs(60)),
            )
            .await
            .unwrap()
    }
}
fn project() -> Project {
    serde_json::from_value(json!({"repository":"o/r","branch":"main","target":"commit","gates":[{"name":"main tests","workflow":"ci.yml","purpose":"validation","jobs":[{"name":"test","steps":["Run tests"]}]}]})).unwrap()
}
fn run(id: u64, sha: &str, conclusion: &str) -> Value {
    json!({"id":id,"run_attempt":1,"head_sha":sha,"head_branch":"main","repository":{"full_name":"o/r"},"path":".github/workflows/ci.yml","event":"push","status":"completed","conclusion":conclusion,"created_at":format!("2026-10-04T0{id}:00:00Z"),"updated_at":format!("2026-10-04T0{id}:10:00Z")})
}
fn job(id: u64, sha: &str, conclusion: &str) -> Value {
    json!({"id":id+10,"run_id":id,"run_attempt":1,"head_sha":sha,"name":"test","status":"completed","conclusion":conclusion,"completed_at":format!("2026-10-04T0{id}:09:00Z"),"steps":[{"name":"Run tests","status":"completed","conclusion":conclusion}]})
}
async fn handler(State(mock): State<Mock>, OriginalUri(uri): OriginalUri) -> Response {
    let path = uri.path();
    mock.calls.lock().unwrap().push(uri.to_string());
    let mode = mock.mode.as_str();
    if mode == "paced_metadata" && path.starts_with("/repos/o/r/commits/") {
        tokio::time::sleep(Duration::from_secs(21)).await;
    }
    let result = if path.starts_with("/repos/o/r/commits/") {
        json!({"sha":path.rsplit('/').next().unwrap(),"commit":{"committer":{"date":if mode.starts_with("archive_") {now().format("%Y-%m-%dT00:00:00Z").to_string()} else {"2026-10-04T00:00:00Z".to_owned()}}}})
    } else if path == "/repos/o/r/branches/main" {
        let count = mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with(path))
            .count();
        json!({"name":"main","commit":{"sha":if mode.starts_with("archive_") {E} else if matches!(mode,"force_push"|"fast_forward"|"partial_confirmation") && count>1 {D}else{C}}})
    } else if path.contains("/compare/") {
        let (base, head) = path.rsplit('/').next().unwrap().split_once("...").unwrap();
        let ahead = base <= head
            && !(mode == "unrelated" && head == C)
            && !(mode == "side_branch" && base == A && head == B)
            && !(mode == "paged_branch_side" && base == A && head == B)
            && !(matches!(mode, "force_push" | "partial_confirmation") && head == D);
        let mut result = json!({"status":if ahead {"ahead"}else{"diverged"},"base_commit":{"sha":base},"merge_base_commit":{"sha":if ahead {base}else{D}}});
        if mode.starts_with("paged_branch")
            && base == A
            && head == C
            && uri
                .query()
                .is_some_and(|query| query.contains("per_page=100"))
        {
            let page = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                .find(|(key, _)| key == "page")
                .map(|(_, value)| value.parse::<usize>().unwrap())
                .unwrap_or(1);
            if mode == "paged_branch_denied" && page == 2 {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"message":"synthetic access denial"})),
                )
                    .into_response();
            }
            let mut parent = if mode == "paged_branch_side" { D } else { A }.to_owned();
            let mut commits: Vec<_> = std::iter::once(B.to_owned())
                .chain((1..100).map(|n| format!("b{n:039x}")))
                .chain(std::iter::once(C.to_owned()))
                .map(|sha| {
                    let mut parents = vec![json!({"sha":parent})];
                    if sha == C && mode == "paged_branch_side" {
                        parents.push(json!({"sha":A}));
                    }
                    parent = sha.clone();
                    json!({"sha":sha,"parents":parents})
                })
                .collect();
            if mode == "paged_branch_reverse" {
                commits[..100].reverse();
            }
            result["total_commits"] = json!(if mode == "paged_branch_bound" {
                10001
            } else if mode == "paged_branch_changed" && page == 2 {
                102
            } else {
                101
            });
            let start = (page - 1) * 100;
            result["commits"] = json!(
                commits
                    .into_iter()
                    .skip(start)
                    .take(100)
                    .collect::<Vec<_>>()
            );
            if page == 2 {
                match mode {
                    "paged_branch_short" => result["commits"] = json!([]),
                    "paged_branch_duplicate" => result["commits"][0]["sha"] = json!(B),
                    "paged_branch_invalid" => result["commits"][0]["sha"] = json!("invalid"),
                    _ => {}
                }
            }
        }
        if matches!(
            mode,
            "branch_roster" | "truncated_branch_roster" | "side_branch" | "branch_parents"
        ) || mode.starts_with("archive_")
        {
            let commits: Vec<_> = [A, B, C, D, E]
                .into_iter()
                .filter(|sha| *sha > base && *sha <= head)
                .map(|sha| {
                    if mode == "branch_parents" || mode == "side_branch" {
                        let parents = match (mode, sha) {
                            ("side_branch", B) => vec![D],
                            ("side_branch", C) => vec![A, B],
                            (_, B) => vec![A],
                            (_, C) => vec![B],
                            _ => vec![C],
                        };
                        let parents: Vec<_> =
                            parents.into_iter().map(|sha| json!({"sha":sha})).collect();
                        json!({"sha":sha,"parents":parents})
                    } else {
                        json!({"sha":sha})
                    }
                })
                .collect();
            result["total_commits"] = json!(commits.len());
            result["commits"] = json!(commits);
            if mode == "truncated_branch_roster" {
                result["commits"] = json!([]);
            }
        }
        result
    } else if matches!(
        path,
        "/repos/o/r/actions/workflows/ci.yml/runs" | "/repos/o/r/actions/workflows/early.yml/runs"
    ) {
        if mode == "forbidden" {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"message":"synthetic access denial"})),
            )
                .into_response();
        }
        let created = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .find(|(k, _)| k == "created")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        let mut rows = vec![
            run(3, C, "success"),
            run(2, if mode == "old_failure" { A } else { B }, "success"),
            run(
                1,
                A,
                if matches!(mode, "old_failure" | "partial_error") {
                    "failure"
                } else {
                    "cancelled"
                },
            ),
        ];
        if path.ends_with("/early.yml/runs") {
            rows.retain(|row| row["id"] == 2);
            rows[0]["path"] = json!(".github/workflows/early.yml");
        }
        if mode == "archive_many" {
            rows.extend([run(4, D, "cancelled"), run(5, E, "cancelled")]);
        }
        if mode.starts_with("archive_") {
            for row in &mut rows {
                let yesterday = (now() - chrono::Duration::days(1))
                    .format("%Y-%m-%d")
                    .to_string();
                row["created_at"] = json!(
                    row["created_at"]
                        .as_str()
                        .unwrap()
                        .replace("2026-10-04", &yesterday)
                );
            }
        }
        if mode == "past_attempt" {
            rows[0]["run_attempt"] = json!(2);
        }
        let head_query = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .find(|(key, _)| key == "head_sha")
            .map(|(_, value)| value.into_owned());
        if let Some(head) = head_query {
            if mode == "archive_rerun" && head == C {
                rows[0]["run_attempt"] = json!(2);
                rows[0]["status"] = json!("in_progress");
                rows[0]["conclusion"] = Value::Null;
            }
            let rechecking = {
                let calls = mock.calls.lock().unwrap();
                calls
                    .iter()
                    .filter(|p| p.contains(&format!("head_sha={head}")))
                    .count()
                    > 1
                    || calls.iter().any(|p| p.contains("created="))
            };
            if mode == "publication_progress" && rechecking {
                rows[0]["conclusion"] = json!("failure");
                rows[0]["updated_at"] = json!("2026-10-04T04:00:00Z");
            }
            if mode == "new_run" && rechecking {
                let mut new = run(4, C, "success");
                new["status"] = json!("pending");
                new["conclusion"] = Value::Null;
                rows.insert(0, new);
            }
            if mode == "rerun" && rechecking {
                rows[0]["run_attempt"] = json!(2);
                rows[0]["status"] = json!("in_progress");
                rows[0]["conclusion"] = Value::Null;
            }
            rows.retain(|r| r["head_sha"] == head);
        }
        let range = created.split_once("..").and_then(|(a, b)| {
            Some((
                chrono::DateTime::parse_from_rfc3339(a).ok()?,
                chrono::DateTime::parse_from_rfc3339(b).ok()?,
            ))
        });
        let dense = mode == "large_history" && range.is_none_or(|(a, b)| (b - a).num_hours() > 12);
        if let Some((start, end)) = range {
            rows.retain(|r| {
                let at = chrono::DateTime::parse_from_rfc3339(r["created_at"].as_str().unwrap())
                    .unwrap();
                at >= start && at <= end
            });
        }
        json!({"total_count":if mode=="truncated" || dense {1000}else{rows.len()},"workflow_runs":rows})
    } else if path.ends_with("/jobs") {
        let id = path.split('/').nth(6).unwrap().parse::<u64>().unwrap();
        if mode.starts_with("partial_") && id == 3 {
            if mode == "partial_deadline" {
                tokio::time::sleep(Duration::from_secs(4)).await;
            }
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"message":"synthetic access denial"})),
            )
                .into_response();
        }
        let mut jobs = match id {
            4 | 5 if mode == "archive_many" => vec![],
            2 if matches!(mode, "side_branch" | "paged_branch_side") => vec![job(2, B, "success")],
            3 if matches!(mode, "side_branch" | "paged_branch_side") => vec![job(3, C, "skipped")],
            1 if mode == "old_failure" => vec![job(1, A, "failure")],
            1 => vec![],
            2 => vec![job(2, if mode == "old_failure" { A } else { B }, "skipped")],
            _ => vec![job(3, C, "success")],
        };
        if mode.starts_with("partial_") {
            jobs = vec![job(
                id,
                if id == 1 { A } else { B },
                if id == 1 { "failure" } else { "success" },
            )];
            // The later run overlaps this candidate confirmation, so its
            // collection must finish before certifying the gate.
            jobs[0]["completed_at"] = json!("2026-10-04T04:09:00Z");
        }
        if mode == "past_attempt" && id == 3 {
            let attempt = path.split('/').nth(8).unwrap().parse::<u64>().unwrap();
            jobs = vec![job(3, C, if attempt == 1 { "failure" } else { "success" })];
            jobs[0]["run_attempt"] = json!(attempt);
        }
        if mode == "archive_rerun" && id == 3 && path.contains("/attempts/2/") {
            jobs.clear();
        }
        json!({"total_count":if mode=="missing_job_page" {jobs.len()+1}else{jobs.len()},"jobs":jobs})
    } else if path == "/repos/o/r/actions/runs/3/attempts/1" {
        run(
            3,
            C,
            if mode == "archive_rerun" {
                "success"
            } else {
                "failure"
            },
        )
    } else if path == "/repos/o/r/actions/runs/3" {
        let mut r = run(3, C, "success");
        if mode == "rerun" {
            r["run_attempt"] = json!(2);
            r["status"] = json!("in_progress");
            r["conclusion"] = Value::Null;
        }
        r
    } else if path == "/repos/o/r/pulls/10" {
        json!({"number":10,"state":if mode=="merged_pr" {"closed"} else {"open"},"merged":mode=="merged_pr","merge_commit_sha":A,"merged_at":"2026-10-04T00:00:00Z","head":{"sha":D},"base":{"ref":"main","repo":{"full_name":"o/r"}}})
    } else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"message":format!("unhandled synthetic endpoint {path}")})),
        )
            .into_response();
    };
    Json(result).into_response()
}
#[tokio::test]
async fn partial_gate_deadline_retains_evidence_without_certifying() {
    assert_partial_gate("partial_deadline").await;
}

#[tokio::test]
async fn partial_gate_error_retains_evidence_without_certifying() {
    assert_partial_gate("partial_error").await;
}

async fn assert_partial_gate(mode: &str) {
    let h = Harness::new(mode).await;
    let batch = h.report(&[A]).await;
    let report = &batch.reports[0];
    assert_eq!(report.state, "unknown");
    assert!(!report.errors.is_empty());
    if mode == "partial_deadline" {
        assert!(report.errors.iter().any(|error| error.contains("deadline")));
    }
    assert_eq!(
        report.gates.len(),
        1,
        "already collected evidence survives interruption"
    );
    let gate = &report.gates[0];
    assert_eq!(gate.runs.len(), 2);
    assert!(!gate.satisfied);
    assert!(!gate.history_complete);
    assert!(gate.confirmation.is_none());
    assert_eq!(
        gate.runs[0].verdict.state,
        if mode == "partial_error" {
            hey_gh::release::RunState::Failed
        } else {
            hey_gh::release::RunState::Cancelled
        }
    );
    assert_eq!(gate.runs[0].verdict.failed_jobs, ["test"]);
    assert_eq!(
        gate.runs[1].verdict.state,
        hey_gh::release::RunState::Passed
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.sqlite");
    let mut queue = hey_gh::release::queue::Queue::open(&path).unwrap();
    queue.add(&project(), &[A.into()]).unwrap();
    queue.record(&batch).unwrap();
    drop(queue);
    let queue = hey_gh::release::queue::Queue::open(&path).unwrap();
    let entries = queue.entries().unwrap();
    assert_eq!(entries[0].failures.len(), 1);
    assert_eq!(entries[0].failures[0].id, 1);
    assert!(entries[0].confirmations.is_empty());
    assert_eq!(
        h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.as_str() == "/repos/o/r/branches/main")
            .count(),
        1,
        "an entirely incomplete report has no confirmation needing a final branch read"
    );
}

#[tokio::test]
async fn partial_gate_does_not_skip_branch_validation_for_a_completed_gate() {
    let h = Harness::new("partial_confirmation").await;
    let mut config = project();
    let mut early = config.gates[0].clone();
    early.name = "early tests".into();
    early.workflow = "early.yml".into();
    config.gates.insert(0, early);
    let batch = h
        .client
        .release_report(
            &Request {
                project: config,
                targets: vec![A.into()],
            },
            Freshness::default(),
        )
        .await
        .unwrap();
    let report = &batch.reports[0];
    assert_eq!(report.state, "unknown");
    assert_eq!(report.gates.len(), 2);
    assert!(report.gates[0].history_complete);
    assert!(!report.gates[1].history_complete);
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains("branch changed")),
        "{:?}",
        report.errors
    );
    assert!(
        report
            .gates
            .iter()
            .all(|gate| !gate.satisfied && gate.confirmation.is_none())
    );
}

#[tokio::test]
async fn follows_cancelled_and_skipped_runs_to_a_containing_successor() {
    let h = Harness::new("").await;
    let batch = h.report(&[A, B]).await;
    for report in &batch.reports {
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.state, "verified");
        let c = report.gates[0].confirmation.as_ref().unwrap();
        assert_eq!(c.sha, C);
        assert_eq!(c.coverage, "successor");
        assert_eq!(
            c.verdict.completed_at.as_deref(),
            Some("2026-10-04T03:09:00Z")
        );
    }
    assert_eq!(batch.reports[0].gates[0].runs.len(), 3);
    assert_eq!(
        h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.ends_with("/jobs?per_page=100"))
            .count(),
        3,
        "one jobs fetch per run, shared by the batch"
    );
    assert_eq!(
        h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p == &&"/repos/o/r/branches/main".to_string())
            .count(),
        2,
        "one initial branch read and one final validation for the entire queue batch"
    );
}
#[tokio::test]
async fn revalidates_successful_attempt_before_certifying() {
    let h = Harness::new("rerun").await;
    let batch = h.report(&[C]).await;
    assert_ne!(batch.reports[0].state, "verified");
    assert!(!batch.reports[0].errors.is_empty());
}

#[tokio::test]
async fn unrelated_publication_progress_preserves_completed_checks() {
    let h = Harness::new("publication_progress").await;
    let batch = h.report(&[C]).await;
    assert_eq!(batch.reports[0].state, "verified");
    let evidence = serde_json::to_value(&batch.reports[0].gates[0].confirmation).unwrap();
    assert_eq!(evidence["workflow_conclusion"], "failure");
}

#[tokio::test]
async fn merged_pr_tracks_its_merge_commit_instead_of_the_pr_head() {
    let h = Harness::new("merged_pr").await;
    let mut project = project();
    project.target = hey_gh::release::TargetKind::PullRequest;
    let batch = h
        .client
        .release_report(
            &Request {
                project,
                targets: vec!["10".into()],
            },
            Freshness::default(),
        )
        .await
        .unwrap();
    assert_eq!(batch.reports[0].commit.as_deref(), Some(A));
    assert_eq!(batch.reports[0].state, "verified");
}

#[tokio::test]
async fn a_new_workflow_run_during_collection_withdraws_old_success() {
    let h = Harness::new("new_run").await;
    let batch = h.report(&[C]).await;
    assert_eq!(batch.reports[0].state, "unknown");
    assert!(!batch.reports[0].errors.is_empty());
}

#[tokio::test]
async fn a_newer_same_commit_run_does_not_erase_an_observed_failure() {
    let h = Harness::new("old_failure").await;
    let batch = h.report(&[A]).await;
    assert_eq!(batch.reports[0].state, "recovered");
    assert!(
        batch.reports[0].gates[0]
            .runs
            .iter()
            .any(|r| r.id == 1 && r.verdict.state == hey_gh::release::RunState::Failed)
    );
}

#[tokio::test]
async fn a_successful_rerun_retains_the_failed_earlier_attempt() {
    let h = Harness::new("past_attempt").await;
    let batch = h.report(&[C]).await;
    assert_eq!(
        batch.reports[0].state, "recovered",
        "{:?}",
        batch.reports[0].errors
    );
    let gate = &batch.reports[0].gates[0];
    assert_eq!(gate.runs.len(), 2);
    assert_eq!(
        gate.runs[0].verdict.state,
        hey_gh::release::RunState::Failed
    );
    assert_eq!(gate.confirmation.as_ref().unwrap().attempt, 2);
}

#[tokio::test]
async fn a_small_release_batch_uses_its_existing_budget_for_paced_metadata() {
    let h = Harness::new("paced_metadata").await;
    let started = tokio::time::Instant::now();
    let batch = h.report(&[C]).await;
    assert_eq!(
        batch.reports[0].state, "verified",
        "{:?}",
        batch.reports[0].errors
    );
    assert!(started.elapsed() < Duration::from_secs(120));
}

#[tokio::test]
async fn a_release_at_the_branch_tip_uses_complete_head_history_without_scanning_other_days() {
    let h = Harness::new("").await;
    let batch = h.report(&[C]).await;
    assert_eq!(batch.reports[0].state, "verified");
    assert!(
        !h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("created="))
    );
}

#[tokio::test]
async fn a_complete_branch_roster_limits_comparisons_to_possible_successors() {
    let h = Harness::new("branch_roster").await;
    let batch = h.report(&[A]).await;
    assert_eq!(batch.reports[0].state, "verified");
    assert_eq!(
        h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.contains("/compare/"))
            .count(),
        2
    );
}

#[tokio::test]
async fn large_relevant_archives_keep_using_paged_history() {
    let h = Harness::new("archive_many").await;
    let report = h.report(&[A]).await;
    assert_eq!(
        report.reports[0].state, "verified",
        "{:?}",
        report.reports[0].errors
    );
    let calls = h.mock.calls.lock().unwrap();
    assert_eq!(
        calls.iter().filter(|p| p.contains("head_sha=")).count(),
        1,
        "only the successful run's final validation needs a per-head request: {calls:?}"
    );
}

#[tokio::test]
async fn archived_discovery_is_reused_but_relevant_commit_history_stays_fresh() {
    let h = Harness::new("archive_history").await;
    assert_eq!(h.report(&[A]).await.reports[0].state, "verified");
    h.mock.calls.lock().unwrap().clear();
    // Force ordinary sources to validate again without a wall-clock wait.
    let report = h
        .client
        .release_report(
            &Request {
                project: project(),
                targets: vec![A.into()],
            },
            Freshness::MaxAge(Duration::ZERO),
        )
        .await
        .unwrap();
    assert_eq!(report.reports[0].state, "verified");
    let calls = h.mock.calls.lock().unwrap();
    let today = now().format("created=%Y-%m-%d").to_string();
    let yesterday = (now() - chrono::Duration::days(1))
        .format("created=%Y-%m-%d")
        .to_string();
    assert!(
        !calls.iter().any(|p| p.contains(&yesterday)),
        "closed-day discovery should remain cached: {calls:?}"
    );
    assert!(
        calls.iter().any(|p| p.contains(&today)),
        "today's discovery still needs its normal freshness"
    );
    assert!(
        calls.iter().any(|p| p.contains(&format!("head_sha={C}"))),
        "success must still be validated"
    );
}

#[tokio::test]
async fn archived_discovery_cached_before_the_day_closed_is_revalidated() {
    let h = Harness::new("archive_history").await;
    assert_eq!(h.report(&[A]).await.reports[0].state, "verified");
    let db = rusqlite::Connection::open(h._dir.path().join("cache")).unwrap();
    let before_midnight = now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis()
        - 1;
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%created=%'", [before_midnight]).unwrap();
    h.mock.calls.lock().unwrap().clear();
    let cached = h
        .client
        .release_report(
            &Request {
                project: project(),
                targets: vec![A.into()],
            },
            Freshness::CachedOnly,
        )
        .await
        .unwrap();
    assert!(!cached.reports[0].gates[0].history_complete);
    assert!(!cached.reports[0].gates[0].satisfied);
    assert!(h.mock.calls.lock().unwrap().is_empty());
    let report = h.report(&[A]).await;
    assert_eq!(report.reports[0].state, "verified");
    let yesterday = (now() - chrono::Duration::days(1))
        .format("created=%Y-%m-%d")
        .to_string();
    assert!(
        h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains(&yesterday)),
        "a partial-day snapshot cannot become complete merely because midnight passed"
    );
}

#[tokio::test]
async fn archived_success_does_not_hide_a_current_rerun() {
    let h = Harness::new("archive_rerun").await;
    let report = h.report(&[A]).await;
    assert_eq!(
        report.reports[0].state, "watching",
        "{:?}",
        report.reports[0].errors
    );
    let gate = &report.reports[0].gates[0];
    assert!(!gate.satisfied);
    assert!(gate.runs.iter().any(|r| r.id == 3
        && r.attempt == 2
        && r.verdict.state == hey_gh::release::RunState::Pending));
}

#[tokio::test]
async fn complete_parent_links_prove_containing_runs_without_more_comparisons() {
    let h = Harness::new("branch_parents").await;
    let batch = h.report(&[A]).await;
    assert_eq!(batch.reports[0].state, "verified");
    assert_eq!(
        h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("/compare/"))
            .count(),
        1
    );
}

#[tokio::test]
async fn paged_branch_comparisons_reuse_complete_parent_proofs_and_cached_pages() {
    for mode in ["paged_branch", "paged_branch_reverse"] {
        let h = Harness::new(mode).await;
        let batch = h.report(&[A]).await;
        assert_eq!(
            batch.reports[0].state, "verified",
            "{mode}: {:?}",
            batch.reports[0].errors
        );
        let calls = h.mock.calls.lock().unwrap().clone();
        let compares: Vec<_> = calls.iter().filter(|p| p.contains("/compare/")).collect();
        assert_eq!(
            compares.len(),
            2,
            "complete pages replace individual ancestry calls: {compares:?}"
        );
        assert!(compares.iter().any(|p| p.contains("page=2")));
        let cached = h
            .client
            .release_report(
                &Request {
                    project: project(),
                    targets: vec![A.into()],
                },
                Freshness::CachedOnly,
            )
            .await
            .unwrap();
        assert_eq!(
            cached.reports[0].state, "verified",
            "{:?}",
            cached.reports[0].errors
        );
        assert_eq!(h.mock.calls.lock().unwrap().len(), calls.len());
    }
}

#[tokio::test]
async fn paged_branch_incomplete_rosters_keep_individual_ancestry_checks() {
    for mode in [
        "paged_branch_short",
        "paged_branch_duplicate",
        "paged_branch_invalid",
        "paged_branch_bound",
    ] {
        let h = Harness::new(mode).await;
        let batch = h.report(&[A]).await;
        assert_eq!(
            batch.reports[0].state, "verified",
            "{mode}: {:?}",
            batch.reports[0].errors
        );
        let calls = h.mock.calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .any(|p| p.contains(&format!("/compare/{A}...{B}?per_page=1"))),
            "{mode}"
        );
        if mode == "paged_branch_bound" {
            assert!(!calls.iter().any(|p| p.contains("page=2")));
        }
    }
}

#[tokio::test]
async fn paged_branch_changed_or_denied_pages_do_not_certify() {
    for mode in ["paged_branch_changed", "paged_branch_denied"] {
        let h = Harness::new(mode).await;
        let batch = h.report(&[A]).await;
        assert_eq!(batch.reports[0].state, "unknown", "{mode}");
        assert!(!batch.reports[0].errors.is_empty());
    }
}

#[tokio::test]
async fn paged_branch_comparisons_obey_the_collection_byte_budget() {
    let h = Harness::new("paged_branch_budget").await;
    let batch = h.report(&[A]).await;
    assert_eq!(batch.reports[0].state, "unknown");
    assert!(
        batch.reports[0]
            .errors
            .iter()
            .any(|error| error
                .contains("branch comparison exceeds configured collection byte limit"))
    );
    assert!(
        !h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("page=2"))
    );
}

#[tokio::test]
async fn paged_branch_side_branch_membership_is_not_target_coverage() {
    let h = Harness::new("paged_branch_side").await;
    let batch = h.report(&[A]).await;
    assert_eq!(batch.reports[0].state, "watching");
    assert!(!batch.reports[0].gates[0].satisfied);
}

#[tokio::test]
async fn a_truncated_branch_roster_falls_back_to_individual_ancestry_proofs() {
    let h = Harness::new("truncated_branch_roster").await;
    let batch = h.report(&[A]).await;
    assert_eq!(batch.reports[0].state, "verified");
    assert!(
        h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.contains("/compare/"))
            .count()
            > 1
    );
}

#[tokio::test]
async fn a_merged_side_branch_run_does_not_prove_it_contains_the_release_target() {
    let h = Harness::new("side_branch").await;
    let batch = h.report(&[A]).await;
    assert_eq!(batch.reports[0].state, "watching");
    assert!(!batch.reports[0].gates[0].satisfied);
}

#[tokio::test]
async fn incomplete_job_pagination_cannot_certify() {
    let h = Harness::new("missing_job_page").await;
    let batch = h.report(&[C]).await;
    assert_eq!(batch.reports[0].state, "unknown");
    assert!(!batch.reports[0].errors.is_empty());
}

#[tokio::test]
async fn sdk_release_observation_uses_shared_daemon_without_registering_a_watch() {
    let h = Harness::new("").await;
    let api = hey_gh::api::Api::new(h.client.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sdk = hey_gh::ApiClient::new(
        format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap()
    .background();
    let server = tokio::spawn(async move {
        axum::serve(listener, api.router()).await.unwrap();
    });
    let batch = sdk
        .release_report(
            &Request {
                project: project(),
                targets: vec![A.into()],
            },
            Freshness::default(),
        )
        .await
        .unwrap();
    assert_eq!(batch.reports[0].state, "verified");
    assert!(!batch.validations.is_empty());
    assert!(h.client.watches().await.unwrap().is_empty());
    server.abort();
}
#[tokio::test]
async fn force_push_during_observation_cannot_certify_retired_work() {
    let h = Harness::new("force_push").await;
    let batch = h.report(&[A]).await;
    assert_eq!(batch.reports[0].state, "unknown");
    assert!(
        batch.reports[0]
            .errors
            .iter()
            .any(|e| e.contains("branch changed"))
    );
}

#[tokio::test]
async fn a_safe_main_advance_does_not_restart_a_valid_release_observation() {
    let h = Harness::new("fast_forward").await;
    let batch = h.report(&[A]).await;
    assert_eq!(
        batch.reports[0].state, "verified",
        "{:?}",
        batch.reports[0].errors
    );
    assert_eq!(batch.reports[0].branch_sha.as_deref(), Some(D));
}
#[tokio::test]
async fn truncated_roster_and_denied_access_do_not_certify() {
    for mode in ["truncated", "forbidden"] {
        for target in [A, C] {
            let h = Harness::new(mode).await;
            let batch = h.report(&[target]).await;
            assert_ne!(batch.reports[0].state, "verified");
        }
    }
}
#[tokio::test]
async fn newer_timestamp_without_ancestry_is_not_a_successor() {
    let h = Harness::new("unrelated").await;
    let batch = h.report(&[A]).await;
    assert_eq!(batch.reports[0].state, "not_on_branch");
}
#[tokio::test]
async fn open_pr_waits_for_merge_without_reading_ci() {
    let h = Harness::new("").await;
    let mut project = project();
    project.target = hey_gh::release::TargetKind::PullRequest;
    let batch = h
        .client
        .release_report(
            &Request {
                project,
                targets: vec!["10".into()],
            },
            Freshness::default(),
        )
        .await
        .unwrap();
    assert_eq!(batch.reports[0].state, "awaiting_merge");
    assert_eq!(h.mock.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn settled_cancelled_runs_without_jobs_are_cached_across_polls() {
    let h = Harness::new("").await;
    h.report(&[A]).await;
    h.report(&[A]).await;
    assert_eq!(
        h.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("/runs/1/attempts/1/jobs"))
            .count(),
        1
    );
}

#[tokio::test]
async fn partitions_dense_actions_history_instead_of_getting_stuck_at_search_cap() {
    let h = Harness::new("large_history").await;
    let batch = h.report(&[A]).await;
    assert_eq!(
        batch.reports[0].state, "verified",
        "{:?}",
        batch.reports[0].errors
    );
    assert!(batch.reports[0].gates[0].history_complete);
}

#[test]
fn replay_sanitized_real_workflow_history_for_both_project_profiles() {
    let records: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/releases/history.json")).unwrap();
    assert_eq!(records.len(), 48);
    for record in records {
        let project: Project = serde_json::from_str(if record["project"] == "poe2" {
            include_str!("../src/release/profiles/poe2.json")
        } else {
            include_str!("../src/release/profiles/poe-code.json")
        })
        .unwrap();
        project.validate().unwrap();
        let verdict = hey_gh::release::assess(
            &project.gates[0],
            &record["run"],
            record["jobs"].as_array().unwrap(),
        );
        assert_eq!(
            serde_json::to_value(verdict.state).unwrap(),
            record["expected"],
            "project={} sample={}",
            record["project"],
            record["run"]["id"]
        );
    }
}

#[test]
fn replay_paired_deployment_jobs_against_the_poe2_profile() {
    let recorded: Value =
        serde_json::from_str(include_str!("fixtures/releases/deployment.json")).unwrap();
    let profile: Project =
        serde_json::from_str(include_str!("../src/release/profiles/poe2.json")).unwrap();
    for gate in profile
        .gates
        .iter()
        .filter(|g| g.workflow == "post-merge-poe-convex.yml")
    {
        let verdict =
            hey_gh::release::assess(gate, &recorded["run"], recorded["jobs"].as_array().unwrap());
        assert_eq!(
            verdict.state,
            hey_gh::release::RunState::Passed,
            "{}: {:?}",
            gate.name,
            verdict
        );
        assert!(verdict.completed_at.is_some());
    }
}
