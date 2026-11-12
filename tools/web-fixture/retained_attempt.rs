//! Retained processes, logs and Git source all live in the private fixture.
use super::*;
use std::{io::Write, os::unix::process::CommandExt};

pub(super) fn url(base: &str, number: &Value) -> String {
    format!(
        "{}/#project=named%3ASurviving%20attempt%20QA&issue={number}",
        base.trim_end_matches('/')
    )
}

pub(super) fn seed(f: &Fixture, project: &str) -> Result<Value> {
    let root = f.root.path();
    let source = root.join("retained-source");
    fs::create_dir(&source)?;
    for args in [
        &["init", "-q"][..],
        &[
            "-c",
            "user.name=QA",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "Fixture",
        ],
    ] {
        let output = Command::new("git")
            .current_dir(&source)
            .env_clear()
            .env("HOME", root.join("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let owner = "codex:attempt-qa";
    let issue = |args: &[&str]| {
        let mut cmd = f.issue(project, args);
        cmd.env("HEY_BOSS_AGENT_ID", owner);
        cmd
    };
    let rejected = |args: &[&str], code, reason| -> Result<()> {
        let o = issue(args).output()?;
        assert_eq!(o.status.code(), Some(code), "{args:?}: {o:?}");
        let response: Value = serde_json::from_slice(&o.stdout)?;
        assert_eq!(response["error"]["code"], reason, "{response}");
        Ok(())
    };
    let hold = |title: &str, pid: u32, log: &Path| -> Result<Value> {
        let created = run(issue(&["create", "--title", title]))?;
        let n = created["issue"]["number"].to_string();
        let claim = run(issue(&["claim", &n]))?;
        let version = claim["issue"]["version"].to_string();
        fs::write(log, "validation started\n")?;
        fs::write(
            root.join("attempt.json"),
            serde_json::to_vec(
                &json!({"attempt_id":format!("validation-{n}"),"owner":owner,"pid":pid,"log_path":log,"worktree":source}),
            )?,
        )?;
        let args = [
            "attempt",
            "hold",
            &n,
            "--if-version",
            &version,
            "--file",
            "attempt.json",
        ];
        let held = run(issue(&args))?;
        assert_eq!(held["issue"]["attempt_hold"]["pid"], pid);
        rejected(&args, 4, "conflict")?;
        rejected(
            &["unassign", &n, "--agent", "codex:other"],
            1,
            "attempt_held",
        )?;
        let released = run(issue(&["unassign", &n]))?;
        assert!(released["issue"]["assignee"].is_null());
        assert_eq!(released["issue"]["attempt_hold"]["pid"], pid);
        rejected(&["claim", &n, "--force"], 1, "attempt_held")?;
        Ok(released["issue"].clone())
    };
    for (exit, title) in [
        (0, "Retained validation success"),
        (7, "Retained validation failure"),
    ] {
        let mut child = Service(
            Command::new("/bin/sh")
                .args(["-c", "read -r outcome; exit \"$outcome\""])
                .current_dir(&source)
                .stdin(Stdio::piped())
                .process_group(0)
                .spawn()?,
        );
        let log = root.join(format!("validation-{exit}.log"));
        let held = hold(title, child.0.id(), &log)?;
        let n = held["number"].to_string();
        let inspect = || run(issue(&["attempt", "inspect", &n]));
        let live = inspect()?;
        assert_eq!(live["evidence"]["process"], "live");
        let evidence = root.join("evidence.json");
        fs::write(&evidence, serde_json::to_vec(&live)?)?;
        let version = held["version"].to_string();
        let outcome = format!("Reviewed exit {exit}; retained source has no changes.");
        let reconcile = [
            "attempt",
            "reconcile",
            &n,
            "--if-version",
            &version,
            "--file",
            "evidence.json",
            "--outcome",
            &outcome,
        ];
        rejected(&reconcile, 4, "conflict")?;
        writeln!(child.0.stdin.take().unwrap(), "{exit}")?;
        assert_eq!(child.0.wait()?.code(), Some(exit));
        fs::write(log, format!("validation exited {exit}\n"))?;
        let terminal = inspect()?;
        assert_eq!(terminal["evidence"]["process"], "terminal");
        assert_eq!(terminal["evidence"]["git_status"], "");
        fs::write(&evidence, serde_json::to_vec(&terminal)?)?;
        let reviewed = run(issue(&reconcile))?;
        assert!(reviewed["issue"]["attempt_hold"].is_null());
        assert_eq!(run(issue(&["claim", &n]))?["issue"]["assignee"], owner);
    }

    // The serving process itself provides a live identity for the visual check.
    let held = hold(
        "Protect retained validation while its agent is unavailable",
        std::process::id(),
        &root.join("validation-with-a-long-descriptive-name-for-layout-testing.log"),
    )?;
    Ok(json!({"number":held["number"],"root":root}))
}
