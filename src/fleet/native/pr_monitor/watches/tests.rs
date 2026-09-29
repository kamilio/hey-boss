use super::*;
use serde_json::{Value, json};

fn evidence(pending: bool, stale: bool) -> (Value, Value, Value) {
    let now = crate::issues::worker::now();
    let validated = if stale { now - 300_000 } else { now };
    let ci = json!({"data":{"head_sha":"head","merge_sha":null,
        "check_runs":[{"id":1,"name":"test","app":{"id":1},"head_sha":"head","status":"completed","conclusion":"failure"}],
        "commit_statuses":[],"workflow_runs":[],"jobs":[],
        "summary":{"state":"failure","successful":0,"failed":1,"pending":if pending {1} else {0},"skipped":0,"unknown":0},"failures":[],"errors":[]},
        "complete":true,"observed_at_ms":now,"oldest_validation_at_ms":validated,"validations":[]});
    let mut policy = json!({"repository":"o/r","pull_number":1,"head_sha":"head","base_branch":"main",
        "state":"failure","strict":false,"up_to_date":true,"checks":[{"context":"test","app_id":1,"state":"failure","sha":"head","url":"https://github.com/o/r/actions/runs/1"}],"rules":[],"errors":[],"cursor":"unused"});
    let metadata = json!({"data":{"number":1,"state":"open","head":{"sha":"head"},"base":{"repo":{"full_name":"o/r"}}},
        "validated_at_ms":now,"fetched_at_ms":now,"source":"cache"});
    let observed = hey_gh::watcher::observe_ci(
        "o/r",
        1,
        &metadata["data"],
        &serde_json::from_value(ci["data"].clone()).unwrap(),
        &serde_json::from_value(policy.clone()).unwrap(),
    );
    policy["checks"][0]["failure_key"] = json!(observed.blocking[0]);
    policy["pull_request_state"] = json!("open");
    policy["observed_at_ms"] = json!(now);
    policy["oldest_validation_at_ms"] = json!(validated);
    (ci, policy, metadata)
}

#[test]
fn required_failure_is_saved_before_review_collection_can_fail() {
    scenario(false, false, false);
}

#[test]
fn pending_optional_checks_do_not_require_review_collection() {
    scenario(true, false, false);
}

#[test]
fn stale_ci_never_wakes_work_or_starts_review_collection() {
    scenario(false, true, false);
}

#[test]
fn required_failure_survives_unavailable_workflow_details() {
    scenario(false, false, true);
}

fn scenario(pending: bool, stale: bool, ci_denied: bool) {
    let (root, ctx, mut store) = crate::fleet::native::context::tests::test_context();
    let request = |operation| crate::issues::Request {
        version: 1,
        project: crate::issues::Project {
            id: "named:test".into(),
            name: "test".into(),
        },
        project_override: None,
        actor: Some(ctx.actor().unwrap()),
        operation: serde_json::from_value(operation).unwrap(),
        request_id: None,
    };
    store
        .execute(&request(
            json!({"action":"create","title":"Task","body":"","labels":[]}),
        ))
        .unwrap();
    store.execute(&request(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/1","purpose":"fix"}))).unwrap();
    let linked = store
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    store.execute(&request(json!({"action":"assign","number":1,"target":"github","if_version":linked["issue"]["version"]}))).unwrap();
    drop(store);
    let (ci, policy, metadata) = evidence(pending, stale);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let database = ctx.path.clone();
    let serving = std::thread::spawn(move || {
        let expected = if stale {
            1
        } else if pending || ci_denied {
            3
        } else {
            4
        };
        let mut requests = std::collections::BTreeSet::new();
        for _ in 0..expected {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .expect("Watcher request");
            requests.insert(request.url().to_owned());
            let path = request.url().split('?').next().unwrap();
            let (body, code) = match path {
                "/v1/prs/o/r/1/ci" => {
                    let db = Store::open_connection(&database).unwrap();
                    let assignee: Option<String> = db
                        .query_row("SELECT assignee FROM issues WHERE number=1", [], |r| {
                            r.get(0)
                        })
                        .unwrap();
                    assert!(
                        assignee.is_none(),
                        "Required failure must be published before workflow details are requested"
                    );
                    if ci_denied {
                        (&Value::Null, 503)
                    } else {
                        (&ci, 200)
                    }
                }
                "/v1/prs/o/r/1/required-checks" => (&policy, 200),
                "/v1/prs/o/r/1/metadata" => (&metadata, 200),
                "/v1/prs/o/r/1" => {
                    let db = Store::open_connection(&database).unwrap();
                    let assignee: Option<String> = db
                        .query_row("SELECT assignee FROM issues WHERE number=1", [], |r| {
                            r.get(0)
                        })
                        .unwrap();
                    assert!(
                        assignee.is_none(),
                        "Required failure must release the watcher before the review request starts"
                    );
                    (&Value::Null, 503)
                }
                _ => panic!("Unexpected watcher request: {path}"),
            };
            request
                .respond(
                    tiny_http::Response::from_string(body.to_string())
                        .with_status_code(code)
                        .with_header(
                            tiny_http::Header::from_bytes("Content-Type", "application/json")
                                .unwrap(),
                        ),
                )
                .unwrap();
        }
        assert_eq!(requests.len(), expected);
        assert!(
            server
                .recv_timeout(Duration::from_millis(100))
                .unwrap()
                .is_none()
        );
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    poll(&ctx, &runtime, &client).unwrap();
    serving.join().unwrap();
    let mut store = Store::open(&ctx.path).unwrap();
    let view = store
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    if stale {
        assert_eq!(view["issue"]["assignee"], "watcher:github");
    } else {
        assert!(view["issue"]["assignee"].is_null());
    }
    let status = &view["issue"]["github_status"]["prs"]["https://github.com/o/r/pull/1"];
    if stale || !pending {
        assert!(status["error"].is_string(), "{view}");
    }
    if !stale {
        assert_eq!(status["evidence"]["required"][0]["state"], "failure");
    }
    // A due-time checkpoint prevents immediate duplicate network reads, including
    // after process recovery. No server remains for this cycle.
    poll(&ctx, &runtime, &client).unwrap();
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
