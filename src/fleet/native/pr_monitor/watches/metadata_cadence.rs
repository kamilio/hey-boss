use super::*;

#[test]
fn watcher_and_general_metadata_reads_progress_without_serial_batches() {
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
    let view = store
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    store.execute(&request(json!({"action":"assign","number":1,"target":"github","if_version":view["issue"]["version"]}))).unwrap();
    drop(store);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let serving = std::thread::spawn(move || {
        let (ci, policy, metadata) = evidence(true, false);
        let receive = || {
            server
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .expect("Both polling batches must progress while the other waits")
        };
        // Withhold both responses until both batches have started. Serial
        // polling would wait here and also add both batches to the next wakeup.
        let first = receive();
        let second = receive();
        let paths = [first.url().to_owned(), second.url().to_owned()];
        assert!(paths.iter().any(|url| url.contains("required-checks")));
        assert!(
            paths
                .iter()
                .any(|url| url.ends_with("metadata?max_age_seconds=300"))
        );
        let respond = |request: tiny_http::Request| {
            let path = request.url().split('?').next().unwrap();
            let value = if path.ends_with("required-checks") {
                &policy
            } else if path.ends_with("metadata") {
                &metadata
            } else if path.ends_with("ci") {
                &ci
            } else {
                panic!("Unexpected request: {path}")
            };
            request
                .respond(
                    tiny_http::Response::from_string(value.to_string()).with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    ),
                )
                .unwrap();
        };
        respond(first);
        respond(second);
        respond(receive());
        respond(receive());
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
    crate::fleet::native::pr_monitor::poll_cycle(&ctx, &runtime, &client);
    serving.join().unwrap();
    let view = Store::open(&ctx.path)
        .unwrap()
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    assert!(view["issue"]["assignee"].is_null());
    assert!(view["issue"]["github_status"]["event"].is_string());
    assert_eq!(view["issue"]["pull_requests"][0]["status"], "open");
    std::fs::remove_dir_all(root).unwrap();
}
