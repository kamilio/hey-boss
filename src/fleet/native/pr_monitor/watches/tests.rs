use super::*;
use serde_json::{Value, json};
#[path = "end_to_end.rs"]
mod end_to_end;
#[path = "metadata_cadence.rs"]
mod metadata_cadence;
#[path = "review_only.rs"]
mod review_only;

#[test]
fn validation_times_must_be_recent_and_plausible() {
    let now = crate::issues::worker::now() as u64;
    assert!(fresh(now));
    assert!(fresh(now - 60_000));
    assert!(!fresh(now - 300_000));
    assert!(!fresh(now + 300_000));
    assert!(!fresh(u64::MAX));
}

#[test]
fn slow_optional_details_do_not_hold_up_other_required_checks() {
    queue_scenario(true);
}

#[test]
fn slow_required_policy_does_not_starve_other_pr_details() {
    queue_scenario(false);
}

fn queue_scenario(hold_details: bool) {
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
    let count = if hold_details { 6 } else { 8 };
    for number in 1..=count {
        store.execute(&request(json!({"action":"add_pull_request","number":1,"url":format!("https://github.com/o/r/pull/{number}"),"purpose":"fix"}))).unwrap();
    }
    let view = store
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    store.execute(&request(json!({"action":"assign","number":1,"target":"github","if_version":view["issue"]["version"]}))).unwrap();
    if hold_details {
        store
            .execute(&request(json!({"action":"refresh_github","number":1})))
            .unwrap();
    }
    drop(store);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let serving = std::thread::spawn(move || {
        let (ci, policy, metadata) = evidence(true, false);
        let respond = |request: tiny_http::Request, value: &Value| {
            request
                .respond(
                    tiny_http::Response::from_string(value.to_string()).with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    ),
                )
                .unwrap();
        };
        let mut required = std::collections::BTreeSet::new();
        let mut held = Vec::new();
        let mut saw_ci = false;
        for _ in 0..count * 3 {
            let incoming = server
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .expect("Other PRs must progress while one detail read waits");
            let path = incoming.url().split('?').next().unwrap();
            assert!(incoming.url().ends_with(if hold_details {
                "?refresh=true"
            } else {
                "?max_age_seconds=30"
            }));
            let number: u64 = path.split('/').nth(5).unwrap().parse().unwrap();
            if path.ends_with("required-checks") {
                required.insert(number);
                let mut policy = policy.clone();
                policy["pull_number"] = json!(number);
                if !hold_details && number > 4 && !saw_ci {
                    held.push((incoming, policy));
                } else {
                    respond(incoming, &policy);
                }
                if hold_details && required.len() == count {
                    for (waiting, value) in held.drain(..) {
                        respond(waiting, &value);
                    }
                }
            } else if path.ends_with("metadata") {
                let mut metadata = metadata.clone();
                metadata["data"]["number"] = json!(number);
                respond(incoming, &metadata);
            } else if path.ends_with("ci") {
                saw_ci = true;
                if !hold_details {
                    for (waiting, value) in held.drain(..) {
                        respond(waiting, &value);
                    }
                }
                if hold_details && number == 1 && required.len() < count {
                    held.push((incoming, ci.clone()));
                } else {
                    respond(incoming, &ci);
                }
            } else {
                panic!("Unexpected request: {path}");
            }
        }
        assert_eq!(required.len(), count);
        assert!(held.is_empty());
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
    assert_eq!(
        view["issue"]["github_status"]["prs"]
            .as_object()
            .unwrap()
            .len(),
        count
    );
    assert!(view["issue"]["assignee"].is_null());
    let fetches = view["issue"]["github_status"]["fetches"]
        .as_object()
        .unwrap();
    assert_eq!(fetches.len(), count);
    for fetch in fetches.values() {
        assert!(fetch["finished_at"].as_i64().unwrap() >= fetch["started_at"].as_i64().unwrap());
        assert!(fetch["requested_at"].is_null());
    }
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

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
    scenario(Scenario::ReviewsDenied);
}

#[test]
fn pending_optional_checks_do_not_require_review_collection() {
    scenario(Scenario::Pending);
}

#[test]
fn native_stack_trunk_policy_survives_monitor_storage_with_a_different_diff_base() {
    scenario(Scenario::NativeStack);
}

#[test]
fn stale_ci_never_wakes_work_or_starts_review_collection() {
    scenario(Scenario::Stale);
}

#[test]
fn invalid_observation_time_never_wakes_work() {
    scenario(Scenario::InvalidTime);
}

#[test]
fn required_failure_survives_unavailable_workflow_details() {
    scenario(Scenario::CiDenied);
}

#[test]
fn changed_ci_head_preserves_the_early_failure_without_misattributing_it() {
    scenario(Scenario::HeadChanged);
}

#[test]
fn changed_review_head_preserves_the_validated_ci_failure() {
    scenario(Scenario::ReviewHeadChanged);
}

#[derive(Clone, Copy)]
enum Scenario {
    ReviewsDenied,
    Pending,
    NativeStack,
    Stale,
    InvalidTime,
    CiDenied,
    HeadChanged,
    ReviewHeadChanged,
    Closed,
    Merged,
    KeepMergedOpen,
    PolicyErrorClosed,
    PolicyErrorMerged,
    PolicyErrorOpen,
    PolicyErrorStale,
    PolicyErrorDenied,
    PolicyErrorWrongIdentity,
}

#[test]
fn policy_error_still_records_confirmed_merge_and_authorship() {
    scenario(Scenario::PolicyErrorMerged);
}
#[test]
fn policy_error_still_records_confirmed_closure() {
    scenario(Scenario::PolicyErrorClosed);
}
#[test]
fn policy_error_never_completes_work_without_fresh_terminal_identity() {
    for case in [
        Scenario::PolicyErrorOpen,
        Scenario::PolicyErrorStale,
        Scenario::PolicyErrorDenied,
        Scenario::PolicyErrorWrongIdentity,
    ] {
        scenario(case);
    }
}

#[test]
fn closed_unmerged_pr_returns_waiting_issue_to_boss() {
    scenario(Scenario::Closed);
}
#[test]
fn merged_fix_pr_keeps_existing_automatic_completion() {
    scenario(Scenario::Merged);
}
#[test]
fn disabling_automatic_completion_returns_merged_work_to_boss() {
    scenario(Scenario::KeepMergedOpen);
}

fn scenario(scenario: Scenario) {
    let policy_error = matches!(
        scenario,
        Scenario::PolicyErrorClosed
            | Scenario::PolicyErrorMerged
            | Scenario::PolicyErrorOpen
            | Scenario::PolicyErrorStale
            | Scenario::PolicyErrorDenied
            | Scenario::PolicyErrorWrongIdentity
    );
    let unconfirmed = policy_error
        && !matches!(
            scenario,
            Scenario::PolicyErrorClosed | Scenario::PolicyErrorMerged
        );
    let pending = matches!(scenario, Scenario::Pending | Scenario::NativeStack);
    let stale = matches!(scenario, Scenario::Stale);
    let invalid_time = matches!(scenario, Scenario::InvalidTime);
    let ci_denied = matches!(scenario, Scenario::CiDenied);
    let head_changed = matches!(scenario, Scenario::HeadChanged);
    let review_head_changed = matches!(scenario, Scenario::ReviewHeadChanged);
    let terminal = matches!(
        scenario,
        Scenario::Closed
            | Scenario::Merged
            | Scenario::KeepMergedOpen
            | Scenario::PolicyErrorClosed
            | Scenario::PolicyErrorMerged
    );
    let merged = matches!(
        scenario,
        Scenario::Merged | Scenario::KeepMergedOpen | Scenario::PolicyErrorMerged
    );
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
    if matches!(scenario, Scenario::KeepMergedOpen) {
        store
            .execute(&request(
                json!({"action":"configure_global","auto_close_merged_prs":false,"if_version":1}),
            ))
            .unwrap();
    }
    drop(store);
    let (mut ci, mut policy, mut metadata) = evidence(pending, stale);
    if matches!(scenario, Scenario::NativeStack) {
        let trunk = "cccccccccccccccccccccccccccccccccccccccc";
        let stack =
            json!({"id":12,"number":4,"position":2,"size":2,"base":{"ref":"main","sha":trunk}});
        metadata["data"]["base"]["ref"] = json!("layer");
        metadata["data"]["stack"] = stack.clone();
        policy["base_branch"] = json!("layer");
        policy["policy_identity"] = json!({"branch":"main","stack":stack});
        policy["policy_sha"] = json!(trunk);
    }
    if head_changed {
        ci["data"]["head_sha"] = json!("next-head");
        metadata["data"]["head"]["sha"] = json!("next-head");
    }
    if invalid_time {
        policy["observed_at_ms"] = json!(u64::MAX);
    }
    if terminal {
        policy["pull_request_state"] = json!("closed");
        metadata["data"]["state"] = json!("closed");
        metadata["data"]["merged"] = json!(merged);
    }
    if policy_error {
        metadata["data"]["user"] = json!({"id":42});
        metadata["data"]["title"] = json!("Merged change");
        metadata["data"]["merged_at"] = json!("2026-10-05T21:04:49Z");
        if unconfirmed && !matches!(scenario, Scenario::PolicyErrorOpen) {
            metadata["data"]["state"] = json!("closed");
            metadata["data"]["merged"] = json!(true);
        }
        if matches!(scenario, Scenario::PolicyErrorStale) {
            metadata["validated_at_ms"] = json!(crate::issues::worker::now() - 300_000);
        }
        if matches!(scenario, Scenario::PolicyErrorWrongIdentity) {
            metadata["data"]["number"] = json!(2);
        }
    }
    let policy_failure = json!({"error":"PR advertises incomplete native stack metadata"});
    let mut changed_report = json!({"data":{"repository":"o/r","number":1,"pull_request":metadata["data"],"conflicts":"clean",
        "comments":[],"review_comments":[],"reviews":[],"timeline":[],"review_events":[],"review_threads":[],
        "review_status":{"requested_reviewers":[],"requested_teams":[],"latest_reviews":[],"approved_by":[],"changes_requested_by":[],"dismissed_reviews":[],"resolved_threads":0,"unresolved_threads":0,"outdated_threads":0},
        "ci":ci["data"],"errors":[]},"complete":true,"observed_at_ms":ci["observed_at_ms"],"oldest_validation_at_ms":ci["oldest_validation_at_ms"],"validations":[]});
    changed_report["data"]["pull_request"]["head"]["sha"] = json!("next-head");
    changed_report["data"]["ci"]["head_sha"] = json!("next-head");
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let database = ctx.path.clone();
    let serving = std::thread::spawn(move || {
        let expected = if terminal || policy_error {
            2
        } else if stale || invalid_time {
            1
        } else if pending || ci_denied || head_changed {
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
                "/v1/prs/o/r/1/required-checks" if policy_error => (&policy_failure, 422),
                "/v1/prs/o/r/1/required-checks" => (&policy, 200),
                "/v1/prs/o/r/1/metadata" => {
                    assert!(!policy_error || request.url().ends_with("?refresh=true"));
                    if matches!(scenario, Scenario::PolicyErrorDenied) {
                        (&Value::Null, 403)
                    } else {
                        (&metadata, 200)
                    }
                }
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
                    if review_head_changed {
                        (&changed_report, 200)
                    } else {
                        (&Value::Null, 503)
                    }
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
    if terminal && !matches!(scenario, Scenario::Merged | Scenario::PolicyErrorMerged) {
        assert_eq!(view["issue"]["assignee"], "human:boss");
        assert_eq!(view["issue"]["state"], "open");
    } else if stale || invalid_time || unconfirmed {
        assert_eq!(view["issue"]["assignee"], "watcher:github");
    } else {
        assert!(view["issue"]["assignee"].is_null());
    }
    let status = &view["issue"]["github_status"]["prs"]["https://github.com/o/r/pull/1"];
    if !terminal && (stale || invalid_time || !pending) {
        assert!(status["error"].is_string(), "{view}");
    }
    if !stale && !invalid_time && !terminal && !policy_error {
        assert_eq!(status["evidence"]["required"][0]["state"], "failure");
    }
    if matches!(scenario, Scenario::NativeStack) {
        assert_eq!(status["evidence"]["policy_identity"]["branch"], "main");
        assert_eq!(status["evidence"]["policy_identity"]["stack"]["id"], 12);
        assert_eq!(
            status["evidence"]["source_bases"]["required"]["ref"],
            "layer"
        );
        assert_eq!(status["evidence"]["sources_match"], true);
    }
    if head_changed || review_head_changed {
        assert_eq!(
            status["head"], "head",
            "Retain the independently validated early observation"
        );
        assert!(status["error"].as_str().unwrap().contains("changed"));
        assert!(view["issue"]["github_status"]["event"].is_string());
    }
    if terminal {
        assert_eq!(view["issue"]["github_status"]["monitoring"], false);
        assert_eq!(
            view["issue"]["pull_requests"][0]["status"],
            if merged { "merged" } else { "closed" }
        );
        if matches!(scenario, Scenario::Merged | Scenario::PolicyErrorMerged) {
            assert_eq!(view["issue"]["state"], "closed");
        }
    }
    if matches!(scenario, Scenario::PolicyErrorMerged) {
        store.record_github_user(42).unwrap();
        let history = store
            .execute(&request(
                json!({"action":"merged_pull_requests","limit":100,"offset":0}),
            ))
            .unwrap();
        assert_eq!(history["pull_requests"][0]["title"], "Merged change");
        assert_eq!(history["pull_requests"][0]["merged_at"], 1791234289000_i64);
    }
    if unconfirmed {
        assert_eq!(view["issue"]["state"], "open");
        assert_ne!(view["issue"]["pull_requests"][0]["status"], "merged");
        assert!(status["evidence"].is_null());
        assert!(
            status["error"]
                .as_str()
                .unwrap()
                .contains("incomplete native stack metadata")
        );
    }
    // A due-time checkpoint prevents immediate duplicate network reads, including
    // after process recovery. No server remains for this cycle.
    poll(&ctx, &runtime, &client).unwrap();
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
