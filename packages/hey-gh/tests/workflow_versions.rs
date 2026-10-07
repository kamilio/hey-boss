use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::Uri,
    response::{IntoResponse, Response},
};
use hey_gh::{Client, Config, Freshness};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MERGE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const CREATED: &str = "2026-01-01T00:00:00Z";
const UPDATED: &str = "2026-01-01T00:05:00Z";

fn run() -> Value {
    json!({"id":101,"node_id":"WFR_101","head_sha":HEAD,"check_suite_id":201,
        "run_attempt":1,"run_number":7,"workflow_id":301,"name":"Build","display_title":"Test change",
        "path":".github/workflows/build.yml","event":"pull_request","status":"completed","conclusion":"cancelled",
        "created_at":CREATED,"updated_at":UPDATED,"html_url":"https://github.com/acme/demo/actions/runs/101"})
}
fn commit(sha: &str, workflows: bool) -> Value {
    let nodes = if workflows {
        vec![
            json!({"id":"CS_201","databaseId":201,"status":"COMPLETED","conclusion":"CANCELLED","updatedAt":UPDATED,
        "checkRuns":{"totalCount":0},"workflowRun":{"id":"WFR_101","databaseId":101,"runAttempt":1,"runNumber":7,
            "event":"pull_request","createdAt":CREATED,"updatedAt":UPDATED,"displayTitle":"Test change",
            "workflow":{"databaseId":301,"name":"Build"},"file":{"path":".github/workflows/build.yml"},
            "checkSuite":{"databaseId":201,"commit":{"oid":sha}}}}),
        ]
    } else {
        vec![]
    };
    json!({"__typename":"Commit","oid":sha,"status":null,"checkSuites":{"totalCount":nodes.len(),"pageInfo":{"hasNextPage":false},"nodes":nodes}})
}
#[derive(Clone)]
struct Fixture {
    open: bool,
    calls: Arc<Mutex<Vec<String>>>,
    summary: Arc<Mutex<Value>>,
    runs: Arc<Mutex<Vec<Value>>>,
    rest_delay: Arc<AtomicU64>,
    summary_delay: Arc<AtomicU64>,
    gate_entered: Arc<tokio::sync::Notify>,
    gate_release: Arc<tokio::sync::Notify>,
}
async fn handler(State(f): State<Fixture>, uri: Uri, body: Bytes) -> Response {
    f.calls.lock().unwrap().push(uri.to_string());
    if uri.path() == "/graphql" {
        let body: Value = serde_json::from_slice(&body).unwrap();
        if body["query"] == "query Hold { viewer { login } }" {
            f.gate_entered.notify_one();
            f.gate_release.notified().await;
            return Json(json!({"data":{"viewer":{"login":"synthetic"}}})).into_response();
        }
        tokio::time::sleep(Duration::from_millis(
            f.summary_delay.load(Ordering::Relaxed),
        ))
        .await;
        if !body["query"].as_str().unwrap().contains("CommitLists") {
            return Json(json!({"data":{"repository":{"id":"R_demo","nameWithOwner":"acme/demo","pullRequest":null}}})).into_response();
        }
        return Json(f.summary.lock().unwrap().clone()).into_response();
    }
    if uri.path().ends_with("/actions/runs") {
        tokio::time::sleep(Duration::from_millis(f.rest_delay.load(Ordering::Relaxed))).await;
        let runs = if uri.query().unwrap().contains(HEAD) {
            f.runs.lock().unwrap().clone()
        } else {
            vec![]
        };
        return Json(json!({"total_count":runs.len(),"workflow_runs":runs})).into_response();
    }
    Json(if uri.path().ends_with("/pulls/7") {
        json!({"number":7,"node_id":"PR_7","state":if f.open {"open"} else {"closed"},"merged":!f.open,"mergeable":null,
            "head":{"sha":HEAD},"base":{"sha":MERGE,"repo":{"full_name":"acme/demo"}},"merge_commit_sha":MERGE})
    } else if uri.path().ends_with("/check-runs") { json!({"total_count":0,"check_runs":[]})
    } else if uri.path().ends_with("/status") {json!({"total_count":0,"statuses":[]})
    } else if uri.path().ends_with("/jobs") {json!({"total_count":0,"jobs":[]})
    } else {panic!("unexpected {uri}")}).into_response()
}
struct Harness {
    f: Fixture,
    client: Client,
    config: Config,
    _dir: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Harness {
    async fn new() -> Self {
        Self::with_state(false).await
    }
    async fn with_state(open: bool) -> Self {
        let f = Fixture {
            open,
            calls: Default::default(),
            summary: Arc::new(Mutex::new(
                json!({"data":{"repository":{"id":"R_demo","nameWithOwner":"acme/demo","head":commit(HEAD,true),"merge":commit(MERGE,false)}}}),
            )),
            runs: Arc::new(Mutex::new(vec![run()])),
            rest_delay: Default::default(),
            summary_delay: Default::default(),
            gate_entered: Default::default(),
            gate_release: Default::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let router = Router::new().fallback(handler).with_state(f.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: dir.path().join("cache.sqlite"),
            min_spacing: Duration::ZERO,
            ..Config::default()
        };
        let client = Client::with_token(config.clone(), "synthetic-token".into()).unwrap();
        let h = Self {
            f,
            client,
            config,
            _dir: dir,
            server,
        };
        let warm = h
            .client
            .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert!(warm.complete, "{:?}", warm.data.errors);
        h.edit_cache("%", |v| {
            v["validated_at_ms"] = json!(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
                    - 120_000
            )
        });
        h.f.calls.lock().unwrap().clear();
        h
    }
    fn edit_cache(&self, pattern: &str, edit: impl Fn(&mut Value)) {
        let db = rusqlite::Connection::open(&self.config.cache_path).unwrap();
        let rows = db
            .prepare("SELECT key,response FROM cache WHERE key LIKE ?1")
            .unwrap()
            .query_map([pattern], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!rows.is_empty());
        for (key, raw) in rows {
            let mut value: Value = serde_json::from_str(&raw).unwrap();
            edit(&mut value);
            db.execute(
                "UPDATE cache SET response=?1 WHERE key=?2",
                [value.to_string(), key],
            )
            .unwrap();
        }
    }
}
#[tokio::test]
async fn fresh_complete_workflow_versions_reuse_cached_runs_without_waiting_for_rest() {
    let h = Harness::new().await;
    h.f.rest_delay.store(3000, Ordering::Relaxed);
    let observed = tokio::time::timeout(
        Duration::from_secs(1),
        h.client.ci_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .expect("workflow lists waited for REST despite a complete matching GraphQL version proof")
    .unwrap();
    assert!(observed.complete, "{:?}", observed.data.errors);
    assert_eq!(observed.data.workflow_runs, vec![run()]);
    assert!(
        observed
            .validations
            .iter()
            .any(|v| v.resource.contains("workflow-versions"))
    );
    let calls = h.f.calls.lock().unwrap().clone();
    assert!(
        !calls.iter().any(|p| p.contains("/actions/runs")),
        "{calls:?}"
    );
    assert_eq!(calls.iter().filter(|p| *p == "/graphql").count(), 1);
}

#[tokio::test]
async fn incomplete_or_changed_workflow_evidence_keeps_rest_authoritative() {
    let cases = [
        (
            "/data/repository/head/checkSuites/pageInfo/hasNextPage",
            json!(true),
        ),
        ("/data/repository/head/checkSuites/totalCount", json!(2)),
        ("/data/repository/head/checkSuites/nodes/0/id", Value::Null),
        (
            "/data/repository/head/checkSuites/nodes/0/databaseId",
            json!(202),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun",
            Value::Null,
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/status",
            json!("UNKNOWN"),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/conclusion",
            json!("FAILURE"),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/updatedAt",
            json!("2026-01-01T00:06:00Z"),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/id",
            json!("WFR_102"),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/databaseId",
            json!(102),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/runAttempt",
            json!(2),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/runNumber",
            json!(8),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/event",
            json!("push"),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/createdAt",
            json!("2025-12-31T00:00:00Z"),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/updatedAt",
            Value::Null,
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/displayTitle",
            json!("Other change"),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/file",
            Value::Null,
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/file/path",
            json!(".github/workflows/new.yml"),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/workflow/databaseId",
            json!(302),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/workflow/name",
            json!("Renamed"),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/checkSuite/databaseId",
            json!(202),
        ),
        (
            "/data/repository/head/checkSuites/nodes/0/workflowRun/checkSuite/commit/oid",
            json!(MERGE),
        ),
    ];
    for (pointer, value) in cases {
        let h = Harness::new().await;
        *h.f.summary.lock().unwrap().pointer_mut(pointer).unwrap() = value;
        let result = h
            .client
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        let regressed = pointer.ends_with("/workflowRun/databaseId")
            || pointer.ends_with("/workflowRun/runAttempt")
            || pointer.ends_with("/conclusion");
        assert_eq!(
            result.complete, !regressed,
            "{pointer}: {:?}",
            result.data.errors
        );
        if !regressed {
            assert_eq!(result.data.workflow_runs, vec![run()], "{pointer}");
        }
        assert!(
            h.f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.contains("/actions/runs") && p.contains(HEAD)),
            "{pointer}: missing REST refresh"
        );
    }
}

#[tokio::test]
async fn duplicate_or_missing_suite_workflow_identities_cannot_certify_a_roster() {
    for case in [
        "duplicate_suite",
        "duplicate_run",
        "missing_run_field",
        "new_run",
    ] {
        let h = Harness::new().await;
        {
            let mut summary = h.f.summary.lock().unwrap();
            let suites = &mut summary["data"]["repository"]["head"]["checkSuites"];
            if case == "missing_run_field" {
                suites["nodes"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("workflowRun");
            } else {
                let mut node = suites["nodes"][0].clone();
                if case != "duplicate_suite" {
                    node["id"] = json!("CS_202");
                    node["databaseId"] = json!(202);
                    node["workflowRun"]["checkSuite"]["databaseId"] = json!(202);
                }
                if case == "new_run" {
                    node["workflowRun"]["id"] = json!("WFR_102");
                    node["workflowRun"]["databaseId"] = json!(102);
                }
                suites["nodes"].as_array_mut().unwrap().push(node);
                suites["totalCount"] = json!(2);
            }
        }
        let result = h
            .client
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(
            result.complete,
            case != "new_run",
            "{case}: {:?}",
            result.data.errors
        );
        assert!(
            h.f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.contains("/actions/runs") && p.contains(HEAD)),
            "{case}"
        );
    }
}

#[tokio::test]
async fn rerunning_a_cancelled_workflow_fetches_its_new_attempt_and_jobs() {
    let h = Harness::new().await;
    {
        let mut summary = h.f.summary.lock().unwrap();
        let suite = &mut summary["data"]["repository"]["head"]["checkSuites"]["nodes"][0];
        suite["status"] = json!("IN_PROGRESS");
        suite["conclusion"] = Value::Null;
        suite["workflowRun"]["runAttempt"] = json!(2);
    }
    let mut changed = run();
    changed["run_attempt"] = json!(2);
    changed["status"] = json!("in_progress");
    changed["conclusion"] = Value::Null;
    *h.f.runs.lock().unwrap() = vec![changed.clone()];
    let result = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(result.complete, "{:?}", result.data.errors);
    assert_eq!(result.data.workflow_runs, vec![changed]);
    assert!(
        h.f.calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("/attempts/2/jobs"))
    );
}

#[tokio::test]
async fn deleting_all_workflows_can_replace_the_old_nonempty_roster() {
    let h = Harness::new().await;
    h.f.summary.lock().unwrap()["data"]["repository"]["head"] = commit(HEAD, false);
    h.f.runs.lock().unwrap().clear();
    let result = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(result.complete, "{:?}", result.data.errors);
    assert!(result.data.workflow_runs.is_empty());
    assert!(
        h.f.calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("/actions/runs") && p.contains(HEAD))
    );
}

#[tokio::test]
async fn explicit_refresh_and_offline_reads_preserve_their_clocks_and_network_contract() {
    let h = Harness::new().await;
    let result = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(result.complete);
    let calls = h.f.calls.lock().unwrap().len();
    let offline = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(offline.complete, "{:?}", offline.data.errors);
    assert_eq!(calls, h.f.calls.lock().unwrap().len());
    assert!(
        offline
            .validations
            .iter()
            .any(|v| v.resource.contains("/actions/runs")
                && v.validated_at_ms < result.observed_at_ms - 60_000)
    );
    for freshness in [Freshness::Revalidate, Freshness::MaxAge(Duration::ZERO)] {
        h.f.calls.lock().unwrap().clear();
        let refreshed = h.client.ci_for_pr("acme/demo", 7, freshness).await.unwrap();
        assert!(refreshed.complete, "{:?}", refreshed.data.errors);
        assert_eq!(
            h.f.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.contains("/actions/runs?"))
                .count(),
            2
        );
        assert!(
            !refreshed
                .validations
                .iter()
                .any(|v| v.resource.contains("workflow-versions"))
        );
    }
}

#[tokio::test]
async fn missing_old_or_incomplete_rest_payloads_keep_workflow_discovery() {
    for case in [
        "missing_page",
        "too_old",
        "missing_field",
        "wrong_sha",
        "duplicate_run",
    ] {
        let h = Harness::new().await;
        let path = format!("%/actions/runs?head_sha={HEAD}%");
        h.edit_cache(&path, |v| match case {
            "missing_page" => {
                v["link"] = json!(format!(
                    "<{}/repos/acme/demo/actions/runs?head_sha={HEAD}&page=2>; rel=\"next\"",
                    h.config.rest_url.as_str().trim_end_matches('/')
                ))
            }
            "too_old" => v["validated_at_ms"] = json!(1),
            "missing_field" => {
                v["data"].as_object_mut().unwrap().remove("workflow_runs");
            }
            "wrong_sha" => v["data"]["workflow_runs"][0]["head_sha"] = json!(MERGE),
            "duplicate_run" => {
                let r = v["data"]["workflow_runs"][0].clone();
                v["data"]["workflow_runs"].as_array_mut().unwrap().push(r);
                v["data"]["total_count"] = json!(2);
            }
            _ => unreachable!(),
        });
        let result = h
            .client
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(result.complete, "{case}: {:?}", result.data.errors);
        assert_eq!(result.data.workflow_runs, vec![run()]);
        assert!(
            h.f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.contains("/actions/runs") && p.contains(HEAD)),
            "{case}"
        );
    }
}

#[tokio::test]
async fn denied_workflow_proof_cannot_be_hidden_by_a_cached_success() {
    let h = Harness::new().await;
    *h.f.summary.lock().unwrap() =
        json!({"data":null,"errors":[{"type":"FORBIDDEN","message":"synthetic denied"}]});
    let result = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await;
    assert!(!matches!(result,Ok(ref result) if result.complete));
}

#[tokio::test]
async fn a_late_workflow_proof_finishes_while_original_rest_reads_remain_pending() {
    let h = Harness::new().await;
    h.f.summary_delay.store(1300, Ordering::Relaxed);
    h.f.rest_delay.store(4000, Ordering::Relaxed);
    let held = tokio::spawn({
        let client = h.client.clone();
        async move {
            client
                .graphql(
                    "query Hold { viewer { login } }",
                    json!({}),
                    Freshness::Revalidate,
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), h.f.gate_entered.notified())
        .await
        .unwrap();
    h.f.calls.lock().unwrap().clear();
    let release = h.f.gate_release.clone();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        release.notify_one();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        h.client.ci_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .expect("late workflow proof did not release the pending REST collection")
    .unwrap();
    release.await.unwrap();
    held.await.unwrap().unwrap();
    assert!(result.complete, "{:?}", result.data.errors);
    assert_eq!(result.data.workflow_runs, vec![run()]);
    let calls = h.f.calls.lock().unwrap().clone();
    assert!(
        calls.iter().any(|p| p.contains("/actions/runs")),
        "REST fallback never started: {calls:?}"
    );
    assert_eq!(calls.iter().filter(|p| *p == "/graphql").count(), 1);
}

#[tokio::test]
async fn matching_workflow_versions_do_not_memoize_recent_cancelled_empty_jobs() {
    let h = Harness::new().await;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let recent = chrono::DateTime::from_timestamp(seconds as i64, 0)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    {
        let mut summary = h.f.summary.lock().unwrap();
        let suite = &mut summary["data"]["repository"]["head"]["checkSuites"]["nodes"][0];
        suite["updatedAt"] = json!(recent);
        suite["workflowRun"]["updatedAt"] = json!(recent);
    }
    h.edit_cache(&format!("%/actions/runs?head_sha={HEAD}%"), |v| {
        v["data"]["workflow_runs"][0]["updated_at"] = json!(recent)
    });
    let result = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(result.complete);
    assert_eq!(result.data.summary.state, "failure");
    let again = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(again.complete);
    assert_eq!(again.data.summary.state, "failure");
    assert_eq!(
        h.f.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("/attempts/1/jobs"))
            .count(),
        2,
        "a recent cancelled attempt's empty jobs must be revalidated on each read"
    );
}

#[tokio::test]
async fn future_workflow_proof_keeps_old_rest_clocks_offline() {
    let h = Harness::new().await;
    let initial = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(initial.complete);
    h.edit_cache("%graphql%", |v| {
        v["validated_at_ms"] = json!(initial.observed_at_ms + 60_000)
    });
    let calls = h.f.calls.lock().unwrap().len();
    let offline = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(offline.complete);
    assert_eq!(calls, h.f.calls.lock().unwrap().len());
    assert!(
        !offline
            .validations
            .iter()
            .any(|v| v.resource.contains("workflow-versions"))
    );
    assert!(
        offline
            .validations
            .iter()
            .any(|v| v.resource.contains("/actions/runs")
                && v.validated_at_ms < initial.observed_at_ms - 60_000)
    );
}

#[tokio::test]
async fn open_pr_workflows_can_use_commit_versions_without_certifying_pr_selectors() {
    let h = Harness::with_state(true).await;
    h.f.rest_delay.store(3000, Ordering::Relaxed);
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        h.client.ci_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .expect("open PR workflow evidence waited for REST")
    .unwrap();
    assert!(result.complete, "{:?}", result.data.errors);
    assert_eq!(result.data.workflow_runs, vec![run()]);
    assert!(
        result
            .validations
            .iter()
            .any(|v| v.resource.contains("workflow-versions"))
    );
    assert!(
        result
            .validations
            .iter()
            .any(|v| v.resource.ends_with("/pulls/7"))
    );
    assert!(
        !h.f.calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("/actions/runs"))
    );
}

#[tokio::test]
async fn a_newer_workflow_contradiction_cannot_certify_an_old_roster_offline() {
    let h = Harness::new().await;
    assert!(
        h.client
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    h.edit_cache(&format!("%/actions/runs?head_sha={HEAD}%"), |v| {
        v["validated_at_ms"] = json!(1)
    });
    h.edit_cache("%graphql%",|v|v["data"]["data"]["repository"]["head"]["checkSuites"]["nodes"][0]["workflowRun"]["runAttempt"]=json!(2));
    let calls = h.f.calls.lock().unwrap().len();
    let offline = h
        .client
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(calls, h.f.calls.lock().unwrap().len());
    assert!(
        !offline.complete,
        "known newer attempt must remain unavailable until its full REST payload is fetched"
    );
    assert!(
        offline
            .data
            .errors
            .iter()
            .any(|e| e.source.contains("workflow_runs"))
    );
}

#[tokio::test]
async fn stale_rest_success_cannot_replace_a_newer_queued_attempt() {
    for late in [false, true] {
        let h = Harness::new().await;
        {
            let mut summary = h.f.summary.lock().unwrap();
            let suite = &mut summary["data"]["repository"]["head"]["checkSuites"]["nodes"][0];
            suite["status"] = json!("QUEUED");
            suite["conclusion"] = Value::Null;
            suite["workflowRun"]["runAttempt"] = json!(2);
        }
        // Model a lagging REST representation after GraphQL has seen the rerun.
        h.f.runs.lock().unwrap()[0]["conclusion"] = json!("success");
        let held = if late {
            h.f.summary_delay.store(1300, Ordering::Relaxed);
            h.f.rest_delay.store(800, Ordering::Relaxed);
            let held = tokio::spawn({
                let client = h.client.clone();
                async move {
                    client
                        .graphql(
                            "query Hold { viewer { login } }",
                            json!({}),
                            Freshness::Revalidate,
                        )
                        .await
                }
            });
            tokio::time::timeout(Duration::from_secs(1), h.f.gate_entered.notified())
                .await
                .unwrap();
            let release = h.f.gate_release.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                release.notify_one();
            });
            Some(held)
        } else {
            None
        };
        let result = h
            .client
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        if let Some(held) = held {
            held.await.unwrap().unwrap();
        }
        assert!(
            !result.complete,
            "late={late}: an older REST attempt certified success after the rerun was observed"
        );
        assert!(
            result
                .data
                .errors
                .iter()
                .any(|e| e.source.contains("workflow_runs"))
        );
        let calls = h.f.calls.lock().unwrap().len();
        let offline = h
            .client
            .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(
            !offline.complete,
            "late={late}: newer REST response clock hid the known rerun"
        );
        assert_eq!(calls, h.f.calls.lock().unwrap().len());
    }
}

#[tokio::test]
async fn workflow_update_order_uses_github_versions_instead_of_response_clocks() {
    for case in [
        "older_update",
        "same_version_different_outcome",
        "newer_update",
        "newer_attempt",
    ] {
        let h = Harness::new().await;
        {
            let mut summary = h.f.summary.lock().unwrap();
            let suite = &mut summary["data"]["repository"]["head"]["checkSuites"]["nodes"][0];
            suite["status"] = json!("QUEUED");
            suite["conclusion"] = Value::Null;
            suite["updatedAt"] = json!("2026-01-01T00:06:00Z");
            suite["workflowRun"]["updatedAt"] = suite["updatedAt"].clone();
        }
        {
            let mut runs = h.f.runs.lock().unwrap();
            runs[0]["conclusion"] = json!("success");
            if case != "older_update" {
                runs[0]["updated_at"] = json!("2026-01-01T00:06:00Z");
            }
            if case == "newer_update" {
                runs[0]["updated_at"] = json!("2026-01-01T00:07:00Z");
            }
            if case == "newer_attempt" {
                runs[0]["run_attempt"] = json!(2);
            }
        }
        let result = h
            .client
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(
            result.complete,
            case.starts_with("newer"),
            "{case}: {:?}",
            result.data.errors
        );
    }
}
