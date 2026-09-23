use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use hey_gh::{Client, Config, Freshness, WatchKind};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;
use tokio::task::JoinHandle;
const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
const D: &str = "dddddddddddddddddddddddddddddddddddddddd";
const H: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const M: &str = "ffffffffffffffffffffffffffffffffffffffff";
struct Data {
    refs: BTreeMap<String, String>,
    calls: Vec<(String, bool)>,
    ancestry: String,
    compare_fail: bool,
    branch_unavailable: bool,
    paginate: bool,
    policy_fail: bool,
    strict: bool,
    up_to_date: bool,
    required: Vec<Value>,
    rules: Vec<Value>,
    reviews: Vec<Value>,
    resolved: bool,
    check_app: u64,
    merge_failure: bool,
    job_failure: bool,
    pr_head: String,
    pr_merge: String,
    conflicting: bool,
}
#[derive(Clone)]
struct Mock(Arc<Mutex<Data>>);
struct Harness {
    mock: Mock,
    url: String,
    dir: TempDir,
    task: JoinHandle<()>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Harness {
    async fn new() -> Self {
        let mock = Mock(Arc::new(Mutex::new(Data {
            refs: BTreeMap::from([("main".into(), A.into()), ("topic/one".into(), A.into())]),
            calls: Vec::new(),
            ancestry: "ahead".into(),
            compare_fail: false,
            branch_unavailable: false,
            paginate: false,
            policy_fail: false,
            strict: false,
            up_to_date: true,
            required: vec![json!({"context":"tests","app_id":42})],
            rules: Vec::new(),
            reviews: Vec::new(),
            resolved: false,
            check_app: 42,
            merge_failure: false,
            job_failure: false,
            pr_head: H.into(),
            pr_merge: M.into(),
            conflicting: false,
        })));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let router = Router::new().fallback(handler).with_state(mock.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            mock,
            url,
            dir: tempfile::tempdir().unwrap(),
            task,
        }
    }
    fn config(&self) -> Config {
        Config {
            rest_url: self.url.parse().unwrap(),
            graphql_url: format!("{}graphql", self.url).parse().unwrap(),
            cache_path: self.dir.path().join("cache.sqlite"),
            min_spacing: Duration::ZERO,
            max_attempts: 1,
            ..Config::default()
        }
    }
    fn client(&self) -> Client {
        Client::with_token(self.config(), "test-token".into()).unwrap()
    }
    fn change(&self, f: impl FnOnce(&mut Data)) {
        f(&mut self.mock.0.lock().unwrap());
    }
}
fn commit(sha: &str) -> Value {
    json!({"sha":sha,"html_url":format!("https://github.com/acme/demo/commit/{sha}"),"author":{"login":"dev","id":1},"committer":{"login":"dev","id":1},"commit":{"message":format!("commit {sha}"),"author":{"name":"Dev","date":"2026-09-19T00:00:00Z"},"committer":{"name":"Dev","date":"2026-09-19T00:00:00Z"},"tree":{"sha":C}},"parents":[{"sha":A}]})
}
async fn handler(State(mock): State<Mock>, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let path = uri.path();
    let query = uri.query().unwrap_or("");
    let mut data = mock.0.lock().unwrap();
    data.calls.push((
        format!("{path}?{query}"),
        headers.contains_key("if-none-match"),
    ));
    let mut link = None;
    let value = match path {
        "/user" => json!({"login":"me"}),
        "/repos/acme/demo" => json!({"default_branch":"main"}),
        "/repos/acme/demo/branches" => json!(
            data.refs
                .iter()
                .map(|(name, sha)| json!({"name":name,"commit":{"sha":sha}}))
                .collect::<Vec<_>>()
        ),
        p if p.ends_with("/protection/required_status_checks") => {
            if data.policy_fail {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"message":"permission denied"})),
                )
                    .into_response();
            }
            json!({"strict":data.strict,"contexts":[],"checks":data.required})
        }
        p if p.contains("/rules/branches/") => json!(data.rules),
        p if p.starts_with("/repos/acme/demo/branches/") => {
            if data.branch_unavailable {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"message":"branch access denied"})),
                )
                    .into_response();
            }
            let encoded = p.trim_start_matches("/repos/acme/demo/branches/");
            let name = encoded
                .replace("%2F", "/")
                .replace("%2f", "/")
                .replace("%25", "%")
                .replace("%20", " ");
            match data.refs.get(&name) {
                Some(sha) => json!({"name":name,"commit":{"sha":sha},"protected":true}),
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(json!({"message":"branch missing"})),
                    )
                        .into_response();
                }
            }
        }
        p if p.starts_with("/repos/acme/demo/compare/") => {
            if data.compare_fail {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"message":"compare unavailable"})),
                )
                    .into_response();
            }
            let (_, new) = p.rsplit_once("...").unwrap();
            if query.contains("per_page=1") && !query.contains("per_page=100") {
                json!({"status":"ahead","merge_base_commit":{"sha":if data.up_to_date {data.refs["main"].clone()} else {D.into()}},"total_commits":1,"commits":[commit(new)]})
            } else {
                let behind = data.ancestry == "behind";
                let total = if behind {
                    0
                } else if data.paginate {
                    2
                } else {
                    1
                };
                let commits = if behind {
                    vec![]
                } else if data.paginate && !query.contains("page=2") {
                    vec![commit(C)]
                } else {
                    vec![commit(new)]
                };
                if data.paginate && !behind && !query.contains("page=2") {
                    link = Some(format!(
                        "<http://{}{path}?per_page=100&page=2>; rel=\"next\"",
                        headers["host"].to_str().unwrap()
                    ));
                }
                json!({"status":data.ancestry,"total_commits":total,"commits":commits,"html_url":"https://github.com/acme/demo/compare/old...new"})
            }
        }
        "/repos/acme/demo/pulls" if query.contains("state=all") => {
            if query.contains("page=2") {
                json!([{"number":8,"id":8,"state":"closed","merged_at":"2026-09-18T00:00:00Z","closed_at":"2026-09-18T00:00:00Z","user":{"login":"other","type":"User"},"created_at":"2026-09-01T00:00:00Z"}])
            } else {
                link = Some(format!(
                    "<http://{}{path}?state=all&per_page=100&page=2>; rel=\"next\"",
                    headers["host"].to_str().unwrap()
                ));
                json!([{"number":7,"id":7,"state":"open","user":{"login":"me","type":"User"},"created_at":"2026-09-01T00:00:00Z"}])
            }
        }
        "/repos/acme/demo/pulls" => {
            json!([{"number":7,"user":{"login":"me"},"base":{"ref":"main"},"head":{"ref":"topic/one","repo":{"full_name":"acme/demo"}}}])
        }
        "/repos/acme/demo/pulls/7" => {
            json!({"number":7,"state":"open","title":"Test PR","head":{"sha":data.pr_head,"ref":"topic/one","repo":{"full_name":"acme/demo"}},"base":{"sha":data.refs["main"],"ref":"main"},"merge_commit_sha":data.pr_merge,"mergeable":!data.conflicting,"requested_reviewers":[{"login":"reviewer"}],"requested_teams":[{"slug":"maintainers"}]})
        }
        "/repos/acme/demo/pulls/7/reviews" => json!(data.reviews),
        p if p.ends_with("/check-runs") => {
            let sha = p.split('/').nth(5).unwrap();
            json!({"check_runs":[{"id":sha.as_bytes()[0] as u64,"name":"tests","app":{"id":data.check_app},"head_sha":sha,"status":"completed","conclusion":if sha==data.pr_merge && data.merge_failure {"failure"}else {"success"},"details_url":"https://github.com/acme/demo/checks/10"}]})
        }
        p if p.ends_with("/status") => {
            json!({"statuses":[{"id":30,"context":"external","state":"success","target_url":"https://ci.example/run"}]})
        }
        "/repos/acme/demo/actions/runs" => {
            let sha = query
                .split('&')
                .find_map(|p| p.strip_prefix("head_sha="))
                .unwrap_or(&data.pr_head);
            json!({"workflow_runs":[{"id":if sha==data.pr_merge {51}else {50},"workflow_id":1,"run_number":1,"run_attempt":1,"event":"pull_request","head_sha":sha,"name":"CI","status":"completed","conclusion":if data.job_failure {"failure"}else {"success"},"html_url":"https://github.com/acme/demo/actions/runs/50"}]})
        }
        p if p.ends_with("/jobs") => {
            json!({"jobs":[{"id":if p.contains("/51/") {61}else {60},"name":"build","status":"completed","conclusion":if data.job_failure {"failure"}else {"success"},"html_url":"https://github.com/acme/demo/actions/runs/50/job/60","steps":[{"name":"unit tests","number":2,"conclusion":if data.job_failure {"failure"}else {"success"}}]}]})
        }
        "/graphql"
            if body["query"]
                .as_str()
                .unwrap_or("")
                .contains("query ReviewEvents") =>
        {
            if body["variables"]["after"] == "next" {
                json!({"data":{"repository":{"pullRequest":{"timelineItems":{"nodes":[{"id":"ER2","__typename":"ReviewRequestRemovedEvent","createdAt":"2026-09-19T01:00:00Z","requestedReviewer":{"login":"reviewer","__typename":"User"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}})
            } else {
                json!({"data":{"repository":{"pullRequest":{"timelineItems":{"nodes":[{"id":"ER1","__typename":"ReviewRequestedEvent","createdAt":"2026-09-19T00:00:00Z","requestedReviewer":{"login":"reviewer","__typename":"User"}}],"pageInfo":{"hasNextPage":true,"endCursor":"next"}}}}}})
            }
        }
        "/graphql" => {
            json!({"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[{"id":"T1","isResolved":data.resolved,"isOutdated":false,"comments":{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}})
        }
        p if p.starts_with("/repos/acme/demo/commits/") => commit(p.rsplit('/').next().unwrap()),
        p if p.ends_with("/comments") || p.ends_with("/timeline") => json!([]),
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"message":format!("unexpected {path}")})),
            )
                .into_response();
        }
    };
    let etag = format!("\"{}\"", hey_digest(&value));
    if headers.get("if-none-match").and_then(|h| h.to_str().ok()) == Some(&etag) {
        return (StatusCode::NOT_MODIFIED, [("etag", etag)]).into_response();
    }
    let mut response = (StatusCode::OK, Json(value)).into_response();
    response.headers_mut().insert("etag", etag.parse().unwrap());
    if let Some(link) = link {
        response.headers_mut().insert("link", link.parse().unwrap());
    }
    response
}
fn hey_digest(v: &Value) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(v.to_string().as_bytes()))
}

#[tokio::test]
async fn commits_paginate_reconcile_force_pushes_deletions_and_restarts() {
    let h = Harness::new().await;
    let client = h.client();
    let first = client
        .repository_report("acme/demo", &[], true, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(first.errors.is_empty());
    assert_eq!(first.branches.len(), 2);
    assert_eq!(first.branches[0].transition.kind, "baseline");
    let initial = first.cursor;
    client
        .repository_report("acme/demo", &[], true, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(
        client
            .changes(Some(&initial), 100)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    h.change(|d| {
        d.refs.insert("main".into(), B.into());
        d.paginate = true;
    });
    let pushed = client
        .repository_report("acme/demo", &[], true, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(pushed.errors.is_empty(), "{:?}", pushed.errors);
    assert_eq!(pushed.changed_branches, vec!["main"]);
    let main = &pushed.branches[0];
    assert_eq!(main.transition.old_sha.as_deref(), Some(A));
    assert_eq!(main.transition.new_sha.as_deref(), Some(B));
    assert_eq!(main.transition.ancestry, "forward");
    assert_eq!(main.transition.commits.len(), 2);
    let changes = client.changes(Some(&initial), 100).await.unwrap();
    assert_eq!(
        changes
            .changes
            .iter()
            .filter(|c| c.resource.starts_with("commit://"))
            .count(),
        2
    );
    assert!(
        !changes
            .changes
            .iter()
            .any(|c| c.resource.ends_with("topic%2Fone"))
    );
    let sdk_restart = h.client();
    assert_eq!(sdk_restart.bootstrap().await.unwrap().cursor, pushed.cursor);
    h.change(|d| {
        d.refs.insert("main".into(), D.into());
        d.ancestry = "diverged".into();
        d.paginate = false;
    });
    let force = sdk_restart
        .repository_report("acme/demo", &[], true, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(force.branches[0].transition.ancestry, "rewritten");
    h.change(|d| {
        d.refs.insert("main".into(), A.into());
        d.ancestry = "behind".into();
        d.refs.remove("topic/one");
    });
    let removed = sdk_restart
        .repository_report("acme/demo", &[], true, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(removed.branches[0].transition.ancestry, "rewind");
    assert_eq!(removed.branches[1].transition.kind, "deleted");
    assert!(removed.branches[1].sha.is_none());
    h.change(|d| {
        d.refs.insert("topic/one".into(), B.into());
        d.ancestry = "ahead".into();
    });
    let recreated = sdk_restart
        .repository_report("acme/demo", &[], true, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(recreated.branches[1].transition.kind, "created");
    let calls = h.mock.0.lock().unwrap().calls.len();
    let cached = sdk_restart
        .repository_report("acme/demo", &[], true, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cached.cursor, recreated.cursor);
    assert_eq!(calls, h.mock.0.lock().unwrap().calls.len());
}

#[tokio::test]
async fn comparison_failure_publishes_tip_and_retries_missing_history() {
    let h = Harness::new().await;
    let c = h.client();
    let initial = c
        .repository_report("acme/demo", &[], false, Freshness::Revalidate)
        .await
        .unwrap();
    h.change(|d| {
        d.refs.insert("main".into(), B.into());
        d.compare_fail = true;
    });
    let broken = c
        .repository_report("acme/demo", &[], false, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(!broken.errors.is_empty());
    assert!(!broken.branches[0].transition.comparison_complete);
    assert_eq!(broken.branches[0].sha.as_deref(), Some(B));
    assert!(
        c.changes(Some(&initial.cursor), 100)
            .await
            .unwrap()
            .changes
            .iter()
            .any(|c| c.resource.ends_with(B))
    );
    h.change(|d| {
        d.compare_fail = false;
        d.paginate = true;
    });
    let repaired = c
        .repository_report("acme/demo", &[], false, Freshness::default())
        .await
        .unwrap();
    assert!(repaired.errors.is_empty());
    assert_eq!(repaired.branches[0].transition.old_sha.as_deref(), Some(A));
    assert_eq!(repaired.branches[0].transition.commits.len(), 2);
    assert!(
        c.changes(Some(&broken.cursor), 100)
            .await
            .unwrap()
            .changes
            .iter()
            .any(|c| c.resource.ends_with(C))
    );
    let cursor = repaired.cursor;
    h.change(|d| d.branch_unavailable = true);
    let unavailable = c
        .repository_report("acme/demo", &[], false, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(!unavailable.errors.is_empty());
    assert!(
        c.changes(Some(&cursor), 100)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    assert!(
        c.bootstrap()
            .await
            .unwrap()
            .snapshots
            .iter()
            .any(|s| s.resource.starts_with("branch://") && s.data["sha"] == B)
    );
}

#[tokio::test]
async fn required_checks_apply_apps_merge_precedence_strict_ancestry_and_permission_errors() {
    let h = Harness::new().await;
    let c = h.client();
    let ok = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(ok.state, "satisfied", "{:?}", ok.errors);
    assert_eq!(ok.checks[0].sha.as_deref(), Some(M));
    h.change(|d| d.merge_failure = true);
    let fail = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(fail.state, "failure");
    h.change(|d| {
        d.merge_failure = false;
        d.check_app = 99;
    });
    assert_eq!(
        c.required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .state,
        "missing"
    );
    h.change(|d| {
        d.check_app = 42;
        d.strict = true;
        d.up_to_date = false;
    });
    assert_eq!(
        c.required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .state,
        "pending"
    );
    h.change(|d|{d.up_to_date=true;d.rules=vec![json!({"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":true,"required_status_checks":[{"context":"external","integration_id":null}]}})];});
    let rules = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(rules.state, "satisfied");
    assert_eq!(rules.checks.len(), 2);
    h.change(|d| d.policy_fail = true);
    let unknown = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(unknown.state, "unknown");
    assert!(!unknown.errors.is_empty());
    assert_eq!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .data
            .summary
            .state,
        "success"
    );
    let policy_calls = h
        .mock
        .0
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|(path, _)| path.contains("/protection/required_status_checks"))
        .count();
    for _ in 0..2 {
        assert_eq!(
            c.required_checks_for_pr("acme/demo", 7, Freshness::default())
                .await
                .unwrap()
                .state,
            "unknown"
        );
    }
    assert_eq!(
        policy_calls,
        h.mock
            .0
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(path, _)| path.contains("/protection/required_status_checks"))
            .count()
    );
    h.change(|d| {
        d.policy_fail = false;
    });
    assert_eq!(
        c.required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .state,
        "satisfied"
    );
    assert_eq!(
        c.required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .state,
        "satisfied"
    );
    h.change(|d| {
        d.policy_fail = false;
        d.rules = vec![json!({"parameters":{}})];
    });
    let malformed = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(malformed.state, "unknown");
    assert!(malformed.errors.iter().any(|e| e.source == "rulesets"));
    let feed = c.changes(Some(&ok.cursor), 100).await.unwrap();
    assert!(
        feed.changes
            .iter()
            .any(|e| e.resource.starts_with("required_checks://"))
    );
}

#[tokio::test]
async fn reviews_and_failed_jobs_expose_actionable_cursor_updates() {
    let h = Harness::new().await;
    let c = h.client();
    h.change(|d|d.reviews=vec![json!({"id":1,"user":{"login":"Reviewer"},"state":"APPROVED","submitted_at":"2026-09-19T00:00:00Z"}),json!({"id":2,"user":{"login":"Reviewer"},"state":"COMMENTED","submitted_at":"2026-09-19T01:00:00Z"})]);
    let first = c
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(first.complete, "{:?}", first.data.errors);
    assert_eq!(first.data.review_events.len(), 2);
    assert_eq!(first.data.review_events[1]["id"], "ER2");
    assert_eq!(first.data.review_status.approved_by, vec!["Reviewer"]);
    assert_eq!(first.data.review_status.unresolved_threads, 1);
    assert_eq!(
        first.data.review_status.requested_reviewers[0]["login"],
        "reviewer"
    );
    h.change(|d| {
        d.reviews[0]["state"] = json!("DISMISSED");
        d.resolved = true;
        d.job_failure = true;
    });
    let changed = c
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(changed.data.review_status.approved_by.is_empty());
    assert_eq!(changed.data.review_status.dismissed_reviews.len(), 1);
    assert_eq!(changed.data.review_status.resolved_threads, 1);
    let job = changed
        .data
        .ci
        .failures
        .iter()
        .find(|r| r.kind == "job")
        .unwrap();
    assert_eq!(job.failed_steps[0]["name"], "unit tests");
    assert!(job.url.as_deref().unwrap().contains("/job/60"));
    let feed = c.changes(first.cursor.as_deref(), 100).await.unwrap();
    assert!(
        feed.changes
            .iter()
            .any(|e| e.resource.starts_with("review_status://"))
    );
    assert!(feed.changes.iter().any(|e| e.resource.starts_with("ci://")));
    h.change(|d| d.pr_head = D.into());
    assert_eq!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .data
            .head_sha,
        D
    );
}

#[tokio::test]
async fn repository_watch_sdk_api_persist_and_reject_invalid_refs() {
    let h = Harness::new().await;
    let c = h.client();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let sdk = hey_gh::ApiClient::new(origin.parse().unwrap()).unwrap();
    let watch = sdk
        .watch_repository("acme/demo", vec!["topic/one".into()], false, 10)
        .await
        .unwrap();
    assert_eq!(watch.kind, WatchKind::Branches);
    let report = sdk
        .repository_report(
            "acme/demo",
            &["topic/one".into()],
            false,
            Freshness::Revalidate,
        )
        .await
        .unwrap();
    assert!(report.errors.is_empty());
    assert_eq!(report.branches.len(), 2);
    assert_eq!(
        sdk.required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .state,
        "satisfied"
    );
    assert_eq!(
        h.client().watches().await.unwrap()[0].branches,
        vec!["topic/one"]
    );
    for bad in [
        "../escape",
        "bad..ref",
        "bad.lock",
        "bad name",
        "x@{y",
        ".hidden",
    ] {
        assert!(
            c.save_repository_watch("acme/demo", vec![bad.into()], false, 10)
                .await
                .is_err()
        );
    }
    sdk.unwatch(&watch.id).await.unwrap();
    assert!(c.watches().await.unwrap().is_empty());
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn repository_rosters_include_closed_prs_and_new_branch_creation() {
    let h = Harness::new().await;
    let c = h.client();
    let all = c
        .list_pull_requests("acme/demo", "all", Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1]["state"], "closed");
    assert!(all[1]["merged_at"].is_string());
    c.repository_report("acme/demo", &[], true, Freshness::Revalidate)
        .await
        .unwrap();
    h.change(|d| {
        d.refs.insert("new-branch".into(), B.into());
    });
    let next = c
        .repository_report("acme/demo", &[], true, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(
        next.branches
            .iter()
            .find(|b| b.branch == "new-branch")
            .unwrap()
            .transition
            .kind,
        "created"
    );
    let calls = h.mock.0.lock().unwrap().calls.len();
    assert_eq!(
        c.list_pull_requests("acme/demo", "all", Freshness::CachedOnly)
            .await
            .unwrap(),
        all
    );
    assert_eq!(calls, h.mock.0.lock().unwrap().calls.len());
    assert!(
        c.list_pull_requests("acme/demo", "invalid", Freshness::Revalidate)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn base_branch_updates_automatically_refresh_watched_pr_merge_ci_and_conflicts() {
    let h = Harness::new().await;
    let c = h.client();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    // Persist a PR watch without starting a separate PR loop: the repository
    // monitor itself must refresh its affected PR after observing a base update.
    c.save_watch("acme/demo", 7, 10).await.unwrap();
    let watch = api
        .watch_repository("acme/demo", Vec::new(), false, 10)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if c.bootstrap()
                .await
                .unwrap()
                .snapshots
                .iter()
                .any(|s| s.resource.starts_with("ci://"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let before = c.bootstrap().await.unwrap().cursor;
    h.change(|d| {
        d.refs.insert("main".into(), B.into());
        d.pr_merge = D.into();
        d.conflicting = true;
    });
    tokio::time::timeout(Duration::from_secs(13), async {
        loop {
            let snapshots = c.bootstrap().await.unwrap();
            let ci = snapshots
                .snapshots
                .iter()
                .any(|s| s.resource.starts_with("ci://") && s.data["merge_sha"] == D);
            let metadata = snapshots.snapshots.iter().any(|s| {
                s.resource.starts_with("metadata://") && s.data["conflicts"] == "conflicting"
            });
            if ci && metadata {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let feed = c.changes(Some(&before), 100).await.unwrap();
    assert!(
        feed.changes
            .iter()
            .any(|s| s.resource.starts_with("branch://"))
    );
    assert!(
        feed.changes
            .iter()
            .any(|s| s.resource.starts_with("ci://") && s.data["merge_sha"] == D)
    );
    assert!(
        h.mock
            .0
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(path, _)| path.contains(&format!("/commits/{D}/check-runs")))
    );
    c.delete_watch(&watch.id).await.unwrap();
    api.stop().await;
}
