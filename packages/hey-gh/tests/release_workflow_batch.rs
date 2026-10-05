use axum::{
    Json, Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use hey_gh::{
    Client, Config, Freshness,
    release::{Batch, Request, RunState},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn sha(n: u64) -> String {
    format!("{n:040x}")
}
#[derive(Clone)]
struct Mock {
    mode: &'static str,
    count: u64,
    day: String,
    today: String,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}
impl Mock {
    fn run(&self, id: u64) -> Value {
        json!({"id":id,"node_id":format!("WFR_{id}"),"run_attempt":1,
            "head_sha":sha(id.min(self.count)),"head_branch":"main",
            "repository":{"full_name":"o/r"},"path":".github/workflows/ci.yml",
            "event":"push","status":"completed",
            "conclusion":if id == self.count {"success"} else {"cancelled"},
            "created_at":format!("{}T01:00:00Z",self.day),
            "updated_at":format!("{}T01:10:00Z",self.day)})
    }
    fn current(&self, id: u64) -> Value {
        let mut run = self.run(id);
        if self.mode == "rerun" && id == self.count {
            run["run_attempt"] = json!(2);
            run["status"] = json!("in_progress");
            run["conclusion"] = Value::Null;
            run["updated_at"] = json!(format!("{}T00:01:00Z", self.today));
        }
        if self.mode == "new_run" && id == self.count + 1 {
            run["status"] = json!("queued");
            run["conclusion"] = Value::Null;
            run["created_at"] = json!(format!("{}T00:00:00Z", self.today));
            run["updated_at"] = run["created_at"].clone();
        }
        run
    }
    fn node(&self, id: u64) -> Value {
        let run = self.current(id);
        json!({"__typename":"WorkflowRun","id":run["node_id"],"databaseId":id,
            "runAttempt":run["run_attempt"],"event":run["event"],
            "createdAt":run["created_at"],"updatedAt":run["updated_at"],
            "file":{"path":run["path"]},
            "checkSuite":{"commit":{"oid":run["head_sha"]},"branch":{"name":"main"},
                "repository":{"nameWithOwner":"o/r"},"updatedAt":run["updated_at"],
                "status":run["status"].as_str().unwrap().to_ascii_uppercase(),
                "conclusion":run["conclusion"].as_str().map(str::to_ascii_uppercase)}})
    }
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
    async fn new(mode: &'static str, count: u64) -> Self {
        let now = chrono::DateTime::from_timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            0,
        )
        .unwrap();
        let mock = Mock {
            mode,
            count,
            day: (now - chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string(),
            today: now.format("%Y-%m-%d").to_string(),
            calls: Default::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new().fallback(handler).with_state(mock.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: dir.path().join("cache"),
            min_spacing: Duration::ZERO,
            max_attempts: 1,
            report_timeout: Duration::from_secs(3),
            ..Config::default()
        };
        if mode == "app" {
            config.installation = Some(
                hey_gh::AppInstallation::new(
                    "synthetic".into(),
                    1,
                    vec!["o/r".into()],
                    include_str!("fixtures/github-app-test-key.pem"),
                )
                .unwrap(),
            );
        }
        if mode == "budget" {
            let discovery = json!({"total_count":count,"workflow_runs":(1..=count).map(|id|mock.run(id)).collect::<Vec<_>>()}).to_string().len();
            let metadata =
                json!({"data":{"nodes":(1..=count).map(|id|mock.node(id)).collect::<Vec<_>>()}})
                    .to_string()
                    .len();
            // Each response fits independently; their accumulated size does not.
            config.max_collection_bytes = discovery + metadata - 1;
        }
        let client = Client::with_token(config, "synthetic".into()).unwrap();
        Self {
            client,
            mock,
            _dir: dir,
            task,
        }
    }
    async fn report(&self, freshness: Freshness) -> Batch {
        self.client.release_report(&Request {
            project:serde_json::from_value(json!({"repository":"o/r","branch":"main","target":"commit","gates":[{"name":"tests","workflow":"ci.yml","purpose":"validation","jobs":[{"name":"test","steps":["Run tests"]}]}]})).unwrap(),
            targets:vec![sha(1)],
        },freshness).await.unwrap()
    }
    fn calls(&self) -> Vec<(String, Value)> {
        self.mock.calls.lock().unwrap().clone()
    }
    fn graphql_calls(&self) -> Vec<Value> {
        self.calls()
            .into_iter()
            .filter(|(p, _)| p == "/graphql")
            .map(|(_, b)| b)
            .collect()
    }
}
async fn handler(State(mock): State<Mock>, OriginalUri(uri): OriginalUri, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    mock.calls
        .lock()
        .unwrap()
        .push((uri.to_string(), body.clone()));
    let path = uri.path();
    let query: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let page = query
        .get("page")
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(1);
    let value = if path == "/graphql" {
        if mock.mode == "denied" {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"message":"synthetic denial"})),
            )
                .into_response();
        }
        if mock.mode == "rate_limit" {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "60")],
                Json(json!({"message":"synthetic limit"})),
            )
                .into_response();
        }
        if mock.mode == "deadline" {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let ids = body["variables"]["ids"].as_array().unwrap();
        let mut nodes: Vec<_> = ids
            .iter()
            .map(|v| {
                mock.node(
                    v.as_str()
                        .unwrap()
                        .strip_prefix("WFR_")
                        .unwrap()
                        .parse()
                        .unwrap(),
                )
            })
            .collect();
        match mock.mode {
            "missing" => { nodes.pop(); },
            "duplicate" => nodes[1] = nodes[0].clone(),
            "null" => nodes[0] = Value::Null,
            "wrong_id" => nodes[0]["databaseId"] = json!(999),
            "wrong_node" => nodes[0]["id"] = json!("WFR_unknown"),
            "wrong_repo" => nodes[0]["checkSuite"]["repository"]["nameWithOwner"] = json!("o/other"),
            "wrong_head" => nodes[0]["checkSuite"]["commit"]["oid"] = json!(sha(99)),
            "wrong_branch" => nodes[0]["checkSuite"]["branch"]["name"] = json!("other"),
            "wrong_path" => nodes[0]["file"]["path"] = json!(".github/workflows/other.yml"),
            "wrong_event" => nodes[0]["event"] = json!("pull_request"),
            "wrong_created" => nodes[0]["createdAt"] = json!(format!("{}T02:00:00Z",mock.day)),
            "regression" => nodes[0]["runAttempt"] = json!(0),
            "null_branch" => nodes[0]["checkSuite"]["branch"] = Value::Null,
            "timestamp_skew" => nodes[0]["checkSuite"]["updatedAt"] = json!(format!("{}T02:00:00Z",mock.day)),
            "invalid_updated" => nodes[0]["updatedAt"] = json!("bad timestamp"),
            "missing_conclusion" => { nodes[0]["checkSuite"].as_object_mut().unwrap().remove("conclusion"); },
            "older_updated" => { nodes[0]["updatedAt"] = json!(format!("{}T01:01:00Z",mock.day)); },
            "fallback_then_mismatch" => {
                nodes[0]["checkSuite"]["branch"] = Value::Null;
                nodes[1]["event"] = json!("pull_request");
            },
            "partial_error" => return Json(json!({"data":{"nodes":nodes},"errors":[{"message":"synthetic denial","type":"FORBIDDEN"}]})).into_response(),
            _ => {},
        }
        json!({"data":{"nodes":nodes}})
    } else if path.contains("/access_tokens") {
        return (
            StatusCode::CREATED,
            Json(json!({"token":"synthetic-installation","expires_at":"2099-01-01T00:00:00Z"})),
        )
            .into_response();
    } else if path.contains("/commits/") {
        json!({"sha":sha(1),"commit":{"committer":{"date":format!("{}T00:00:00Z",mock.day)}}})
    } else if path.ends_with("/branches/main") {
        json!({"commit":{"sha":sha(mock.count)}})
    } else if path.contains("/compare/") {
        let (base, head) = path.rsplit('/').next().unwrap().split_once("...").unwrap();
        let rows: Vec<_> = (2..=mock.count)
            .skip((page - 1) * 100)
            .take(100)
            .map(|n| json!({"sha":sha(n),"parents":[{"sha":sha(n-1)}]}))
            .collect();
        json!({"status":"ahead","base_commit":{"sha":base},"merge_base_commit":{"sha":base},"total_commits":mock.count-1,"commits":if head == sha(mock.count) {rows}else{vec![]}})
    } else if path.contains("/workflows/") {
        let created = query.get("created");
        let is_archive = created.is_some_and(|s| s.starts_with(&mock.day));
        let mut rows: Vec<_> = if created.is_none() || is_archive {
            (1..=mock.count)
                .map(|id| {
                    let mut run = if created.is_some() {
                        mock.run(id)
                    } else {
                        mock.current(id)
                    };
                    if mock.mode == "missing_node_ids" {
                        run.as_object_mut().unwrap().remove("node_id");
                    }
                    run
                })
                .collect()
        } else {
            vec![]
        };
        if mock.mode == "new_run"
            && (created.is_none() || created.is_some_and(|s| s.starts_with(&mock.today)))
        {
            rows.push(mock.current(mock.count + 1));
        }
        if let Some(head) = query.get("head_sha") {
            rows.retain(|r| r["head_sha"] == *head);
        }
        let total = rows.len();
        let rows: Vec<_> = rows.into_iter().skip((page - 1) * 100).take(100).collect();
        let total = if mock.mode == "incomplete" && is_archive {
            total + 1
        } else {
            total
        };
        let value = json!({"total_count":total,"workflow_runs":rows});
        if page * 100 < total && total > 100 {
            return (
                [(
                    "link",
                    format!(
                        "<{}{}page={}>; rel=\"next\"",
                        uri,
                        if uri.query().is_some() { "&" } else { "?" },
                        page + 1
                    ),
                )],
                Json(value),
            )
                .into_response();
        }
        value
    } else if path.contains("/actions/runs/") {
        let parts: Vec<_> = path.split('/').collect();
        let id = parts[6].parse::<u64>().unwrap();
        let attempt = parts[8].parse::<u64>().unwrap();
        if !path.ends_with("/jobs") {
            return Json(mock.run(id)).into_response();
        }
        let run = mock.current(id);
        let passed = id == mock.count && (mock.mode != "rerun" || attempt == 1);
        let jobs = if passed {
            vec![
                json!({"id":id+1000,"run_id":id,"run_attempt":attempt,"head_sha":run["head_sha"],"name":"test","status":"completed","conclusion":"success","completed_at":format!("{}T01:09:00Z",mock.day),"steps":[{"name":"Run tests","status":"completed","conclusion":"success"}]}),
            ]
        } else {
            vec![]
        };
        json!({"total_count":jobs.len(),"jobs":jobs})
    } else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"message":format!("unmocked {uri}")})),
        )
            .into_response();
    };
    Json(value).into_response()
}
fn fresh() -> Freshness {
    Freshness::MaxAge(Duration::ZERO)
}
fn assert_state(batch: &Batch, expected: &str) {
    assert_eq!(
        batch.reports[0].state, expected,
        "{:?}",
        batch.reports[0].errors
    );
}

#[tokio::test]
async fn archived_metadata_batches_preserve_discovery_cache_and_final_validation() {
    let h = Harness::new("normal", 6).await;
    assert_state(
        &h.report(Freshness::MaxAge(Duration::from_secs(60))).await,
        "verified",
    );
    h.mock.calls.lock().unwrap().clear();
    let batch = h.report(fresh()).await;
    assert_state(&batch, "verified");
    let calls = h.calls();
    assert!(
        !calls
            .iter()
            .any(|(p, _)| p.contains(&format!("created={}", h.mock.day))),
        "closed-day discovery reread: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|(p, _)| p.contains(&format!("created={}", h.mock.today)))
    );
    assert!(
        calls
            .iter()
            .any(|(p, _)| p.contains(&format!("head_sha={}", sha(6))))
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(p, _)| p.ends_with("/branches/main"))
            .count(),
        2
    );
    assert_eq!(h.graphql_calls().len(), 1);
    assert!(
        batch
            .validations
            .iter()
            .any(|v| v.resource.ends_with("/graphql"))
    );
}
#[tokio::test]
async fn rerun_metadata_retains_old_attempt_without_confirming_it() {
    let h = Harness::new("rerun", 6).await;
    let batch = h.report(fresh()).await;
    assert_state(&batch, "watching");
    let gate = &batch.reports[0].gates[0];
    assert!(gate.confirmation.is_none());
    assert!(
        gate.runs
            .iter()
            .any(|r| r.id == 6 && r.attempt == 1 && r.verdict.state == RunState::Passed)
    );
    assert!(
        gate.runs
            .iter()
            .any(|r| r.id == 6 && r.attempt == 2 && r.verdict.state == RunState::Pending)
    );
    assert_eq!(h.graphql_calls().len(), 1);
}
#[tokio::test]
async fn new_current_day_run_on_archived_head_still_supersedes_success() {
    let h = Harness::new("new_run", 6).await;
    let batch = h.report(fresh()).await;
    assert_state(&batch, "watching");
    assert!(batch.reports[0].gates[0].confirmation.is_none());
    assert!(
        batch.reports[0].gates[0]
            .runs
            .iter()
            .any(|r| r.id == 7 && r.verdict.state == RunState::Pending)
    );
    assert_eq!(h.graphql_calls().len(), 1);
}
#[tokio::test]
async fn incomplete_or_changed_graphql_identity_cannot_confirm_or_fallback() {
    for mode in [
        "missing",
        "duplicate",
        "null",
        "wrong_id",
        "wrong_node",
        "wrong_repo",
        "wrong_head",
        "wrong_branch",
        "wrong_path",
        "wrong_event",
        "wrong_created",
        "regression",
        "invalid_updated",
        "missing_conclusion",
        "older_updated",
        "fallback_then_mismatch",
    ] {
        let h = Harness::new(mode, 6).await;
        let batch = h.report(fresh()).await;
        assert_state(&batch, "unknown");
        assert!(!batch.reports[0].errors.is_empty(), "{mode}");
        assert_eq!(h.graphql_calls().len(), 1, "{mode}");
        assert_eq!(
            h.calls()
                .iter()
                .filter(|(p, _)| p.contains(&format!("created={}", h.mock.day)))
                .count(),
            1,
            "{mode}: invalid identity must not trigger REST fallback"
        );
    }
}
#[tokio::test]
async fn graphql_errors_do_not_bypass_access_quota_or_deadline_guards() {
    for mode in ["denied", "rate_limit", "deadline", "partial_error"] {
        let h = Harness::new(mode, 6).await;
        assert_state(&h.report(fresh()).await, "unknown");
        assert_eq!(h.graphql_calls().len(), 1, "{mode}");
        assert_eq!(
            h.calls()
                .iter()
                .filter(|(p, _)| p.contains(&format!("created={}", h.mock.day)))
                .count(),
            1,
            "{mode}"
        );
    }
}
#[tokio::test]
async fn unrepresentable_suite_metadata_uses_fresh_rest_history() {
    for mode in ["null_branch", "timestamp_skew"] {
        let h = Harness::new(mode, 6).await;
        assert_state(&h.report(fresh()).await, "verified");
        assert_eq!(h.graphql_calls().len(), 1, "{mode}");
        assert_eq!(
            h.calls()
                .iter()
                .filter(|(p, _)| p.contains(&format!("created={}", h.mock.day)))
                .count(),
            2,
            "{mode}"
        );
    }
}
#[tokio::test]
async fn app_missing_nodes_explicit_refresh_and_incomplete_discovery_keep_rest() {
    for mode in ["app", "missing_node_ids", "normal", "incomplete"] {
        let h = Harness::new(mode, 6).await;
        let batch = h
            .report(if mode == "normal" {
                Freshness::Revalidate
            } else {
                fresh()
            })
            .await;
        assert_state(
            &batch,
            if mode == "incomplete" {
                "watching"
            } else {
                "verified"
            },
        );
        assert!(h.graphql_calls().is_empty(), "{mode}");
        h.mock.calls.lock().unwrap().clear();
        let cached = h.report(Freshness::CachedOnly).await;
        assert_state(
            &cached,
            if mode == "incomplete" {
                "watching"
            } else {
                "verified"
            },
        );
        assert!(
            h.calls().is_empty(),
            "cached-only read made network requests"
        );
    }
}
#[tokio::test]
async fn metadata_batches_are_bounded_and_cover_every_archived_run() {
    let h = Harness::new("normal", 130).await;
    let batch = h.report(fresh()).await;
    assert_state(&batch, "verified");
    assert_eq!(batch.reports[0].gates[0].runs.len(), 130);
    let calls = h.graphql_calls();
    assert_eq!(calls.len(), 2);
    let ids: Vec<_> = calls
        .iter()
        .flat_map(|v| v["variables"]["ids"].as_array().unwrap())
        .collect();
    assert_eq!(ids.len(), 130);
    assert!(
        calls
            .iter()
            .all(|v| v["variables"]["ids"].as_array().unwrap().len() <= 100)
    );
    assert_eq!(
        ids.iter()
            .map(|id| id.as_str().unwrap())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        130
    );
}
#[tokio::test]
async fn graphql_bytes_share_the_discovery_collection_budget() {
    let h = Harness::new("budget", 6).await;
    let batch = h.report(fresh()).await;
    assert_state(&batch, "unknown");
    assert_eq!(h.graphql_calls().len(), 1);
    assert!(
        batch.reports[0]
            .errors
            .iter()
            .any(|s| s.contains("byte limit"))
    );
}
