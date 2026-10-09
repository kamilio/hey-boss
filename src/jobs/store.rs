//! Authoritative definitions, durable receipts and runner transaction boundaries.
use super::*;
use crate::jobs::{
    Occurrence, Operation as JobOperation, Run, Snapshot, files, schedule::Schedule,
};
use std::path::PathBuf;
#[path = "runner_store.rs"]
mod runner;

const SCHEMA:&str="
CREATE TABLE IF NOT EXISTS scheduled_jobs(
 id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES projects(id),revision INTEGER NOT NULL CHECK(revision>0),
 enabled INTEGER NOT NULL CHECK(enabled IN(0,1)),deleted_at INTEGER,schedule_from INTEGER NOT NULL,next_at INTEGER,
 snapshot TEXT NOT NULL CHECK(json_valid(snapshot)),created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,
 CHECK((enabled=1 AND deleted_at IS NULL AND next_at IS NOT NULL) OR (enabled=0 AND next_at IS NULL)));
CREATE INDEX IF NOT EXISTS scheduled_jobs_due ON scheduled_jobs(next_at,id) WHERE enabled=1 AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS scheduled_jobs_project ON scheduled_jobs(project_id,id);
CREATE TABLE IF NOT EXISTS scheduled_job_revisions(
 job_id TEXT NOT NULL REFERENCES scheduled_jobs(id),revision INTEGER NOT NULL,snapshot TEXT NOT NULL CHECK(json_valid(snapshot)),created_at INTEGER NOT NULL,
 PRIMARY KEY(job_id,revision));
CREATE TABLE IF NOT EXISTS scheduled_job_runs(
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,job_id TEXT NOT NULL,job_revision INTEGER NOT NULL,
 project_id TEXT NOT NULL,scheduled_at INTEGER NOT NULL,trigger TEXT NOT NULL CHECK(trigger IN('scheduled','manual')),request_key TEXT,
 state TEXT NOT NULL CHECK(state IN('pending','running','skipped','succeeded','failed','cancelled')),reason TEXT,
 task_number INTEGER,machine TEXT,session_id TEXT,created_at INTEGER NOT NULL,started_at INTEGER,finished_at INTEGER,
 snapshot TEXT NOT NULL CHECK(json_valid(snapshot)),
 FOREIGN KEY(job_id,job_revision) REFERENCES scheduled_job_revisions(job_id,revision),
 FOREIGN KEY(project_id,task_number) REFERENCES issues(project_id,number),
 CHECK((trigger='scheduled' AND request_key IS NULL) OR (trigger='manual' AND request_key IS NOT NULL)),
 CHECK((state='skipped' AND task_number IS NULL AND finished_at IS NOT NULL) OR (state<>'skipped' AND task_number IS NOT NULL)));
CREATE UNIQUE INDEX IF NOT EXISTS scheduled_job_occurrence ON scheduled_job_runs(job_id,scheduled_at) WHERE trigger='scheduled';
CREATE UNIQUE INDEX IF NOT EXISTS scheduled_job_manual ON scheduled_job_runs(job_id,request_key) WHERE trigger='manual';
CREATE UNIQUE INDEX IF NOT EXISTS scheduled_job_active ON scheduled_job_runs(job_id) WHERE state IN('pending','running');
CREATE INDEX IF NOT EXISTS scheduled_job_history ON scheduled_job_runs(job_id,sequence DESC);
CREATE TABLE IF NOT EXISTS scheduled_job_requests(project_id TEXT NOT NULL,actor TEXT NOT NULL,request_id TEXT NOT NULL,fingerprint TEXT NOT NULL,response TEXT NOT NULL,
 PRIMARY KEY(project_id,actor,request_id));
CREATE TABLE IF NOT EXISTS scheduled_job_revision_copies(job_id TEXT NOT NULL,revision INTEGER NOT NULL,snapshot TEXT NOT NULL,PRIMARY KEY(job_id,revision));
CREATE TABLE IF NOT EXISTS scheduled_job_run_copies(id TEXT PRIMARY KEY,job_id TEXT NOT NULL,revision INTEGER NOT NULL,
 FOREIGN KEY(job_id,revision) REFERENCES scheduled_job_revision_copies(job_id,revision));
";
pub(super) fn migrate(db: &Connection) -> Result<()> {
    if !db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='scheduled_job_run_copies')",[],|r|r.get::<_,bool>(0))? {
        let tx=crate::database::Transaction::new_unchecked(db,TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?; tx.commit()?;
    }
    runner::migrate(db)
}
fn authority(db: &Connection) -> Result<()> {
    if db.query_row("SELECT role='agent' FROM fleet_meta WHERE id=1", [], |r| {
        r.get::<_, bool>(0)
    })? {
        return Err(Error::new(
            "fleet_unavailable",
            "Jobs require the authoritative supervisor; no replica write was made",
        ));
    }
    Ok(())
}
#[derive(Clone, serde::Serialize)]
struct Job {
    id: String,
    project_id: String,
    revision: i64,
    enabled: bool,
    deleted_at: Option<i64>,
    schedule_from: i64,
    next_at: Option<i64>,
    snapshot: Snapshot,
    created_at: i64,
    updated_at: i64,
}
const JOB_COLUMNS: &str = "id,project_id,revision,enabled,deleted_at,schedule_from,next_at,snapshot,created_at,updated_at";
fn snapshot_column(r: &crate::database::Row<'_>, index: usize) -> rusqlite::Result<Snapshot> {
    serde_json::from_str(&r.get::<_, String>(index)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}
fn job_row(r: &crate::database::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: r.get(0)?,
        project_id: r.get(1)?,
        revision: r.get(2)?,
        enabled: r.get(3)?,
        deleted_at: r.get(4)?,
        schedule_from: r.get(5)?,
        next_at: r.get(6)?,
        snapshot: snapshot_column(r, 7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}
fn get_job(db: &Connection, project: &str, id: &str) -> Result<Job> {
    db.query_row(
        &format!("SELECT {JOB_COLUMNS} FROM scheduled_jobs WHERE project_id=?1 AND id=?2"),
        params![project, id],
        job_row,
    )
    .optional()?
    .ok_or_else(|| Error::new("not_found", "Job not found in this project"))
}
const RUN_COLUMNS: &str = "sequence,id,snapshot,scheduled_at,trigger,request_key,state,reason,task_number,machine,session_id,created_at,started_at,finished_at";
fn run_row(r: &crate::database::Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        sequence: r.get(0)?,
        id: r.get(1)?,
        snapshot: snapshot_column(r, 2)?,
        scheduled_at: r.get(3)?,
        trigger: r.get(4)?,
        request_key: r.get(5)?,
        state: r.get(6)?,
        reason: r.get(7)?,
        task_number: r.get(8)?,
        machine: r.get(9)?,
        session_id: r.get(10)?,
        created_at: r.get(11)?,
        started_at: r.get(12)?,
        finished_at: r.get(13)?,
    })
}
fn get_run(db: &Connection, id: &str) -> Result<Run> {
    db.query_row(
        &format!("SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE id=?1"),
        [id],
        run_row,
    )
    .optional()?
    .ok_or_else(|| Error::new("not_found", "Job run not found"))
}
// Indexed, bounded lookups keep controls truthful even when the latest
// occurrence was skipped while an older execution still owns the job.
fn job_summary(db: &Connection, job: Job) -> Result<Value> {
    let last: Option<Run> = db.query_row(
        &format!("SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE job_id=?1 ORDER BY sequence DESC LIMIT 1"),
        [&job.id], run_row,
    ).optional()?;
    let active: Option<Run> = db.query_row(
        &format!("SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE job_id=?1 AND state IN ('pending','running') LIMIT 1"),
        [&job.id], run_row,
    ).optional()?;
    let mut value = serde_json::to_value(job)?;
    value["last_run"] = serde_json::to_value(last)?;
    value["active_run"] = serde_json::to_value(active)?;
    Ok(value)
}
fn existing(db: &Connection, o: &Occurrence) -> Result<Option<Run>> {
    let sql = if o.request_key.is_some() {
        format!(
            "SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE job_id=?1 AND request_key=?2 AND trigger='manual'"
        )
    } else {
        format!(
            "SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE job_id=?1 AND scheduled_at=?2 AND trigger='scheduled'"
        )
    };
    if let Some(key) = &o.request_key {
        Ok(db
            .query_row(&sql, params![o.snapshot.job_id, key], run_row)
            .optional()?)
    } else {
        Ok(db
            .query_row(&sql, params![o.snapshot.job_id, o.scheduled_at], run_row)
            .optional()?)
    }
}
fn receipt(
    db: &Connection,
    project: &str,
    actor: &str,
    key: &str,
    fingerprint: &str,
) -> Result<Option<Value>> {
    let saved:Option<(String,String)>=db.query_row("SELECT fingerprint,response FROM scheduled_job_requests WHERE project_id=?1 AND actor=?2 AND request_id=?3",params![project,actor,key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    match saved {
        None => Ok(None),
        Some((old, response)) if old == fingerprint => Ok(Some(serde_json::from_str(&response)?)),
        Some(_) => Err(Error::conflict(
            "Request ID was already used with different job content",
        )),
    }
}
fn bundle(root: &Path, snapshot: &Snapshot) -> Result<Value> {
    Ok(
        json!({"digest":snapshot.instruction_digest,"markdown":files::load(root,&snapshot.instruction_digest)?}),
    )
}
impl Store {
    pub(crate) fn cache_job_response(&self, response: &Value) -> Result<()> {
        if response.get("instructions").is_none() {
            return Ok(());
        }
        let snapshot: Snapshot = serde_json::from_value(
            response
                .get("snapshot")
                .unwrap_or(&response["run"]["snapshot"])
                .clone(),
        )?;
        if response["instructions"]["digest"] != snapshot.instruction_digest {
            return Err(Error::invalid(
                "Instruction transfer does not match its revision",
            ));
        }
        files::cache_response(&self.job_root()?, response)?;
        let encoded = serde_json::to_string(&snapshot)?;
        let tx =
            crate::database::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO scheduled_job_revision_copies VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
            params![snapshot.job_id, snapshot.revision, encoded],
        )?;
        let stored: String = tx.query_row(
            "SELECT snapshot FROM scheduled_job_revision_copies WHERE job_id=?1 AND revision=?2",
            params![snapshot.job_id, snapshot.revision],
            |r| r.get(0),
        )?;
        if stored != encoded {
            return Err(Error::conflict("An immutable cached revision changed"));
        }
        if let Some(id) = response["run"]["id"].as_str() {
            tx.execute(
                "INSERT INTO scheduled_job_run_copies VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
                params![id, snapshot.job_id, snapshot.revision],
            )?;
            let matches: bool = tx.query_row(
                "SELECT job_id=?2 AND revision=?3 FROM scheduled_job_run_copies WHERE id=?1",
                params![id, snapshot.job_id, snapshot.revision],
                |r| r.get(0),
            )?;
            if !matches {
                return Err(Error::conflict(
                    "A cached run changed its immutable revision",
                ));
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Offline execution prerequisite, populated by an authoritative run read.
    pub fn cached_job_run(&self, id: &str) -> Result<Snapshot> {
        let raw:String=self.db.query_row("SELECT c.snapshot FROM scheduled_job_run_copies r JOIN scheduled_job_revision_copies c ON c.job_id=r.job_id AND c.revision=r.revision WHERE r.id=?1",[id],|r|r.get(0)).optional()?.ok_or_else(||Error::new("not_found","Job run has not been synchronized to this machine"))?;
        let snapshot = serde_json::from_str(&raw)?;
        self.job_instructions(&snapshot)?;
        Ok(snapshot)
    }
    fn job_root(&self) -> Result<PathBuf> {
        Ok(Path::new(
            self.db
                .path()
                .ok_or_else(|| Error::invalid("Jobs require a persistent store"))?,
        )
        .with_extension("jobs"))
    }
    pub(super) fn execute_job_at(&mut self, request: &Request, now: i64) -> Result<Value> {
        validate(request)?;
        let Operation::Job { operation } = &request.operation else {
            return Err(Error::invalid("Expected job operation"));
        };
        operation.validate()?;
        authority(&self.db)?;
        let project = request_project(&self.db, request)?;
        let root = self.job_root()?;
        if let JobOperation::RunNow { id } = operation {
            let key = request
                .request_id
                .as_deref()
                .ok_or_else(|| Error::invalid("Run now requires a stable request_id"))?;
            let occurrence = self.manual_job(&project.id, id, key, now)?;
            let run = self.enqueue_job(&occurrence)?;
            return Ok(with_project(json!({"run":run}), &project));
        }
        if let JobOperation::Stop { id, run_id } = operation {
            self.stop_scheduled_job(&project.id, id, run_id, now)?;
            return Ok(with_project(
                json!({"run":get_run(&self.db, run_id)?,"stop_requested":true}),
                &project,
            ));
        }
        if !operation.writes() {
            let tx = self.db.read_transaction()?;
            let response = match operation {
                JobOperation::View { id } => {
                    json!({"job":job_summary(&tx,get_job(&tx,&project.id,id)?)?})
                }
                JobOperation::List {
                    after,
                    limit,
                    include_deleted,
                } => {
                    let jobs=tx.query_collect(&format!("SELECT {JOB_COLUMNS} FROM scheduled_jobs WHERE project_id=?1 AND id>?2 AND (?3 OR deleted_at IS NULL) ORDER BY id LIMIT ?4"),params![project.id,after.as_deref().unwrap_or(""),include_deleted,limit+1],job_row)?;
                    let next = (jobs.len() > *limit).then(|| jobs[limit - 1].id.clone());
                    let summaries = jobs
                        .into_iter()
                        .take(*limit)
                        .map(|job| job_summary(&tx, job))
                        .collect::<Result<Vec<_>>>()?;
                    json!({"jobs":summaries,"next_cursor":next})
                }
                JobOperation::Preview {
                    cron,
                    timezone,
                    after,
                    through,
                    limit,
                } => {
                    json!({"occurrences":Schedule::parse(cron,timezone)?.preview(*after,*through,*limit)?})
                }
                JobOperation::Next {
                    id,
                    after,
                    through,
                    limit,
                } => {
                    let job = get_job(&tx, &project.id, id)?;
                    let after = (*after).max(job.schedule_from);
                    let dates = if job.enabled && after <= *through {
                        Schedule::parse(
                            &job.snapshot.definition.cron,
                            &job.snapshot.definition.timezone,
                        )?
                        .preview(after, *through, *limit)?
                    } else {
                        Vec::new()
                    };
                    json!({"job_id":id,"revision":job.revision,"occurrences":dates})
                }
                JobOperation::History { id, before, limit } => {
                    get_job(&tx, &project.id, id)?;
                    let runs=tx.query_collect(&format!("SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE job_id=?1 AND sequence<?2 ORDER BY sequence DESC LIMIT ?3"),params![id,before.unwrap_or(i64::MAX),limit+1],run_row)?;
                    let next = (runs.len() > *limit).then(|| runs[limit - 1].sequence);
                    json!({"runs":runs.into_iter().take(*limit).collect::<Vec<_>>(),"next_cursor":next})
                }
                JobOperation::Revision { id, revision } => {
                    get_job(&tx, &project.id, id)?;
                    let raw:String=tx.query_row("SELECT snapshot FROM scheduled_job_revisions WHERE job_id=?1 AND revision=?2",params![id,revision],|r|r.get(0)).optional()?.ok_or_else(||Error::new("not_found","Job revision not found"))?;
                    let snapshot: Snapshot = serde_json::from_str(&raw)?;
                    json!({"snapshot":snapshot,"instructions":bundle(&root,&snapshot)?})
                }
                JobOperation::Run { id, run_id } => {
                    get_job(&tx, &project.id, id)?;
                    let run = get_run(&tx, run_id)?;
                    if run.snapshot.job_id != *id {
                        return Err(Error::new("not_found", "Run does not belong to this job"));
                    }
                    json!({"instructions":bundle(&root,&run.snapshot)?,"run":run})
                }
                _ => unreachable!(),
            };
            return Ok(with_project(response, &project));
        }
        let actor = request.actor.as_ref().unwrap();
        let key=request.request_id.as_deref().ok_or_else(||Error::invalid("Job mutations require a stable request_id; retry the identical request after an uncertain response"))?;
        use sha2::{Digest, Sha256};
        let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(operation)?));
        if let Some(saved) = receipt(&self.db, &project.id, &actor.id, key, &fingerprint)? {
            return Ok(saved);
        }
        // Preflight validation, calendar work and disk fsync never hold the writer.
        let (id, expected, definition, markdown, enabled, deleted) = match operation {
            JobOperation::Create {
                id,
                definition,
                markdown,
                enabled,
            } => (
                id,
                None,
                Some(definition),
                Some(markdown),
                Some(*enabled),
                false,
            ),
            JobOperation::Edit {
                id,
                if_revision,
                definition,
                markdown,
            } => (
                id,
                Some(*if_revision),
                Some(definition),
                markdown.as_ref(),
                None,
                false,
            ),
            JobOperation::SetEnabled {
                id,
                if_revision,
                enabled,
            } => (id, Some(*if_revision), None, None, Some(*enabled), false),
            JobOperation::Delete { id, if_revision } => {
                (id, Some(*if_revision), None, None, Some(false), true)
            }
            _ => unreachable!(),
        };
        let old = if let Some(expected) = expected {
            let old = get_job(&self.db, &project.id, id)?;
            if old.revision != expected || old.deleted_at.is_some() {
                return Err(Error::conflict(
                    "Job changed or was deleted; refresh its revision",
                ));
            }
            Some(old)
        } else {
            None
        };
        let definition = definition
            .cloned()
            .unwrap_or_else(|| old.as_ref().unwrap().snapshot.definition.clone());
        let enabled = enabled.unwrap_or_else(|| old.as_ref().unwrap().enabled);
        let next_at = if enabled {
            Some(Schedule::parse(&definition.cron, &definition.timezone)?.next(now)?)
        } else {
            None
        };
        let instruction_digest = match markdown {
            Some(markdown) => files::persist(&root, markdown)?,
            None => {
                let sha = &old.as_ref().unwrap().snapshot.instruction_digest;
                files::load(&root, sha)?;
                sha.clone()
            }
        };
        let revision = expected.unwrap_or(0) + 1;
        let snapshot = Snapshot {
            job_id: id.clone(),
            project_id: project.id.clone(),
            revision,
            definition,
            instruction_digest,
        };
        let encoded = serde_json::to_string(&snapshot)?;
        let tx =
            crate::database::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        if let Some(saved) = receipt(&tx, &project.id, &actor.id, key, &fingerprint)? {
            return Ok(saved);
        }
        if let Some(expected) = expected {
            let changed=tx.execute("UPDATE scheduled_jobs SET revision=?3,enabled=?4,deleted_at=?5,schedule_from=?6,next_at=?7,snapshot=?8,updated_at=?6 WHERE project_id=?1 AND id=?2 AND revision=?9 AND deleted_at IS NULL",params![project.id,id,revision,enabled,deleted.then_some(now),now,next_at,encoded,expected])?;
            if changed != 1 {
                return Err(Error::conflict(
                    "Job changed or was deleted; refresh its revision",
                ));
            }
        } else {
            let changed=tx.execute("INSERT INTO scheduled_jobs(id,project_id,revision,enabled,schedule_from,next_at,snapshot,created_at,updated_at) VALUES(?1,?2,1,?3,?4,?5,?6,?4,?4) ON CONFLICT(id) DO NOTHING",params![id,project.id,enabled,now,next_at,encoded])?;
            if changed != 1 {
                return Err(Error::conflict("Job ID already exists"));
            }
        }
        tx.execute(
            "INSERT INTO scheduled_job_revisions VALUES(?1,?2,?3,?4)",
            params![id, revision, encoded, now],
        )?;
        tx.execute(
            "INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING",
            params![actor.id, serde_json::to_string(actor)?, now],
        )?;
        let response = with_project(
            json!({"job":get_job(&tx,&project.id,id)?,"changed":true}),
            &project,
        );
        tx.execute(
            "INSERT INTO scheduled_job_requests VALUES(?1,?2,?3,?4,?5)",
            params![
                project.id,
                actor.id,
                key,
                fingerprint,
                serde_json::to_string(&response)?
            ],
        )?;
        tx.commit()?;
        Ok(response)
    }
    /// Indexed, bounded read; no occurrences/tasks are pre-created by previews or a timer.
    pub fn due_jobs(&self, now: i64, limit: usize) -> Result<Vec<Occurrence>> {
        authority(&self.db)?;
        crate::jobs::page(limit)?;
        let jobs=self.db.query_collect(&format!("SELECT {JOB_COLUMNS} FROM scheduled_jobs WHERE enabled=1 AND deleted_at IS NULL AND next_at<=?1 ORDER BY next_at,id LIMIT ?2"),params![now,limit],job_row)?;
        jobs.into_iter()
            .map(|job| {
                let schedule = Schedule::parse(
                    &job.snapshot.definition.cron,
                    &job.snapshot.definition.timezone,
                )?;
                let after = job.next_at.unwrap() - 1;
                let scheduled_at = schedule
                    .latest(after, now)?
                    .ok_or_else(|| Error::invalid("Due cursor has no occurrence"))?;
                Ok(Occurrence {
                    snapshot: job.snapshot,
                    scheduled_at,
                    observed_at: now,
                    expected_next: job.next_at,
                    next_at: Some(schedule.next(now)?),
                    request_key: None,
                })
            })
            .collect()
    }
    /// Manual deduplication is scoped to the stable job ID, including after edits/deletion.
    pub fn manual_job(&self, project: &str, id: &str, key: &str, now: i64) -> Result<Occurrence> {
        authority(&self.db)?;
        identifier(key, "Run now request key", 256)?;
        let job = get_job(&self.db, project, id)?;
        let saved=self.db.query_row(&format!("SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE job_id=?1 AND request_key=?2 AND trigger='manual'"),params![id,key],run_row).optional()?;
        let snapshot = if let Some(saved) = saved {
            saved.snapshot
        } else {
            if job.deleted_at.is_some() {
                return Err(Error::conflict("Job was deleted"));
            }
            job.snapshot
        };
        Ok(Occurrence {
            snapshot,
            scheduled_at: now,
            observed_at: now,
            expected_next: None,
            next_at: None,
            request_key: Some(key.into()),
        })
    }
    /// The runner creates and reserves its task using this transaction. Failure rolls back both.
    /// The callback must do only database work; launch the harness after commit.
    pub fn commit_job_occurrence(
        &mut self,
        o: &Occurrence,
        create_task: impl FnOnce(&Connection, &Snapshot, &str) -> Result<i64>,
    ) -> Result<Run> {
        authority(&self.db)?;
        if let Some(saved) = existing(&self.db, o)? {
            return Ok(saved);
        }
        // Verify runtime instructions before acquiring the writer.
        self.job_instructions(&o.snapshot)?;
        let id = super::super::worker::random_id()?;
        let tx =
            crate::database::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        if let Some(saved) = existing(&tx, o)? {
            return Ok(saved);
        }
        let job = get_job(&tx, &o.snapshot.project_id, &o.snapshot.job_id)?;
        if job.deleted_at.is_some()
            || job.snapshot != o.snapshot
            || (o.request_key.is_none() && (!job.enabled || job.next_at != o.expected_next))
        {
            return Err(Error::conflict(
                "Job changed after the occurrence was prepared",
            ));
        }
        let overlap:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM scheduled_job_runs WHERE job_id=?1 AND state IN('pending','running'))",[&o.snapshot.job_id],|r|r.get(0))?;
        let task_number = if overlap {
            None
        } else {
            Some(create_task(&tx, &o.snapshot, &id)?)
        };
        let state = if overlap { "skipped" } else { "pending" };
        tx.execute("INSERT INTO scheduled_job_runs(id,job_id,job_revision,project_id,scheduled_at,trigger,request_key,state,reason,task_number,created_at,finished_at,snapshot) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![id,o.snapshot.job_id,o.snapshot.revision,o.snapshot.project_id,o.scheduled_at,if o.request_key.is_some(){"manual"}else{"scheduled"},o.request_key,state,overlap.then_some("overlap"),task_number,o.observed_at,overlap.then_some(o.observed_at),serde_json::to_string(&o.snapshot)?])?;
        if let Some(number) = task_number {
            // Publish the marker with the task and occurrence, never in a later tick.
            tx.execute(
                "UPDATE issues SET job_run_id=?3 WHERE project_id=?1 AND number=?2",
                params![o.snapshot.project_id, number, id],
            )?;
            tx.execute(
                "INSERT INTO scheduled_job_owners(run_id,generation) VALUES(?1,1)",
                [&id],
            )?;
        }
        if o.request_key.is_none() {
            tx.execute(
                "UPDATE scheduled_jobs SET next_at=?2 WHERE id=?1",
                params![o.snapshot.job_id, o.next_at],
            )?;
        }
        let run = get_run(&tx, &id)?;
        tx.commit()?;
        Ok(run)
    }
    pub fn start_job_run(
        &mut self,
        id: &str,
        machine: &str,
        session: &str,
        now: i64,
    ) -> Result<Run> {
        authority(&self.db)?;
        identifier(machine, "execution machine", 256)?;
        identifier(session, "execution session", 256)?;
        let tx =
            crate::database::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        let run = get_run(&tx, id)?;
        if run.state == "running"
            && run.machine.as_deref() == Some(machine)
            && run.session_id.as_deref() == Some(session)
        {
            return Ok(run);
        }
        if run.state != "pending" || now < run.created_at {
            return Err(Error::conflict(
                "Only a pending run can start, at or after its creation",
            ));
        }
        tx.execute("UPDATE scheduled_job_runs SET state='running',machine=?2,session_id=?3,started_at=?4 WHERE id=?1",params![id,machine,session,now])?;
        let run = get_run(&tx, id)?;
        tx.commit()?;
        Ok(run)
    }
    pub fn finish_job_run(
        &mut self,
        id: &str,
        state: &str,
        reason: Option<&str>,
        now: i64,
    ) -> Result<Run> {
        authority(&self.db)?;
        if !matches!(state, "succeeded" | "failed" | "cancelled") {
            return Err(Error::invalid("Invalid terminal run state"));
        }
        if let Some(reason) = reason {
            identifier(reason, "run reason", 4096)?;
        }
        let tx =
            crate::database::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        let run = get_run(&tx, id)?;
        if run.state == state && run.reason.as_deref() == reason {
            return Ok(run);
        }
        if !matches!(run.state.as_str(), "pending" | "running")
            || (state == "succeeded" && run.state != "running")
            || now < run.started_at.unwrap_or(run.created_at)
        {
            return Err(Error::conflict("Invalid job run completion transition"));
        }
        tx.execute(
            "UPDATE scheduled_job_runs SET state=?2,reason=?3,finished_at=?4 WHERE id=?1",
            params![id, state, reason, now],
        )?;
        let run = get_run(&tx, id)?;
        tx.commit()?;
        Ok(run)
    }
    /// Works offline for revisions already durably synchronized to this machine.
    pub fn job_instructions(&self, snapshot: &Snapshot) -> Result<String> {
        files::load(&self.job_root()?, &snapshot.instruction_digest)
    }
}
fn with_project(mut response: Value, project: &Project) -> Value {
    response["ok"] = json!(true);
    response["project"] = json!(project);
    response
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
