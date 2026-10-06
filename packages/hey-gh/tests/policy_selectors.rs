use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use hey_gh::{Client, Config, Freshness};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MERGE: &str = "cccccccccccccccccccccccccccccccccccccccc";

#[path = "policy_selectors/branches.rs"]
mod branches;
#[path = "policy_selectors/ci.rs"]
mod ci;
#[path = "policy_selectors/rules.rs"]
mod rules;

#[path = "policy_selectors/ci_retry.rs"]
mod ci_retry;

#[path = "policy_selectors/merge_ref.rs"]
mod merge_ref;

#[path = "policy_selectors/conflicts.rs"]
mod conflicts;

fn metadata() -> Value {
    json!({"node_id":"PR_demo_7","number":7,"title":"REST title","state":"open","merged":false,"mergeable":true,
        "head":{"sha":HEAD},"base":{"ref":"main","sha":BASE,"repo":{"id":123,"node_id":"R_demo","full_name":"acme/demo"}},
        "merge_commit_sha":MERGE,"stack":null})
}

fn selectors() -> Value {
    json!({"data":{"repository":{"id":"R_demo","databaseId":123,"nameWithOwner":"acme/demo","pullRequest":{
        "id":"PR_demo_7","number":7,"state":"OPEN","merged":false,"mergeable":"MERGEABLE",
        "headRefOid":HEAD,"baseRefOid":BASE,"baseRefName":"main",
        "baseRepository":{"id":"R_demo","databaseId":123,"nameWithOwner":"acme/demo"},
        "potentialMergeCommit":{"oid":MERGE,"parents":{"totalCount":2,"nodes":[{"oid":BASE},{"oid":HEAD}]}},
        "stack":null,"stackEntry":null
    }}}})
}

struct Data {
    rest: Value,
    graph: Value,
    merge_ref: Value,
    deny_merge_ref: bool,
    missing_merge_ref: bool,
    stall_merge_ref: bool,
    branch: Value,
    branch_graph: Value,
    ci_graph: Value,
    deny_ci_app_metadata: bool,
    ci_graph_gate: Option<Arc<tokio::sync::Notify>>,
    policy_graph_gate: Option<Arc<tokio::sync::Notify>>,
    check_conclusion: &'static str,
    status_state: Option<&'static str>,
    branch_graph_gate: Option<Arc<tokio::sync::Notify>>,
    stall_branch: bool,
    rules: Value,
    deny_rules: bool,
    stall_rules: bool,
    stall_checks: bool,
    deny_checks: bool,
    merge_base: &'static str,
    deny_rest: bool,
    stall_rest: bool,
    rest_after_read: Option<Value>,
    stall_graph: bool,
    change_rest_on_checks: Option<Value>,
    deny_rest_on_checks: bool,
    checks_gate: Option<Arc<tokio::sync::Notify>>,
    calls: Vec<(String, Value)>,
    tokens: Vec<String>,
}

async fn handler(
    State(state): State<Arc<Mutex<Data>>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ((value, denied, stalled), gate) = {
        let mut s = state.lock().unwrap();
        let path = uri.path();
        s.tokens.push(
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        s.calls.push((
            path.into(),
            serde_json::from_slice(&body).unwrap_or(Value::Null),
        ));
        if path.ends_with("/git/ref/pull/7/merge") && s.missing_merge_ref {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(json!({"message":"Not Found"})),
            )
                .into_response();
        }
        let result = if path == "/app/installations/42/access_tokens" {
            (
                json!({"token":"synthetic-app-token","expires_at":"2099-01-01T00:00:00Z"}),
                false,
                false,
            )
        } else if path == "/graphql" {
            let query = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            let value = if query["query"]
                .as_str()
                .is_some_and(|q| q.contains("RequiredPolicyCi"))
            {
                if s.deny_ci_app_metadata
                    && query["query"]
                        .as_str()
                        .is_some_and(|q| q.contains("app { databaseId }"))
                {
                    json!({"errors":[{"type":"FORBIDDEN","message":"Resource not accessible by integration","path":["repository","head","checkSuites","nodes",0,"checkRuns","nodes",0,"checkSuite","app"]}]})
                } else {
                    s.ci_graph.clone()
                }
            } else if query["query"]
                .as_str()
                .is_some_and(|query| query.contains("RequiredPolicyBranch"))
            {
                s.branch_graph.clone()
            } else {
                s.graph.clone()
            };
            (value, false, s.stall_graph)
        } else if path.ends_with("/git/ref/pull/7/merge") {
            (s.merge_ref.clone(), s.deny_merge_ref, s.stall_merge_ref)
        } else if path.ends_with("/pulls/7") {
            let result = (s.rest.clone(), s.deny_rest, s.stall_rest);
            if let Some(next) = s.rest_after_read.take() {
                s.rest = next;
            }
            result
        } else if path.contains("/branches/") && !path.contains("/rules/") {
            (s.branch.clone(), false, s.stall_branch)
        } else if path.ends_with("/check-runs") {
            let sha = path.rsplit('/').nth(1).unwrap();
            if sha != HEAD && sha != MERGE {
                if let Some(next) = s.change_rest_on_checks.take() {
                    s.rest = next;
                }
                s.deny_rest |= s.deny_rest_on_checks;
            }
            (
                json!({"total_count":1,"check_runs":[{"id":sha.as_bytes()[0],"node_id":format!("CR_{}",sha.as_bytes()[0]),"name":"tests","app":{"id":1},"check_suite":{"id":sha.as_bytes()[0]},"head_sha":sha,"status":"completed","conclusion":s.check_conclusion,"started_at":null,"completed_at":null,"details_url":null}]}),
                s.deny_checks,
                s.stall_checks,
            )
        } else if path.ends_with("/status") {
            let sha = path.rsplit('/').nth(1).unwrap();
            let statuses=s.status_state.map(|state|json!({"id":u64::from(sha.as_bytes()[0])+100,"node_id":format!("S_{}",sha.as_bytes()[0]),"context":"deploy","state":state,"updated_at":"2026-10-01T00:00:00Z","target_url":null})).into_iter().collect::<Vec<_>>();
            (
                json!({"sha":sha,"total_count":statuses.len(),"statuses":statuses}),
                s.deny_checks,
                s.stall_checks,
            )
        } else if path.contains("/rules/branches/") {
            (s.rules.clone(), s.deny_rules, s.stall_rules)
        } else if path.contains("/compare/") {
            (
                json!({"merge_base_commit":{"sha":s.merge_base}}),
                false,
                false,
            )
        } else {
            (json!([]), false, false)
        };
        let branch_query = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
        let gate = if branch_query["query"]
            .as_str()
            .is_some_and(|q| q.contains("RequiredPolicyCi"))
        {
            s.ci_graph_gate.take()
        } else if branch_query["query"]
            .as_str()
            .is_some_and(|query| query.contains("RequiredPolicyBranch"))
        {
            s.branch_graph_gate.take()
        } else if branch_query["query"]
            .as_str()
            .is_some_and(|query| query.contains("RequiredPolicySelectors"))
        {
            s.policy_graph_gate.take()
        } else if path.ends_with("/check-runs")
            && path
                .rsplit('/')
                .nth(1)
                .is_some_and(|sha| sha != HEAD && sha != MERGE)
        {
            s.checks_gate.take()
        } else {
            None
        };
        (result, gate)
    };
    if let Some(gate) = gate {
        gate.notified().await;
    }
    if stalled {
        std::future::pending::<()>().await;
    }
    if denied {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"message":"metadata inaccessible"})),
        )
            .into_response();
    }
    let mut response = axum::Json(value).into_response();
    if uri.path() == "/app/installations/42/access_tokens" {
        *response.status_mut() = StatusCode::CREATED;
    }
    if uri.path().starts_with("/pace-graphql") {
        let headers = response.headers_mut();
        headers.insert("x-ratelimit-resource", "graphql".parse().unwrap());
        headers.insert(
            "x-ratelimit-remaining",
            if uri.path().ends_with("-short") {
                "10000"
            } else {
                "1000"
            }
            .parse()
            .unwrap(),
        );
        let reset = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        headers.insert("x-ratelimit-reset", reset.to_string().parse().unwrap());
    }
    response
}

struct Fixture {
    client: Client,
    data: Arc<Mutex<Data>>,
    dir: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    fn terminal(&self, merged: bool) {
        let mut data = self.data.lock().unwrap();
        data.rest["state"] = json!("closed");
        data.rest["merged"] = json!(merged);
        let node = &mut data.graph["data"]["repository"]["pullRequest"];
        node["state"] = json!(if merged { "MERGED" } else { "CLOSED" });
        node["merged"] = json!(merged);
        if merged {
            node["mergeable"] = json!("UNKNOWN");
            node["potentialMergeCommit"] = Value::Null;
            node["mergeCommit"] = json!({"oid":MERGE});
            data.rest["mergeable"] = Value::Null;
        }
    }

    async fn new() -> Self {
        Self::with_installation(false).await
    }

    async fn with_installation(installation: bool) -> Self {
        let data = Arc::new(Mutex::new(Data {
            rest: metadata(),
            graph: selectors(),
            merge_ref: json!({"ref":"refs/pull/7/merge","object":{"type":"commit","sha":MERGE}}),
            deny_merge_ref: false,
            missing_merge_ref: false,
            stall_merge_ref: false,
            branch: json!({"commit":{"sha":BASE},"protected":false,"protection":{"enabled":false,"required_status_checks":{"enforcement_level":"off","contexts":[],"checks":[]}}}),
            branch_graph: Value::Null,
            ci_graph: Value::Null,
            deny_ci_app_metadata: false,
            ci_graph_gate: None,
            policy_graph_gate: None,
            check_conclusion: "success",
            status_state: None,
            branch_graph_gate: None,
            stall_branch: false,
            rules: json!([{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[{"context":"tests","integration_id":1}]}}]),
            deny_rules: false,
            stall_rules: false,
            stall_checks: false,
            deny_checks: false,
            merge_base: BASE,
            deny_rest: false,
            stall_rest: false,
            rest_after_read: None,
            stall_graph: false,
            change_rest_on_checks: None,
            deny_rest_on_checks: false,
            checks_gate: None,
            calls: vec![],
            tokens: vec![],
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            Config {
                installation: installation.then(|| {
                    hey_gh::AppInstallation::new(
                        "synthetic-client".into(),
                        42,
                        vec!["acme/demo".into()],
                        include_str!("fixtures/github-app-test-key.pem"),
                    )
                    .unwrap()
                }),
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::ZERO,
                queue_timeout: Duration::from_secs(5),
                report_timeout: Duration::from_secs(5),
                max_attempts: 1,
                ..Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let router = Router::new().fallback(handler).with_state(data.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            client,
            data,
            dir,
            server,
        }
    }

    async fn seed(&self) -> u64 {
        let report = self
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied", "{:?}", report.errors);
        let old = report.observed_at_ms.unwrap() - 120_000;
        rusqlite::Connection::open(self.dir.path().join("cache.sqlite")).unwrap().execute(
            "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/pulls/7'", [old]
        ).unwrap();
        self.data.lock().unwrap().calls.clear();
        self.data.lock().unwrap().tokens.clear();
        old
    }

    fn cache_ci_seed(&self, data: &Value, clock: u64) {
        let db = rusqlite::Connection::open(self.dir.path().join("cache.sqlite")).unwrap();
        assert_eq!(db.execute(
            "INSERT OR REPLACE INTO cache(scope,key,response) SELECT scope,key||'#installation-ci-pr',json_set(response,'$.data',json(?1),'$.validated_at_ms',?2) FROM cache WHERE key LIKE '%/pulls/7'",
            rusqlite::params![data.to_string(), clock],
        ).unwrap(), 1);
    }
}

#[tokio::test]
async fn terminal_policy_confirms_exact_lifecycle_and_merge_without_waiting_for_rest() {
    for merged in [false, true] {
        let f = Fixture::new().await;
        f.terminal(merged);
        let old = f.seed().await;
        f.data.lock().unwrap().stall_rest = true;
        let report = tokio::time::timeout(
            Duration::from_secs(1),
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::default()),
        )
        .await
        .expect("terminal policy confirmation waited for REST")
        .unwrap();
        assert_eq!(report.state, "satisfied");
        assert_eq!(report.pull_request_state.as_deref(), Some("closed"));
        assert_eq!(report.merge_sha.as_deref(), Some(MERGE));
        assert!(
            report
                .validations
                .iter()
                .any(|v| v.resource.ends_with("/graphql") && v.validated_at_ms > old)
        );
        let cached = f
            .client
            .pull_request("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert_eq!(
            cached.validated_at_ms, old,
            "selector confirmation cannot freshen the REST body"
        );
        let data = f.data.lock().unwrap();
        assert_eq!(data.calls.len(), 1, "{:?}", data.calls);
        assert_eq!(data.calls[0].0, "/graphql");
        assert!(data.tokens.iter().all(|t| t == "Bearer synthetic-token"));
    }
}

#[tokio::test]
async fn terminal_selector_changes_and_incomplete_merge_evidence_require_rest() {
    for merged in [false, true] {
        for field in [
            "state",
            "merged",
            "headRefOid",
            "baseRefOid",
            "merge",
            "missing_merge",
            "stack",
            "id",
        ] {
            let f = Fixture::new().await;
            f.terminal(merged);
            f.seed().await;
            {
                let mut data = f.data.lock().unwrap();
                data.deny_rest = true;
                let node = &mut data.graph["data"]["repository"]["pullRequest"];
                let merge_field = if merged {
                    "mergeCommit"
                } else {
                    "potentialMergeCommit"
                };
                match field {
                    "state" => node["state"] = json!("OPEN"),
                    "merged" => node["merged"] = json!(!merged),
                    "headRefOid" | "baseRefOid" => node[field] = json!(MERGE),
                    "merge" => node[merge_field]["oid"] = json!(HEAD),
                    "missing_merge" => {
                        node.as_object_mut().unwrap().remove(merge_field);
                    }
                    "stack" => node["stack"] = json!({"id":"native"}),
                    "id" => node["id"] = json!("PR_replaced"),
                    _ => unreachable!(),
                }
            }
            assert!(
                matches!(
                    f.client
                        .required_checks_for_pr("acme/demo", 7, Freshness::default())
                        .await,
                    Err(hey_gh::Error::GitHub { status: 403, .. })
                ),
                "merged={merged}, field={field}"
            );
            let data = f.data.lock().unwrap();
            assert_eq!(
                data.calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
                ["/graphql", "/repos/acme/demo/pulls/7"],
                "merged={merged}, field={field}"
            );
        }
    }
}

#[tokio::test]
async fn terminal_policy_retains_forced_rest_and_offline_freshness() {
    for merged in [false, true] {
        let f = Fixture::new().await;
        f.terminal(merged);
        let old = f.seed().await;
        let offline = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert_eq!(offline.state, "satisfied");
        assert_eq!(offline.oldest_validation_at_ms, Some(old));
        assert!(offline.pull_request_confirmation.is_none());
        assert!(f.data.lock().unwrap().calls.is_empty());
        for freshness in [Freshness::Revalidate, Freshness::MaxAge(Duration::ZERO)] {
            f.data.lock().unwrap().calls.clear();
            let report = f
                .client
                .required_checks_for_pr("acme/demo", 7, freshness)
                .await
                .unwrap();
            assert_eq!(report.state, "satisfied");
            let data = f.data.lock().unwrap();
            assert_eq!(
                data.calls
                    .iter()
                    .filter(|c| c.0.ends_with("/pulls/7"))
                    .count(),
                2
            );
            assert!(data.calls.iter().all(|c| c.0 != "/graphql"));
        }
    }
}

#[tokio::test]
async fn empty_policy_finishes_without_collecting_or_publishing_ci() {
    for installation in [false, true] {
        let f = Fixture::with_installation(installation).await;
        {
            let mut data = f.data.lock().unwrap();
            data.rules = json!([{"type":"pull_request"}]);
            data.stall_checks = true;
        }
        for freshness in [Freshness::Revalidate, Freshness::CachedOnly] {
            let report = tokio::time::timeout(
                Duration::from_secs(2),
                f.client.required_checks_for_pr("acme/demo", 7, freshness),
            )
            .await
            .expect("empty required-check policy waited for unrelated CI")
            .unwrap();
            assert_eq!(report.state, "not_required", "{:?}", report.errors);
            assert_eq!(report.head_sha, HEAD);
            assert_eq!(report.merge_sha.as_deref(), Some(MERGE));
            assert!(report.checks.is_empty());
            assert!(
                !report
                    .validations
                    .iter()
                    .any(|v| v.resource.contains("/commits/"))
            );
        }
        assert!(
            !f.data
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|(path, _)| { path.contains("/commits/") || path.contains("/access_tokens") })
        );
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        let ci_rows: u64 = db
            .query_row(
                "SELECT count(*) FROM snapshots WHERE resource LIKE 'ci://%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            ci_rows, 0,
            "policy absence must not publish a fabricated CI result"
        );
    }
}

#[tokio::test]
async fn empty_policy_still_checks_strict_ancestry_and_revalidates_new_requirements() {
    let f = Fixture::new().await;
    let required = f.data.lock().unwrap().rules.clone();
    for merge_base in [HEAD, BASE] {
        {
            let mut data = f.data.lock().unwrap();
            data.rules = json!([{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":true,"required_status_checks":[]}}]);
            data.merge_base = merge_base;
            data.stall_checks = true;
        }
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert!(report.strict);
        assert_eq!(report.up_to_date, Some(merge_base == BASE));
        assert_eq!(
            report.state,
            if merge_base == BASE {
                "not_required"
            } else {
                "pending"
            }
        );
        assert!(
            !f.data
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|(path, _)| path.contains("/commits/"))
        );
    }
    {
        let mut data = f.data.lock().unwrap();
        data.rules = required;
        data.stall_checks = false;
    }
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(report.state, "satisfied", "{:?}", report.errors);
    assert_eq!(report.checks.len(), 1);
    assert!(
        f.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(path, _)| path.ends_with("/check-runs"))
    );
    for denied in [false, true] {
        {
            let mut data = f.data.lock().unwrap();
            data.deny_rules = denied;
            data.rules = json!([{"type":"required_status_checks","parameters":{}}]);
        }
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert_eq!(report.state, "unknown");
        assert!(report.errors.iter().any(|error| error.source == "rulesets"));
    }
}

#[tokio::test]
async fn uncertain_merge_policy_seeds_refresh_before_collecting_obsolete_checks() {
    for installation in [false, true] {
        for kind in ["unknown", "conflicting"] {
            let f = Fixture::with_installation(installation).await;
            let mut seed = metadata();
            match kind {
                "unknown" => seed["mergeable"] = Value::Null,
                "conflicting" => seed["mergeable"] = json!(false),
                _ => unreachable!(),
            }
            f.data.lock().unwrap().rest = seed.clone();
            let old = f.seed().await;
            // Both caches agree. The newer-CI mismatch shortcut cannot help.
            if installation {
                f.cache_ci_seed(&seed, old + 60_000);
            }
            let merge = "dddddddddddddddddddddddddddddddddddddddd";
            f.data.lock().unwrap().rest["merge_commit_sha"] = json!(merge);
            let report = f
                .client
                .required_checks_for_pr("acme/demo", 7, Freshness::default())
                .await
                .unwrap();
            assert_eq!(report.state, "satisfied", "{kind}");
            assert_eq!(report.merge_sha.as_deref(), Some(merge));
            assert!(
                report
                    .checks
                    .iter()
                    .all(|check| check.sha.as_deref() == Some(merge))
            );
            let data = f.data.lock().unwrap();
            let reads: Vec<_> = data
                .calls
                .iter()
                .enumerate()
                .filter(|(_, call)| call.0.ends_with("/pulls/7"))
                .collect();
            assert_eq!(
                reads.len(),
                1,
                "{kind}: collecting old selectors caused a second metadata read: {:?}",
                data.calls
            );
            assert_eq!(reads[0].0, 0, "metadata must precede commit collection");
            assert_eq!(data.tokens[0], "Bearer synthetic-token");
            assert!(!data.calls.iter().any(|call| call.0 == "/graphql"));
        }
    }
}

#[tokio::test]
async fn early_rest_seed_expiry_still_confirms_and_recollects_a_changed_merge() {
    for (age, expired_ms) in [(30, 16_000), (1, 2_000)] {
        let f = Fixture::new().await;
        f.data.lock().unwrap().rest["mergeable"] = Value::Null;
        f.seed().await;
        let merge = "dddddddddddddddddddddddddddddddddddddddd";
        let changed_merge = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let gate = Arc::new(tokio::sync::Notify::new());
        {
            let mut data = f.data.lock().unwrap();
            data.rest["merge_commit_sha"] = json!(merge);
            let mut changed = data.rest.clone();
            changed["merge_commit_sha"] = json!(changed_merge);
            data.change_rest_on_checks = Some(changed);
            data.checks_gate = Some(gate.clone());
        }
        let reader = tokio::spawn({
            let client = f.client.clone();
            async move {
                client
                    .required_checks_for_pr(
                        "acme/demo",
                        7,
                        Freshness::MaxAge(Duration::from_secs(age)),
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if f.data
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .any(|call| call.0 == format!("/repos/acme/demo/commits/{merge}/check-runs"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap().execute(
            "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/pulls/7'", [now - expired_ms]
        ).unwrap();
        gate.notify_one();
        let report = reader.await.unwrap().unwrap();
        assert_eq!(report.state, "satisfied");
        assert_eq!(report.merge_sha.as_deref(), Some(changed_merge));
        assert!(
            report
                .checks
                .iter()
                .all(|check| check.sha.as_deref() == Some(changed_merge))
        );
        assert_eq!(
            f.data
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|call| call.0.ends_with("/pulls/7"))
                .count(),
            3,
            "early read, expired final confirmation, and changed-merge retry must each validate"
        );
    }
}

#[tokio::test]
async fn newer_cached_ci_selectors_refresh_the_personal_policy_seed_before_collection() {
    let f = Fixture::with_installation(true).await;
    let old = f.seed().await;
    let merge = "dddddddddddddddddddddddddddddddddddddddd";
    let mut next = metadata();
    next["merge_commit_sha"] = json!(merge);
    next["mergeable"] = Value::Null;
    f.cache_ci_seed(&next, old + 60_000);
    {
        let mut data = f.data.lock().unwrap();
        data.rest = next;
        data.graph["data"]["repository"]["pullRequest"]["mergeable"] = json!("UNKNOWN");
        data.graph["data"]["repository"]["pullRequest"]["potentialMergeCommit"] = Value::Null;
    }
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "satisfied");
    assert_eq!(report.merge_sha.as_deref(), Some(merge));
    let data = f.data.lock().unwrap();
    assert_eq!(
        data.calls[0].0, "/repos/acme/demo/pulls/7",
        "known obsolete selectors must refresh before collecting"
    );
    let metadata_reads: Vec<_> = data
        .calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.0.ends_with("/pulls/7"))
        .collect();
    assert_eq!(
        metadata_reads.len(),
        1,
        "avoid a forced second confirmation after collecting a known obsolete merge"
    );
    assert_eq!(data.tokens[metadata_reads[0].0], "Bearer synthetic-token");
    assert!(!data.calls.iter().any(|c| c.0 == "/graphql"));
}

#[tokio::test]
async fn policy_seed_hints_do_not_replace_personal_evidence_or_change_offline_reads() {
    let f = Fixture::with_installation(true).await;
    let old = f.seed().await;
    let mut next = metadata();
    next["merge_commit_sha"] = json!("dddddddddddddddddddddddddddddddddddddddd");
    f.cache_ci_seed(&next, old + 60_000);
    f.data.lock().unwrap().deny_rest = true;
    let offline = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap_err();
    assert!(matches!(offline, hey_gh::Error::CacheMiss));
    assert!(f.data.lock().unwrap().calls.is_empty());
    let error = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap_err();
    assert!(matches!(error, hey_gh::Error::GitHub { status: 403, .. }));
    let cached = f
        .client
        .get("repos/acme/demo/pulls/7", Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cached.data, metadata());
    assert_eq!(cached.validated_at_ms, old);
    let data = f.data.lock().unwrap();
    assert_eq!(data.calls.len(), 1);
    assert_eq!(data.calls[0].0, "/repos/acme/demo/pulls/7");
    assert_eq!(data.tokens[0], "Bearer synthetic-token");
}

#[tokio::test]
async fn older_future_or_unrelated_ci_metadata_does_not_force_a_personal_refresh() {
    for kind in ["older", "future", "unrelated", "no_app"] {
        let f = Fixture::with_installation(kind != "no_app").await;
        let old = f.seed().await;
        let mut next = metadata();
        let clock = match kind {
            "older" => old - 1,
            "future" => old + 3_600_000,
            _ => old + 60_000,
        };
        if kind == "unrelated" {
            next["title"] = json!("Updated title");
            next["updated_at"] = json!("2026-10-05T00:00:00Z");
            // REST can omit absent native membership instead of using null.
            next.as_object_mut().unwrap().remove("stack");
        } else {
            next["merge_commit_sha"] = json!("dddddddddddddddddddddddddddddddddddddddd");
        }
        f.cache_ci_seed(&next, clock);
        f.data.lock().unwrap().deny_rest = true;
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied", "{kind}");
        assert_eq!(report.merge_sha.as_deref(), Some(MERGE));
        let data = f.data.lock().unwrap();
        assert_eq!(data.calls.len(), 1, "{kind}: {:?}", data.calls);
        assert_eq!(data.calls[0].0, "/graphql");
        assert_eq!(data.tokens[0], "Bearer synthetic-token");
    }
}

#[tokio::test]
async fn standalone_policy_confirms_selectors_without_freshening_or_waiting_for_rest() {
    let f = Fixture::new().await;
    let old = f.seed().await;
    let before = f
        .client
        .bootstrap()
        .await
        .unwrap()
        .snapshots
        .into_iter()
        .find(|s| s.resource.starts_with("metadata://"))
        .unwrap();
    f.data.lock().unwrap().stall_rest = true;
    let report = tokio::time::timeout(
        Duration::from_millis(750),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30))),
    )
    .await
    .expect("standalone selector validation waited for REST metadata")
    .unwrap();
    assert_eq!(report.state, "satisfied");
    let confirmation = serde_json::to_value(&report).unwrap()["pull_request_confirmation"].clone();
    assert_eq!(confirmation["selectors"]["mergeable"], true);
    assert_eq!(confirmation["selectors"]["head"]["sha"], HEAD);
    assert!(confirmation["validated_at_ms"].as_u64().unwrap() > old);
    assert!(
        confirmation["selectors"].get("title").is_none(),
        "Selector confirmation is not a fresh REST body"
    );
    assert!(report.errors.is_empty());
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.ends_with("/graphql") && v.validated_at_ms > old)
    );
    assert!(
        report
            .validations
            .iter()
            .all(|v| !v.resource.ends_with("/pulls/7")),
        "selector validation cannot freshen the full REST resource"
    );
    let cached = f
        .client
        .pull_request("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cached.validated_at_ms, old);
    assert_eq!(cached.data, metadata());
    let after = f
        .client
        .bootstrap()
        .await
        .unwrap()
        .snapshots
        .into_iter()
        .find(|s| s.resource == before.resource)
        .unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap(),
        "selector validation must not republish the full REST observation"
    );
    let s = f.data.lock().unwrap();
    assert_eq!(s.calls.len(), 1);
    assert_eq!(s.calls[0].0, "/graphql");
    assert_eq!(
        s.calls[0].1["variables"],
        json!({"owner":"acme","repo":"demo","number":7})
    );
}

#[tokio::test]
async fn changed_or_incomplete_graphql_selectors_require_an_independent_rest_confirmation() {
    let prefix = "/data/repository/pullRequest";
    for (field, value) in [
        ("id", json!("PR_replaced")),
        ("number", json!(8)),
        ("state", json!("CLOSED")),
        ("merged", json!(true)),
        ("mergeable", json!("UNKNOWN")),
        ("headRefOid", json!(BASE)),
        ("baseRefOid", json!(HEAD)),
        ("baseRefName", json!("release")),
        ("baseRepository/id", json!("R_other")),
        ("baseRepository/databaseId", json!(456)),
        ("baseRepository/nameWithOwner", json!("acme/other")),
        ("potentialMergeCommit/oid", json!(HEAD)),
        ("potentialMergeCommit/parents/totalCount", json!(1)),
        (
            "potentialMergeCommit/parents/nodes",
            json!([{"oid":BASE},{"oid":MERGE}]),
        ),
        (
            "potentialMergeCommit/parents/nodes",
            json!([{"oid":"invalid"},{"oid":HEAD}]),
        ),
        ("stack", json!({"id":"S_native"})),
        ("stackEntry", json!({"id":"SE_native"})),
    ] {
        let f = Fixture::new().await;
        f.seed().await;
        {
            let mut s = f.data.lock().unwrap();
            *s.graph.pointer_mut(&format!("{prefix}/{field}")).unwrap() = value;
            s.deny_rest = true;
        }
        assert!(
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
                .await
                .is_err(),
            "accepted changed {field}"
        );
        let calls = &f.data.lock().unwrap().calls;
        assert_eq!(
            calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            vec!["/graphql", "/repos/acme/demo/pulls/7"],
            "{field}"
        );
    }
    for field in [
        "stack",
        "stackEntry",
        "baseRepository",
        "potentialMergeCommit",
    ] {
        let f = Fixture::new().await;
        f.seed().await;
        {
            let mut s = f.data.lock().unwrap();
            s.graph
                .pointer_mut(prefix)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            s.deny_rest = true;
        }
        assert!(
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
                .await
                .is_err(),
            "accepted missing {field}"
        );
    }
}

#[tokio::test]
async fn graphql_partial_permission_errors_cannot_use_matching_data_or_fall_back_around_denial() {
    let f = Fixture::new().await;
    f.seed().await;
    f.data.lock().unwrap().graph["errors"] =
        json!([{"type":"FORBIDDEN","message":"repository access denied"}]);
    assert!(matches!(
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await,
        Err(hey_gh::Error::GraphQL {
            access_denied: true,
            ..
        })
    ));
    let s = f.data.lock().unwrap();
    assert_eq!(s.calls.len(), 1);
    assert_eq!(s.calls[0].0, "/graphql");
}

#[tokio::test]
async fn selector_change_recollects_the_new_head_before_returning() {
    let f = Fixture::new().await;
    f.seed().await;
    let head = "dddddddddddddddddddddddddddddddddddddddd";
    let merge = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    {
        let mut s = f.data.lock().unwrap();
        s.rest["head"]["sha"] = json!(head);
        s.rest["merge_commit_sha"] = json!(merge);
        let node = &mut s.graph["data"]["repository"]["pullRequest"];
        node["headRefOid"] = json!(head);
        node["potentialMergeCommit"]["oid"] = json!(merge);
        node["potentialMergeCommit"]["parents"]["nodes"][1]["oid"] = json!(head);
    }
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert_eq!(report.state, "satisfied");
    assert_eq!(report.head_sha, head);
    assert_eq!(report.merge_sha.as_deref(), Some(merge));
    assert!(
        report
            .checks
            .iter()
            .all(|c| c.sha.as_deref() == Some(merge)),
        "recollected checks used an old commit: {:?}",
        report.checks
    );
    let calls = &f.data.lock().unwrap().calls;
    assert_eq!(calls.iter().filter(|c| c.0 == "/graphql").count(), 1);
    assert_eq!(
        calls.iter().filter(|c| c.0.ends_with("/pulls/7")).count(),
        2,
        "the confirmation that discovers a new head is already a usable retry seed"
    );
}

#[tokio::test]
async fn a_reused_retry_seed_still_requires_final_metadata_confirmation() {
    for denied in [false, true] {
        let f = Fixture::new().await;
        f.seed().await;
        let head = "dddddddddddddddddddddddddddddddddddddddd";
        let merge = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        {
            let mut s = f.data.lock().unwrap();
            s.rest["head"]["sha"] = json!(head);
            s.rest["merge_commit_sha"] = json!(merge);
            s.graph["data"]["repository"]["pullRequest"]["headRefOid"] = json!(head);
            if denied {
                s.deny_rest_on_checks = true;
            } else {
                // A second head change occurs while the retry's CI is read.
                // It cannot certify either the old or new head without a
                // further collection, beyond this report's retry bound.
                s.change_rest_on_checks = Some(metadata());
            }
        }
        let error = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap_err();
        if denied {
            assert!(matches!(error, hey_gh::Error::GitHub { status: 403, .. }));
        } else {
            assert!(
                matches!(error, hey_gh::Error::Invalid(ref message) if message.contains("changed repeatedly"))
            );
        }
        let s = f.data.lock().unwrap();
        let metadata_reads: Vec<_> = s
            .calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.0.ends_with("/pulls/7"))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(metadata_reads.len(), 2);
        assert!(
            s.calls[metadata_reads[0] + 1..metadata_reads[1]]
                .iter()
                .any(|call| call.0 == format!("/repos/acme/demo/commits/{head}/check-runs")),
            "retry must recollect its new head before the final confirmation"
        );
    }
}

#[tokio::test]
async fn selector_cache_obeys_both_the_completion_bound_and_a_stricter_caller_age() {
    let f = Fixture::new().await;
    f.seed().await;
    f.data.lock().unwrap().stall_rest = true;
    for _ in 0..2 {
        assert_eq!(
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
                .await
                .unwrap()
                .state,
            "satisfied"
        );
    }
    assert_eq!(
        f.data.lock().unwrap().calls.len(),
        1,
        "fresh selectors should be shared"
    );
    for (elapsed, age) in [(16_000, 30), (2_000, 1)] {
        let old = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - elapsed;
        rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap().execute(
            "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/graphql#%'",[old]
        ).unwrap();
        let before = f.data.lock().unwrap().calls.len();
        assert_eq!(
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(age)))
                .await
                .unwrap()
                .state,
            "satisfied"
        );
        assert_eq!(
            f.data.lock().unwrap().calls.len(),
            before + 1,
            "stale selectors were reused"
        );
    }
}

#[tokio::test]
async fn paced_optional_graphql_falls_back_without_consuming_its_timeout() {
    for denied in [false, true] {
        let f = Fixture::new().await;
        let old = f.seed().await;
        f.client
            .get("pace-graphql", Freshness::Revalidate)
            .await
            .unwrap();
        f.data.lock().unwrap().deny_rest = denied;
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            f.client.required_checks_for_pr(
                "acme/demo",
                7,
                Freshness::MaxAge(Duration::from_secs(30)),
            ),
        )
        .await
        .expect("known GraphQL pacing must not consume the optional two-second wait");
        if denied {
            assert!(matches!(
                result,
                Err(hey_gh::Error::GitHub { status: 403, .. })
            ));
        } else {
            let report = result.unwrap();
            assert_eq!(report.state, "satisfied");
            assert!(
                report
                    .validations
                    .iter()
                    .any(|v| v.resource.ends_with("/pulls/7") && v.validated_at_ms > old)
            );
            assert!(
                report
                    .validations
                    .iter()
                    .all(|v| !v.resource.ends_with("/graphql"))
            );
        }
        let calls = f.data.lock().unwrap().calls.clone();
        assert!(!calls.iter().any(|(path, _)| path == "/graphql"));
        assert_eq!(
            calls
                .iter()
                .filter(|(path, _)| path.ends_with("/pulls/7"))
                .count(),
            1
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while f.client.status().outstanding_requests != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn short_graphql_pacing_still_uses_selectors_without_spending_rest_quota() {
    let f = Fixture::new().await;
    let old = f.seed().await;
    f.client
        .get("pace-graphql-short", Freshness::Revalidate)
        .await
        .unwrap();
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert_eq!(report.state, "satisfied");
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.ends_with("/graphql") && v.validated_at_ms > old)
    );
    assert!(
        !f.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(path, _)| path.ends_with("/pulls/7"))
    );
}

#[tokio::test]
async fn stalled_graphql_falls_back_to_real_rest_evidence_within_the_report_budget() {
    let f = Fixture::new().await;
    let old = f.seed().await;
    f.data.lock().unwrap().stall_graph = true;
    let report = tokio::time::timeout(
        Duration::from_secs(3),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30))),
    )
    .await
    .expect("optional GraphQL validation consumed the report deadline")
    .unwrap();
    assert_eq!(report.state, "satisfied");
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.ends_with("/pulls/7") && v.validated_at_ms > old)
    );
    assert!(
        report
            .validations
            .iter()
            .all(|v| !v.resource.ends_with("/graphql"))
    );
    assert!(
        f.client
            .pull_request("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap()
            .validated_at_ms
            > old
    );
}

#[tokio::test]
async fn explicit_offline_native_and_already_fresh_rest_reads_keep_their_existing_source() {
    for freshness in [
        Freshness::Revalidate,
        Freshness::MaxAge(Duration::ZERO),
        Freshness::CachedOnly,
    ] {
        let f = Fixture::new().await;
        f.seed().await;
        f.data.lock().unwrap().deny_rest = true;
        let result = f
            .client
            .required_checks_for_pr("acme/demo", 7, freshness)
            .await;
        if matches!(freshness, Freshness::CachedOnly) {
            let report = result.unwrap();
            assert_eq!(report.state, "satisfied");
            assert!(report.pull_request_confirmation.is_none());
        } else {
            assert!(result.is_err());
        }
        assert!(
            f.data
                .lock()
                .unwrap()
                .calls
                .iter()
                .all(|c| c.0 != "/graphql")
        );
    }
    let f = Fixture::new().await;
    f.data.lock().unwrap().rest["stack"] =
        json!({"id":1,"number":1,"position":1,"size":2,"base":{"ref":"main","sha":BASE}});
    f.seed().await;
    f.data.lock().unwrap().deny_rest = true;
    assert!(
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .is_err()
    );
    assert!(
        f.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|c| c.0 != "/graphql")
    );

    let f = Fixture::new().await;
    f.seed().await;
    f.client
        .pull_request("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    f.data.lock().unwrap().calls.clear();
    assert_eq!(
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap()
            .state,
        "satisfied"
    );
    assert!(f.data.lock().unwrap().calls.is_empty());
}
