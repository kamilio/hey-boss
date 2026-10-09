//! Worker-independent job execution, driven by durable supervisor assignments.
use super::{Context, Result, projects, replica};
use crate::{
    agent_runtime::{AgentSession, Event, Launch, ModelSelection, Provider, TurnStatus},
    issues::Store,
    jobs::execution::{Dispatch, Report},
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

fn now() -> i64 {
    crate::issues::worker::now()
}

/// Capability is a property of this service and its checkouts, not its workers.
pub(super) fn capability(ctx: &Context) -> Result<Value> {
    let saved = ctx.read_json(
        &ctx.state.join(
            if ctx
                .db()?
                .query_row("SELECT role='agent' FROM fleet_meta WHERE id=1", [], |r| {
                    r.get::<_, bool>(0)
                })?
            {
                "fleet-agent.json"
            } else {
                "fleet-main.json"
            },
        ),
        json!({}),
    )?;
    let checkouts = projects::project_status(
        saved.get("projects").unwrap_or(&json!({})),
        &projects::resolved(ctx)?,
    );
    let mut paths = BTreeMap::<String, String>::new();
    for (project, entry) in checkouts.as_object().into_iter().flatten() {
        if let Some(path) = entry["resolved_path"]
            .as_str()
            .filter(|p| std::path::Path::new(p).is_dir())
        {
            paths.insert(project.clone(), path.into());
        }
    }
    // Legacy configurations already carry checkout mappings. Enabled/pause/slots
    // have no bearing on whether those configured paths can run a scheduled job.
    for worker in saved["workers"].as_array().into_iter().flatten() {
        for (project, path) in worker["config"]["directories"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if let Some(path) = path.as_str().filter(|p| std::path::Path::new(p).is_dir()) {
                paths.entry(project.clone()).or_insert_with(|| path.into());
            }
        }
    }
    let runtimes: BTreeMap<_, _> = [Provider::Codex, Provider::Claude, Provider::Pi]
        .into_iter()
        .map(|provider| {
            (
                provider.name(),
                match provider.binary() {
                    Ok(_) => json!({"available":true}),
                    Err(e) => json!({"available":false,"reason":e.to_string()}),
                },
            )
        })
        .collect();
    Ok(json!({"available":true,"checkouts":paths,"runtimes":runtimes}))
}

pub(super) fn reports(ctx: &Context) -> Result<Vec<Report>> {
    Ok(ctx
        .db()?
        .query_collect(
            "SELECT report FROM local_job_executions WHERE acknowledged=0 ORDER BY rowid",
            [],
            |r| r.get::<_, String>(0),
        )?
        .into_iter()
        .map(|s| serde_json::from_str(&s))
        .collect::<std::result::Result<_, _>>()?)
}

/// The transfer is repeatable and fenced. Never overwrite a newer generation or
/// regress a revocation. Instructions are verified/fsynced before publication.
pub(super) fn receive(ctx: &Context, dispatch: &Dispatch, markdown: &str) -> Result<()> {
    crate::jobs::validate_id(&dispatch.run.id)?;
    if dispatch.node != ctx.node || dispatch.generation < 1 {
        return Err(replica::invalid(
            "Job dispatch addressed to another machine",
        ));
    };
    if let Some((generation, saved)) = ctx
        .db()?
        .query_row(
            "SELECT generation,dispatch FROM local_job_executions WHERE run_id=?1",
            [&dispatch.run.id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    {
        let saved: Dispatch = serde_json::from_str(&saved)?;
        if generation > dispatch.generation
            || (generation == dispatch.generation
                && saved.run.snapshot == dispatch.run.snapshot
                && (saved.revoke || !dispatch.revoke))
        {
            return Ok(());
        };
    }
    let store = Store::open(&ctx.path)?;
    store.cache_job_response(&json!({"run":dispatch.run,"instructions":{"digest":dispatch.run.snapshot.instruction_digest,"markdown":markdown}}))?;
    let db = ctx.db()?;
    let tx = crate::database::Transaction::new_unchecked(&db, TransactionBehavior::Immediate)?;
    let previous: Option<(i64, String)> = tx
        .query_row(
            "SELECT generation,dispatch FROM local_job_executions WHERE run_id=?1",
            [&dispatch.run.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((generation, old)) = previous {
        let old: Dispatch = serde_json::from_str(&old)?;
        if generation > dispatch.generation {
            return Ok(());
        };
        if generation == dispatch.generation {
            if old.run.snapshot != dispatch.run.snapshot {
                return Err(replica::invalid(
                    "Job snapshot changed within its generation",
                ));
            };
            if dispatch.revoke && !old.revoke {
                tx.execute(
                    "UPDATE local_job_executions SET dispatch=?2 WHERE run_id=?1",
                    params![dispatch.run.id, serde_json::to_string(dispatch)?],
                )?;
            }
            tx.commit()?;
            return Ok(());
        }
        let report: Report = serde_json::from_str(&tx.query_row(
            "SELECT report FROM local_job_executions WHERE run_id=?1",
            [&dispatch.run.id],
            |r| r.get::<_, String>(0),
        )?)?;
        if report.state != "released" {
            return Err(replica::invalid(
                "Previous job generation has not acknowledged release",
            ));
        };
    }
    tx.execute("INSERT INTO local_job_executions(run_id,generation,dispatch,report) VALUES(?1,?2,?3,?4) ON CONFLICT(run_id) DO UPDATE SET generation=excluded.generation,dispatch=excluded.dispatch,report=excluded.report,owner_pid=NULL,owner_start=NULL,pid=NULL,process_start=NULL,submitted=0,acknowledged=0",params![dispatch.run.id,dispatch.generation,serde_json::to_string(dispatch)?,serde_json::to_string(&Report::pending(dispatch))?])?;
    tx.commit()?;
    Ok(())
}
fn save(ctx: &Context, report: &Report) -> Result<()> {
    let changed=ctx.db()?.execute("UPDATE local_job_executions SET report=?3,acknowledged=0 WHERE run_id=?1 AND generation=?2",params![report.run_id,report.generation,serde_json::to_string(report)?])?;
    if changed != 1 {
        return Err(replica::invalid("Job execution generation was fenced"));
    };
    Ok(())
}
fn revoked(ctx: &Context, d: &Dispatch) -> Result<bool> {
    let current: Option<(i64, String)> = ctx
        .db()?
        .query_row(
            "SELECT generation,dispatch FROM local_job_executions WHERE run_id=?1",
            [&d.run.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match current {
        Some((g, s)) if g == d.generation => serde_json::from_str::<Dispatch>(&s)?.revoke,
        _ => true,
    })
}
fn finish(ctx: &Context, report: &mut Report, state: &str, reason: Option<String>) -> Result<()> {
    report.state = state.into();
    report.reason = reason.map(|r| r.chars().take(4096).collect());
    report.finished_at = Some(now().max(report.started_at.unwrap_or(0)));
    save(ctx, report)
}

/// One lock per run guards concurrent service ticks and survives reconfiguration.
/// An interrupted executor is recovered, never relaunched from an uncertain phase.
pub(super) fn tick(ctx: &Context) -> Result<()> {
    let rows=ctx.db()?.query_collect("SELECT dispatch,report FROM local_job_executions WHERE json_extract(report,'$.state') IN ('pending','running') ORDER BY rowid",[],|r|Ok::<_,rusqlite::Error>((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?;
    for (dispatch, _report) in rows {
        let d: Dispatch = serde_json::from_str(&dispatch)?;
        let Some(lock) = ctx.lock(&format!("job-{}.lock", d.run.id), false)? else {
            continue;
        };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _lock = lock;
            if let Err(error) = execute(&ctx, &d) {
                // Keep nonterminal custody on a storage/stop failure. The next
                // tick reconciles its recorded group before publishing failure.
                let _ = ctx.atomic_json(
                    &ctx.state.join("jobs-error.json"),
                    &json!({"run":d.run.id,"error":error.to_string(),"at":now()}),
                );
            }
        });
    }
    Ok(())
}

fn group_empty(pid: u32) -> Result<bool> {
    let output = std::process::Command::new("ps")
        .args(["-axo", "pgid=,stat="])
        .output()?;
    if !output.status.success() {
        return Err(replica::invalid(
            "Cannot verify the surviving job process group",
        ));
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut columns = line.split_whitespace();
        if columns.next().and_then(|s| s.parse::<u32>().ok()) == Some(pid)
            && columns.next().is_some_and(|s| !s.starts_with('Z'))
        {
            return Ok(false);
        }
    }
    Ok(true)
}
fn recover_group(pid: u32, identity: &str) -> Result<bool> {
    // Only signal a still-matching leader. If tools survived its disappearance,
    // hold the run rather than risk killing an unrelated reused process group.
    if crate::agents::process_identity(pid).as_deref() == Some(identity) {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        for _ in 0..20 {
            if group_empty(pid)? {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    group_empty(pid)
}
fn execute(ctx: &Context, d: &Dispatch) -> Result<()> {
    let db = ctx.db()?;
    // A queued tick may have observed pending just before the preceding executor
    // committed completion. Re-read under the run lock.
    let mut report: Report = serde_json::from_str(&db.query_row(
        "SELECT report FROM local_job_executions WHERE run_id=?1",
        [&d.run.id],
        |r| r.get::<_, String>(0),
    )?)?;
    if !matches!(report.state.as_str(), "pending" | "running") {
        return Ok(());
    };
    let custody:(Option<u32>,Option<String>,Option<u32>,Option<String>)=db.query_row("SELECT owner_pid,owner_start,pid,process_start FROM local_job_executions WHERE run_id=?1 AND generation=?2",params![d.run.id,d.generation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    if let Some(owner) = custody.0 {
        // The lock is ours, but an older service may still have a live executor.
        if owner != std::process::id() && crate::agents::process_identity(owner) == custody.1 {
            return Ok(());
        };
        if let (Some(pid), Some(start)) = (custody.2, custody.3)
            && !recover_group(pid, &start)?
        {
            report.reason=Some("Execution owner exited; waiting for surviving job processes to stop before reconciliation".into());
            return save(ctx, &report);
        }
        return finish(ctx,&mut report,if revoked(ctx,d)?{"cancelled"}else{"failed"},Some("Execution service restarted; interrupted attempt was stopped and will not be replayed".into()));
    }
    if revoked(ctx, d)? {
        return finish(
            ctx,
            &mut report,
            "released",
            Some("Revoked before launch".into()),
        );
    };
    let owner = std::process::id();
    let start = crate::agents::process_identity(owner)
        .ok_or_else(|| replica::invalid("Cannot verify job service process identity"))?;
    db.execute("UPDATE local_job_executions SET owner_pid=?3,owner_start=?4 WHERE run_id=?1 AND generation=?2",params![d.run.id,d.generation,owner,start])?;
    let result = run_session(ctx, d, &mut report);
    match result {
        Ok((state, reason)) => finish(ctx, &mut report, &state, reason),
        Err(error) => {
            // AgentSession drops its owned group on all normal errors. Recovery
            // still verifies no live descendants before the terminal report.
            let process:Option<(u32,String)>=db.query_row("SELECT pid,process_start FROM local_job_executions WHERE run_id=?1 AND pid IS NOT NULL",[&d.run.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((pid, start)) = process
                && !recover_group(pid, &start)?
            {
                return Err(error);
            }
            finish(
                ctx,
                &mut report,
                if revoked(ctx, d)? {
                    "cancelled"
                } else {
                    "failed"
                },
                Some(error.to_string()),
            )
        }
    }
}
fn stop_agent(agent: &mut AgentSession) -> Result<()> {
    let pid = agent.pid();
    agent.stop()?;
    for _ in 0..40 {
        if group_empty(pid)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(replica::invalid(
        "Waiting for the owned job process group to stop",
    ))
}
fn run_session(
    ctx: &Context,
    d: &Dispatch,
    report: &mut Report,
) -> Result<(String, Option<String>)> {
    let capability = capability(ctx)?;
    let cwd=capability["checkouts"][&d.run.snapshot.project_id].as_str().ok_or_else(||replica::invalid("Project checkout is unavailable on the assigned machine; configure or repair its existing fleet checkout"))?;
    report.cwd = Some(cwd.into());
    let provider: Provider = serde_json::from_value(json!(d.run.snapshot.definition.harness))?;
    let model = &d.run.snapshot.definition.model;
    let model = if provider == Provider::Pi {
        let (route, id) = model
            .split_once('/')
            .ok_or_else(|| replica::invalid("Pi model requires the configured route/model ID"))?;
        ModelSelection {
            id: id.into(),
            route: Some(route.into()),
        }
    } else {
        ModelSelection {
            id: model.clone(),
            route: None,
        }
    };
    let instructions = Store::open(&ctx.path)?.job_instructions(&d.run.snapshot)?;
    let mut env = BTreeMap::new();
    env.insert("HEY_BOSS_JOB_RUN".into(), d.run.id.clone().into());
    env.insert(
        "HEY_BOSS_ISSUE_PROJECT".into(),
        d.run.snapshot.project_id.clone().into(),
    );
    let launch = Launch {
        provider,
        model: Some(model),
        binary: None,
        cwd: PathBuf::from(cwd),
        resume: None,
        env,
        output_schema: None,
    };
    let mut agent = AgentSession::launch_recorded(launch, |pid| {
        let start = crate::agents::process_identity(pid)
            .ok_or_else(|| std::io::Error::other("Cannot verify job runtime identity"))?;
        ctx.db().and_then(|db|Ok(db.execute("UPDATE local_job_executions SET pid=?3,process_start=?4 WHERE run_id=?1 AND generation=?2",params![d.run.id,d.generation,pid,start])?)).map_err(std::io::Error::other)?;
        Ok(())
    })?;
    if revoked(ctx, d)? || ctx.stopped() {
        stop_agent(&mut agent)?;
        return Ok((
            if revoked(ctx, d)? {
                "released"
            } else {
                "cancelled"
            }
            .into(),
            Some("Stopped before instructions were submitted".into()),
        ));
    };
    // Persist intent before the potentially ambiguous prompt write. Recovery must
    // never submit a second turn after a crash around prompt acknowledgement.
    report.started_at = Some(now().max(d.run.created_at));
    report.session_id = agent.state().session.map(|s| s.id);
    report.state = if report.session_id.is_some() {
        "running"
    } else {
        "pending"
    }
    .into();
    ctx.db()?.execute(
        "UPDATE local_job_executions SET submitted=1 WHERE run_id=?1 AND generation=?2",
        params![d.run.id, d.generation],
    )?;
    save(ctx, report)?;
    agent.prompt(&instructions, None)?;
    loop {
        if revoked(ctx, d)? || ctx.stopped() {
            stop_agent(&mut agent)?;
            return Ok((
                "cancelled".into(),
                Some("Owned job execution stopped".into()),
            ));
        }
        match agent.receive(Duration::from_millis(250))? {
            Some(Event::Session(session)) => {
                report.session_id = Some(session.id);
                report.state = "running".into();
                save(ctx, report)?;
            }
            Some(Event::TurnCompleted { status, .. }) => {
                report.session_id = agent
                    .state()
                    .session
                    .map(|s| s.id)
                    .or(report.session_id.take());
                stop_agent(&mut agent)?;
                return Ok(match status {
                    TurnStatus::Completed => ("succeeded".into(), None),
                    TurnStatus::Interrupted => (
                        "cancelled".into(),
                        Some("Harness interrupted the job".into()),
                    ),
                    TurnStatus::Failed => (
                        "failed".into(),
                        Some("Harness reported a failed turn; inspect the linked session".into()),
                    ),
                });
            }
            Some(Event::Approval { .. } | Event::Input { .. }) => {
                stop_agent(&mut agent)?;
                return Ok(("failed".into(),Some("Harness requires interactive input or approval; inspect the linked session and run again after resolving it".into())));
            }
            _ => {}
        }
    }
}

pub(super) fn acknowledge(ctx: &Context, report: &Report) -> Result<()> {
    ctx.db()?.execute("UPDATE local_job_executions SET acknowledged=1 WHERE run_id=?1 AND generation=?2 AND report=?3 AND json_extract(report,'$.state') NOT IN ('pending','running')",params![report.run_id,report.generation,serde_json::to_string(report)?])?;
    Ok(())
}

pub(super) fn schedule(ctx: &Context, machines: &[Value]) -> Result<()> {
    let mut store = Store::open(&ctx.path)?;
    for report in reports(ctx)? {
        if store.accept_job_report(&ctx.node, &report)? {
            acknowledge(ctx, &report)?;
        }
    }
    for occurrence in store.due_jobs(now(), 100)? {
        if let Err(error) = store.enqueue_job(&occurrence)
            && error.code != "conflict"
        {
            return Err(error.into());
        }
    }
    for run in store.active_job_runs()? {
        if let Some((node, generation, revoking)) = store.job_owner(&run.id)? {
            if !revoking
                && run.started_at.is_none()
                && let Some(machine) = machines.iter().find(|m| {
                    m["node"] == node
                        && m["state"] == "connected"
                        && super::context::now() - m["heartbeat"].as_f64().unwrap_or(0.0) < 15.0
                        && m["jobs"]["available"] == true
                })
                && (!machine["jobs"]["checkouts"][&run.snapshot.project_id].is_string()
                    || machine["jobs"]["runtimes"][&run.snapshot.definition.harness]["available"]
                        != true)
            {
                store.revoke_job(&run.id, &node, generation)?;
            }
            continue;
        }
        let candidate = machines.iter().find(|m| {
            m["state"] == "connected"
                && super::context::now() - m["heartbeat"].as_f64().unwrap_or(0.0) < 15.0
                && m["jobs"]["available"] == true
                && m["jobs"]["checkouts"][&run.snapshot.project_id].is_string()
                && m["jobs"]["runtimes"][&run.snapshot.definition.harness]["available"] == true
        });
        if let Some(node) = candidate.and_then(|m| m["node"].as_str()) {
            store.assign_job(&run.id, node)?;
        } else {
            store.unavailable_job(&run.id,"No connected job service has the configured project checkout and chosen harness. Repair the fleet checkout or install/authenticate the selected harness.")?;
        }
    }
    for dispatch in store.job_dispatches(&ctx.node)? {
        let markdown = store.job_instructions(&dispatch.run.snapshot)?;
        receive(ctx, &dispatch, &markdown)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
