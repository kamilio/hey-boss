use super::*;
use serde_json::{Value, json};

#[test]
fn one_lifecycle_batch_records_merges_for_ordinary_watched_and_closed_tasks() {
    let (root, ctx, mut store) = super::super::context::tests::test_context();
    crate::database::Connection::open(&ctx.path)
        .unwrap()
        .execute(
            "INSERT INTO projects(id,name,next_number) VALUES('named:test','test',1)",
            [],
        )
        .unwrap();
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
    for number in 1..=3 {
        store
            .execute(&request(
                json!({"action":"create","title":"Fix","body":"","labels":[]}),
            ))
            .unwrap();
        store.execute(&request(json!({"action":"add_pull_request","number":number,"url":format!("https://github.com/o/r/pull/{number}"),"purpose":"fix"}))).unwrap();
    }
    let view = store
        .execute(&request(json!({"action":"view","number":2})))
        .unwrap();
    store.execute(&request(json!({"action":"assign","number":2,"target":"github","if_version":view["issue"]["version"]}))).unwrap();
    store
        .execute(&request(json!({"action":"close","number":3,"force":false})))
        .unwrap();
    drop(store);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let serving = std::thread::spawn(move || {
        let viewer = server
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert!(viewer.url().starts_with("/v1/viewer?"));
        respond(
            viewer,
            json!({"data":{"id":42},"validated_at_ms":crate::issues::worker::now(),"fetched_at_ms":crate::issues::worker::now(),"source":"cache"}),
        );
        let batch = server
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert!(
            batch.url().starts_with("/v1/repos/o/r/pr-lifecycles?"),
            "{}",
            batch.url()
        );
        assert!(batch.url().contains("numbers=1%2C2%2C3"), "{}", batch.url());
        tests::read_budget(&batch, 20_000);
        let now = crate::issues::worker::now();
        respond(
            batch,
            json!({"repository":"o/r","pull_requests":(1..=3).map(|number|json!({"number":number,"node_id":format!("PR_{number}"),"state":"merged","title":format!("Merge {number}"),"author_id":42,"merged_at":"2026-10-01T00:00:00Z"})).collect::<Vec<_>>(),"errors":[],"complete":true,"fetched_at_ms":now,"validated_at_ms":now,"source":"network"}),
        );
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
    for number in 1..=3 {
        let view = store
            .execute(&request(json!({"action":"view","number":number})))
            .unwrap();
        assert_eq!(view["issue"]["state"], "closed");
        assert_eq!(view["issue"]["pull_requests"][0]["status"], "merged");
    }
    let history = store
        .execute(&request(
            json!({"action":"merged_pull_requests","limit":100,"offset":0}),
        ))
        .unwrap();
    assert_eq!(history["pull_requests"].as_array().unwrap().len(), 3);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
fn respond(request: tiny_http::Request, value: Value) {
    request
        .respond(
            tiny_http::Response::from_string(value.to_string()).with_header(
                tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
            ),
        )
        .unwrap();
}

pub(super) fn from_metadata(response: &Value, numbers: &[u64]) -> Value {
    let data = &response["data"];
    let state = if data["merged"] == true {
        "merged"
    } else if data["state"] == "closed" {
        "closed"
    } else {
        "open"
    };
    json!({"repository":data["base"]["repo"]["full_name"],"pull_requests":numbers.iter().map(|number|json!({"number":number,"node_id":format!("PR_{number}"),"state":state,"title":data["title"].as_str().unwrap_or("PR"),"author_id":data["user"]["id"],"merged_at":if state=="merged" {data["merged_at"].clone()}else{Value::Null}})).collect::<Vec<_>>(),"errors":[],"complete":true,"fetched_at_ms":response["fetched_at_ms"],"validated_at_ms":response["validated_at_ms"],"source":response["source"]})
}

fn linked_fixture(count: u64) -> (std::path::PathBuf, Context) {
    let (root, ctx, mut store) = super::super::context::tests::test_context();
    crate::database::Connection::open(&ctx.path)
        .unwrap()
        .execute(
            "INSERT INTO projects(id,name,next_number) VALUES('named:test','test',1)",
            [],
        )
        .unwrap();
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
            json!({"action":"create","title":"Fix","body":"","labels":[]}),
        ))
        .unwrap();
    for number in 1..=count {
        store.execute(&request(json!({"action":"add_pull_request","number":1,"url":format!("https://github.com/o/r/pull/{number}"),"purpose":"fix"}))).unwrap();
    }
    drop(store);
    (root, ctx)
}
fn receive_batch(server: &tiny_http::Server) -> (tiny_http::Request, Vec<u64>) {
    loop {
        let request = server
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("Bounded lifecycle request");
        if request.url().starts_with("/v1/viewer?") {
            let now = crate::issues::worker::now();
            respond(
                request,
                json!({"data":{"id":42},"validated_at_ms":now,"fetched_at_ms":now,"source":"cache"}),
            );
            continue;
        }
        assert!(
            request.url().starts_with("/v1/repos/o/r/pr-lifecycles?"),
            "{}",
            request.url()
        );
        let numbers = request
            .url()
            .split("numbers=")
            .nth(1)
            .unwrap()
            .split("%2C")
            .map(|number| number.parse().unwrap())
            .collect::<Vec<_>>();
        assert!(!numbers.is_empty() && numbers.len() <= 25);
        tests::read_budget(&request, 20_000);
        return (request, numbers);
    }
}
fn batch_response(numbers: &[u64], merged: bool) -> Value {
    let now = crate::issues::worker::now();
    from_metadata(
        &json!({"data":{"state":if merged {"closed"}else{"open"},"merged":merged,"title":"Fix","user":{"id":42},"merged_at":if merged {json!("2026-10-01T00:00:00Z")}else{Value::Null},"base":{"repo":{"full_name":"o/r"}}},"validated_at_ms":now,"fetched_at_ms":now,"source":"network"}),
        numbers,
    )
}

#[test]
fn rate_limit_drains_an_admitted_success_and_stops_further_batches() {
    let (root, ctx) = linked_fixture(75);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let serving = std::thread::spawn(move || {
        // Neither response is sent until both requests arrive: concurrency is
        // bounded at two but a queued first read cannot serialize the second.
        let (first, first_numbers) = receive_batch(&server);
        let (second, second_numbers) = receive_batch(&server);
        assert_eq!(first_numbers.len(), 25);
        assert_eq!(second_numbers.len(), 25);
        first
            .respond(
                tiny_http::Response::from_string(
                    json!({"code":"rate_limited","error":"cooldown"}).to_string(),
                )
                .with_status_code(503)
                .with_header(tiny_http::Header::from_bytes("Retry-After", "600").unwrap()),
            )
            .unwrap();
        respond(second, batch_response(&second_numbers, true));
        assert!(
            server
                .recv_timeout(Duration::from_millis(100))
                .unwrap()
                .is_none(),
            "No third batch may bypass the shared cooldown"
        );
        second_numbers
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    poll(&ctx, &runtime, &client).unwrap();
    let confirmed = serving.join().unwrap();
    let mut store = Store::open(&ctx.path).unwrap();
    let request = crate::issues::Request {
        version: 1,
        project: crate::issues::Project {
            id: "named:test".into(),
            name: "test".into(),
        },
        project_override: None,
        actor: Some(ctx.actor().unwrap()),
        operation: serde_json::from_value(
            json!({"action":"merged_pull_requests","limit":100,"offset":0}),
        )
        .unwrap(),
        request_id: None,
    };
    let history = store.execute(&request).unwrap();
    assert_eq!(
        history["pull_requests"].as_array().unwrap().len(),
        confirmed.len()
    );
    let schedule: Value =
        serde_json::from_slice(&std::fs::read(ctx.state.join("pr-monitor-schedule.json")).unwrap())
            .unwrap();
    assert!(schedule["cooldown_until"].as_i64().unwrap() > crate::issues::worker::now() + 590_000);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_lifecycle_sweep_persists_admission_and_resumes_unattempted_links() {
    let (root, ctx) = linked_fixture(75);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let (admitted, ready) = tokio::sync::oneshot::channel();
    let serving = std::thread::spawn(move || {
        let (first, a) = receive_batch(&server);
        let (second, b) = receive_batch(&server);
        assert_eq!(a.len() + b.len(), 50);
        admitted.send(()).unwrap();
        let (next, numbers) = receive_batch(&server);
        assert_eq!(numbers.len(), 25);
        assert!(
            numbers
                .iter()
                .all(|number| !a.contains(number) && !b.contains(number))
        );
        respond(next, batch_response(&numbers, false));
        assert!(
            server
                .recv_timeout(Duration::from_millis(100))
                .unwrap()
                .is_none()
        );
        drop((first, second));
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::select! { result=poll_once(&ctx,&client)=>panic!("Unexpected completed sweep: {result:?}"), result=ready=>result.unwrap() }
        let schedule:Value=serde_json::from_slice(&std::fs::read(ctx.state.join("pr-monitor-schedule.json")).unwrap()).unwrap();
        assert_eq!(schedule["entries"].as_object().unwrap().len(),50);
        tokio::time::timeout(Duration::from_secs(5),poll_once(&ctx,&client)).await.unwrap().unwrap();
    });
    serving.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsupported_pr_number_does_not_poison_valid_lifecycle_batch() {
    let (root, ctx) = linked_fixture(1);
    let request=crate::issues::Request {version:1,project:crate::issues::Project{id:"named:test".into(),name:"test".into()},project_override:None,actor:Some(ctx.actor().unwrap()),operation:serde_json::from_value(json!({"action":"add_pull_request","number":1,"url":format!("https://github.com/o/r/pull/{}",u64::MAX),"purpose":"fix"})).unwrap(),request_id:None};
    Store::open(&ctx.path).unwrap().execute(&request).unwrap();
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let serving = std::thread::spawn(move || {
        let (incoming, numbers) = receive_batch(&server);
        assert_eq!(numbers, vec![1]);
        respond(incoming, batch_response(&numbers, true));
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    poll(&ctx, &runtime, &client).unwrap();
    serving.join().unwrap();
    let mut view_request = request;
    view_request.operation = serde_json::from_value(json!({"action":"view","number":1})).unwrap();
    let view = Store::open(&ctx.path)
        .unwrap()
        .execute(&view_request)
        .unwrap();
    assert_eq!(view["issue"]["state"], "open");
    let prs = view["issue"]["pull_requests"].as_array().unwrap();
    assert_eq!(prs.iter().filter(|pr| pr["status"] == "merged").count(), 1);
    assert_eq!(
        prs.iter()
            .filter(|pr| pr["error"]
                .as_str()
                .is_some_and(|error| error.contains("Unsupported PR URL")))
            .count(),
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_identity_revalidates_instead_of_publishing_an_old_cached_merge() {
    let (root, ctx) = linked_fixture(1);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let serving = std::thread::spawn(move || {
        let (incoming, numbers) = receive_batch(&server);
        let mut batch = batch_response(&numbers, false);
        batch["pull_requests"] = json!([]);
        batch["errors"] = json!([{"number":1,"code":"identity_changed"}]);
        batch["complete"] = json!(false);
        respond(incoming, batch);
        let repair = server
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        tests::read_budget(&repair, 20_000);
        let revalidated = repair.url().ends_with("metadata?refresh=true");
        let now = crate::issues::worker::now();
        // A cache-accepting read can still return the retired, merged identity.
        respond(
            repair,
            json!({"data":{"number":1,"node_id":if revalidated {"NEW"} else {"OLD"},"state":if revalidated {"open"} else {"closed"},"merged":!revalidated,"title":"Fix","user":{"id":42},"merged_at":if revalidated {Value::Null} else {json!("2026-10-01T00:00:00Z")},"base":{"repo":{"full_name":"o/r"}}},"validated_at_ms":now,"fetched_at_ms":now,"source":if revalidated {"network"} else {"cache"}}),
        );
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    poll(&ctx, &runtime, &client).unwrap();
    serving.join().unwrap();
    let request = crate::issues::Request {
        version: 1,
        project: crate::issues::Project {
            id: "named:test".into(),
            name: "test".into(),
        },
        project_override: None,
        actor: Some(ctx.actor().unwrap()),
        operation: serde_json::from_value(json!({"action":"view","number":1})).unwrap(),
        request_id: None,
    };
    let view = Store::open(&ctx.path).unwrap().execute(&request).unwrap();
    assert_eq!(view["issue"]["state"], "open");
    assert_eq!(view["issue"]["pull_requests"][0]["status"], "open");
    std::fs::remove_dir_all(root).unwrap();
}
