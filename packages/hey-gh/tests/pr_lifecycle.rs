use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Uri},
    response::{IntoResponse, Response},
};
use hey_gh::{Client, Config, Freshness, PrLifecycleState};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Data {
    calls: Vec<(String, Value, String)>,
    mode: &'static str,
}
struct Fixture {
    client: Client,
    data: Arc<Mutex<Data>>,
    _dir: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn handler(
    State(data): State<Arc<Mutex<Data>>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let mut data = data.lock().unwrap();
    data.calls.push((
        uri.path().to_owned(),
        body.clone(),
        headers["authorization"].to_str().unwrap().to_owned(),
    ));
    assert_eq!(
        uri.path(),
        "/graphql",
        "Lifecycle batches must not issue per-PR REST or installation requests"
    );
    let repository = format!(
        "{}/{}",
        body["variables"]["owner"].as_str().unwrap(),
        body["variables"]["repo"].as_str().unwrap()
    );
    let repo_id = format!("R_{repository}");
    let mut repo = json!({"id":repo_id, "nameWithOwner":repository});
    let mut numbers: Vec<_> = body["variables"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix('n')
                .and_then(|s| s.parse::<usize>().ok())
                .map(|i| (i, v.as_u64().unwrap()))
        })
        .collect();
    numbers.sort_unstable();
    for (index, number) in numbers {
        let state = match number % 3 {
            0 => "MERGED",
            1 => "OPEN",
            _ => "CLOSED",
        };
        repo[format!("p{index}")] = json!({"id":format!("PR_{repository}_{number}"),"number":number,"state":state,"merged":state=="MERGED","title":format!("PR {number}"),"mergedAt":if state=="MERGED"{json!("2026-10-01T00:00:00Z")}else{Value::Null},"closedAt":if state!="OPEN"{json!("2026-10-01T00:00:00Z")}else{Value::Null},"updatedAt":"2026-10-01T00:00:00Z","author":{"__typename":"User","databaseId":42},"repository":{"id":repo_id,"nameWithOwner":repository}});
    }
    let mut value = json!({"data":{"repository":repo}});
    match data.mode {
        "partial" => {
            value["errors"] =
                json!([{"type":"FORBIDDEN","path":["repository","p1","author"],"message":"Denied"}])
        }
        "missing" => {
            value["errors"] =
                json!([{"type":"NOT_FOUND","path":["repository","p1"],"message":"Missing"}])
        }
        "global" => {
            value["errors"] = json!([{"type":"FORBIDDEN","path":["repository"],"message":"Denied"}])
        }
        "unscoped" => value["errors"] = json!([{"type":"FORBIDDEN","message":"Denied"}]),
        "wrong-number" => value["data"]["repository"]["p1"]["number"] = json!(999),
        "wrong-repo" => {
            value["data"]["repository"]["p1"]["repository"]["nameWithOwner"] = json!("other/repo")
        }
        "wrong-state" => value["data"]["repository"]["p0"]["merged"] = json!(true),
        "null" => value["data"]["repository"]["p1"] = Value::Null,
        _ => {}
    }
    Json(value).into_response()
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let data = Arc::new(Mutex::new(Data::default()));
        let router = Router::new().fallback(handler).with_state(data.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = Client::with_token(
            Config {
                installation: Some(
                    hey_gh::AppInstallation::new(
                        "synthetic-client".into(),
                        42,
                        vec!["acme/demo".into()],
                        include_str!("fixtures/github-app-test-key.pem"),
                    )
                    .unwrap(),
                ),
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::ZERO,
                queue_timeout: Duration::from_secs(5),
                max_attempts: 1,
                ..Config::default()
            },
            "synthetic-personal".into(),
        )
        .unwrap();
        Self {
            client,
            data,
            _dir: dir,
            task,
        }
    }
}
#[tokio::test]
async fn twenty_five_lifecycles_share_one_personal_query_and_one_cache_key() {
    let f = Fixture::new().await;
    let mut numbers: Vec<_> = (1..=25).collect();
    let report = f
        .client
        .pr_lifecycles("acme/demo", &numbers, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(report.complete);
    assert_eq!(report.pull_requests.len(), 25);
    assert!(report.errors.is_empty());
    assert_eq!(report.pull_requests[2].state, PrLifecycleState::Merged);
    assert_eq!(report.pull_requests[2].author_id, Some(42));
    assert!(report.validated_at_ms > 0);
    numbers.reverse();
    let cached = f
        .client
        .pr_lifecycles("ACME/Demo", &numbers, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cached.validated_at_ms, report.validated_at_ms);
    assert_eq!(cached.pull_requests, report.pull_requests);
    let data = f.data.lock().unwrap();
    assert_eq!(data.calls.len(), 1);
    assert_eq!(data.calls[0].2, "Bearer synthetic-personal");
}
#[tokio::test]
async fn repositories_keep_separate_lifecycle_identity_and_cache() {
    let f = Fixture::new().await;
    let mut ids = Vec::new();
    for repository in ["acme/demo", "other/code"] {
        let report = f
            .client
            .pr_lifecycles(repository, &[1, 2], Freshness::Revalidate)
            .await
            .unwrap();
        assert!(report.complete);
        assert_eq!(report.repository, repository);
        ids.push(report.pull_requests[0].node_id.clone());
    }
    assert_ne!(ids[0], ids[1]);
    for (repository, expected) in ["acme/demo", "other/code"].into_iter().zip(ids) {
        let cached = f
            .client
            .pr_lifecycles(repository, &[1, 2], Freshness::CachedOnly)
            .await
            .unwrap();
        assert_eq!(cached.pull_requests[0].node_id, expected);
    }
    let data = f.data.lock().unwrap();
    assert_eq!(data.calls.len(), 2);
    assert!(
        data.calls
            .iter()
            .all(|call| call.2 == "Bearer synthetic-personal")
    );
}

#[tokio::test]
async fn invalid_batch_selectors_never_reach_github() {
    let f = Fixture::new().await;
    for numbers in [
        vec![],
        vec![0],
        vec![1, 1],
        vec![u64::MAX],
        (1..=26).collect(),
    ] {
        assert!(
            f.client
                .pr_lifecycles("acme/demo", &numbers, Freshness::Revalidate)
                .await
                .is_err()
        );
    }
    assert!(
        f.client
            .pr_lifecycles("bad/repo/path", &[1], Freshness::Revalidate)
            .await
            .is_err()
    );
    assert!(f.data.lock().unwrap().calls.is_empty());
}
#[tokio::test]
async fn scoped_denials_and_missing_prs_preserve_only_independent_nodes() {
    for mode in ["partial", "missing"] {
        let f = Fixture::new().await;
        f.data.lock().unwrap().mode = mode;
        let report = f
            .client
            .pr_lifecycles("acme/demo", &[1, 2, 3], Freshness::Revalidate)
            .await
            .unwrap();
        assert!(!report.complete);
        assert_eq!(
            report
                .pull_requests
                .iter()
                .map(|pr| pr.number)
                .collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].number, 2);
        let cached = f
            .client
            .pr_lifecycles("acme/demo", &[1, 2, 3], Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(!cached.complete);
        assert_eq!(cached.validated_at_ms, report.validated_at_ms);
        assert_eq!(f.data.lock().unwrap().calls.len(), 1);
        f.data.lock().unwrap().mode = "global";
        assert!(
            f.client
                .pr_lifecycles("acme/demo", &[1, 2, 3], Freshness::Revalidate)
                .await
                .is_err(),
            "An old permitted subset cannot mask a new global denial"
        );
    }
}
#[tokio::test]
async fn malformed_or_unavailable_nodes_are_errors_not_closures() {
    for mode in ["wrong-number", "wrong-repo", "wrong-state", "null"] {
        let f = Fixture::new().await;
        f.data.lock().unwrap().mode = mode;
        let report = f
            .client
            .pr_lifecycles("acme/demo", &[1, 2, 3], Freshness::Revalidate)
            .await
            .unwrap();
        assert!(!report.complete, "{mode}");
        assert_eq!(report.pull_requests.len(), 2, "{mode}");
        assert_eq!(report.errors.len(), 1, "{mode}");
    }
    for mode in ["global", "unscoped"] {
        let f = Fixture::new().await;
        f.data.lock().unwrap().mode = mode;
        assert!(
            f.client
                .pr_lifecycles("acme/demo", &[1, 2, 3], Freshness::Revalidate)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn generic_graphql_does_not_inherit_the_typed_partial_success_policy() {
    let f = Fixture::new().await;
    f.data.lock().unwrap().mode = "partial";
    let typed = f
        .client
        .pr_lifecycles("acme/demo", &[1, 2, 3], Freshness::Revalidate)
        .await
        .unwrap();
    let request = f.data.lock().unwrap().calls[0].1.clone();
    assert!(
        f.client
            .graphql(
                request["query"].as_str().unwrap(),
                request["variables"].clone(),
                Freshness::Revalidate
            )
            .await
            .is_err()
    );
    let cached = f
        .client
        .pr_lifecycles("acme/demo", &[1, 2, 3], Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cached.validated_at_ms, typed.validated_at_ms);
    assert!(!cached.complete);
    assert_eq!(f.data.lock().unwrap().calls.len(), 2);
}

#[tokio::test]
async fn sdk_and_http_route_preserve_batch_freshness_and_partial_errors() {
    let f = Fixture::new().await;
    f.data.lock().unwrap().mode = "partial";
    let api = hey_gh::api::Api::new(f.client.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let serving = tokio::spawn(async move {
        axum::serve(listener, api.router()).await.unwrap();
    });
    let client = hey_gh::ApiClient::new(url.parse().unwrap())
        .unwrap()
        .background()
        .with_read_deadline(tokio::time::Instant::now() + Duration::from_secs(3));
    let report = client
        .pr_lifecycles("acme/demo", &[3, 1, 2], Freshness::Revalidate)
        .await
        .unwrap();
    assert!(!report.complete);
    assert_eq!(report.pull_requests.len(), 2);
    let cached = client
        .pr_lifecycles("acme/demo", &[1, 2, 3], Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cached.validated_at_ms, report.validated_at_ms);
    assert_eq!(f.data.lock().unwrap().calls.len(), 1);
    serving.abort();
}
