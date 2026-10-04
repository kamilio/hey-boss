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
        json!({"sha":path.rsplit('/').next().unwrap(),"commit":{"committer":{"date":"2026-10-04T00:00:00Z"}}})
    } else if path == "/repos/o/r/branches/main" {
        let count = mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with(path))
            .count();
        json!({"name":"main","commit":{"sha":if matches!(mode,"force_push"|"fast_forward") && count>1 {D}else{C}}})
    } else if path.contains("/compare/") {
        let (base, head) = path.rsplit('/').next().unwrap().split_once("...").unwrap();
        let ahead = base <= head
            && !(mode == "unrelated" && head == C)
            && !(mode == "side_branch" && base == A && head == B)
            && !(mode == "force_push" && head == D);
        let mut result = json!({"status":if ahead {"ahead"}else{"diverged"},"base_commit":{"sha":base},"merge_base_commit":{"sha":if ahead {base}else{D}}});
        if matches!(
            mode,
            "branch_roster" | "truncated_branch_roster" | "side_branch"
        ) {
            let commits: Vec<_> = [A, B, C, D]
                .into_iter()
                .filter(|sha| *sha > base && *sha <= head)
                .map(|sha| json!({"sha":sha}))
                .collect();
            result["total_commits"] = json!(commits.len());
            result["commits"] = json!(commits);
            if mode == "truncated_branch_roster" {
                result["commits"] = json!([]);
            }
        }
        result
    } else if path == "/repos/o/r/actions/workflows/ci.yml/runs" {
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
                if mode == "old_failure" {
                    "failure"
                } else {
                    "cancelled"
                },
            ),
        ];
        if mode == "past_attempt" {
            rows[0]["run_attempt"] = json!(2);
        }
        let head_query = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .find(|(key, _)| key == "head_sha")
            .map(|(_, value)| value.into_owned());
        if let Some(head) = head_query {
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
        let mut jobs = match id {
            2 if mode == "side_branch" => vec![job(2, B, "success")],
            3 if mode == "side_branch" => vec![job(3, C, "skipped")],
            1 if mode == "old_failure" => vec![job(1, A, "failure")],
            1 => vec![],
            2 => vec![job(2, if mode == "old_failure" { A } else { B }, "skipped")],
            _ => vec![job(3, C, "success")],
        };
        if mode == "past_attempt" && id == 3 {
            let attempt = path.split('/').nth(8).unwrap().parse::<u64>().unwrap();
            jobs = vec![job(3, C, if attempt == 1 { "failure" } else { "success" })];
            jobs[0]["run_attempt"] = json!(attempt);
        }
        json!({"total_count":if mode=="missing_job_page" {jobs.len()+1}else{jobs.len()},"jobs":jobs})
    } else if path == "/repos/o/r/actions/runs/3/attempts/1" {
        run(3, C, "failure")
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
