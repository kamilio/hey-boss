//! Worker reservations share the issue transaction and SQLite's write lock.
use super::*;
use crate::issues::worker::{Job, ProjectConfig, now};
use crate::issues::worker_infrastructure;
pub(super) const HISTORY_INDEX: &str = "CREATE INDEX IF NOT EXISTS worker_issue_history ON worker_runs(project_id,issue_number,finished_at DESC,started_at DESC,id DESC) WHERE finished_at IS NOT NULL;";
type ActiveProcess = (Job, Option<u32>, Option<String>);

impl Store {
    /// Release only a legacy scheduler-created hold whose authoritative block
    /// event and exact comment still match the last failed run. Later human
    /// changes, approval requests, dependencies and live agents are preserved.
    pub(crate) fn worker_release_automatic_holds(&mut self, machine: &str) -> Result<()> {
        const CANDIDATES: &str = "SELECT r.job,r.id FROM issues i
         JOIN worker_runs r ON r.id=(SELECT id FROM worker_runs WHERE project_id=i.project_id AND issue_number=i.number AND finished_at IS NOT NULL ORDER BY finished_at DESC,started_at DESC,id DESC LIMIT 1)
         JOIN events e ON e.id=(SELECT id FROM events WHERE project_id=i.project_id AND issue_number=i.number ORDER BY id DESC LIMIT 1)
         WHERE i.state='blocked' AND i.manual_blocked=1 AND i.deleted_at IS NULL AND i.assignee IS NULL
         AND r.machine=?1 AND r.retry_count=0 AND r.retry_at IS NULL AND r.retry_allowed=0
         AND r.state!='completed' AND r.summary NOT LIKE 'Codex needs input or approval:%'
         AND e.action='blocked' AND e.actor=r.actor_id AND e.created_at BETWEEN r.started_at AND r.finished_at
         AND NOT EXISTS(SELECT 1 FROM worker_runs live WHERE live.project_id=i.project_id AND live.issue_number=i.number AND live.finished_at IS NULL)
         AND EXISTS(SELECT 1 FROM comments c WHERE c.project_id=i.project_id AND c.issue_number=i.number AND c.author=e.actor AND c.created_at=e.created_at AND c.id=(SELECT max(id) FROM comments WHERE project_id=i.project_id AND issue_number=i.number) AND
          (c.body='Automatic retries exhausted after five unsuccessful agent attempts. Review the session findings, resolve the blocker or ask the user for help via hey-boss ask, then reopen to resume pickup.'
           OR r.state='infrastructure_blocked' AND c.body=r.summary AND c.body LIKE '%Automatic pickup is held; this run does not consume an implementation retry.%'))";
        let read = |db: &Connection| -> Result<Vec<(String, String)>> {
            Ok(db
                .prepare(CANDIDATES)?
                .query_map([machine], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        };
        if read(&self.db)?.is_empty() {
            return Ok(());
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (job, id) in read(&tx)? {
            let job: Job = serde_json::from_str(&job)?;
            if super::super::blockers::has_dependencies(&tx, &job.project.id, job.number())? {
                continue;
            }
            mutate(
                &tx,
                &job.project,
                &job.actor,
                &Operation::Reopen {
                    number: job.number(),
                    if_version: None,
                },
                now(),
            )?;
            tx.execute(
                "UPDATE worker_runs SET retry_allowed=0,retry_count=1,retry_at=?2 WHERE id=?1",
                params![id, now() + 30_000],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn worker_database_path(&self) -> Option<std::path::PathBuf> {
        self.db.path().map(std::path::PathBuf::from)
    }

    pub(crate) fn worker_process_identity(
        &self,
        id: &str,
    ) -> Result<Option<(Option<u32>, Option<String>)>> {
        Ok(self
            .db
            .query_row(
                "SELECT pid,process_start FROM worker_runs WHERE id=?1 AND finished_at IS NULL",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
}

fn retry_count(db: &Connection, job: &Job) -> Result<i64> {
    let previous: Option<(i64, bool, String)> = db.query_row(
        "SELECT retry_count,retry_allowed,state FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NOT NULL ORDER BY finished_at DESC,started_at DESC,id DESC LIMIT 1",
        params![job.project.id,job.number()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?;
    let Some((count, reset, state)) = previous else {
        return Ok(1);
    };
    if reset || state == "completed" {
        return Ok(1);
    }
    if count > 0 {
        return Ok(count.saturating_add(1));
    }
    // Older attempts predate the counter. Four consecutive failures already
    // reach the delay cap; never scan an unbounded run history during pickup.
    let history = db.prepare("SELECT state,retry_allowed FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NOT NULL ORDER BY finished_at DESC,started_at DESC,id DESC LIMIT 4")?.query_map(params![job.project.id,job.number()], |r| Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(1 + history
        .iter()
        .take_while(|(state, reset)| state != "completed" && !reset)
        .count() as i64)
}

// Only the owning agent's deliberate handoff may finish after changing owners.
// A human takeover must still stop the session and invalidate its completion.
fn own_pr_handoff(db: &Connection, job: &Job, issue: &Issue) -> Result<bool> {
    if !job.requires_pr()
        || issue.state != "open"
        || issue.deleted_at.is_some()
        || issue.assignee.as_deref() != Some("human:boss")
        || issue.title != job.issue["title"]
        || issue.body != job.issue["body"]
    {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT coalesce((SELECT actor=?3 AND json_extract(data,'$.assignee')='human:boss' AND json_extract(data,'$.previous_assignee')=?3 FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('claimed','unassigned','closed','reopened') ORDER BY id DESC LIMIT 1),0)",
        params![job.project.id, job.number(), job.actor.id],
        |r| r.get(0),
    )?)
}

pub(super) const SCHEMA: &str = "
CREATE TABLE worker_pool(id INTEGER PRIMARY KEY CHECK(id=1),concurrency INTEGER NOT NULL CHECK(concurrency BETWEEN 1 AND 16));
INSERT INTO worker_pool VALUES(1,2);
CREATE TABLE project_workers(project_id TEXT PRIMARY KEY REFERENCES projects(id),config TEXT NOT NULL,version INTEGER NOT NULL,updated_at INTEGER NOT NULL);
CREATE TABLE worker_runs(
 id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES projects(id),issue_number INTEGER NOT NULL,
 job TEXT NOT NULL,actor_id TEXT NOT NULL,session_id TEXT,state TEXT NOT NULL,
 owner_pid INTEGER NOT NULL,owner_start TEXT NOT NULL,machine TEXT NOT NULL,
 pid INTEGER,process_start TEXT,started_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,finished_at INTEGER,
 stop_requested INTEGER NOT NULL DEFAULT 0,retry_allowed INTEGER NOT NULL DEFAULT 0,
 summary TEXT NOT NULL DEFAULT '',last_event TEXT NOT NULL DEFAULT '',goal TEXT,expanded_prompt TEXT NOT NULL DEFAULT '',
 FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number));
CREATE INDEX worker_runs_project ON worker_runs(project_id,started_at DESC);
CREATE UNIQUE INDEX worker_issue_reservation ON worker_runs(project_id,issue_number)
 WHERE finished_at IS NULL;
CREATE TABLE worker_events(id INTEGER PRIMARY KEY,run_id TEXT NOT NULL REFERENCES worker_runs(id),created_at INTEGER NOT NULL,text TEXT NOT NULL);
CREATE INDEX worker_events_run ON worker_events(run_id,id DESC);
";

fn default_config(db: &Connection, project: &Project) -> Result<ProjectConfig> {
    let cwd: Option<String> = db
        .query_row(registry::PROJECT_DIRECTORIES, params![project.id, 1], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(ProjectConfig {
        cwd: cwd.unwrap_or_default(),
        ..ProjectConfig::default()
    })
}

fn config(db: &Connection, project: &Project) -> Result<(ProjectConfig, i64)> {
    let saved: Option<(String, i64)> = db
        .query_row(
            "SELECT config,version FROM project_workers WHERE project_id=?1",
            [&project.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match saved {
        Some((text, version)) => Ok((serde_json::from_str(&text)?, version)),
        None => Ok((default_config(db, project)?, 0)),
    }
}
fn status(db: &Connection, project: &Project) -> Result<Value> {
    let (config, version) = config(db, project)?;
    let concurrency: u32 =
        db.query_row("SELECT concurrency FROM worker_pool WHERE id=1", [], |r| {
            r.get(0)
        })?;
    let active: u32 = db.query_row(
        "SELECT count(*) FROM worker_runs WHERE finished_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let mut stmt = db.prepare("SELECT r.id,r.issue_number,json_extract(r.job,'$.issue.title'),r.actor_id,r.session_id,r.state,r.pid,r.started_at,r.updated_at,r.finished_at,r.stop_requested,r.summary,r.last_event,r.goal,CASE WHEN r.retry_allowed=0 AND i.state='open' AND i.assignee IS NULL AND i.deleted_at IS NULL AND r.id=(SELECT id FROM worker_runs WHERE project_id=r.project_id AND issue_number=r.issue_number AND finished_at IS NOT NULL ORDER BY finished_at DESC,started_at DESC,id DESC LIMIT 1) AND NOT EXISTS(SELECT 1 FROM worker_runs live WHERE live.project_id=r.project_id AND live.issue_number=r.issue_number AND live.finished_at IS NULL) THEN r.retry_at END,r.retry_count FROM worker_runs r JOIN issues i ON i.project_id=r.project_id AND i.number=r.issue_number WHERE r.project_id=?1 ORDER BY r.started_at DESC,r.id DESC LIMIT 20")?;
    let rows = stmt.query_map([&project.id], |r| Ok(json!({
        "id":r.get::<_,String>(0)?,"number":r.get::<_,i64>(1)?,"title":r.get::<_,String>(2)?,
        "actor_id":r.get::<_,String>(3)?,"session_id":r.get::<_,Option<String>>(4)?,"state":r.get::<_,String>(5)?,
        "pid":r.get::<_,Option<u32>>(6)?,"started_at":r.get::<_,i64>(7)?,"updated_at":r.get::<_,i64>(8)?,
        "finished_at":r.get::<_,Option<i64>>(9)?,"stop_requested":r.get::<_,bool>(10)?,
        "summary":r.get::<_,String>(11)?,"last_event":r.get::<_,String>(12)?,"goal":r.get::<_,Option<String>>(13)?,"retry_at":r.get::<_,Option<i64>>(14)?,"retry_count":r.get::<_,i64>(15)?
    })))?;
    let mut runs = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for run in &mut runs {
        if let Some(goal) = run["goal"].as_str() {
            run["goal"] = serde_json::from_str(goal)?;
        }
        let mut events = db.prepare(
            "SELECT created_at,text FROM worker_events WHERE run_id=?1 ORDER BY id DESC LIMIT 12",
        )?;
        run["events"] = json!(
            events
                .query_map([run["id"].as_str().unwrap()], |r| Ok(
                    json!({"at":r.get::<_,i64>(0)?,"text":r.get::<_,String>(1)?})
                ))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        );
    }
    let mut stmt = db.prepare(&format!("SELECT i.number FROM issues i WHERE i.project_id=?1 AND i.state='open' AND i.deleted_at IS NULL AND i.assignee IS NULL
        AND NOT EXISTS(SELECT 1 FROM json_each(?2) wanted WHERE NOT EXISTS(SELECT 1 FROM json_each(i.labels) existing WHERE existing.value=wanted.value))
        {}", super::registry::PICKUP_READY))?;
    let eligible = stmt
        .query_map(
            params![project.id, serde_json::to_string(&config.labels)?],
            |r| r.get::<_, i64>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .len();
    Ok(
        json!({"ok":true,"project":project,"config":config,"version":version,"pool":{"concurrency":concurrency,"active":active},"eligible":eligible,"runs":runs}),
    )
}

pub(super) fn execute(
    db: &Connection,
    project: &Project,
    op: &Operation,
    at: i64,
) -> Result<Value> {
    match op {
        Operation::WorkerPreview { config, number } => {
            let next: Option<i64> = if let Some(number) = number {
                Some(*number)
            } else {
                db.query_row(&format!("SELECT i.number FROM issues i WHERE i.project_id=?1 AND i.state='open' AND i.deleted_at IS NULL AND i.assignee IS NULL
                    AND NOT EXISTS(SELECT 1 FROM json_each(?2) wanted WHERE NOT EXISTS(SELECT 1 FROM json_each(i.labels) existing WHERE existing.value=wanted.value))
                    {} ORDER BY i.sort_order,i.created_at,i.project_id,i.number LIMIT 1", super::registry::PICKUP_READY), params![project.id,serde_json::to_string(&config.labels)?], |r|r.get(0)).optional()?
            };
            let issue = if let Some(n) = next {
                super::subtasks::worker_issue(db, &project.id, n)?
            } else {
                json!({"number":1,"title":"<issue title>","body":"<issue body>"})
            };
            let (mut prompt, goal, objective) =
                crate::issues::worker::preview(config, project, issue);
            if next.is_none() {
                prompt = prompt.replace("issue view 1 --project", "issue view <number> --project");
            }
            return Ok(
                json!({"ok":true,"prompt":prompt,"use_goal":goal,"objective":objective,"number":next}),
            );
        }
        Operation::WorkerRun { run_id } => {
            let result: Option<(String, String)> = db
                .query_row(
                    "SELECT expanded_prompt,job FROM worker_runs WHERE id=?1 AND project_id=?2",
                    params![run_id, project.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (prompt, job) =
                result.ok_or_else(|| Error::new("not_found", "Worker run was not found"))?;
            return Ok(
                json!({"ok":true,"project":project,"prompt":prompt,"config":serde_json::from_str::<Job>(&job)?.config}),
            );
        }
        Operation::WorkerStatus => {}
        Operation::WorkerConfigure {
            config: new,
            if_version,
        } => {
            let (_, version) = config(db, project)?;
            if if_version.is_some_and(|v| v != version) {
                return Err(Error::conflict(
                    "Worker settings changed in another window. Reload settings before saving.",
                ));
            }
            crate::issues::worker::validate_config(new, project)?;
            db.execute("INSERT INTO project_workers VALUES(?1,?2,?3,?4) ON CONFLICT(project_id) DO UPDATE SET config=excluded.config,version=excluded.version,updated_at=excluded.updated_at",
                params![project.id,serde_json::to_string(new)?,version+1,at])?;
        }
        Operation::WorkerPool { concurrency } => {
            if !(1..=16).contains(concurrency) {
                return Err(Error::invalid(
                    "Global concurrency must be between 1 and 16",
                ));
            }
            db.execute(
                "UPDATE worker_pool SET concurrency=?1 WHERE id=1",
                [concurrency],
            )?;
        }
        Operation::WorkerControl { run_id, command } => {
            if command == "start" || command == "pause" || command == "stop_all" {
                let (mut c, version) = config(db, project)?;
                c.enabled = command == "start";
                if c.enabled {
                    crate::issues::worker::validate_config(&c, project)?;
                }
                db.execute("INSERT INTO project_workers VALUES(?1,?2,?3,?4) ON CONFLICT(project_id) DO UPDATE SET config=excluded.config,version=excluded.version,updated_at=excluded.updated_at",
                    params![project.id,serde_json::to_string(&c)?,version+1,at])?;
                if command == "stop_all" {
                    db.execute("UPDATE worker_runs SET stop_requested=1 WHERE project_id=?1 AND finished_at IS NULL", [&project.id])?;
                }
            } else {
                let id = run_id
                    .as_deref()
                    .ok_or_else(|| Error::invalid("A worker run ID is required"))?;
                let changed = match command.as_str() {
                    "stop" => db.execute("UPDATE worker_runs SET stop_requested=1 WHERE id=?1 AND project_id=?2 AND finished_at IS NULL",params![id,project.id])?,
                    "retry" => {
                        let number: Option<i64> = db.query_row("SELECT issue_number FROM worker_runs WHERE id=?1 AND project_id=?2 AND finished_at IS NOT NULL AND state!='completed'",params![id,project.id],|r|r.get(0)).optional()?;
                        let number=number.ok_or_else(||Error::conflict("Only a stopped, blocked or failed run can be retried"))?;
                        if get_issue(db, &project.id, number, false)?.state != "open" {
                            return Err(Error::conflict("Reopen the issue before retrying its agent"));
                        }
                        db.execute("UPDATE worker_runs SET retry_allowed=1 WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NOT NULL",params![project.id,number])?
                    },
                    _ => return Err(Error::invalid("Unknown worker control")),
                };
                if changed == 0 {
                    return Err(Error::conflict(
                        "The worker run has already changed. Refresh before retrying.",
                    ));
                }
            }
        }
        _ => unreachable!(),
    }
    status(db, project)
}

impl Store {
    /// Assign and cancel under one write lock; retries never seize a newer claim.
    pub(crate) fn worker_takeover(&mut self, id: &str, boss: &Actor) -> Result<Value> {
        if boss.id != "human:boss" {
            return Err(Error::invalid("Take over is a Boss action"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved: Option<(String, Option<String>, bool, bool)> = tx.query_row(
            "SELECT r.job,r.session_id,r.finished_at IS NOT NULL,r.stop_requested FROM worker_runs r JOIN projects p ON p.id=r.project_id WHERE r.id=?1 AND p.hidden_at IS NULL",
            [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let (text, session, finished, stopping) =
            saved.ok_or_else(|| Error::conflict("This agent is no longer available"))?;
        let job: Job = serde_json::from_str(&text)?;
        let issue = get_issue(&tx, &job.project.id, job.number(), false)?;
        let repeated = stopping && issue.assignee.as_deref() == Some("human:boss");
        if !repeated {
            if finished
                || issue.state != "open"
                || (issue.assignee.is_some()
                    && issue.assignee.as_deref() != Some(&job.actor.id)
                    && !own_pr_handoff(&tx, &job, &issue)?)
            {
                return Err(Error::conflict(
                    "The agent or issue owner changed. Refresh before taking over.",
                ));
            }
            tx.execute(
                "UPDATE worker_runs SET stop_requested=1,updated_at=?2 WHERE id=?1",
                params![id, now()],
            )?;
            mutate(
                &tx,
                &job.project,
                boss,
                &Operation::AssignBoss {
                    number: job.number(),
                    force: true,
                },
                now(),
            )?;
        }
        tx.commit()?;
        Ok(json!({"ok":true,"stopped":finished,"session_id":session,"directory":job.config.cwd}))
    }

    pub(crate) fn worker_prompt(&self, job: &Job, text: &str) -> Result<()> {
        self.db.execute(
            "UPDATE worker_runs SET expanded_prompt=?2,job=?3 WHERE id=?1",
            params![job.id, text, serde_json::to_string(job)?],
        )?;
        Ok(())
    }
    pub(crate) fn worker_begin_claim(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE worker_runs SET state='awaiting_claim',reservation_expires=?2+coalesce((SELECT json_extract(config,'$.reservation_seconds') FROM issue_workers WHERE id=worker_runs.worker_id),?3)*1000 WHERE id=?1 AND state='awaiting_model' AND claimed_at IS NULL AND finished_at IS NULL", params![id, now(), crate::issues::worker::DEFAULT_CLAIM_TIMEOUT_SECONDS])?;
        Ok(())
    }
    pub(crate) fn worker_model_expired(&self, job: &Job) -> Result<bool> {
        Ok(self.db.query_row("SELECT state='awaiting_model' AND claimed_at IS NULL AND reservation_expires<=?2 FROM worker_runs WHERE id=?1", params![job.id, now()], |row| row.get(0))?)
    }
    /// Atomically reserve within this worker's capacity, leaving the issue unassigned.
    pub(crate) fn worker_reserve(
        &mut self,
        machine: &str,
        worker_id: Option<&str>,
    ) -> Result<Option<Job>> {
        super::registry::reserve(self, machine, worker_id)
    }
    pub(crate) fn worker_process(&mut self, id: &str, pid: u32) -> Result<()> {
        let start = crate::agents::process_identity(pid).ok_or_else(|| {
            Error::new(
                "worker_error",
                "Codex exited before its process could be recorded",
            )
        })?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let at = now();
        tx.execute("UPDATE worker_runs SET pid=?2,process_start=?3,updated_at=?4 WHERE id=?1 AND finished_at IS NULL",params![id,pid,start,at])?;
        tx.execute("INSERT OR IGNORE INTO issue_agent_launches(run_id,project_id,issue_number,launched_at) SELECT id,project_id,issue_number,?2 FROM worker_runs WHERE id=?1 AND finished_at IS NULL", params![id,at])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn worker_attach(&mut self, job: &mut Job, session: &str) -> Result<()> {
        identifier(session, "Codex session", 128)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let issue = get_issue(&tx, &job.project.id, job.number(), true)?;
        if issue.assignee.is_some() || issue.state != "open" || issue.deleted_at.is_some() {
            return Err(Error::conflict(
                "Issue ownership changed before Codex started",
            ));
        }
        let mut actor = job.actor.clone();
        actor.id = format!("codex:{session}");
        actor.kind = "codex".into();
        actor.session_id = Some(session.into());
        actor.source = "issue worker Codex session".into();
        let (pid, start): (Option<u32>, Option<String>) = tx.query_row(
            "SELECT pid,process_start FROM worker_runs WHERE id=?1",
            [&job.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        actor.pid = pid;
        actor.process_start = start;
        tx.execute("INSERT INTO agents VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET metadata=excluded.metadata,last_seen=excluded.last_seen",params![actor.id,serde_json::to_string(&actor)?,now()])?;
        job.actor = actor;
        tx.execute("UPDATE worker_runs SET actor_id=?2,session_id=?3,state='awaiting_model',reservation_expires=?5+900000,job=?4,updated_at=?5 WHERE id=?1",params![job.id,job.actor.id,session,serde_json::to_string(job)?,now()])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn worker_claim_expired(&self, job: &Job) -> Result<bool> {
        Ok(self.db.query_row("SELECT state<>'awaiting_model' AND claimed_at IS NULL AND reservation_expires IS NOT NULL AND reservation_expires<=?2 FROM worker_runs WHERE id=?1",params![job.id,now()],|r|r.get(0))?)
    }
    pub(crate) fn worker_cancelled(&self, job: &Job) -> Result<bool> {
        let (stop, expiry, claimed): (bool, Option<i64>, Option<i64>) = self.db.query_row(
            "SELECT stop_requested,reservation_expires,claimed_at FROM worker_runs WHERE id=?1",
            [&job.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let issue = get_issue(&self.db, &job.project.id, job.number(), true)?;
        let waiting = claimed.is_none();
        Ok(stop
            || expiry.is_some_and(|t| t <= now())
            || issue.deleted_at.is_some()
            || if waiting {
                issue.state != "open"
                    || (issue.assignee.is_some()
                        && issue.assignee.as_deref() != Some(&job.actor.id))
            } else {
                issue.assignee.as_deref() != Some(&job.actor.id)
                    && !(issue.state == "closed"
                        && issue.closed_by.as_deref() == Some(&job.actor.id))
                    && !own_pr_handoff(&self.db, job, &issue)?
            })
    }
    pub(crate) fn worker_event(
        &mut self,
        id: &str,
        text: &str,
        goal: Option<&Value>,
    ) -> Result<()> {
        let text: String = text.chars().take(2000).collect();
        let goal = goal.map(serde_json::to_string).transpose()?;
        let result = (|| -> Result<()> {
            let tx = self
                .db
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let timestamp = now();
            tx.execute(
                "INSERT INTO worker_events(run_id,created_at,text) VALUES(?1,?2,?3)",
                params![id, timestamp, text],
            )?;
            tx.execute("UPDATE worker_runs SET last_event=?2,updated_at=?3,goal=coalesce(?4,goal) WHERE id=?1 AND finished_at IS NULL",params![id,text,timestamp,goal])?;
            tx.execute("DELETE FROM worker_events WHERE run_id=?1 AND id NOT IN(SELECT id FROM worker_events WHERE run_id=?1 ORDER BY id DESC LIMIT 100)",[id])?;
            tx.commit()?;
            Ok(())
        })();
        match result {
            // Progress is observational: a busy logger must not kill Codex.
            Err(error) if error.code == "database_busy" => {
                crate::worker_tui::diagnostics::report(format_args!(
                    "Worker progress database busy; skipped an activity update"
                ));
                Ok(())
            }
            result => result,
        }
    }
    pub(crate) fn worker_finish(&mut self, job: &Job, state: &str, summary: &str) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let done: bool = tx.query_row(
            "SELECT finished_at IS NOT NULL FROM worker_runs WHERE id=?1",
            [&job.id],
            |r| r.get(0),
        )?;
        if done {
            return Ok(());
        }
        let issue = get_issue(&tx, &job.project.id, job.number(), true)?;
        let own = issue.assignee.as_deref() == Some(&job.actor.id);
        let own_closed = issue.state == "closed"
            && issue.closed_by.as_deref() == Some(&job.actor.id)
            && issue.deleted_at.is_none();
        let handed_off = own_pr_handoff(&tx, job, &issue)?;
        let mut state = state;
        let mut summary: String = summary.chars().take(16_000).collect();
        let approval_hold = summary.starts_with("Codex needs input or approval:");
        // A captured outage earlier in the turn must not relabel a later
        // permission request or promise an automatic retry for that request.
        if approval_hold {
            state = "blocked";
        }
        if state == "completed"
            && !own_closed
            && ((!own && !handed_off)
                || issue.title != job.issue["title"]
                || issue.body != job.issue["body"]
                || crate::issues::worker::artifact_task(&json!({"labels": issue.labels}))
                    != crate::issues::worker::artifact_task(&job.issue))
        {
            state = "blocked";
            summary = format!(
                "Issue ownership or requirements changed while Codex worked. Review the session before closing.\n\n{summary}"
            );
        }
        if state == "completed" && !own_closed {
            let delivered_pr = job.requires_pr() && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2 AND purpose IN ('fix','unspecified'))",
                params![job.project.id,job.number()], |r| r.get::<_,bool>(0),
            )?;
            if !delivered_pr {
                state = "failed";
                summary = format!(
                    "Agent ended without resolving the issue or delivering its required PR. Saved work will be retried.\n\n{summary}"
                );
            }
        }
        if matches!(state, "blocked" | "failed" | "startup_failed")
            && !approval_hold
            && worker_infrastructure::unavailable(&summary).is_some()
        {
            state = worker_infrastructure::STATE;
        }
        if state == worker_infrastructure::STATE {
            summary = format!(
                "{}\n\n{summary}",
                worker_infrastructure::unavailable(&summary)
                    .unwrap_or(worker_infrastructure::GUIDANCE)
            );
        }
        if (own || handed_off) && issue.deleted_at.is_none() && issue.state == "open" {
            // Delivery/goal completion is not an issue-resolution decision.
            // Only the owning agent's explicit Close may resolve the issue.
            // Failed attempts remain in run history; retries must not flood the
            // task with duplicate handoff comments or imply successful delivery.
            if state == "completed" {
                mutate(
                    &tx,
                    &job.project,
                    &job.actor,
                    &Operation::Comment {
                        number: job.number(),
                        body: format!("### Worker completed\n\n{summary}"),
                    },
                    now(),
                )?;
            }
            if state == "completed" && job.requires_pr() {
                mutate(
                    &tx,
                    &job.project,
                    &job.actor,
                    &Operation::AssignBoss {
                        number: job.number(),
                        force: false,
                    },
                    now(),
                )?;
            } else if own {
                mutate(
                    &tx,
                    &job.project,
                    &job.actor,
                    &Operation::Unassign {
                        number: job.number(),
                        force: false,
                    },
                    now(),
                )?;
            }
        }
        if approval_hold
            && issue.state == "open"
            && issue.deleted_at.is_none()
            && (own || issue.assignee.is_none())
            && issue.title == job.issue["title"]
            && issue.body == job.issue["body"]
        {
            // A launch can fail before worker_attach registers the reservation's
            // actor. Blocking still writes comments and events referencing it.
            // Register within this transaction without overwriting live metadata.
            tx.execute(
                "INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING",
                params![job.actor.id, serde_json::to_string(&job.actor)?, now()],
            )?;
            mutate(
                &tx,
                &job.project,
                &job.actor,
                &Operation::Block {
                    blockers: None,
                    number: job.number(),
                    comment: Some(format!(
                        "{summary}\n\nResolve this request, then reopen the issue to resume pickup."
                    )),
                    force: false,
                },
                now(),
            )?;
        }
        super::super::blockers::reconcile(&tx, &job.project.id, Some(&job.actor.id), now())?;
        let finished = now();
        let retrying =
            !matches!(state, "completed" | "cancelled" | "interrupted") && !approval_hold;
        let count = if retrying { retry_count(&tx, job)? } else { 0 };
        let retry_at = (retrying
            && issue.state == "open"
            && issue.deleted_at.is_none()
            && (own || issue.assignee.is_none()))
        .then(|| finished + (30_000_i64 * (1 << count.saturating_sub(1).min(4))).min(300_000));
        tx.execute(
            "UPDATE worker_runs SET state=?2,summary=?3,finished_at=?4,updated_at=?4,retry_count=?5,retry_at=?6,retry_allowed=CASE WHEN ?2 IN ('cancelled','interrupted') THEN 1 ELSE retry_allowed END WHERE id=?1",
            params![job.id, state, summary, finished, count, retry_at],
        )?;
        tx.execute("UPDATE agent_steering SET state='rejected',error='The agent stopped before this message was delivered. Saved issue and project instructions remain in place.' WHERE run_id=?1 AND state='queued'", [&job.id])?;
        tx.execute(
            "UPDATE projects SET activity_at=max(activity_at,?2) WHERE id=?1",
            params![job.project.id, now()],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn worker_orphans(&self, machine: &str) -> Result<Vec<ActiveProcess>> {
        let mut stmt=self.db.prepare("SELECT job,pid,process_start FROM worker_runs WHERE finished_at IS NULL AND machine=?1")?;
        let rows = stmt
            .query_map([machine], |r| {
                Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(text, pid, start)| Ok((serde_json::from_str(&text)?, pid, start)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HandoffFixture {
        store: Store,
        job: Job,
        root: std::path::PathBuf,
    }
    impl HandoffFixture {
        fn new(prs: bool) -> Self {
            let root = std::env::temp_dir().join(format!(
                "hb-pr-handoff-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            std::fs::create_dir(&root).unwrap();
            let store = Store::open(&root.join("issues.db")).unwrap();
            let actor = Actor {
                id: "codex:handoff".into(),
                kind: "codex".into(),
                session_id: Some("handoff".into()),
                machine: "unit".into(),
                host: "unit".into(),
                pid: None,
                process_start: None,
                cwd: root.clone(),
                source: "test".into(),
                invocation: None,
                creation_run: None,
                model: None,
            };
            let project = Project {
                id: "named:Handoff".into(),
                name: "Handoff".into(),
            };
            store
                .db
                .execute(
                    "INSERT INTO projects(id,name,next_number) VALUES(?1,?2,2)",
                    params![project.id, project.name],
                )
                .unwrap();
            store
                .db
                .execute(
                    "INSERT INTO agents VALUES(?1,?2,0)",
                    params![actor.id, serde_json::to_string(&actor).unwrap()],
                )
                .unwrap();
            store.db.execute("INSERT INTO issues(project_id,number,title,body,state,assignee,created_by,created_at,updated_at,version,labels) VALUES(?1,1,'Task','Requirements','open',?2,?2,0,0,1,'[]')", params![project.id,actor.id]).unwrap();
            let issue =
                serde_json::to_value(get_issue(&store.db, &project.id, 1, false).unwrap()).unwrap();
            let job = Job {
                id: "handoff-run".into(),
                worker_id: String::new(),
                resume_session: None,
                project,
                issue,
                comments: vec![],
                config: ProjectConfig {
                    prs_enabled: prs,
                    ..Default::default()
                },
                actor,
                owner_pid: 1,
                owner_start: "start".into(),
                machine: "unit".into(),
            };
            store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,claimed_at) VALUES(?1,?2,1,?3,?4,'running',1,'start','unit',0,0,0)", params![job.id,job.project.id,serde_json::to_string(&job).unwrap(),job.actor.id]).unwrap();
            let mut fixture = Self { store, job, root };
            fixture.apply(Operation::Comment {
                number: 1,
                body: "Existing history".into(),
            });
            for (number, purpose) in [
                (1, crate::issues::PrPurpose::Fix),
                (2, crate::issues::PrPurpose::SupportingEvidence),
            ] {
                fixture.apply(Operation::AddPullRequest {
                    number: 1,
                    url: format!("https://github.com/example/repo/pull/{number}"),
                    purpose,
                });
            }
            fixture
        }
        fn apply(&mut self, operation: Operation) {
            self.store
                .execute(&Request {
                    version: 1,
                    project: self.job.project.clone(),
                    project_override: None,
                    actor: Some(self.job.actor.clone()),
                    request_id: None,
                    operation,
                })
                .unwrap();
        }
        fn issue(&self) -> Issue {
            get_issue(&self.store.db, &self.job.project.id, 1, false).unwrap()
        }
        fn state(&self) -> String {
            self.store
                .db
                .query_row(
                    "SELECT state FROM worker_runs WHERE id=?1",
                    [&self.job.id],
                    |r| r.get(0),
                )
                .unwrap()
        }
    }
    impl Drop for HandoffFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn legacy_automatic_holds_resume_but_manual_and_permission_blocks_remain() {
        for mode in [
            "exhausted",
            "infrastructure",
            "manual",
            "approval",
            "later-edit",
        ] {
            let mut f = HandoffFixture::new(false);
            let summary = match mode {
                "infrastructure" => {
                    "Approval service unavailable. Automatic pickup is held; this run does not consume an implementation retry."
                }
                "manual" => "Waiting for a human decision",
                "approval" => "Codex needs input or approval: permission denied",
                _ => {
                    "Automatic retries exhausted after five unsuccessful agent attempts. Review the session findings, resolve the blocker or ask the user for help via hey-boss ask, then reopen to resume pickup."
                }
            };
            f.apply(Operation::Block {
                number: 1,
                blockers: None,
                comment: Some(summary.into()),
                force: false,
            });
            f.store.db.execute("UPDATE worker_runs SET state=?2,summary=?3,finished_at=?4,retry_at=NULL,retry_count=0", params![f.job.id,if mode=="infrastructure" {"infrastructure_blocked"} else {"failed"},summary,now()]).unwrap();
            if mode == "later-edit" {
                f.apply(Operation::Comment {
                    number: 1,
                    body: "Keep this held; I am investigating".into(),
                });
            }
            f.store.worker_release_automatic_holds("unit").unwrap();
            assert_eq!(
                f.issue().state,
                if matches!(mode, "exhausted" | "infrastructure") {
                    "open"
                } else {
                    "blocked"
                },
                "{mode}"
            );
        }
    }

    #[test]
    fn exponential_retry_deadlines_survive_restarts_and_reset_after_manual_retry() {
        let mut f = HandoffFixture::new(false);
        for (attempt, delay) in [30_000, 60_000, 120_000, 240_000, 300_000, 300_000]
            .into_iter()
            .enumerate()
        {
            if attempt > 0 {
                f.apply(Operation::Claim {
                    number: 1,
                    force: false,
                });
                f.job.id = format!("retry-{attempt}");
                f.store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at) VALUES(?1,?2,1,?3,?4,'running',1,'start','unit',?5,?5)", params![f.job.id,f.job.project.id,serde_json::to_string(&f.job).unwrap(),f.job.actor.id,now()]).unwrap();
            }
            f.store
                .worker_finish(&f.job, "failed", "Proxy disconnected")
                .unwrap();
            let path = f.root.join("issues.db");
            f.store = Store::open(&path).unwrap();
            let (wait, count): (i64, i64) = f
                .store
                .db
                .query_row(
                    "SELECT retry_at-finished_at,retry_count FROM worker_runs WHERE id=?1",
                    [&f.job.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(wait, delay);
            assert_eq!(count, attempt as i64 + 1);
            assert!(f.issue().assignee.is_none());
            // Ensure deterministic history ordering even on a millisecond clock.
            f.store
                .db
                .execute(
                    "UPDATE worker_runs SET finished_at=finished_at-1 WHERE id=?1",
                    [&f.job.id],
                )
                .unwrap();
        }
        f.apply(Operation::Reopen {
            number: 1,
            if_version: None,
        });
        assert_eq!(retry_count(&f.store.db, &f.job).unwrap(), 1);
        assert!(f.store.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE project_id=?1 AND number=1)", [&f.job.project.id], |r| r.get::<_, bool>(0)).unwrap());
    }

    #[test]
    fn failed_database_finalization_keeps_the_original_result_for_later_recovery() {
        let mut f = HandoffFixture::new(true);
        let path = f.root.join("issues.db");
        f.store.db.execute_batch("CREATE TRIGGER unavailable_result BEFORE UPDATE OF finished_at ON worker_runs BEGIN SELECT RAISE(ABORT,'database result stream disconnected'); END;").unwrap();
        let start = std::time::Instant::now();
        assert!(
            crate::issues::worker::finish_job(
                &path,
                &mut f.store,
                &f.job,
                "completed",
                "Verified before disconnect"
            )
            .is_err()
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(f.state(), "running");
        assert_eq!(
            crate::issues::worker_results::pending(&path).unwrap().len(),
            1
        );
        f.store
            .db
            .execute_batch("DROP TRIGGER unavailable_result")
            .unwrap();
        f.store = Store::open(&path).unwrap();
        crate::issues::worker::recover(&mut f.store, "unit").unwrap();
        assert_eq!(f.state(), "completed");
        assert!(
            crate::issues::worker_results::pending(&path)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            f.store
                .db
                .query_row(
                    "SELECT count(*) FROM comments WHERE body LIKE '%Verified before disconnect%'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn recovery_uses_the_durable_result_once_instead_of_a_generic_crash() {
        let mut f = HandoffFixture::new(true);
        let path = f.root.join("issues.db");
        crate::issues::worker_results::save(&path, &f.job, "completed", "Saved delivery").unwrap();
        f.store = Store::open(&path).unwrap();
        crate::issues::worker::finish_job(&path, &mut f.store, &f.job, "failed", "Worker died")
            .unwrap();
        assert_eq!(f.state(), "completed");
        assert!(
            !path
                .with_added_extension("worker-results")
                .join(format!("{}.json", f.job.id))
                .exists()
        );
        crate::issues::worker::finish_job(
            &path,
            &mut f.store,
            &f.job,
            "failed",
            "Duplicate recovery",
        )
        .unwrap();
        assert_eq!(
            f.store
                .db
                .query_row(
                    "SELECT count(*) FROM comments WHERE body LIKE '%Saved delivery%'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        let mut wrong = f.job.clone();
        wrong.issue["number"] = json!(2);
        crate::issues::worker_results::save(&path, &f.job, "failed", "Original").unwrap();
        assert!(
            crate::issues::worker_results::save(&path, &wrong, "completed", "Wrong issue").is_err()
        );
    }

    #[test]
    fn failures_keep_retrying_without_blocking_or_retaining_capacity() {
        let mut f = HandoffFixture::new(false);
        for n in 0..6 {
            f.store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at) VALUES(?1,?2,1,'{}',?3,'failed',1,'start','unit',0,0,1)", params![format!("previous-{n}"),f.job.project.id,f.job.actor.id]).unwrap();
        }
        f.store.db.execute("INSERT INTO issue_agent_launches SELECT id,project_id,issue_number,started_at FROM worker_runs", []).unwrap();
        f.store
            .worker_finish(
                &f.job,
                "failed",
                "Agent disconnected before reporting completion",
            )
            .unwrap();
        assert_eq!(
            f.issue().state,
            "open",
            "An agent failure must never require manual reopening"
        );
        assert!(f.issue().assignee.is_none());
        assert_eq!(f.state(), "failed");
        assert_eq!(
            f.store
                .db
                .query_row(
                    "SELECT count(*) FROM worker_runs WHERE finished_at IS NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let (finished, retry): (i64, i64) = f
            .store
            .db
            .query_row(
                "SELECT finished_at,retry_at FROM worker_runs WHERE id=?1",
                [&f.job.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(retry - finished, 300_000);
        assert!(!f.store.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE project_id=?1 AND number=1)", [&f.job.project.id], |r| r.get::<_, bool>(0)).unwrap());
        f.store
            .db
            .execute("UPDATE worker_runs SET retry_at=0 WHERE id=?1", [&f.job.id])
            .unwrap();
        assert!(f.store.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE project_id=?1 AND number=1)", [&f.job.project.id], |r| r.get::<_, bool>(0)).unwrap());
    }

    #[test]
    fn a_crash_after_closing_an_issue_is_still_a_failed_run() {
        let mut f = HandoffFixture::new(false);
        f.apply(Operation::Close {
            number: 1,
            comment: None,
            force: false,
        });
        f.store
            .worker_finish(&f.job, "failed", "Agent transport disconnected")
            .unwrap();
        assert_eq!(
            f.state(),
            "failed",
            "Issue state cannot manufacture successful agent completion"
        );
        assert_eq!(
            f.issue().state,
            "closed",
            "Do not undo an authoritative resolution"
        );
    }

    #[test]
    fn unfinished_delivery_is_not_recorded_as_success() {
        let mut f = HandoffFixture::new(false);
        f.store
            .worker_finish(
                &f.job,
                "completed",
                "Server was unavailable; remaining work is saved",
            )
            .unwrap();
        assert_eq!(f.state(), "failed");
        assert_eq!(f.issue().state, "open");
        assert!(f.issue().assignee.is_none());
    }

    #[test]
    fn repeatedly_failed_reservation_without_registered_actor_can_finalize() {
        let mut f = HandoffFixture::new(false);
        f.apply(Operation::Unassign {
            number: 1,
            force: false,
        });
        f.job.actor.id = format!("reservation:{}", f.job.id);
        f.job.actor.session_id = None;
        f.store.db.execute(
            "UPDATE worker_runs SET actor_id=?2,job=?3,state='reserved',claimed_at=NULL WHERE id=?1",
            params![f.job.id, f.job.actor.id, serde_json::to_string(&f.job).unwrap()],
        ).unwrap();
        for n in 0..4 {
            f.store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at) VALUES(?1,?2,1,'{}',?3,'failed',1,'start','unit',0,0,1)", params![format!("failed-{n}"), f.job.project.id, f.job.actor.id]).unwrap();
        }
        f.store.db.execute("INSERT INTO issue_agent_launches SELECT id,project_id,issue_number,started_at FROM worker_runs", []).unwrap();
        f.store
            .worker_finish(&f.job, "failed", "Session failed before attachment")
            .unwrap();
        assert_eq!(f.issue().state, "open");
        assert!(f.issue().assignee.is_none());
        assert_eq!(f.state(), "failed");
        let version = f.issue().version;
        f.store
            .worker_finish(&f.job, "failed", "Duplicate recovery")
            .unwrap();
        assert_eq!(f.issue().version, version);
        let violations: i64 = f
            .store
            .db
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(violations, 0);
    }

    #[test]
    fn fifth_failure_retries_and_explicit_reopen_resets_backoff() {
        let mut f = HandoffFixture::new(false);
        f.store
            .db
            .execute(
                "UPDATE worker_runs SET session_id='session' WHERE id=?1",
                [&f.job.id],
            )
            .unwrap();
        for n in 0..4 {
            f.store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,session_id) VALUES(?1,?2,1,'{}',?3,'failed',1,'start','unit',0,0,1,'session')", params![format!("failed-{n}"),f.job.project.id,f.job.actor.id]).unwrap();
        }
        f.store.db.execute("INSERT INTO issue_agent_launches SELECT id,project_id,issue_number,started_at FROM worker_runs", []).unwrap();
        f.store
            .worker_finish(&f.job, "failed", "Still unable to resolve dependency")
            .unwrap();
        assert_eq!(f.issue().state, "open");
        assert!(f.issue().assignee.is_none());
        let version = f.issue().version;
        f.store
            .worker_finish(&f.job, "failed", "duplicate")
            .unwrap();
        assert_eq!(f.issue().version, version);
        f.apply(Operation::Reopen {
            number: 1,
            if_version: None,
        });
        assert_eq!(
            f.store
                .db
                .query_row(
                    "SELECT count(*) FROM worker_runs WHERE retry_allowed=0",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0,
            "Reopening releases old holds and cooldowns"
        );
        f.apply(Operation::Claim {
            number: 1,
            force: false,
        });
        f.store
            .db
            .execute(
                "UPDATE worker_runs SET finished_at=NULL,started_at=?2 WHERE id=?1",
                params![f.job.id, now()],
            )
            .unwrap();
        f.store
            .worker_finish(&f.job, "failed", "First new attempt")
            .unwrap();
        assert_eq!(f.issue().state, "open");
    }

    #[test]
    fn retries_preserve_changed_issues_and_human_ownership() {
        for mode in [
            "fourth",
            "unlaunched",
            "unlaunched_current",
            "cancelled",
            "infrastructure_blocked",
            "unassigned",
            "new_owner",
            "changed_scope",
        ] {
            let mut f = HandoffFixture::new(false);
            for n in 0..if mode == "fourth" { 3 } else { 4 } {
                let state = if mode == "cancelled" {
                    "cancelled"
                } else if mode == "infrastructure_blocked" {
                    "infrastructure_blocked"
                } else {
                    "failed"
                };
                f.store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,session_id) VALUES(?1,?2,1,'{}',?3,?4,1,'start','unit',0,0,1,'resumed-session')", params![format!("previous-{n}"),f.job.project.id,f.job.actor.id,state]).unwrap();
            }
            if mode != "unlaunched" {
                f.store.db.execute("INSERT INTO issue_agent_launches SELECT id,project_id,issue_number,started_at FROM worker_runs WHERE finished_at IS NOT NULL", []).unwrap();
            }
            if mode != "unlaunched_current" {
                f.store.db.execute("INSERT INTO issue_agent_launches SELECT id,project_id,issue_number,started_at FROM worker_runs WHERE finished_at IS NULL", []).unwrap();
            }
            match mode {
                "unassigned" => f.apply(Operation::Unassign {
                    number: 1,
                    force: false,
                }),
                "new_owner" => f.apply(Operation::AssignBoss {
                    number: 1,
                    force: false,
                }),
                "changed_scope" => {
                    f.store
                        .db
                        .execute("UPDATE issues SET title='New requirements'", [])
                        .unwrap();
                }
                _ => {}
            }
            f.store
                .worker_finish(
                    &f.job,
                    if mode == "unassigned" {
                        "claim_timeout"
                    } else {
                        "failed"
                    },
                    "Unsuccessful attempt",
                )
                .unwrap();
            assert_eq!(f.issue().state, "open", "{mode}");
            if mode == "new_owner" {
                assert_eq!(f.issue().assignee.as_deref(), Some("human:boss"));
            }
        }
    }

    #[test]
    fn steering_is_scoped_durable_and_idempotent() {
        let mut f = HandoffFixture::new(false);
        let input =
            json!({"scope":"session","text":"Check the narrow layout","request_id":"first"});
        assert_eq!(
            f.store.worker_steer(&f.job.id, &input).unwrap()["state"],
            "queued"
        );
        f.store.worker_steer(&f.job.id, &input).unwrap();
        let queued = f.store.worker_steering(&f.job.id).unwrap().unwrap();
        assert_eq!(queued["text"], "Check the narrow layout");
        assert!(
            f.store
                .worker_steer(
                    &f.job.id,
                    &json!({"text":"Different","scope":"session","request_id":"first"})
                )
                .is_err()
        );
        f.store
            .worker_steering_result("first", "delivered", None)
            .unwrap();
        assert!(f.store.worker_steering(&f.job.id).unwrap().is_none());
        assert_eq!(
            f.store.worker_steer(&f.job.id, &input).unwrap()["state"],
            "delivered"
        );
        f.store.worker_steer(&f.job.id, &json!({"scope":"issue","text":"Preserve keyboard navigation","request_id":"issue"})).unwrap();
        assert!(
            get_issue(&f.store.db, &f.job.project.id, 1, false)
                .unwrap()
                .body
                .contains("Preserve keyboard navigation")
        );
        f.store
            .worker_steer(
                &f.job.id,
                &json!({"scope":"project","text":"Verify dark mode","request_id":"project"}),
            )
            .unwrap();
        assert!(
            registry::project_settings(&f.store.db, &f.job.project).unwrap()["prompt"]
                .as_str()
                .unwrap()
                .contains("Verify dark mode")
        );
        assert_eq!(
            f.store.worker_steering(&f.job.id).unwrap().unwrap()["request_id"],
            "issue"
        );
    }

    #[test]
    fn steering_rejects_stopped_hidden_changed_owner_and_invalid_input() {
        for mode in ["finished", "stopping", "owner", "hidden", "closed"] {
            let mut f = HandoffFixture::new(false);
            match mode {
                "finished" => {
                    f.store
                        .db
                        .execute("UPDATE worker_runs SET finished_at=1", [])
                        .unwrap();
                }
                "stopping" => {
                    f.store
                        .db
                        .execute("UPDATE worker_runs SET stop_requested=1", [])
                        .unwrap();
                }
                "owner" => {
                    f.apply(Operation::AssignBoss {
                        number: 1,
                        force: true,
                    });
                }
                "hidden" => {
                    f.store
                        .db
                        .execute("UPDATE projects SET hidden_at=1", [])
                        .unwrap();
                }
                "closed" => {
                    f.apply(Operation::Close {
                        number: 1,
                        comment: None,
                        force: false,
                    });
                }
                _ => unreachable!(),
            }
            assert!(
                f.store
                    .worker_steer(
                        &f.job.id,
                        &json!({"scope":"issue","text":"New requirement","request_id":mode})
                    )
                    .is_err(),
                "{mode}"
            );
        }
        let mut f = HandoffFixture::new(false);
        for input in [
            json!({"scope":"unknown","text":"Focus","request_id":"invalid"}),
            json!({"scope":"session","text":"  ","request_id":"blank"}),
            json!({"scope":"session","text":"Focus"}),
        ] {
            assert!(f.store.worker_steer(&f.job.id, &input).is_err());
        }
    }

    #[test]
    fn takeover_stops_exact_run_assigns_boss_and_is_idempotent() {
        let mut f = HandoffFixture::new(true);
        let mut boss = f.job.actor.clone();
        boss.id = "human:boss".into();
        boss.kind = "human".into();
        let result = f.store.worker_takeover(&f.job.id, &boss).unwrap();
        assert_eq!(result["stopped"], false);
        assert_eq!(f.issue().assignee.as_deref(), Some("human:boss"));
        assert!(f.store.worker_cancelled(&f.job).unwrap());
        let version = f.issue().version;
        f.store.worker_takeover(&f.job.id, &boss).unwrap();
        assert_eq!(f.issue().version, version);
        f.store
            .worker_finish(&f.job, "stopped", "Taken over")
            .unwrap();
        assert_eq!(
            f.store.worker_takeover(&f.job.id, &boss).unwrap()["stopped"],
            true
        );
        assert_eq!(f.issue().state, "open");
    }

    #[test]
    fn takeover_rejects_changed_ownership_hidden_projects_and_finished_runs() {
        for mode in ["owner", "hidden", "finished", "actor"] {
            let mut f = HandoffFixture::new(false);
            let mut boss = f.job.actor.clone();
            boss.id = "human:boss".into();
            match mode {
                "owner" => {
                    f.store
                        .db
                        .execute("INSERT INTO agents VALUES('someone-else','{}',0)", [])
                        .unwrap();
                    f.store
                        .db
                        .execute("UPDATE issues SET assignee='someone-else'", [])
                        .unwrap();
                }
                "hidden" => {
                    f.store
                        .db
                        .execute("UPDATE projects SET hidden_at=1", [])
                        .unwrap();
                }
                "finished" => {
                    f.store.worker_finish(&f.job, "failed", "Failed").unwrap();
                }
                _ => {
                    boss.id = "agent".into();
                }
            }
            assert!(f.store.worker_takeover(&f.job.id, &boss).is_err(), "{mode}");
            assert!(
                !f.store
                    .db
                    .query_row("SELECT stop_requested FROM worker_runs", [], |r| r
                        .get::<_, bool>(0))
                    .unwrap()
            );
        }
    }

    #[test]
    fn takeover_also_stops_an_agent_that_already_handed_its_issue_to_boss() {
        let mut f = HandoffFixture::new(true);
        f.apply(Operation::AssignBoss {
            number: 1,
            force: false,
        });
        assert!(!f.store.worker_cancelled(&f.job).unwrap());
        let mut boss = f.job.actor.clone();
        boss.id = "human:boss".into();
        f.store.worker_takeover(&f.job.id, &boss).unwrap();
        assert!(f.store.worker_cancelled(&f.job).unwrap());
    }

    #[test]
    fn takeover_rolls_back_stop_when_assignment_fails() {
        let mut f = HandoffFixture::new(false);
        let mut boss = f.job.actor.clone();
        boss.id = "human:boss".into();
        f.store.db.execute_batch("CREATE TRIGGER reject_takeover BEFORE UPDATE ON issues BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(f.store.worker_takeover(&f.job.id, &boss).is_err());
        assert_eq!(f.issue().assignee.as_deref(), Some(f.job.actor.id.as_str()));
        assert!(
            !f.store
                .db
                .query_row("SELECT stop_requested FROM worker_runs", [], |r| r
                    .get::<_, bool>(0))
                .unwrap()
        );
    }

    #[test]
    fn partial_delivery_retries_without_trusting_summary_prose() {
        for summary in [
            "Pushed the partial fix. Filter and citeproc engines remain unimplemented.",
            "Implemented everything. Meaningful checks passed.",
        ] {
            let mut f = HandoffFixture::new(false);
            f.store.worker_finish(&f.job, "completed", summary).unwrap();
            assert_eq!(f.state(), "failed");
            assert_eq!(f.issue().state, "open");
            assert!(f.issue().assignee.is_none());
            assert!(f.issue().closed_at.is_none());
            assert!(f.issue().closed_by.is_none());
            assert_eq!(
                f.store
                    .db
                    .query_row(
                        "SELECT count(*) FROM events WHERE action='closed'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
            assert_eq!(
                f.store
                    .db
                    .query_row(
                        "SELECT body FROM comments ORDER BY id DESC LIMIT 1",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                "Existing history"
            );
            f.store
                .worker_finish(&f.job, "completed", "Duplicate delivery")
                .unwrap();
            assert_eq!(
                f.store
                    .db
                    .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }

    #[test]
    fn pr_completion_hands_open_issue_to_boss_and_preserves_history() {
        let mut f = HandoffFixture::new(true);
        f.store
            .worker_finish(
                &f.job,
                "completed",
                "Fix PR is ready for review; not merged.",
            )
            .unwrap();
        assert_eq!(f.issue().state, "open");
        assert_eq!(f.issue().assignee.as_deref(), Some("human:boss"));
        assert_eq!(f.state(), "completed");
        assert!(f.issue().closed_at.is_none());
        assert_eq!(
            f.store
                .db
                .query_row("SELECT count(*) FROM issue_pull_requests", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            f.store
                .db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            f.store
                .db
                .query_row(
                    "SELECT count(*) FROM events WHERE action='closed'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        f.store
            .worker_finish(&f.job, "completed", "Duplicate report")
            .unwrap();
        assert_eq!(
            f.store
                .db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn agent_pr_handoff_is_not_cancelled_or_reported_as_an_ownership_conflict() {
        let mut f = HandoffFixture::new(true);
        f.apply(Operation::AssignBoss {
            number: 1,
            force: false,
        });
        assert!(!f.store.worker_cancelled(&f.job).unwrap());
        f.store
            .worker_finish(&f.job, "completed", "Ready for Boss.")
            .unwrap();
        assert_eq!(f.state(), "completed");
        assert_eq!(f.issue().state, "open");
        assert_eq!(f.issue().assignee.as_deref(), Some("human:boss"));
        assert_eq!(
            f.store
                .db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn external_boss_takeover_and_changed_requirements_are_not_ready_handoffs() {
        for changed in [false, true] {
            let mut f = HandoffFixture::new(true);
            if changed {
                f.apply(Operation::AssignBoss {
                    number: 1,
                    force: false,
                });
                f.store
                    .db
                    .execute("UPDATE issues SET body='Changed requirements'", [])
                    .unwrap();
            } else {
                let mut boss = f.job.actor.clone();
                boss.id = "human:boss".into();
                f.store
                    .db
                    .execute(
                        "INSERT INTO agents VALUES(?1,?2,0)",
                        params![boss.id, serde_json::to_string(&boss).unwrap()],
                    )
                    .unwrap();
                mutate(
                    &f.store.db,
                    &f.job.project,
                    &boss,
                    &Operation::AssignBoss {
                        number: 1,
                        force: true,
                    },
                    now(),
                )
                .unwrap();
                assert!(f.store.worker_cancelled(&f.job).unwrap());
            }
            f.store
                .worker_finish(&f.job, "completed", "Stale completion")
                .unwrap();
            assert_eq!(f.state(), "blocked");
            assert_eq!(f.issue().state, "open");
            assert_eq!(f.issue().assignee.as_deref(), Some("human:boss"));
        }
    }

    #[test]
    fn explicit_owning_closure_preserves_completed_deliveries_in_both_modes() {
        for prs in [false, true] {
            let mut f = HandoffFixture::new(prs);
            f.apply(Operation::Close {
                number: 1,
                comment: Some("Source/group explicitly completed".into()),
                force: false,
            });
            f.store
                .worker_finish(&f.job, "completed", "Finished.")
                .unwrap();
            assert_eq!(f.issue().state, "closed");
            assert_eq!(f.state(), "completed");
            assert!(f.issue().assignee.is_none());
        }
    }

    #[test]
    fn artifact_delivery_requires_explicit_closure_and_changed_intent_blocks_completion() {
        for changed in [false, true] {
            let mut f = HandoffFixture::new(true);
            f.job.issue["labels"] = json!(["task:plan"]);
            f.store
                .db
                .execute(
                    "UPDATE issues SET labels=?1 WHERE project_id=?2 AND number=1",
                    params![
                        if changed { "[]" } else { "[\"task:plan\"]" },
                        f.job.project.id
                    ],
                )
                .unwrap();
            f.store
                .worker_finish(&f.job, "completed", "Artifact saved.")
                .unwrap();
            assert_eq!(f.issue().state, "open");
            assert_eq!(f.state(), if changed { "blocked" } else { "failed" });
            assert!(f.issue().assignee.is_none());
        }
    }

    #[test]
    fn unfinished_pr_reviews_release_for_retry_without_boss_handoff() {
        let mut f = HandoffFixture::new(true);
        f.store
            .worker_finish(&f.job, "blocked", "Review findings still need fixes.")
            .unwrap();
        assert_eq!(f.issue().state, "open");
        assert!(f.issue().assignee.is_none());
        assert_eq!(f.state(), "blocked");
    }

    #[test]
    fn launches_are_atomic_idempotent_and_backfill_survives_reopening() {
        let root = std::env::temp_dir().join(format!(
            "hb-launches-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        {
            let path = root.join("issues.db");
            let mut store = Store::open(&path).unwrap();
            store.db.execute("INSERT INTO projects(id,name,next_number) VALUES('named:Launches','Launches',2)", []).unwrap();
            store
                .db
                .execute("INSERT INTO agents VALUES('agent','{}',0)", [])
                .unwrap();
            store.db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('named:Launches',1,'Task','','open','agent',0,0,1,'[]')", []).unwrap();
            store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at) VALUES('run','named:Launches',1,'{}','agent','reserved',1,'start','unit',0,0)", []).unwrap();
            assert_eq!(
                get_issue(&store.db, "named:Launches", 1, false)
                    .unwrap()
                    .agent_launch_count,
                0
            );
            let pid = std::process::id();
            store.db.execute_batch("CREATE TRIGGER reject_launch BEFORE INSERT ON issue_agent_launches BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
            assert!(store.worker_process("run", pid).is_err());
            assert!(
                store
                    .db
                    .query_row("SELECT pid IS NULL FROM worker_runs", [], |r| r
                        .get::<_, bool>(0))
                    .unwrap()
            );
            store
                .db
                .execute_batch("DROP TRIGGER reject_launch;")
                .unwrap();
            store.worker_process("run", pid).unwrap();
            store.worker_process("run", pid).unwrap();
            assert_eq!(
                get_issue(&store.db, "named:Launches", 1, false)
                    .unwrap()
                    .agent_launch_count,
                1
            );
            let unchanged = get_issue(&store.db, "named:Launches", 1, false).unwrap();
            assert_eq!((unchanged.version, unchanged.updated_at), (1, 0));
            store.db.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('named:Launches',2,'Other','','open','agent',0,0,1,'[]');
              WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000)
              INSERT INTO issue_agent_launches SELECT 'unrelated-'||x,'named:Launches',2,x FROM n;").unwrap();
            {
                let mut query = store.db.prepare("SELECT count(*) FROM issue_agent_launches WHERE project_id='named:Launches' AND issue_number=1").unwrap();
                assert_eq!(query.query_row([], |r| r.get::<_, i64>(0)).unwrap(), 1);
                assert!(
                    query.get_status(rusqlite::StatementStatus::VmStep) < 100,
                    "Counting one issue must not scan unrelated launches"
                );
            }
            // Simulate the old schema with one launched and one merely reserved run.
            store.db.execute_batch("DROP TABLE issue_agent_launches;
              INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at) VALUES('reserved','named:Launches',1,'{}','agent','blocked',1,'start','unit',0,0,1);
              UPDATE fleet_meta SET role='agent',node='unit';").unwrap();
            drop(store);
            for _ in 0..2 {
                let store = Store::open(&path).unwrap();
                assert_eq!(
                    get_issue(&store.db, "named:Launches", 1, false)
                        .unwrap()
                        .agent_launch_count,
                    1
                );
                assert_eq!(store.db.query_row("SELECT count(*) FROM fleet_outbox WHERE table_name='issue_agent_launches'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn progress_writes_are_atomic_and_contention_is_nonfatal() {
        let root = std::env::temp_dir().join(format!(
            "hb-worker-progress-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        {
            let path = root.join("issues.db");
            let mut store = Store::open(&path).unwrap();
            store.db.execute("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES('named:Progress','Progress',2,0,0)", []).unwrap();
            store
                .db
                .execute("INSERT INTO agents VALUES('agent','{}',0)", [])
                .unwrap();
            store.db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('named:Progress',1,'Task','','open','agent',0,0,1,'[]')", []).unwrap();
            store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at) VALUES('run','named:Progress',1,'{}','agent','running',1,'start','unit',0,0)", []).unwrap();
            store.db.execute_batch("CREATE TRIGGER reject_progress BEFORE UPDATE ON worker_runs BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
            assert!(store.worker_event("run", "must roll back", None).is_err());
            assert_eq!(
                store
                    .db
                    .query_row("SELECT count(*) FROM worker_events", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            store
                .db
                .execute_batch("DROP TRIGGER reject_progress;")
                .unwrap();

            store.db.busy_timeout(Duration::from_millis(25)).unwrap();
            let mut other = crate::database::Connection::open(&path).unwrap();
            let tx = other
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            store
                .worker_event("run", "temporary contention", None)
                .unwrap();
            tx.rollback().unwrap();
            for number in 0..105 {
                store
                    .worker_event("run", &number.to_string(), None)
                    .unwrap();
            }
            assert_eq!(
                store
                    .db
                    .query_row("SELECT count(*) FROM worker_events", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                100
            );
            assert_eq!(
                store
                    .db
                    .query_row(
                        "SELECT last_event FROM worker_runs WHERE id='run'",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                "104"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finalization_reconciles_disconnects_before_and_after_commit() {
        for commit in [false, true] {
            let mut f = HandoffFixture::new(true);
            let path = f.root.join("issues.db");
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            let (connection, transport) =
                crate::database::tests::lose_commit_response(&path, commit);
            f.store.db = connection;
            crate::issues::worker::finish_job(
                &path,
                &mut f.store,
                &f.job,
                "completed",
                "Verified delivery",
            )
            .unwrap();
            transport.join().unwrap();
            assert_eq!(f.state(), "completed");
            // Reconciliation must neither duplicate a committed handoff nor
            // omit a transaction that rolled back with the lost connection.
            assert_eq!(
                f.store
                    .db
                    .query_row(
                        "SELECT count(*) FROM comments WHERE body LIKE '%Verified delivery%'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
            assert_eq!(f.issue().assignee.as_deref(), Some("human:boss"));
            owner.stop();
        }
    }

    #[test]
    fn captured_infrastructure_outages_release_claims_and_retry() {
        let summaries = [
            "Database service disconnected; write outcome may be unknown; mutations are never automatically replayed.",
            "HTTP 504 from local hey-proxy: internal recovery time budget exhausted before a response could be forwarded.",
            "Automatic approval review itself cannot access configured wisp-alpha (404); this is service failure, not an unsafe-action verdict. Final review sweep/check confirmation and cleanup remain pending. Preserve worktree and logs for continuation.",
            "Approval review cannot execute: configured wisp-alpha model returns404. Restore approval-model routing to finish enabled-hook commit, fresh native verification, push and final CI/reviews.",
            "Automatic approval review returned HTTP 404 for missing `wisp-alpha`, preventing the final CI read. Please restore the reviewer so I can finish.",
            "Automatic approval review failed with HTTP 404 for missing `wisp-alpha`, preventing GitHub reads—not an unsafe-action rejection. Please restore the approval service so I can finish verification and cleanup.",
            "Automatic approval review still fails with HTTP 404 for missing `wisp-alpha`, preventing GitHub CI/review reads. Restore the approval service to finish notification and cleanup.",
            "Automatic approval review failed: configured model `wisp-alpha` is unavailable (404). Restore the approval service so I can apply the fixes and finish.",
            "Automatic approval review rejected publication twice because `wisp-alpha` is unavailable (HTTP 404). Restore the approval service so I can finish.",
        ];
        for summary in summaries {
            let mut f = HandoffFixture::new(false);
            f.store
                .db
                .execute("UPDATE worker_runs SET session_id='saved-session'", [])
                .unwrap();
            for n in 0..4 {
                f.store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at) VALUES(?1,?2,1,'{}',?3,'failed',1,'start','unit',0,0,1)", params![format!("failed-{n}"), f.job.project.id, f.job.actor.id]).unwrap();
                f.store.db.execute("INSERT INTO issue_agent_launches(project_id,issue_number,run_id,launched_at) VALUES(?1,1,?2,0)", params![f.job.project.id, format!("failed-{n}")]).unwrap();
            }
            f.store.db.execute("INSERT INTO issue_agent_launches(project_id,issue_number,run_id,launched_at) VALUES(?1,1,?2,0)", params![f.job.project.id, f.job.id]).unwrap();
            f.store.worker_finish(&f.job, "blocked", summary).unwrap();
            assert_eq!(f.state(), "infrastructure_blocked");
            assert_eq!(f.issue().state, "open");
            assert!(f.issue().assignee.is_none());
            let comments = f
                .store
                .db
                .prepare("SELECT body FROM comments WHERE project_id=?1 AND issue_number=1")
                .unwrap()
                .query_map([&f.job.project.id], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(
                !comments
                    .iter()
                    .any(|c| c.contains("Automatic retries exhausted"))
            );
            assert_eq!(comments, vec!["Existing history"]);
            assert!(
                f.store
                    .db
                    .query_row(
                        "SELECT retry_at IS NOT NULL FROM worker_runs WHERE id=?1",
                        [&f.job.id],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap()
            );
            assert_eq!(
                f.store
                    .db
                    .query_row(
                        "SELECT session_id FROM worker_runs WHERE id=?1",
                        [&f.job.id],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                "saved-session"
            );
        }
    }

    #[test]
    fn approval_hold_blocks_the_issue_on_the_first_attempt() {
        let mut f = HandoffFixture::new(true);
        f.store
            .worker_finish(
                &f.job,
                "blocked",
                "Codex needs input or approval: Delete the temporary worktree?",
            )
            .unwrap();
        assert_eq!(f.issue().state, "blocked");
        assert!(f.issue().assignee.is_none());
        f.apply(Operation::Reopen {
            number: 1,
            if_version: None,
        });
        assert_eq!(f.issue().state, "open");
    }
}
