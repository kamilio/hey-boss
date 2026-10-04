use super::replica;
use crate::issues::{Actor, Operation, Project, Request, Store};
use serde_json::{Value, json};

#[test]
fn equivalent_policy_refresh_propagates_without_rescheduling_ready_dependencies() {
    let root = std::env::temp_dir().join(format!(
        "hb-policy-fleet-{}",
        crate::issues::worker::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let mut main = Store::open(&root.join("main.db")).unwrap();
    Store::open(&root.join("peer.db")).unwrap();
    let main_db = crate::database::Connection::open(root.join("main.db")).unwrap();
    let peer_db = crate::database::Connection::open(root.join("peer.db")).unwrap();
    let mut request = Request {
        version: 1,
        project: Project {
            id: "named:Policy fleet".into(),
            name: "Policy fleet".into(),
        },
        project_override: None,
        actor: Some(Actor {
            id: "human:fixture".into(),
            kind: "human".into(),
            session_id: None,
            machine: "main".into(),
            host: "fixture".into(),
            pid: None,
            process_start: None,
            cwd: root.clone(),
            source: "test".into(),
            invocation: None,
            creation_run: None,
            model: None,
        }),
        operation: Operation::View { number: 1 },
        request_id: None,
    };
    let mut call = |value: Value| {
        request.operation = serde_json::from_value(value).unwrap();
        main.execute(&request).unwrap()
    };
    call(json!({"action":"create","title":"Source","body":"","labels":[]}));
    call(
        json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/1","purpose":"fix"}),
    );
    call(json!({"action":"claim","number":1,"force":false}));
    let ready = call(json!({"action":"ready","number":1,"force":false}));
    call(
        json!({"action":"assign","number":1,"target":"github","if_version":ready["issue"]["version"]}),
    );
    call(json!({"action":"create","title":"Dependent","body":"","labels":[]}));
    call(json!({"action":"set_blockers","number":2,"blockers":[1]}));
    replica::install_capture(&main_db, "controller", "main").unwrap();
    replica::install_capture(&peer_db, "agent", "peer").unwrap();
    let url = "https://github.com/o/r/pull/1";
    let mut policy: hey_gh::RequiredChecksReport = serde_json::from_value(json!({
        "repository":"o/r","pull_number":1,"head_sha":"head","base_branch":"main",
        "base_sha":"base","policy_sha":"base","pr_base_sha":"base","state":"satisfied",
        "strict":false,"up_to_date":true,"checks":[{"context":"test","app_id":15368,
        "state":"satisfied","sha":"head","url":null}],"rules":[],"errors":[],
        "cursor":"unused","pull_request_state":"open"
    }))
    .unwrap();
    main.record_github_observation(url, &hey_gh::watcher::observe_required(&policy), 100)
        .unwrap();
    replica::apply_pull(
        &peer_db,
        "peer",
        &replica::snapshot(&main_db, "peer").unwrap(),
        &[],
    )
    .unwrap();
    let issue = |number| json!({"project_id":"named:Policy fleet","number":number});
    let source = replica::current_row(&peer_db, "issues", &issue(1)).unwrap();
    let child = replica::current_row(&peer_db, "issues", &issue(2)).unwrap();
    assert_eq!(source["state"], "ready");
    assert_eq!(child["state"], "open");
    policy.base_sha = Some("advanced-main".into());
    policy.policy_sha = Some("advanced-main".into());
    for timestamp in [200, 300] {
        main.record_github_observation(url, &hey_gh::watcher::observe_required(&policy), timestamp)
            .unwrap();
        replica::apply_pull(
            &peer_db,
            "peer",
            &replica::snapshot(&main_db, "peer").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(
            replica::current_row(&peer_db, "issues", &issue(1)).unwrap(),
            source
        );
        assert_eq!(
            replica::current_row(&peer_db, "issues", &issue(2)).unwrap(),
            child
        );
        let status = replica::current_row(
            &peer_db,
            "issue_github_watches",
            &json!({"project_id":"named:Policy fleet","issue_number":1}),
        )
        .unwrap();
        let status: Value = serde_json::from_str(status["status"].as_str().unwrap()).unwrap();
        assert_eq!(
            status["prs"][url]["evidence"]["policy_sha"],
            "advanced-main"
        );
    }
    policy.state = "failure".into();
    policy.checks[0].state = "failure".into();
    policy.checks[0].failure_key = Some("new-required-failure".into());
    main.record_github_observation(url, &hey_gh::watcher::observe_required(&policy), 400)
        .unwrap();
    replica::apply_pull(
        &peer_db,
        "peer",
        &replica::snapshot(&main_db, "peer").unwrap(),
        &[],
    )
    .unwrap();
    assert_eq!(
        replica::current_row(&peer_db, "issues", &issue(1)).unwrap()["state"],
        "open"
    );
    assert_eq!(
        replica::current_row(&peer_db, "issues", &issue(2)).unwrap()["state"],
        "blocked"
    );
}
