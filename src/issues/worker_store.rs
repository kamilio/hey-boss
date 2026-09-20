//! Worker reservations share the issue transaction and SQLite's write lock.
use super::*;
use crate::issues::worker::{Job, ProjectConfig, now};
pub(super) const HISTORY_INDEX: &str = "CREATE INDEX IF NOT EXISTS worker_issue_history ON worker_runs(project_id,issue_number,finished_at DESC,started_at DESC,id DESC) WHERE finished_at IS NOT NULL;";
type ActiveProcess = (Job, Option<u32>, Option<String>);

// Only the owning agent's deliberate handoff may finish after changing owners.
// A human takeover must still stop the session and invalidate its completion.
fn own_pr_handoff(db: &Connection, job: &Job, issue: &Issue) -> Result<bool> {
    if !job.config.prs_enabled
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
    let mut stmt = db.prepare("SELECT id,issue_number,json_extract(job,'$.issue.title'),actor_id,session_id,state,pid,started_at,updated_at,finished_at,stop_requested,summary,last_event,goal FROM worker_runs WHERE project_id=?1 ORDER BY started_at DESC,id DESC LIMIT 20")?;
    let rows = stmt.query_map([&project.id], |r| Ok(json!({
        "id":r.get::<_,String>(0)?,"number":r.get::<_,i64>(1)?,"title":r.get::<_,String>(2)?,
        "actor_id":r.get::<_,String>(3)?,"session_id":r.get::<_,Option<String>>(4)?,"state":r.get::<_,String>(5)?,
        "pid":r.get::<_,Option<u32>>(6)?,"started_at":r.get::<_,i64>(7)?,"updated_at":r.get::<_,i64>(8)?,
        "finished_at":r.get::<_,Option<i64>>(9)?,"stop_requested":r.get::<_,bool>(10)?,
        "summary":r.get::<_,String>(11)?,"last_event":r.get::<_,String>(12)?,"goal":r.get::<_,Option<String>>(13)?
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
                json!(get_issue(db, &project.id, n, false)?)
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
    pub(crate) fn worker_prompt(&self, id: &str, text: &str) -> Result<()> {
        self.db.execute(
            "UPDATE worker_runs SET expanded_prompt=?2 WHERE id=?1",
            params![id, text],
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
                eprintln!("Worker progress database busy; skipped an activity update");
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
        let mut state = if own_closed { "completed" } else { state };
        let mut summary: String = summary.chars().take(16_000).collect();
        if state == "completed"
            && !own_closed
            && ((!own && !handed_off)
                || issue.title != job.issue["title"]
                || issue.body != job.issue["body"])
        {
            state = "blocked";
            summary = format!(
                "Issue ownership or requirements changed while Codex worked. Review the session before closing.\n\n{summary}"
            );
        }
        if (own || handed_off) && issue.deleted_at.is_none() && issue.state == "open" {
            let report = format!("### Worker {}\n\n{}", state, summary);
            if state == "completed" && !job.config.prs_enabled {
                mutate(
                    &tx,
                    &job.project,
                    &job.actor,
                    &Operation::Close {
                        number: job.number(),
                        comment: Some(report),
                        force: false,
                    },
                    now(),
                )?;
            } else {
                mutate(
                    &tx,
                    &job.project,
                    &job.actor,
                    &Operation::Comment {
                        number: job.number(),
                        body: report,
                    },
                    now(),
                )?;
                if state == "completed" && job.config.prs_enabled {
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
        }
        tx.execute(
            "UPDATE worker_runs SET state=?2,summary=?3,finished_at=?4,updated_at=?4,retry_allowed=CASE WHEN ?2 IN ('cancelled','interrupted') THEN 1 ELSE retry_allowed END WHERE id=?1",
            params![job.id, state, summary, now()],
        )?;
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
    fn explicit_pr_closure_and_non_pr_completion_still_close() {
        for prs in [false, true] {
            let mut f = HandoffFixture::new(prs);
            if prs {
                f.apply(Operation::Close {
                    number: 1,
                    comment: Some("Source/group explicitly completed".into()),
                    force: false,
                });
            }
            f.store
                .worker_finish(&f.job, "completed", "Finished.")
                .unwrap();
            assert_eq!(f.issue().state, "closed");
            assert_eq!(f.state(), "completed");
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
            let mut other = rusqlite::Connection::open(&path).unwrap();
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
}
