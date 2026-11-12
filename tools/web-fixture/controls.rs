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
    if matches!(scenario, Scenario::JobsUi)
        && req.method() == &Method::Get
        && req.url() == "/fixture-instructions.md"
    {
        let _ = req.respond(
            Response::from_string(include_str!("../../src/jobs/fixtures/original.md"))
                .with_header(Header::from_bytes("Content-Type", "text/markdown").unwrap()),
        );
        return;
    }
    let result = (|| -> Result<(u16, Value)> {
        match (scenario, req.method(), req.url()) {
            (Scenario::JobsUi, Method::Post, "/fixture-session" | "/fixture-calendar-run") => {
                let Ok(body) = serde_json::from_reader::<_, Value>(req.as_reader()) else {
                    return Ok((400, json!({"ok":false})));
                };
                jobs_control(f, project, req.url(), body)
            }
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

fn jobs_control(f: &Fixture, project: &str, path: &str, body: Value) -> Result<(u16, Value)> {
    let invalid = (400, json!({"ok":false}));
    let Some(run) = body["run"].as_str() else {
        return Ok(invalid);
    };
    let project = format!("named:{project}");
    let mut db = rusqlite::Connection::open(&f.db)?;
    if path == "/fixture-session" {
        let tx = db.transaction()?;
        let node: String =
            tx.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
        let session = uuid::Uuid::new_v4().to_string();
        if tx.execute("UPDATE scheduled_job_runs SET session_id=?1,machine=?2 WHERE id=?3 AND project_id=?4 AND state='cancelled'", [&session,&node,run,&project])? == 0 {
            return Ok(invalid);
        }
        let line = json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Synthetic saved job result."}]}});
        fs::write(
            f.root
                .path()
                .join(format!("codex/sessions/rollout-test-{session}.jsonl")),
            format!("{line}\n"),
        )?;
        tx.commit()?;
        return Ok((200, json!({"ok":true,"node":node})));
    }
    let (Some(state), Some(at)) = (body["state"].as_str(), body["at"].as_i64()) else {
        return Ok(invalid);
    };
    if at.unsigned_abs() > (1 << 53) - 1 {
        return Ok(invalid);
    }
    let reason = match state {
        "pending" => Some("Chosen runtime is unavailable; waiting for a connected job service."),
        "running" | "succeeded" => None,
        "failed" => Some("Synthetic runtime failure"),
        "cancelled" => Some("Stopped by user"),
        "skipped" => Some("overlap"),
        _ => return Ok(invalid),
    };
    let start = matches!(state, "running" | "failed" | "succeeded").then_some(at + 180_000);
    let end = (!matches!(state, "running" | "pending")).then_some(at + 240_000);
    let n = db.execute("UPDATE scheduled_job_runs SET scheduled_at=?1,created_at=?2,started_at=?3,finished_at=?4,state=?5,reason=?6 WHERE id=?7 AND project_id=?8 AND (state='skipped')=(?5='skipped')",rusqlite::params![at,at+120_000,start,end,state,reason,run,project])?;
    Ok(if n == 1 {
        (200, json!({"ok":true}))
    } else {
        invalid
    })
}
