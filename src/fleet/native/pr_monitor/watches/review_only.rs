use super::*;

#[test]
fn checkless_pr_waits_for_feedback_and_repeated_reviews_do_not_repeat_work() {
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
    let url = "https://github.com/o/r/pull/1";
    store
        .execute(&request(
            json!({"action":"create","title":"Task","body":"","labels":[]}),
        ))
        .unwrap();
    store
        .execute(&request(
            json!({"action":"add_pull_request","number":1,"url":url,"purpose":"fix"}),
        ))
        .unwrap();
    let view = store
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    store.execute(&request(json!({"action":"assign","number":1,"target":"github","if_version":view["issue"]["version"]}))).unwrap();
    drop(store);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut previous_event = Value::Null;
    for cycle in 0..3 {
        let (mut ci, mut policy, metadata) = evidence(false, false);
        ci["data"]["check_runs"] = json!([]);
        ci["data"]["summary"]["state"] = json!("unknown");
        ci["data"]["summary"]["failed"] = json!(0);
        policy["checks"] = json!([]);
        policy["state"] = json!("not_required");
        let reviews = if cycle == 0 {
            json!([])
        } else {
            json!([{"id":5,"state":"CHANGES_REQUESTED","body":"fix race"}])
        };
        let report = json!({"data":{"repository":"o/r","number":1,"pull_request":metadata["data"],"conflicts":"clean",
            "comments":[],"review_comments":[],"reviews":reviews,"timeline":[],"review_events":[],"review_threads":[],
            "review_status":{"requested_reviewers":[],"requested_teams":[],"latest_reviews":[],"approved_by":[],"changes_requested_by":[],"dismissed_reviews":[],"resolved_threads":0,"unresolved_threads":0,"outdated_threads":0},
            "ci":ci["data"],"errors":[]},"complete":true,"observed_at_ms":ci["observed_at_ms"],"oldest_validation_at_ms":ci["oldest_validation_at_ms"],"validations":[]});
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let client =
            ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
        let database = ctx.path.clone();
        let serving = std::thread::spawn(move || {
            let mut paths = std::collections::BTreeSet::new();
            for _ in 0..4 {
                let incoming = server
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .expect("Checkless PR must collect review sources");
                let path = incoming.url().split('?').next().unwrap();
                let policy_read = path.ends_with("required-checks");
                let budget = crate::fleet::native::pr_monitor::tests::read_budget(
                    &incoming,
                    if policy_read { 10_000 } else { 60_000 },
                );
                if !policy_read {
                    assert!(
                        budget > 50_000,
                        "Later stages must retain their own read lifetime"
                    );
                }
                assert!(paths.insert(path.to_owned()));
                let value = match path {
                    "/v1/prs/o/r/1/required-checks" => &policy,
                    "/v1/prs/o/r/1/metadata" => &metadata,
                    "/v1/prs/o/r/1/ci" => &ci,
                    "/v1/prs/o/r/1" => {
                        if cycle <= 1 {
                            let db = Store::open_connection(&database).unwrap();
                            let assignee: Option<String> = db
                                .query_row("SELECT assignee FROM issues WHERE number=1", [], |r| {
                                    r.get(0)
                                })
                                .unwrap();
                            assert_eq!(
                                assignee.as_deref(),
                                Some("watcher:github"),
                                "No checks is not a completion event"
                            );
                        }
                        &report
                    }
                    _ => panic!("Unexpected watcher request: {path}"),
                };
                incoming
                    .respond(
                        tiny_http::Response::from_string(value.to_string()).with_header(
                            tiny_http::Header::from_bytes("Content-Type", "application/json")
                                .unwrap(),
                        ),
                    )
                    .unwrap();
            }
        });
        runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let required = poll_required(&ctx, &client, url, "o/r", 1, deadline, false)
                .await
                .unwrap()
                .unwrap();
            poll_details(&ctx, &client, url, "o/r", 1, deadline, required)
                .await
                .unwrap();
        });
        serving.join().unwrap();
        let view = Store::open(&ctx.path)
            .unwrap()
            .execute(&request(json!({"action":"view","number":1})))
            .unwrap();
        let status = &view["issue"]["github_status"];
        let evidence = &status["prs"][url]["evidence"];
        assert_eq!(evidence["complete"], true);
        assert_eq!(evidence["has_checks"], false);
        if cycle == 0 {
            assert_eq!(view["issue"]["assignee"], "watcher:github");
            assert!(status["event"].is_null());
        } else {
            assert!(view["issue"]["assignee"].is_null());
            assert_eq!(evidence["reviews"][0]["body"], "fix race");
            assert!(status["event"].is_string());
            if cycle == 2 {
                assert_eq!(status["event"], previous_event);
            }
        }
        previous_event = status["event"].clone();
    }
    std::fs::remove_dir_all(root).unwrap();
}
