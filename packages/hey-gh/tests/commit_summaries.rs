use axum::{
    Json, Router,
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
const MERGE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[derive(Clone)]
struct Fixture {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    summary: Arc<Mutex<Value>>,
    tokens: Arc<Mutex<Vec<(String, String)>>>,
    delay: Arc<std::sync::atomic::AtomicU64>,
}

fn commit(sha: &str) -> Value {
    json!({"__typename":"Commit","oid":sha,"status":null,
        "checkSuites":{"totalCount":0,"pageInfo":{"hasNextPage":false},"nodes":[]}})
}

async fn handler(State(f): State<Fixture>, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    f.calls.lock().unwrap().push((uri.path().into(), body));
    f.tokens.lock().unwrap().push((
        uri.path().into(),
        headers["authorization"].to_str().unwrap().into(),
    ));
    if uri.path() == "/app/installations/42/access_tokens" {
        return (
            StatusCode::CREATED,
            Json(json!({"token":"synthetic-app-token","expires_at":"2099-01-01T00:00:00Z"})),
        )
            .into_response();
    }
    Json(if uri.path() == "/graphql" {
        tokio::time::sleep(Duration::from_millis(
            f.delay.load(std::sync::atomic::Ordering::Relaxed),
        ))
        .await;
        f.summary.lock().unwrap().clone()
    } else if uri.path().ends_with("/pulls/7") {
        json!({"number":7,"node_id":"PR_7","state":"closed","merged":true,"mergeable":null,
            "head":{"sha":HEAD},"base":{"sha":MERGE,"repo":{"full_name":"acme/demo"}},
            "merge_commit_sha":MERGE})
    } else if uri.path().ends_with("/check-runs") {
        json!({"total_count":0,"check_runs":[]})
    } else if uri.path().ends_with("/status") {
        json!({"total_count":0,"statuses":[]})
    } else if uri.path().ends_with("/actions/runs") {
        json!({"total_count":0,"workflow_runs":[]})
    } else {
        panic!("Unexpected request {uri}")
    })
    .into_response()
}

struct Harness {
    f: Fixture,
    config: Config,
    _dir: tempfile::TempDir,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Harness {
    async fn new() -> Self {
        let f = Fixture {
            calls: Default::default(),
            tokens: Default::default(),
            delay: Default::default(),
            summary: Arc::new(Mutex::new(json!({"data":{"repository":{
                "id":"R_demo","nameWithOwner":"acme/demo","head":commit(HEAD),"merge":commit(MERGE)
            }}}))),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new().fallback(handler).with_state(f.clone()),
            )
            .into_future(),
        );
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: dir.path().join("cache.sqlite"),
            min_spacing: Duration::ZERO,
            ..Config::default()
        };
        Self {
            f,
            config,
            _dir: dir,
            server,
        }
    }
    fn client(&self) -> Client {
        Client::with_token(self.config.clone(), "synthetic-token".into()).unwrap()
    }
    fn calls(&self) -> Vec<(String, Value)> {
        self.f.calls.lock().unwrap().clone()
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
        assert!(!rows.is_empty(), "{pattern}");
        for (key, raw) in rows {
            let mut v: Value = serde_json::from_str(&raw).unwrap();
            edit(&mut v);
            db.execute(
                "UPDATE cache SET response=?1 WHERE key=?2",
                [v.to_string(), key],
            )
            .unwrap();
        }
    }
}

#[tokio::test]
async fn closed_pr_batches_empty_commit_lists_and_reuses_the_proof_offline() {
    let h = Harness::new().await;
    let c = h.client();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    let calls = h.calls();
    assert_eq!(
        calls.iter().filter(|(path, _)| path == "/graphql").count(),
        1,
        "{calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|(p, _)| p.ends_with("/check-runs") || p.ends_with("/status")),
        "{calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(p, _)| p.ends_with("/actions/runs"))
            .count(),
        2
    );
    assert_eq!(
        calls.iter().find(|(p, _)| p == "/graphql").unwrap().1["variables"]["head"],
        HEAD
    );
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.contains("commit-lists"))
    );
    drop(c);
    let c = h.client();
    let offline = c
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(offline.complete, "{:?}", offline.data.errors);
    assert_eq!(h.calls().len(), calls.len());
}

#[tokio::test]
async fn incomplete_or_nonempty_suites_retain_rest_even_when_rollups_are_empty() {
    for case in [
        "paged",
        "count",
        "missing",
        "duplicate",
        "nonempty",
        "sha",
        "type",
        "repository",
        "null",
    ] {
        let h = Harness::new().await;
        {
            let mut v = h.f.summary.lock().unwrap();
            let repo = &mut v["data"]["repository"];
            repo["head"]["statusCheckRollup"] = Value::Null;
            match case {
                "paged" => repo["head"]["checkSuites"]["pageInfo"]["hasNextPage"] = json!(true),
                "count" => repo["head"]["checkSuites"]["totalCount"] = json!(1),
                "missing" => {
                    repo["head"].as_object_mut().unwrap().remove("checkSuites");
                }
                "duplicate" => {
                    repo["head"]["checkSuites"] = json!({"totalCount":2,"pageInfo":{"hasNextPage":false},"nodes":[{"id":"S","checkRuns":{"totalCount":0}},{"id":"S","checkRuns":{"totalCount":0}}]})
                }
                "nonempty" => {
                    repo["head"]["checkSuites"] = json!({"totalCount":1,"pageInfo":{"hasNextPage":false},"nodes":[{"id":"S","checkRuns":{"totalCount":1}}]})
                }
                "sha" => repo["head"]["oid"] = json!(MERGE),
                "type" => repo["head"]["__typename"] = json!("Tree"),
                "repository" => repo["nameWithOwner"] = json!("acme/other"),
                "null" => repo["head"] = Value::Null,
                _ => unreachable!(),
            }
        }
        let report = h
            .client()
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        if case == "nonempty" {
            assert!(
                !report.complete,
                "contradictory empty REST must remain unknown"
            );
        } else {
            assert!(report.complete, "{case}: {:?}", report.data.errors);
        }
        let calls = h.calls();
        assert!(
            calls
                .iter()
                .any(|(p, _)| p.contains(HEAD) && p.ends_with("/check-runs")),
            "{case}: {calls:?}"
        );
        assert_eq!(
            calls.iter().filter(|(p, _)| p == "/graphql").count(),
            1,
            "{case}"
        );
    }
}

#[tokio::test]
async fn malformed_status_and_partial_graphql_errors_never_prove_empty() {
    for case in ["missing", "empty-id", "partial-error"] {
        let h = Harness::new().await;
        {
            let mut v = h.f.summary.lock().unwrap();
            match case {
                "missing" => {
                    v["data"]["repository"]["head"]
                        .as_object_mut()
                        .unwrap()
                        .remove("status");
                }
                "empty-id" => v["data"]["repository"]["head"]["status"] = json!({"id":""}),
                "partial-error" => {
                    v["errors"] = json!([{"type":"INTERNAL","message":"unavailable","path":["repository","head","status"]}])
                }
                _ => unreachable!(),
            }
        }
        let report = h
            .client()
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(report.complete, "{case}: {:?}", report.data.errors);
        assert!(
            h.calls()
                .iter()
                .any(|(p, _)| p.contains(HEAD) && p.ends_with("/status")),
            "{case}"
        );
    }
}

#[tokio::test]
async fn denied_summary_does_not_synthesize_success_or_retry_through_personal_rest() {
    let h = Harness::new().await;
    *h.f.summary.lock().unwrap() =
        json!({"errors":[{"type":"FORBIDDEN","message":"access denied"}]});
    let report = h
        .client()
        .ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert!(!report.complete);
    assert!(
        report
            .data
            .errors
            .iter()
            .any(|e| e.message.contains("access denied"))
    );
    let calls = h.calls();
    assert_eq!(calls.iter().filter(|(p, _)| p == "/graphql").count(), 1);
    assert!(
        !calls
            .iter()
            .any(|(p, _)| p.ends_with("/check-runs") || p.ends_with("/status"))
    );
}

#[tokio::test]
async fn explicit_refresh_and_fresh_rest_do_not_spend_summary_queries() {
    let h = Harness::new().await;
    let c = h.client();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    assert!(!h.calls().iter().any(|(p, _)| p == "/graphql"));
}

#[tokio::test]
async fn newer_rest_failure_wins_over_an_older_empty_summary() {
    let h = Harness::new().await;
    let c = h.client();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    c.get(
        &format!("repos/acme/demo/commits/{HEAD}/check-runs?filter=latest&per_page=100"),
        Freshness::Revalidate,
    )
    .await
    .unwrap();
    h.edit_cache("%/graphql#%", |v| v["validated_at_ms"] = json!(1));
    h.edit_cache(&format!("%/{HEAD}/check-runs?%"),|v|v["data"]=json!({"total_count":1,"check_runs":[{"id":1,"name":"tests","head_sha":HEAD,"status":"completed","conclusion":"failure"}]}));
    let calls = h.calls().len();
    let offline = c
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(offline.complete);
    assert_eq!(offline.data.check_runs.len(), 1);
    assert_eq!(offline.data.summary.state, "failure");
    assert_eq!(h.calls().len(), calls);
}

#[tokio::test]
async fn unknown_future_and_retired_generation_proofs_remain_unknown_offline() {
    for case in ["zero", "future", "generation"] {
        let h = Harness::new().await;
        let c = h.client();
        assert!(
            c.ci_for_pr("acme/demo", 7, Freshness::default())
                .await
                .unwrap()
                .complete
        );
        if case == "generation" {
            let db = rusqlite::Connection::open(&h.config.cache_path).unwrap();
            db.execute("INSERT INTO repository_generation(scope,repository,generation) SELECT DISTINCT scope,'acme/demo',1 FROM cache",[]).unwrap();
        } else {
            h.edit_cache("%/graphql#%", |v| {
                v["validated_at_ms"] = json!(if case == "zero" { 0 } else { u64::MAX })
            });
        }
        let calls = h.calls().len();
        let result = c.ci_for_pr("acme/demo", 7, Freshness::CachedOnly).await;
        assert!(result.is_err() || !result.unwrap().complete, "{case}");
        assert_eq!(h.calls().len(), calls);
    }
}

#[tokio::test]
async fn a_stalled_optional_query_is_bounded_while_workflows_continue() {
    let h = Harness::new().await;
    h.f.delay.store(2300, std::sync::atomic::Ordering::Relaxed);
    let c = h.client();
    let read = tokio::spawn(async move { c.ci_for_pr("acme/demo", 7, Freshness::default()).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let calls = h.calls();
            if calls.iter().any(|(p, _)| p == "/graphql")
                && calls
                    .iter()
                    .filter(|(p, _)| p.ends_with("/actions/runs"))
                    .count()
                    == 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("workflow reads waited for the optional query");
    let report = tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(report.complete);
    assert!(h.calls().iter().any(|(p, _)| p.ends_with("/check-runs")));
}

#[tokio::test]
async fn fixed_ci_query_uses_installation_but_identical_generic_graphql_stays_personal() {
    let mut h = Harness::new().await;
    h.config.installation = Some(
        hey_gh::AppInstallation::new(
            "test-client".into(),
            42,
            vec!["acme/demo".into()],
            include_str!("fixtures/github-app-test-key.pem"),
        )
        .unwrap(),
    );
    let c = h.client();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    assert!(
        h.f.tokens
            .lock()
            .unwrap()
            .iter()
            .any(|(p, t)| p == "/graphql" && t == "Bearer synthetic-app-token")
    );
    let query = h
        .calls()
        .into_iter()
        .find(|(p, _)| p == "/graphql")
        .unwrap()
        .1;
    assert!(matches!(
        c.graphql(
            query["query"].as_str().unwrap(),
            query["variables"].clone(),
            Freshness::CachedOnly
        )
        .await,
        Err(hey_gh::Error::CacheMiss)
    ));
    c.graphql(
        query["query"].as_str().unwrap(),
        query["variables"].clone(),
        Freshness::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        h.f.tokens.lock().unwrap().last().unwrap().1,
        "Bearer synthetic-token"
    );
}
