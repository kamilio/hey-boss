// Run the native conversation harness in the worker checkout, independent of the desktop chat.
use super::*;
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, process::Command};

pub(super) fn binary() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| replica::invalid("Missing HOME"))?;
    let path = crate::desktop_identity::desktop_app(Path::new(&home))?
        .map(|app| app.join("Contents/Resources/conversation-job"));
    crate::agent_runtime::find_executable(path)?.ok_or_else(|| {
        replica::invalid(
            "Install the current Hey Boss desktop app on this worker to use the hey-boss harness",
        )
    })
}

pub(super) fn run(
    ctx: &Context,
    d: &Dispatch,
    report: &mut Report,
    cwd: &Path,
    input: Option<&Path>,
    binary: &Path,
) -> Result<(String, Option<String>)> {
    let instructions = Store::open(&ctx.path)?.job_instructions(&d.run.snapshot)?;
    let saved: Option<Value> = previous_session(ctx, d, "hey-boss")?
        .map(|s| serde_json::from_str(&s))
        .transpose()?;
    let session = match &saved {
        Some(v) => v["id"]
            .as_str()
            .ok_or_else(|| replica::invalid("Invalid saved Hey Boss session"))?,
        None => &d.run.id,
    };
    crate::jobs::validate_id(session)?;
    let state = ctx
        .state
        .join("job-notebooks")
        .join(format!("{session}.json"));
    if saved.is_some() && !state.with_extension("jsonl").is_file() {
        return Err(replica::invalid(
            "The saved Hey Boss notebook is missing; edit this job to start a new thread",
        ));
    }
    let mut cmd = Command::new(binary);
    cmd.arg(state)
        .current_dir(cwd)
        .env("HEY_BOSS_CLI_PATH", std::env::current_exe()?)
        .env("HEY_BOSS_JOB_RUN", &d.run.id)
        .env("HEY_BOSS_ISSUE_PROJECT", &d.run.snapshot.project_id);
    if let Some(path) = input {
        cmd.env("HEY_BOSS_JOB_FILE", path);
    } else {
        cmd.env_remove("HEY_BOSS_JOB_FILE");
    }
    let mut child = crate::agent_process::Process::spawn_recorded(&mut cmd, |pid| {
        let start = crate::agents::process_identity(pid)
            .ok_or_else(|| std::io::Error::other("Cannot verify Hey Boss job process"))?;
        ctx.db().and_then(|db| Ok(db.execute("UPDATE local_job_executions SET pid=?3,process_start=?4 WHERE run_id=?1 AND generation=?2",params![d.run.id,d.generation,pid,start])?)).map_err(std::io::Error::other)?;
        Ok(())
    })?;
    if revoked(ctx, d)? || ctx.stopped() {
        return Ok((
            if revoked(ctx, d)? {
                "released"
            } else {
                "cancelled"
            }
            .into(),
            Some("Stopped before submission".into()),
        ));
    }
    ctx.db()?.execute(
        "UPDATE local_job_executions SET submitted=1 WHERE run_id=?1 AND generation=?2",
        params![d.run.id, d.generation],
    )?;
    report.started_at = Some(now().max(d.run.created_at));
    save(ctx, report)?;
    child.send(&json!({"turn":instructions}))?;
    let outcome = loop {
        if revoked(ctx, d)? || ctx.stopped() {
            break ("cancelled".into(), Some("Stopped Hey Boss job".into()));
        }
        let Some(event) = child.receive(Duration::from_millis(250))? else {
            continue;
        };
        if let Some(error) = event["error"].as_str() {
            break ("failed".into(), Some(error.into()));
        }
        if event["AgentAccepted"].is_object() {
            remember_thread(ctx, d, &json!({"provider":"hey-boss","id":session}))?;
            report.session_id = Some(session.into());
            report.state = "running".into();
            save(ctx, report)?;
        }
        if let Some(done) = event.get("TurnCompleted") {
            if done["status"] != "Completed" || report.session_id.is_none() {
                return Err(replica::invalid(
                    "Hey Boss did not complete the accepted turn",
                ));
            }
            let mut log = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(script::log_path(ctx, report))?;
            writeln!(
                log,
                "{}",
                done["output"]
                    .as_str()
                    .ok_or_else(|| replica::invalid("Missing Hey Boss reply"))?
            )?;
            break ("succeeded".into(), None);
        }
    };
    let pid = child.pid();
    child.stop()?;
    if !group_empty(pid)? {
        return Err(replica::invalid(
            "Waiting for Hey Boss job processes to stop",
        ));
    }
    Ok(outcome)
}
