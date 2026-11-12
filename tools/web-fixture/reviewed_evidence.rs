//! Exercise the installed CLI with ten full PR reports; optionally replay REVIEWED_EVIDENCE_FILE.
use super::*;

fn report(number: u32) -> Value {
    json!({
        "report":{"data":{"repository":"example/repo","number":number,
            "pull_request":{"number":number,"state":"open","head":{"sha":"head"},"base":{"ref":"main","sha":"base","repo":{"full_name":"example/repo"}},"user":{"login":"author"}},
            "conflicts":"clean","comments":[],"review_comments":[],
            "reviews":[{"id":1,"state":"COMMENTED","body":"Full reviewed diagnostic output.\n".repeat(30000),"user":{"login":"reviewer"}}],
            "timeline":[],"review_events":[],"review_threads":[],
            "review_status":{"requested_reviewers":[],"requested_teams":[],"latest_reviews":[],"approved_by":[],"changes_requested_by":[],"dismissed_reviews":[],"resolved_threads":0,"unresolved_threads":0,"outdated_threads":0},
            "ci":{"head_sha":"head","merge_sha":null,"check_runs":[],"commit_statuses":[],"workflow_runs":[],"jobs":[],"summary":{"state":"success","successful":0,"failed":0,"pending":0,"skipped":0,"unknown":0},"failures":[],"errors":[]},"errors":[]},
            "complete":true,"observed_at_ms":100,"oldest_validation_at_ms":100,"validations":[]},
        "policy":{"repository":"example/repo","pull_number":number,"head_sha":"head","base_branch":"main","base_sha":"base","pr_base_sha":"base","state":"not_required","strict":false,"up_to_date":true,"checks":[],"rules":[],"errors":[],"cursor":"unused","pull_request_state":"open"}
    })
}

pub(super) fn verify(f: &Fixture, project: &str) -> Result<Value> {
    let issue = |args: &[&str]| {
        let mut cmd = f.issue(project, args);
        cmd.env("HEY_BOSS_AGENT_ID", "human:boss");
        cmd
    };
    let mut evidence: Vec<Value> = match std::env::var_os("REVIEWED_EVIDENCE_FILE") {
        Some(path) => serde_json::from_reader(fs::File::open(path)?)?,
        None => (1..=10).map(report).collect(),
    };
    let raw = serde_json::to_vec(&evidence)?;
    if raw.len() <= 8 * 1024 * 1024 || evidence.len() != 10 {
        return Err("Expected ten reviewed reports exceeding 8 MiB together".into());
    }
    run(issue(&[
        "create",
        "--title",
        "Ten reviewed pull requests",
        "--body",
        "Full evidence handoff verification.",
    ]))?;
    for entry in &evidence {
        let data = &entry["report"]["data"];
        let repo = data["repository"].as_str().ok_or("Missing repository")?;
        let number = data["number"].as_u64().ok_or("Missing PR number")?;
        run(issue(&[
            "pr",
            "add",
            "1",
            &format!("https://github.com/{repo}/pull/{number}"),
        ]))?;
    }
    run(issue(&["claim", "1"]))?;
    run(issue(&["ready", "1"]))?;
    let view = || run(issue(&["view", "1"]));
    let before = view()?;
    let file = f.root.path().join("reviewed.json");
    evidence[9]["policy"]["pr_base_sha"] = json!("mismatched-base");
    fs::write(&file, serde_json::to_vec(&evidence)?)?;
    let args = [
        "assign",
        "1",
        "github",
        "--reviewed-evidence",
        "reviewed.json",
    ];
    let rejected = issue(&args).output()?;
    assert_eq!(
        rejected.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&rejected.stdout)?["error"]["code"],
        "conflict"
    );
    assert_eq!(view()?, before, "Invalid final report changed the handoff");
    fs::write(file, &raw)?;
    let version = before["issue"]["version"]
        .as_u64()
        .ok_or("Missing version")?
        .to_string();
    let args = [
        args.as_slice(),
        &["--if-version", &version, "--request-id", "reviewed-once"],
    ]
    .concat();
    let assigned = run(issue(&args))?;
    assert_eq!(assigned["issue"]["state"], "ready");
    assert_eq!(assigned["issue"]["assignee"], "watcher:github");
    assert_eq!(run(issue(&args))?, assigned);
    let after = view()?;
    assert_eq!(
        after["issue"]["pull_requests"]
            .as_array()
            .ok_or("Missing pull requests")?
            .len(),
        10
    );
    assert_eq!(after["comment_count"], before["comment_count"]);
    let help = f.command(&["issue", "assign", "--help"]).output()?;
    assert!(help.status.success() && String::from_utf8_lossy(&help.stdout).contains("64 MiB"));
    Ok(
        json!({"issue":1,"bytes":raw.len(),"reports":evidence.len(),"atomic_rejection":true,"ready_handoff":true,"idempotent_retry":true}),
    )
}
