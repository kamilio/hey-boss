//! Supervisor-only ownership and task lifecycle. No process I/O inside transactions.
use super::*;
use crate::jobs::execution::{Dispatch, Report};

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='job_task_marker_immutable')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let tx = crate::database::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS scheduled_job_owners(
        run_id TEXT PRIMARY KEY REFERENCES scheduled_job_runs(id),node TEXT,generation INTEGER NOT NULL,
        revoking INTEGER NOT NULL DEFAULT 0,stop_requested INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS local_job_executions(run_id TEXT PRIMARY KEY,generation INTEGER NOT NULL,dispatch TEXT NOT NULL,report TEXT NOT NULL,
        owner_pid INTEGER,owner_start TEXT,pid INTEGER,process_start TEXT,submitted INTEGER NOT NULL DEFAULT 0,acknowledged INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS local_jobs_active ON local_job_executions(run_id) WHERE json_extract(report,'$.state') IN ('pending','running');
        CREATE INDEX IF NOT EXISTS local_jobs_unacknowledged ON local_job_executions(run_id) WHERE acknowledged=0;
        CREATE TRIGGER IF NOT EXISTS job_task_marker_immutable BEFORE UPDATE OF job_run_id ON issues
        WHEN OLD.job_run_id IS NOT NULL AND NEW.job_run_id IS NOT OLD.job_run_id
        BEGIN SELECT RAISE(ABORT,'Scheduled task execution ownership is immutable'); END;")?;
    tx.commit()?;
    Ok(())
}

fn progress(db: &Connection, run: &Run, state: &str, reason: Option<&str>, now: i64) -> Result<()> {
    let actor = format!("job:{}", run.id);
    let metadata = json!({"id":actor,"kind":"job","machine":"scheduled-service","session_id":run.session_id,"host":"","cwd":"/","source":"scheduled-job"});
    db.execute("INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET metadata=excluded.metadata,last_seen=excluded.last_seen",params![actor,metadata.to_string(),now])?;
    let comment = format!(
        "Job {state}{}",
        reason.map(|r| format!(": {r}")).unwrap_or_default()
    );
    let comment: String = comment
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(500)
        .collect();
    db.execute("INSERT INTO issue_status_updates(id,project_id,issue_number,author,level,comment,created_at) VALUES(lower(hex(randomblob(16))),?1,?2,?3,?4,?5,max(?6,coalesce((SELECT max(created_at)+1 FROM issue_status_updates WHERE project_id=?1 AND issue_number=?2),?6)))",params![run.snapshot.project_id,run.task_number,actor,match state {"succeeded"=>"green","failed"|"cancelled"=>"red",_=>"orange"},comment,now])?;
    Ok(())
}

impl Store {
    pub fn enqueue_job(&mut self, occurrence: &Occurrence) -> Result<Run> {
        let markdown = self.job_instructions(&occurrence.snapshot)?;
        self.commit_job_occurrence(occurrence, |db,snapshot,id| {
            let number:i64=db.query_row("SELECT next_number FROM projects WHERE id=?1",[&snapshot.project_id],|r|r.get(0))?;
            let actor=format!("job:{id}");
            let now=occurrence.observed_at;
            db.execute("INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3)",params![actor,json!({"id":actor,"kind":"job","machine":"","host":"","cwd":"/","source":"scheduled-job"}).to_string(),now])?;
            db.execute("UPDATE projects SET next_number=next_number+1,issue_order_version=issue_order_version+1 WHERE id=?1",[&snapshot.project_id])?;
            db.execute("INSERT INTO issues(project_id,number,title,body,state,assignee,created_by,created_at,updated_at,version,labels,sort_order,job_run_id) VALUES(?1,?2,?3,?4,'open',?5,?5,?6,?6,1,'[\"job\"]',(SELECT coalesce(min(sort_order),1)-1 FROM issues WHERE project_id=?1),?7)",params![snapshot.project_id,number,snapshot.definition.name,markdown,actor,now,id])?;
            event(db,&snapshot.project_id,number,&actor,"created",now,&json!({"job_id":snapshot.job_id,"job_run_id":id,"placement":"top"}))?;
            Ok(number)
        })
    }
    pub(crate) fn active_job_runs(&self) -> Result<Vec<Run>> {
        authority(&self.db)?;
        Ok(self.db.query_collect(&format!("SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE state IN('pending','running') ORDER BY sequence"),[],run_row)?)
    }
    /// A generation is never reassigned on timeout. Only an acknowledged revoke
    /// before submission can release its owner; submitted work completes once.
    pub(crate) fn assign_job(&self, id: &str, node: &str) -> Result<bool> {
        authority(&self.db)?;
        Ok(self.db.execute("UPDATE scheduled_job_owners SET node=?2 WHERE run_id=?1 AND node IS NULL AND stop_requested=0 AND EXISTS(SELECT 1 FROM scheduled_job_runs WHERE id=?1 AND state='pending')",params![id,node])? == 1)
    }
    pub(crate) fn job_dispatches(&self, node: &str) -> Result<Vec<Dispatch>> {
        authority(&self.db)?;
        let owners=self.db.query_collect("SELECT o.run_id,o.generation,o.revoking FROM scheduled_job_owners o JOIN scheduled_job_runs r ON r.id=o.run_id WHERE o.node=?1 AND r.state IN('pending','running') ORDER BY r.sequence",[node],|r|Ok::<_,rusqlite::Error>((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,bool>(2)?)))?;
        owners
            .into_iter()
            .map(|(id, generation, revoke)| {
                Ok(Dispatch {
                    run: get_run(&self.db, &id)?,
                    node: node.into(),
                    generation,
                    revoke,
                })
            })
            .collect()
    }
    pub(crate) fn job_owner(&self, id: &str) -> Result<Option<(String, i64, bool)>> {
        Ok(self.db.query_row("SELECT node,generation,revoking FROM scheduled_job_owners WHERE run_id=?1 AND node IS NOT NULL",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?)
    }
    pub(crate) fn revoke_job(&self, id: &str, node: &str, generation: i64) -> Result<()> {
        authority(&self.db)?;
        self.db.execute("UPDATE scheduled_job_owners SET revoking=1 WHERE run_id=?1 AND node=?2 AND generation=?3",params![id,node,generation])?;
        Ok(())
    }
    pub(crate) fn unavailable_job(&self, id: &str, reason: &str) -> Result<()> {
        let tx =
            crate::database::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        let run = get_run(&tx, id)?;
        if run.state == "pending" && run.reason.as_deref() != Some(reason) {
            tx.execute(
                "UPDATE scheduled_job_runs SET reason=?2 WHERE id=?1",
                params![id, reason],
            )?;
            progress(
                &tx,
                &run,
                "pending",
                Some(reason),
                super::super::super::worker::now(),
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(super) fn stop_scheduled_job(
        &self,
        project: &str,
        job: &str,
        id: &str,
        now: i64,
    ) -> Result<()> {
        let tx =
            crate::database::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        let run = get_run(&tx, id)?;
        if run.snapshot.project_id != project || run.snapshot.job_id != job {
            return Err(Error::new("not_found", "Run does not belong to this job"));
        }
        if matches!(run.state.as_str(), "pending" | "running") {
            tx.execute(
                "UPDATE scheduled_job_owners SET revoking=1,stop_requested=1 WHERE run_id=?1",
                [id],
            )?;
            let assigned: bool = tx.query_row(
                "SELECT node IS NOT NULL FROM scheduled_job_owners WHERE run_id=?1",
                [id],
                |r| r.get(0),
            )?;
            if !assigned {
                complete(&tx, &run, "cancelled", Some("Stopped before dispatch"), now)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn accept_job_report(&self, node: &str, report: &Report) -> Result<bool> {
        authority(&self.db)?;
        let tx =
            crate::database::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        let owner:Option<(bool,bool)>=tx.query_row("SELECT revoking,stop_requested FROM scheduled_job_owners WHERE run_id=?1 AND node=?2 AND generation=?3",params![report.run_id,node,report.generation],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((revoking, stop_requested)) = owner else {
            return Ok(false);
        };
        let mut run = get_run(&tx, &report.run_id)?;
        if !matches!(run.state.as_str(), "pending" | "running") {
            return Ok(true);
        };
        if report.state == "pending" {
            if run.reason != report.reason && report.reason.is_some() {
                tx.execute(
                    "UPDATE scheduled_job_runs SET reason=?2 WHERE id=?1",
                    params![run.id, report.reason],
                )?;
                progress(
                    &tx,
                    &run,
                    "pending",
                    report.reason.as_deref(),
                    super::super::super::worker::now(),
                )?;
            }
            tx.commit()?;
            return Ok(true);
        };
        if !matches!(
            report.state.as_str(),
            "running" | "succeeded" | "failed" | "cancelled" | "released"
        ) {
            return Err(Error::invalid("Invalid execution report"));
        };
        if run.session_id.is_some() && run.session_id != report.session_id {
            return Err(Error::conflict("Job session cannot change"));
        };
        if report.state == "released"
            && (!revoking
                || report.started_at.is_some()
                || report.session_id.is_some()
                || run.started_at.is_some())
        {
            return Err(Error::conflict(
                "Only a revoked, unsubmitted job may release ownership",
            ));
        };
        if (report.state == "running" || report.state == "succeeded")
            && (report.started_at.is_none()
                || report.session_id.as_deref().is_none_or(str::is_empty))
        {
            return Err(Error::invalid(
                "Running job requires its exact session and start timestamp",
            ));
        };
        if report.started_at.is_some_and(|at| at < run.created_at)
            || report
                .finished_at
                .is_some_and(|at| at < report.started_at.unwrap_or(run.created_at))
        {
            return Err(Error::invalid("Invalid execution timestamps"));
        };
        if report.state != "running" && report.finished_at.is_none() {
            return Err(Error::invalid("Terminal report requires a stop timestamp"));
        };
        if report.state == "released" && !stop_requested {
            tx.execute("UPDATE scheduled_job_owners SET node=NULL,generation=generation+1,revoking=0 WHERE run_id=?1",[&report.run_id])?;
        } else {
            if run.started_at.is_none()
                && let Some(started_at) = report.started_at
            {
                tx.execute("UPDATE scheduled_job_runs SET state='running',machine=?2,session_id=?3,started_at=?4,reason=NULL WHERE id=?1",params![run.id,node,report.session_id,report.started_at])?;
                tx.execute("INSERT OR IGNORE INTO issue_agent_launches(run_id,project_id,issue_number,launched_at) VALUES(?1,?2,?3,?4)",params![run.id,run.snapshot.project_id,run.task_number,report.started_at])?;
                if let Some(session) = &report.session_id {
                    // Use the exact harness session for task ownership; the job marker
                    // remains authoritative even if a user later clears the assignee.
                    let actor = format!("{}:{}", run.snapshot.definition.harness, session);
                    tx.execute("INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING",params![actor,json!({"id":actor,"kind":run.snapshot.definition.harness,"session_id":report.session_id,"machine":node,"host":"","cwd":report.cwd.as_deref().unwrap_or("/"),"source":"scheduled-job","model":run.snapshot.definition.model}).to_string(),report.started_at])?;
                    tx.execute("UPDATE issues SET assignee=?3,version=version+1,updated_at=?4 WHERE project_id=?1 AND number=?2",params![run.snapshot.project_id,run.task_number,actor,report.started_at])?;
                }
                run = get_run(&tx, &run.id)?;
                progress(&tx, &run, "running", None, started_at)?;
            }
            if report.state != "running" {
                let state = if stop_requested {
                    "cancelled"
                } else {
                    report.state.as_str()
                };
                complete(
                    &tx,
                    &run,
                    state,
                    report.reason.as_deref(),
                    report.finished_at.unwrap(),
                )?;
            }
        }
        tx.commit()?;
        Ok(true)
    }
}
fn complete(db: &Connection, run: &Run, state: &str, reason: Option<&str>, now: i64) -> Result<()> {
    db.execute(
        "UPDATE scheduled_job_runs SET state=?2,reason=?3,finished_at=?4 WHERE id=?1",
        params![run.id, state, reason, now],
    )?;
    db.execute("UPDATE issues SET state=?3,closed_at=?4,closed_by=?5,assignee=NULL,version=version+1,updated_at=?6 WHERE project_id=?1 AND number=?2",params![run.snapshot.project_id,run.task_number,if state=="succeeded"{"closed"}else{"open"},(state=="succeeded").then_some(now),(state=="succeeded").then(||format!("job:{}",run.id)),now])?;
    progress(db, run, state, reason, now)?;
    let actor = format!("job:{}", run.id);
    event(
        db,
        &run.snapshot.project_id,
        run.task_number.unwrap(),
        &actor,
        if state == "succeeded" {
            "closed"
        } else {
            "job_finished"
        },
        now,
        &json!({"job_id":run.snapshot.job_id,"job_run_id":run.id,"state":state,"reason":reason,"session_id":run.session_id}),
    )?;
    super::super::super::blockers::reconcile(db, &run.snapshot.project_id, Some(&actor), now)?;
    Ok(())
}
