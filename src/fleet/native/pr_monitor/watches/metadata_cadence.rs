use super::*;
use crate::fleet::native::pr_monitor::lifecycle_tests::from_metadata;

fn receive_metadata_request(server: &tiny_http::Server) -> tiny_http::Request {
    loop {
        let request = server
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .expect("Both polling batches must progress while the other waits");
        crate::fleet::native::pr_monitor::tests::read_budget(
            &request,
            if request.url().contains("required-checks") {
                60_000
            } else if request.url().starts_with("/v1/viewer?") {
                5_000
            } else if request.url().ends_with("metadata?cached_only=true") {
                10_000
            } else if request.url().contains("/pr-lifecycles?") {
                20_000
            } else {
                60_000
            },
        );
        if request.url().starts_with("/v1/viewer?") {
            request.respond(tiny_http::Response::from_string(json!({"data":{"id":42},"validated_at_ms":123,"fetched_at_ms":123,"source":"cache"}).to_string()).with_header(tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap())).unwrap();
        } else if request.url().ends_with("metadata?cached_only=true") {
            request
                .respond(
                    tiny_http::Response::from_string(
                        json!({"code":"cache_miss","error":"missing cache"}).to_string(),
                    )
                    .with_status_code(404)
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    ),
                )
                .unwrap();
        } else {
            return request;
        }
    }
}

#[test]
fn stale_lifecycle_batch_in_flight_cannot_stop_a_new_watcher() {
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
    let assignment = request(
        json!({"action":"assign","number":1,"target":"github","if_version":view["issue"]["version"]}),
    );
    drop(store);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let database = ctx.path.clone();
    let serving = std::thread::spawn(move || {
        let incoming = receive_metadata_request(&server);
        assert!(incoming.url().contains("/pr-lifecycles?"));
        Store::open(&database)
            .unwrap()
            .execute(&assignment)
            .unwrap();
        let (_, _, mut metadata) = evidence(true, false);
        metadata["data"]["state"] = json!("closed");
        metadata["data"]["merged"] = json!(false);
        metadata["validated_at_ms"] = json!(crate::issues::worker::now() - 60_000);
        incoming
            .respond(
                tiny_http::Response::from_string(from_metadata(&metadata, &[1]).to_string())
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    ),
            )
            .unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    crate::fleet::native::pr_monitor::poll(&ctx, &runtime, &client).unwrap();
    serving.join().unwrap();
    let view = Store::open(&ctx.path)
        .unwrap()
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    assert_eq!(view["issue"]["assignee"], "watcher:github");
    assert_eq!(view["issue"]["assignment"]["kind"], "github");
    assert_ne!(view["issue"]["pull_requests"][0]["status"], "closed");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn watcher_and_general_metadata_reads_progress_without_serial_batches() {
    metadata_batches(false);
}

#[test]
fn merge_metadata_keeps_polling_while_required_checks_wait() {
    metadata_batches(true);
}

fn metadata_batches(repeat: bool) {
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
    store
        .execute(&request(
            json!({"action":"create","title":"Ordinary PR","body":"","labels":[]}),
        ))
        .unwrap();
    store.execute(&request(json!({"action":"add_pull_request","number":2,"url":"https://github.com/o/r/pull/2","purpose":"fix"}))).unwrap();
    drop(store);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let database = ctx.path.clone();
    let add_later = request(
        json!({"action":"add_pull_request","number":2,"url":"https://github.com/o/r/pull/3","purpose":"fix"}),
    );
    let serving = std::thread::spawn(move || {
        let (ci, policy, metadata) = evidence(true, false);
        let mut ordinary_metadata = metadata.clone();
        ordinary_metadata["data"]["number"] = json!(2);
        let ordinary_lifecycles = from_metadata(&ordinary_metadata, &[1, 2]);
        let receive = || receive_metadata_request(&server);
        // Withhold both responses until both batches have started. Serial
        // polling would wait here and also add both batches to the next wakeup.
        let first = receive();
        let second = receive();
        let paths = [first.url().to_owned(), second.url().to_owned()];
        assert!(paths.iter().any(|url| url.contains("required-checks")));
        assert!(
            paths
                .iter()
                .any(|url| url.contains("/pr-lifecycles?") && url.contains("numbers=1%2C2")),
            "Lifecycle polling must include both watched and ordinary PRs: {paths:?}"
        );
        let respond = |request: tiny_http::Request| {
            let path = request.url().split('?').next().unwrap();
            let value = if path.ends_with("required-checks") {
                &policy
            } else if path.ends_with("pr-lifecycles") {
                &ordinary_lifecycles
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
        if repeat {
            let (held, ordinary) = if first.url().contains("required-checks") {
                (first, second)
            } else {
                (second, first)
            };
            Store::open(&database).unwrap().execute(&add_later).unwrap();
            respond(ordinary);
            // Added after the first selection: observe this merge on the next
            // metadata tick, before the slow required-check response arrives.
            let next = loop {
                let next = server
                    .recv_timeout(Duration::from_secs(35))
                    .unwrap()
                    .expect("Metadata must poll again while required checks are pending");
                if next.url().starts_with("/v1/viewer?") {
                    next.respond(tiny_http::Response::from_string(json!({"data":{"id":42},"validated_at_ms":123,"fetched_at_ms":123,"source":"cache"}).to_string()).with_header(tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap())).unwrap();
                } else if next.url().ends_with("metadata?cached_only=true") {
                    next.respond(
                        tiny_http::Response::from_string(
                            json!({"code":"cache_miss","error":"missing cache"}).to_string(),
                        )
                        .with_status_code(404)
                        .with_header(
                            tiny_http::Header::from_bytes("Content-Type", "application/json")
                                .unwrap(),
                        ),
                    )
                    .unwrap();
                } else {
                    break next;
                }
            };
            assert!(next.url().contains("/pr-lifecycles?") && next.url().ends_with("numbers=3"));
            let mut merged = metadata.clone();
            merged["data"]["number"] = json!(3);
            merged["data"]["state"] = json!("closed");
            merged["data"]["merged"] = json!(true);
            merged["data"]["user"] = json!({"id":42});
            merged["data"]["title"] = json!("New merge");
            merged["data"]["merged_at"] = json!("2026-10-05T21:04:49Z");
            next.respond(
                tiny_http::Response::from_string(from_metadata(&merged, &[3]).to_string())
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    ),
            )
            .unwrap();
            respond(held);
        } else {
            respond(first);
            respond(second);
        }
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
    crate::fleet::native::pr_monitor::poll_cycle(&ctx, &runtime, &client, &mut None);
    serving.join().unwrap();
    let view = Store::open(&ctx.path)
        .unwrap()
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    assert!(view["issue"]["assignee"].is_null());
    assert!(view["issue"]["github_status"]["event"].is_string());
    assert_eq!(view["issue"]["pull_requests"][0]["status"], "open");
    if repeat {
        let history = Store::open(&ctx.path)
            .unwrap()
            .execute(&request(
                json!({"action":"merged_pull_requests","limit":100,"offset":0}),
            ))
            .unwrap();
        assert_eq!(
            history["pull_requests"][0]["url"],
            "https://github.com/o/r/pull/3"
        );
        assert_eq!(history["pull_requests"][0]["title"], "New merge");
    }
    std::fs::remove_dir_all(root).unwrap();
}
