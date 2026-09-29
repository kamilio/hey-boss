//! Opt-in real-process coverage; build the CLI before running this test.
use super::*;
use std::{
    fs,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_some() {
            return;
        }
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn wait(label: &str, root: &std::path::Path, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for {label}; fixture logs: {}",
            root.display()
        );
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "requires cargo build --bin hey-boss; launches only isolated synthetic worker processes"]
fn github_http_poll_claim_steer_rearm_and_fresh_session() {
    lifecycle(false);
}

#[test]
#[ignore = "requires cargo build --bin hey-boss; launches only isolated synthetic worker processes"]
fn github_http_poll_rejected_steering_starts_fresh_session_with_pending_findings() {
    lifecycle(true);
}

fn lifecycle(reject_steering: bool) {
    let binary = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("hey-boss");
    assert!(binary.is_file(), "Run cargo build --bin hey-boss first");
    let (root, ctx, mut store) = crate::fleet::native::context::tests::test_context();
    let request = |operation| crate::issues::Request {
        version: 1,
        project: crate::issues::Project {
            id: "named:Watcher E2E".into(),
            name: "Watcher E2E".into(),
        },
        project_override: None,
        actor: Some(ctx.actor().unwrap()),
        operation: serde_json::from_value(operation).unwrap(),
        request_id: None,
    };
    store
        .execute(&request(
            json!({"action":"create","title":"Synthetic watcher task","body":"","labels":[]}),
        ))
        .unwrap();
    store.execute(&request(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/1","purpose":"fix"}))).unwrap();
    let view = store
        .execute(&request(json!({"action":"view","number":1})))
        .unwrap();
    store.execute(&request(json!({"action":"assign","number":1,"target":"github","if_version":view["issue"]["version"]}))).unwrap();
    drop(store);
    // Host the fixture's DB in this test so no detached companion is bootstrapped.
    let owner = crate::database::Owner::start(&ctx.path).unwrap().unwrap();
    let logs = fs::File::create(root.join("worker.log")).unwrap();
    let worker = Worker(
        Command::new(&binary)
            .current_dir(&root)
            .env("HEY_BOSS_ISSUE_DB", &ctx.path)
            .env("HEY_BOSS_FLEET_STATE", &ctx.state)
            .env("HEY_BOSS_INBOX_SOCKET", root.join("absent-inbox.sock"))
            .env("HEY_BOSS_TEST_CLI", &binary)
            .env(
                "HEY_BOSS_TEST_REJECT_STEERING",
                if reject_steering { "1" } else { "0" },
            )
            .env(
                "HEY_BOSS_CODEX",
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/codex-github-watch.mjs"),
            )
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args([
                "worker",
                "run",
                "--project",
                "Watcher E2E",
                "--directory",
                root.to_str().unwrap(),
            ])
            .stdout(Stdio::from(logs.try_clone().unwrap()))
            .stderr(Stdio::from(logs))
            .spawn()
            .unwrap(),
    );
    let scalar = |query: &str| {
        Store::open_connection(&ctx.path)
            .unwrap()
            .query_row(query, [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    wait("worker schema", &root, || {
        scalar("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='issue_workers'") > 0
    });
    wait("worker registration", &root, || {
        scalar("SELECT count(*) FROM issue_workers WHERE owner_pid IS NOT NULL") > 0
    });
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        scalar("SELECT count(*) FROM worker_runs"),
        0,
        "A parked watcher is not worker pickup"
    );

    let phase = Arc::new(AtomicUsize::new(1));
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client = ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap())
        .unwrap()
        .background();
    let stop = Arc::new(AtomicBool::new(false));
    let serving_phase = phase.clone();
    let stopping = stop.clone();
    let server = Server {
        stop,
        thread: Some(thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let Some(request) = server.recv_timeout(Duration::from_millis(100)).unwrap() else {
                    continue;
                };
                let phase = serving_phase.load(Ordering::Acquire);
                let (mut ci, mut policy, metadata) = evidence(phase != 2, false);
                if phase == 3 {
                    ci["data"]["check_runs"][0]["id"] = json!(2);
                    let observation = hey_gh::watcher::observe_ci(
                        "o/r",
                        1,
                        &metadata["data"],
                        &serde_json::from_value(ci["data"].clone()).unwrap(),
                        &serde_json::from_value(policy.clone()).unwrap(),
                    );
                    policy["checks"][0]["failure_key"] = json!(observation.blocking[0]);
                }
                let path = request.url().split('?').next().unwrap();
                let body = if path.ends_with("required-checks") {
                    policy
                } else if path.ends_with("metadata") {
                    metadata
                } else if path.ends_with("ci") {
                    ci
                } else {
                    let now = crate::issues::worker::now();
                    json!({"data":{"repository":"o/r","number":1,"pull_request":metadata["data"],"conflicts":"clean",
                        "comments":[],"review_comments":[],"reviews":[{"id":1,"state":"CHANGES_REQUESTED","body":"Synthetic review finding"}],
                        "timeline":[],"review_events":[],"review_threads":[],
                        "review_status":{"requested_reviewers":[],"requested_teams":[],"latest_reviews":[],"approved_by":[],"changes_requested_by":[],"dismissed_reviews":[],"resolved_threads":0,"unresolved_threads":0,"outdated_threads":0},
                        "ci":ci["data"],"errors":[]},"complete":true,"observed_at_ms":now,"oldest_validation_at_ms":now,"validations":[]})
                };
                request
                    .respond(
                        tiny_http::Response::from_string(body.to_string()).with_header(
                            tiny_http::Header::from_bytes("Content-Type", "application/json")
                                .unwrap(),
                        ),
                    )
                    .unwrap();
            }
        })),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let poll_now = || {
        let _ = fs::remove_file(ctx.state.join("github-watch-schedule.json"));
        poll(&ctx, &runtime, &client).unwrap();
    };
    let records = || {
        fs::read_to_string(root.join("github-worker.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>()
    };
    poll_now();
    wait("first claim", &root, || {
        records().iter().any(|r| r["type"] == "claim")
    });
    let first = records()[0].clone();
    assert_eq!(scalar("SELECT count(*) FROM worker_runs"), 1);
    phase.store(2, Ordering::Release);
    poll_now();
    let after_completion = if reject_steering { 2 } else { 1 };
    wait("steering and return to watcher", &root, || {
        scalar("SELECT count(*) FROM worker_runs WHERE state='completed'") == after_completion
            && scalar("SELECT count(*) FROM issues WHERE assignee='watcher:github'") == 1
    });
    let steered = records()
        .into_iter()
        .find(|r| {
            r["type"]
                == if reject_steering {
                    "steer_rejected"
                } else {
                    "steer"
                }
        })
        .unwrap();
    assert_eq!(steered["session"], first["session"]);
    assert_ne!(steered["status"]["event"], first["status"]["event"]);
    if reject_steering {
        let delivered = records()
            .into_iter()
            .filter(|r| r["type"] == "claim")
            .nth(1)
            .unwrap();
        assert_ne!(delivered["session"], first["session"]);
        assert_eq!(delivered["status"]["event"], steered["status"]["event"]);
        assert_eq!(
            delivered["status"]["prs"]["https://github.com/o/r/pull/1"]["evidence"]["complete"],
            true
        );
        assert_eq!(
            scalar("SELECT count(*) FROM agent_steering WHERE state='rejected'"),
            1
        );
    }
    phase.store(3, Ordering::Release);
    poll_now();
    wait("fresh second session", &root, || {
        scalar("SELECT count(*) FROM worker_runs WHERE state='completed'") == after_completion + 1
            && scalar("SELECT count(*) FROM issues WHERE assignee='watcher:github'") == 1
    });
    let claims: Vec<_> = records()
        .into_iter()
        .filter(|r| r["type"] == "claim")
        .collect();
    assert_eq!(claims.len(), (after_completion + 1) as usize);
    for pair in claims.windows(2) {
        assert_ne!(pair[0]["session"], pair[1]["session"]);
    }
    poll_now();
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        scalar("SELECT count(*) FROM worker_runs"),
        after_completion + 1,
        "Repeated observations cannot launch duplicate work"
    );
    drop(server);
    drop(worker);
    drop(owner);
    fs::remove_dir_all(root).unwrap();
}
