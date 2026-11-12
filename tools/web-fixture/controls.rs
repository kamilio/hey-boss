//! Loopback controls used by browser checks to change data while a page is open.
use super::*;
use tiny_http::{Header, Method, Request, Response};

pub(super) fn respond(
    mut req: Request,
    f: &Fixture,
    scenario: Scenario,
    project: &str,
    records: &Value,
    sequence: &mut u32,
) {
    let result = (|| -> Result<(u16, Value)> {
        match (scenario, req.method(), req.url()) {
            (Scenario::IssueStatus, Method::Get, "/fixture/history") => Ok((
                200,
                run(f.issue(project, &["status-history", "1", "--limit", "100"]))?,
            )),
            (Scenario::IssueStatus, Method::Post, "/fixture/status") => {
                *sequence += 1;
                let mut last = Value::Null;
                for step in 1..=3 {
                    last = run(f.issue(project,&["status","1","green","--comment",&format!("A new update arrived while reading history, check {sequence}, step {step}.")]))?;
                }
                Ok((200, last))
            }
            (Scenario::IssueListActions, Method::Post, "/fixture-project") => {
                *sequence += 1;
                let name = format!("Quick actions {sequence}");
                f.init(&name)?;
                Ok((200, json!({"project":format!("named:{name}")})))
            }
            (Scenario::IssueListActions, Method::Post, "/fixture-hold") => {
                let body: Value = serde_json::from_reader(req.as_reader())?;
                let db = rusqlite::Connection::open(&f.db)?;
                let n = db.execute("UPDATE issues SET manual_blocked=1,version=version+1 WHERE project_id=? AND number=7 AND state='blocked'", [body["project"].as_str().ok_or("missing project")?])?;
                if n != 1 {
                    return Err("expected one blocked fixture issue".into());
                }
                Ok((200, json!({"ok":true})))
            }
            (Scenario::ScheduledJobs, Method::Get, "/fixture-runs.json") => {
                Ok((200, records.clone()))
            }
            _ => Ok((404, json!({"ok":false}))),
        }
    })();
    let (status, value) = result.unwrap_or_else(|e| (500, json!({"error":e.to_string()})));
    let _ = req.respond(
        Response::from_string(value.to_string())
            .with_status_code(status)
            .with_header(Header::from_bytes("Content-Type", "application/json").unwrap()),
    );
}

pub(super) fn jobs(f: &Fixture, project: &str) -> Result<Value> {
    let mut records = Vec::new();
    let job = |args: &[&str]| {
        let mut cmd = f.command(&[
            "job",
            "--project",
            project,
            "--agent",
            "human:boss",
            "--json",
        ]);
        cmd.args(args);
        run(cmd)
    };
    let body = include_str!("../../src/jobs/fixtures/original.md");
    fs::write(f.root.path().join("instructions.md"), body)?;
    let db = rusqlite::Connection::open(&f.db)?;
    for (state, title) in [
        ("pending", "Review nightly build results"),
        ("running", "Check dependency updates"),
        ("failed", "Summarize repository activity"),
        ("cancelled", "Inspect the release checklist"),
        ("succeeded", "Review the weekly changelog"),
    ] {
        let id = format!("visual-{state}");
        job(&[
            "create",
            "--id",
            &id,
            "--name",
            title,
            "--cron",
            "0 9 * * *",
            "--timezone",
            "America/Chicago",
            "--harness",
            "codex",
            "--model",
            "exact-model",
            "--instructions",
            "instructions.md",
            "--paused",
        ])?;
        let result = job(&["run-now", &id])?;
        let row = &result["run"];
        let run_id = row["id"].as_str().ok_or("missing run ID")?;
        // New runs have no issue. Recreate the link retained by pre-migration jobs.
        let actor = format!("job:{run_id}");
        let task = run(f.issue(
            project,
            &[
                "--agent", &actor, "create", "--title", title, "--body", body, "--label", "job",
            ],
        ))?;
        let number = task["issue"]["number"]
            .as_i64()
            .ok_or("missing issue number")?;
        let tx = db.unchecked_transaction()?;
        tx.execute(
            "UPDATE issues SET job_run_id=?1,assignee=?2 WHERE project_id=?3 AND number=?4",
            rusqlite::params![run_id, actor, format!("named:{project}"), number],
        )?;
        tx.execute(
            "UPDATE scheduled_job_runs SET task_number=?1 WHERE id=?2",
            rusqlite::params![number, run_id],
        )?;
        if matches!(state, "running" | "succeeded" | "failed") {
            tx.execute(
                "UPDATE scheduled_job_runs SET state=?2, started_at=created_at,
                 finished_at=CASE WHEN ?2<>'running' THEN created_at END WHERE id=?1",
                [run_id, state],
            )?;
        }
        tx.commit()?;
        records.push(json!({"state":state,"number":number,"id":run_id,"title":title}));
        let number = number.to_string();
        let issue = |args: &[&str]| {
            let mut cmd = f.issue(project, &["--agent", "human:boss"]);
            cmd.args(args);
            run(cmd)
        };
        match state {
            "cancelled" => {
                job(&["stop", &id, run_id])?;
            }
            "pending" => {}
            _ => {
                let (level, comment) = match state {
                    "failed" => (
                        "red",
                        "Job failed: Chosen model is unavailable on this machine.",
                    ),
                    "succeeded" => ("green", "Job succeeded"),
                    _ => ("orange", "Job running"),
                };
                issue(&["status", &number, level, "--comment", comment])?;
                if state == "succeeded" {
                    issue(&["close", &number, "--force"])?;
                }
                if state == "failed" {
                    issue(&["unassign", &number, "--force"])?;
                }
            }
        }
    }
    Ok(json!({"records":records}))
}
