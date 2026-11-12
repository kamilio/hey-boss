use super::*;
use std::time::Instant;
#[path = "../../tests/support/managed_worker.rs"]
mod managed_worker;

#[test]
fn installed_pr_handoff() {
    let f = installed_fixture();
    let project = "Worker fixture";
    f.init(project).unwrap();
    let issue = |args: &[&str]| run(f.issue(project, args)).unwrap();
    let root = f.root.path();
    let id = managed_worker::register(&f.db);
    let agent = worker_fixtures::binary("codex-worker");
    let work = |number: i64, prs: &str, mode: &str| {
        fs::write(root.join("mode.txt"), mode).unwrap();
        fs::write(root.join("worker-args.json"), json!([prs]).to_string()).unwrap();
        let mut worker = Service(
            f.command(&[
                "worker",
                "run",
                "--id",
                &id,
                "--project",
                project,
                "--directory",
                root.to_str().unwrap(),
                "--concurrency",
                "1",
                prs,
            ])
            .env("HEY_BOSS_FLEET_MANAGED", "1")
            .env("HEY_BOSS_TEST_CLI", &f.binary)
            .env("HEY_BOSS_CODEX", &agent)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status = issue(&["worker", "status"]);
            if let Some(run) = status["runs"].as_array().and_then(|runs| {
                runs.iter()
                    .find(|r| r["number"] == number && !r["finished_at"].is_null())
            }) {
                assert_eq!(run["state"], "completed", "{run}");
                assert_eq!(status["eligible"], 0, "{status}");
                break;
            }
            assert!(
                worker.0.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "Worker did not finish: {status}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    };
    issue(&["create", "--title", "Open fix PR"]);
    issue(&["comment", "1", "--body", "Existing history"]);
    for (url, purpose) in [
        ("https://github.com/example/repo/pull/1", "fix"),
        (
            "https://github.com/example/repo/pull/2",
            "supporting-evidence",
        ),
    ] {
        issue(&["pr", "add", "1", url, "--purpose", purpose]);
    }
    work(1, "--prs", "completed");
    let view = issue(&["view", "1"]);
    assert_eq!(view["issue"]["state"], "open");
    assert_eq!(view["issue"]["assignee"], "human:boss");
    assert!(view["issue"]["closed_at"].is_null());
    let comments = view["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert!(comments.iter().any(|c| c["body"] == "Existing history"));
    assert!(comments.iter().any(|c| {
        c["body"]
            .as_str()
            .unwrap()
            .contains("Meaningful checks passed.")
    }));
    let prs = issue(&["pr", "list", "1"]);
    assert_eq!(
        prs["pull_requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["purpose"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["fix", "supporting-evidence"]
    );
    let turn = fs::read_to_string(root.join("protocol.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|m| m["method"] == "turn/start")
        .unwrap();
    let prompt = turn["params"]["input"][0]["text"].as_str().unwrap();
    assert!(
        prompt.contains("attach every PR")
            && !prompt.contains("PR handoff:")
            && !prompt.contains("Record delivery and verification evidence"),
        "{prompt}"
    );
    issue(&["create", "--title", "Direct main completion"]);
    work(2, "--no-prs", "completed");
    assert_eq!(issue(&["view", "2"])["issue"]["state"], "closed");
    issue(&["create", "--title", "Explicit agent handoff"]);
    work(3, "--prs", "assign-to-boss");
    let explicit = issue(&["view", "3"]);
    assert_eq!(explicit["issue"]["state"], "open");
    assert_eq!(explicit["issue"]["assignee"], "human:boss");
    let (_web, base) = web(&f, project);
    let html = reqwest::blocking::get(base)
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .unwrap();
    for setting in [
        r#"aria-describedby="project-prs-help""#,
        r#"data-prompt-file="delivery-pr.md""#,
        r#"data-prompt-file="delivery-main.md""#,
    ] {
        assert!(html.contains(setting), "Missing {setting}");
    }
    assert!(!html.contains(r#"id="project-pr-handoff-help""#));
}
