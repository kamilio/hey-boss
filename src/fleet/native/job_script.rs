//! Execute one installed program. The gate holds its process group until custody is durable.
use super::*;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::Path,
    process::{Command, Stdio},
};

pub(super) fn log_path(ctx: &Context, report: &Report) -> PathBuf {
    ctx.state
        .join(format!("job-{}-{}.log", report.run_id, report.generation))
}

pub(super) fn read_log(
    ctx: &Context,
    report: &Report,
    offset: u64,
) -> Result<Option<crate::jobs::execution::LogChunk>> {
    let path = log_path(ctx, report);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    const SIZE: u64 = 64 * 1024;
    let size = file.metadata()?.len();
    if offset > size {
        return Err(replica::invalid("Script log changed during transfer"));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(SIZE).read_to_end(&mut bytes)?;
    // Carry a partial UTF-8 character into the next page, even following invalid bytes.
    if offset + (bytes.len() as u64) < size {
        let tail = bytes.len().saturating_sub(4);
        for i in tail..bytes.len() {
            if bytes[i] >= 0xc2
                && std::str::from_utf8(&bytes[i..]).is_err_and(|e| e.error_len().is_none())
            {
                bytes.truncate(i);
                break;
            }
        }
    }
    let end = offset + bytes.len() as u64;
    Ok(Some(crate::jobs::execution::LogChunk {
        offset,
        output: String::from_utf8_lossy(&bytes).into_owned(),
        next: (end < size).then_some(end),
    }))
}

pub(super) enum Program<'a> {
    Job(&'a str),
    Condition(&'a str),
}

pub(super) fn run(
    ctx: &Context,
    d: &Dispatch,
    report: &mut Report,
    cwd: &str,
    program: Program<'_>,
) -> Result<(String, Option<String>)> {
    let condition = matches!(program, Program::Condition(_));
    let executable = match program {
        Program::Condition(s) | Program::Job(s) => s,
    };
    let path = log_path(ctx, report);
    let mut log = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .read(condition)
        .open(&path)?;
    // The condition needs only its stdout verdict; no artifact survives the check.
    if condition {
        std::fs::remove_file(&path)?;
    } else {
        writeln!(log, "Executable: {executable}\nDirectory: {cwd}\n")?;
    }
    let cwd = Path::new(cwd);
    let program = if executable.contains('/') {
        cwd.join(executable)
    } else {
        let env_path = std::env::var_os("PATH").unwrap_or_default();
        let Some(program) = crate::agent_runtime::find_executable(
            std::env::split_paths(&env_path).map(|dir| cwd.join(dir).join(executable)),
        )?
        else {
            writeln!(log, "Executable not found on PATH: {executable}")?;
            return Ok((
                "failed".into(),
                Some(format!("Executable not found on PATH: {executable}")),
            ));
        };
        program
    };
    let spec = d.run.snapshot.definition.execution.as_ref();
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            r#"IFS= read -r gate || exit 1; exec "$@""#,
            "hey-boss-job",
        ])
        .arg(program)
        .args(spec.map(|s| s.args.as_slice()).unwrap_or(&[]))
        .current_dir(cwd)
        .env("HEY_BOSS_JOB_RUN", &d.run.id)
        .env("HEY_BOSS_ISSUE_PROJECT", &d.run.snapshot.project_id)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(log.try_clone()?)
        .stderr(if condition {
            Stdio::inherit()
        } else {
            Stdio::from(log.try_clone()?)
        });
    command
        .env_remove("HEY_BOSS_EXEC_HARNESS")
        .env_remove("HEY_BOSS_EXEC_MODEL")
        .env_remove("HEY_BOSS_EXEC_EFFORT");
    if let Some(spec) = spec {
        if let Some(h) = &spec.harness {
            command.env("HEY_BOSS_EXEC_HARNESS", h);
        }
        if let Some(m) = &spec.model {
            command.env("HEY_BOSS_EXEC_MODEL", m);
        }
        if let Some(e) = &spec.effort {
            command.env("HEY_BOSS_EXEC_EFFORT", e);
        }
    }
    let mut child = command.spawn()?;
    let pid = child.id();
    // Do not reap before signalling: even if the leader exits, its children share this group.
    let result = (|| -> Result<bool> {
        let start = crate::agents::process_identity(pid)
            .ok_or_else(|| replica::invalid("Cannot verify script process identity"))?;
        if revoked(ctx, d)? || ctx.stopped() {
            return Ok(false);
        }
        ctx.db()?.execute("UPDATE local_job_executions SET pid=?3,process_start=?4,submitted=1 WHERE run_id=?1 AND generation=?2",params![d.run.id,d.generation,pid,start])?;
        if condition {
            report.reason = Some("Checking job condition".into());
        } else {
            report.reason = None;
            report.started_at = Some(now().max(d.run.created_at));
            report.state = "running".into();
        }
        save(ctx, report)?;
        child.stdin.take().unwrap().write_all(b"start\n")?;
        let mut published = std::time::Instant::now();
        loop {
            if condition && published.elapsed() >= Duration::from_secs(60) {
                return Err(replica::invalid("Condition timed out after 60 seconds"));
            }
            if condition && log.metadata()?.len() > 4096 {
                return Err(replica::invalid(
                    "Condition must print only JSON true or false (at most 4096 bytes)",
                ));
            }
            if published.elapsed() >= Duration::from_secs(1) && spec.is_some() {
                report.log = read_log(ctx, report, 0)?;
                save(ctx, report)?;
                published = std::time::Instant::now();
            }
            if revoked(ctx, d)? || ctx.stopped() {
                return Ok(false);
            }
            if crate::agent_process::exited(pid)? {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    })();
    if !matches!(result, Ok(true)) && spec.is_some() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < until && !crate::agent_process::exited(pid)? {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let status = child.wait()?;
    if spec.is_some() {
        report.result = Some(json!({"exit_code":status.code()}));
    }
    for _ in 0..40 {
        if group_empty(pid)? {
            return Ok(if !result? {
                (
                    if report.started_at.is_none() && !condition && revoked(ctx, d)? {
                        "released"
                    } else {
                        "cancelled"
                    }
                    .into(),
                    Some("Script stopped".into()),
                )
            } else if condition && !status.success() {
                (
                    "failed".into(),
                    Some(format!("Condition {executable}: {status}")),
                )
            } else if condition {
                log.rewind()?;
                let mut output = Vec::new();
                log.take(4097).read_to_end(&mut output)?;
                match (output.len() <= 4096)
                    .then(|| serde_json::from_slice::<bool>(&output).ok())
                    .flatten()
                {
                    Some(true) => ("succeeded".into(), None),
                    Some(false) => ("skipped".into(), Some("Condition returned false".into())),
                    None => (
                        "failed".into(),
                        Some("Condition must print only JSON true or false".into()),
                    ),
                }
            } else if status.success() {
                ("succeeded".into(), None)
            } else {
                ("failed".into(), Some(status.to_string()))
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(replica::invalid(
        "Waiting for the owned script process group to stop",
    ))
}
