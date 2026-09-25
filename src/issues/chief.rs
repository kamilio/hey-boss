//! One organizing turn per hour, independent of issue reservations.
use crate::issues::{Error, Result, Store, worker};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub(in crate::issues) const DEFAULT_PROMPT: &str =
    include_str!("prompts/chief.md").trim_ascii_end();
pub(in crate::issues) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS project_chiefs(
 project_id TEXT NOT NULL REFERENCES projects(id),machine TEXT NOT NULL,cwd TEXT NOT NULL,
 session_id TEXT,next_at INTEGER NOT NULL DEFAULT 0,owner_pid INTEGER,owner_start TEXT,
 pid INTEGER,process_start TEXT,state TEXT NOT NULL DEFAULT 'idle',summary TEXT NOT NULL DEFAULT '',
 worker_id TEXT REFERENCES issue_workers(id),started_at INTEGER,finished_at INTEGER,
 last_event TEXT NOT NULL DEFAULT '',
 PRIMARY KEY(project_id,machine));";
pub(in crate::issues) const ACTIVITY_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS project_chiefs_worker ON project_chiefs(worker_id);";
const INTERVAL_MS: i64 = 60 * 60 * 1000;
type Reservation = (
    i64,
    Option<u32>,
    Option<String>,
    Option<u32>,
    Option<String>,
    Option<String>,
);

pub(in crate::issues) fn migrate(db: &crate::database::Connection) -> Result<()> {
    let columns = db
        .prepare("SELECT name FROM pragma_table_info('project_chiefs')")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let additions = [
        ("worker_id", "TEXT REFERENCES issue_workers(id)"),
        ("started_at", "INTEGER"),
        ("finished_at", "INTEGER"),
        ("last_event", "TEXT NOT NULL DEFAULT ''"),
        ("retry_count", "INTEGER NOT NULL DEFAULT 0"),
    ];
    let complete = additions
        .iter()
        .all(|(name, _)| columns.iter().any(|c| c == name));
    // Older installed workers may resume without updating the new ownership fields.
    // Healthy opens remain read-only; reconcile only an unambiguous live owner.
    let matching_owner = "SELECT MIN(w.id) FROM issue_workers w WHERE w.owner_pid=project_chiefs.owner_pid AND w.owner_start=project_chiefs.owner_start AND w.machine=project_chiefs.machine HAVING COUNT(*)=1";
    let needs_reconcile = complete && db.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM project_chiefs WHERE state='running' AND ({matching_owner}) IS NOT NULL AND (worker_id IS NOT ({matching_owner}) OR started_at IS NULL))"),
        [], |r| r.get::<_, bool>(0),
    )?;
    if complete && !needs_reconcile {
        crate::chief_ownership::migrate(db)?;
        return Ok(());
    }
    let tx = crate::database::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    let adding_retry = !columns.iter().any(|c| c == "retry_count");
    for (name, definition) in additions {
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('project_chiefs') WHERE name=?1)",
            [name],
            |r| r.get::<_, bool>(0),
        )? {
            tx.execute_batch(&format!(
                "ALTER TABLE project_chiefs ADD COLUMN {name} {definition};"
            ))?;
        }
    }
    if adding_retry {
        tx.execute("UPDATE project_chiefs SET retry_count=1,next_at=min(next_at,?1) WHERE state='blocked' AND owner_pid IS NULL",[worker::now()+30_000])?;
    }
    tx.execute(&format!("UPDATE project_chiefs SET worker_id=({matching_owner}),started_at=COALESCE(started_at,next_at-?1) WHERE state='running' AND ({matching_owner}) IS NOT NULL AND (worker_id IS NOT ({matching_owner}) OR started_at IS NULL)"), [INTERVAL_MS])?;
    crate::chief_ownership::migrate(&tx)?;
    tx.commit()?;
    Ok(())
}

#[derive(Clone)]
pub(in crate::issues) struct Job {
    project: String,
    machine: String,
    cwd: String,
    prompt: String,
    session: Option<String>,
    started_at: i64,
    owner_start: String,
    worker_id: String,
}

impl Store {
    pub(in crate::issues) fn reserve_chief(
        &mut self,
        machine: &str,
        worker_id: Option<&str>,
    ) -> Result<Option<Job>> {
        let candidates = self.chief_candidates(worker_id)?;
        let standalone: bool = self.db.query_row(
            "SELECT role='standalone' FROM fleet_meta WHERE id=1",
            [],
            |r| r.get(0),
        )?;
        for (project, cwd, prompt, worker_id) in candidates {
            if !Path::new(&cwd).is_dir() {
                continue;
            }
            // Empty/disabled/not-due projects do not acquire a writer lock.
            let old: Option<Reservation> = self.db.query_row(
                "SELECT next_at,owner_pid,owner_start,pid,process_start,worker_id FROM project_chiefs WHERE project_id=?1 AND machine=?2",
                params![project,machine], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
            if let Some((next, owner, start, pid, process_start, assigned_worker)) = &old {
                if let Some(assigned) = assigned_worker.as_deref().filter(|assigned| standalone && *assigned != worker_id)
                    && self.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_workers WHERE id=?1 AND stop_requested=0 AND json_extract(config,'$.enabled')=1 AND (json_array_length(config,'$.projects')=0 OR EXISTS(SELECT 1 FROM json_each(config,'$.projects') WHERE value=?2)))", params![assigned,project], |r| r.get::<_,bool>(0))? {
                    continue;
                }
                if owner.zip(start.as_deref()).is_some_and(|(pid, start)| {
                    crate::agents::process_identity(pid).as_deref() == Some(start)
                }) {
                    continue;
                }
                if let (Some(pid), Some(start)) = (pid, process_start) {
                    worker::stop_group(*pid, start)?;
                    if crate::agents::process_identity(*pid).as_deref() == Some(start) {
                        return Err(Error::new(
                            "worker_error",
                            "The previous Chief is still alive; its reservation was retained",
                        ));
                    }
                }
                if owner.is_none()
                    && *next > worker::now()
                    && (standalone || assigned_worker.as_deref() == Some(&worker_id))
                {
                    continue;
                }
            }
            let owner = std::process::id();
            let start = crate::agents::process_identity(owner)
                .ok_or_else(|| Error::new("worker_error", "Cannot identify Chief owner"))?;
            let tx = self
                .db
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            // An optimistic comparison makes concurrent schedulers contend only for due work.
            let current: Option<Reservation> = tx.query_row(
                "SELECT next_at,owner_pid,owner_start,pid,process_start,worker_id FROM project_chiefs WHERE project_id=?1 AND machine=?2",
                params![project,machine], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
            if current != old {
                continue;
            }
            let enabled: bool = tx.query_row(
                "SELECT chief_enabled FROM project_settings WHERE project_id=?1",
                [&project],
                |r| r.get(0),
            )?;
            if !enabled || !crate::chief_ownership::allowed(&tx, &project, &worker_id)? {
                continue;
            }
            let started = worker::now();
            tx.execute("INSERT INTO project_chiefs(project_id,machine,cwd,owner_pid,owner_start,next_at,state,worker_id,started_at,last_event) VALUES(?1,?2,?3,?4,?5,?6,'running',?7,?8,'Launching Chief') ON CONFLICT(project_id,machine) DO UPDATE SET session_id=CASE WHEN cwd=excluded.cwd THEN session_id ELSE NULL END,cwd=excluded.cwd,owner_pid=excluded.owner_pid,owner_start=excluded.owner_start,pid=NULL,process_start=NULL,next_at=excluded.next_at,state='running',summary='',worker_id=excluded.worker_id,started_at=excluded.started_at,finished_at=NULL,last_event=excluded.last_event",params![project,machine,cwd,owner,start,started+INTERVAL_MS,worker_id,started])?;
            let session = tx.query_row(
                "SELECT session_id FROM project_chiefs WHERE project_id=?1 AND machine=?2",
                params![project, machine],
                |r| r.get(0),
            )?;
            tx.commit()?;
            return Ok(Some(Job {
                project,
                machine: machine.into(),
                cwd,
                prompt,
                session,
                started_at: started,
                owner_start: start,
                worker_id,
            }));
        }
        Ok(None)
    }
    fn chief_update(&self, job: &Job, state: &str, summary: &str) -> Result<()> {
        // A crashed launch thread may have left a child behind. Keep ownership
        // until that exact child has stopped, and never finalize a newer pass.
        let child: Option<(Option<u32>, Option<String>)> = self.db.query_row(
            "SELECT pid,process_start FROM project_chiefs WHERE project_id=?1 AND machine=?2 AND started_at=?3 AND owner_pid=?4 AND owner_start=?5 AND state='running'",
            params![job.project,job.machine,job.started_at,std::process::id(),job.owner_start],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?;
        let Some((pid, start)) = child else {
            return Ok(());
        };
        if let (Some(pid), Some(start)) = (pid, start) {
            worker::stop_group(pid, &start)?;
            // The launch thread may have panicked before Child::wait. Reap an
            // exited child so a zombie cannot keep the reservation alive.
            unsafe {
                libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG);
            }
            if crate::agents::process_identity(pid).as_deref() == Some(&start) {
                return Err(Error::new(
                    "worker_error",
                    "Chief process is still alive; its result and reservation were retained",
                ));
            }
        }
        let finished = worker::now();
        self.db.execute("UPDATE project_chiefs SET owner_pid=NULL,owner_start=NULL,pid=NULL,process_start=NULL,next_at=?6+CASE WHEN ?4 IN ('idle','cancelled') THEN ?3 ELSE min(300000,30000*(1<<min(4,retry_count))) END,retry_count=CASE WHEN ?4 IN ('idle','cancelled') THEN 0 ELSE min(5,retry_count+1) END,state=?4,summary=?5,finished_at=?6,last_event=?5 WHERE project_id=?1 AND machine=?2 AND started_at=?7 AND owner_pid=?8 AND owner_start=?9 AND state='running'",params![job.project,job.machine,INTERVAL_MS,state,summary,finished,job.started_at,std::process::id(),job.owner_start])?;
        Ok(())
    }

    /// Called before starting any launch threads, including after exec reloads
    /// that preserve the worker PID. Other live workers retain their Chiefs.
    pub(in crate::issues) fn recover_chiefs(
        &self,
        machine: &str,
        worker_id: Option<&str>,
    ) -> Result<()> {
        let owner_start = crate::agents::process_identity(std::process::id())
            .ok_or_else(|| Error::new("worker_error", "Cannot identify Chief owner"))?;
        let mut stmt = self.db.prepare("SELECT project_id,cwd,session_id,started_at,COALESCE(worker_id,'') FROM project_chiefs c WHERE machine=?1 AND (worker_id=?2 OR ?2 IS NULL AND EXISTS(SELECT 1 FROM issue_workers w WHERE w.id=c.worker_id AND w.kind='managed')) AND owner_pid=?3 AND owner_start=?4 AND state='running'")?;
        let jobs = stmt
            .query_map(
                params![machine, worker_id, std::process::id(), owner_start],
                |r| {
                    Ok(Job {
                        project: r.get(0)?,
                        machine: machine.into(),
                        cwd: r.get(1)?,
                        session: r.get(2)?,
                        started_at: r.get(3)?,
                        owner_start: owner_start.clone(),
                        prompt: String::new(),
                        worker_id: r.get(4)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for job in jobs {
            self.chief_update(&job, "blocked", "Chief worker reloaded before saving its result; the saved conversation will resume on the next pass")?;
        }
        Ok(())
    }
}

const STATUS_QUERY: &str = "SELECT c.project_id,p.name,c.machine,c.state,c.pid,c.session_id,c.started_at,c.finished_at,c.next_at,c.summary,c.last_event,c.worker_id,COALESCE(s.chief_enabled,0) FROM project_chiefs c JOIN projects p ON p.id=c.project_id LEFT JOIN project_settings s ON s.project_id=c.project_id WHERE c.worker_id=?1 AND p.hidden_at IS NULL ORDER BY c.state='running' DESC,c.started_at DESC,c.project_id,c.machine";

pub(in crate::issues) fn status(
    db: &crate::database::Connection,
    worker: Option<&str>,
) -> Result<Vec<Value>> {
    let mut stmt = db.prepare(STATUS_QUERY)?;
    Ok(stmt.query_map([worker], |r| {
        let project: String = r.get(0)?;
        let machine: String = r.get(2)?;
        let state: String = r.get(3)?;
        let running = state == "running";
        let next: i64 = r.get(8)?;
        let finished: Option<i64> = r.get(7)?;
        Ok(serde_json::json!({
            "id":format!("chief:{machine}:{project}"),"kind":"chief",
            "project_id":project,"project_name":r.get::<_,String>(1)?,
            "machine":machine,"worker_id":r.get::<_,Option<String>>(11)?,
            "enabled":r.get::<_,bool>(12)?,"title":"Organizing project","state":state,"pid":r.get::<_,Option<u32>>(4)?,
            "session_id":r.get::<_,Option<String>>(5)?,"started_at":r.get::<_,Option<i64>>(6)?,
            "finished_at":if running { None } else { Some(finished.unwrap_or(next-INTERVAL_MS)) },
            "next_at":next,"summary":r.get::<_,String>(9)?,"last_event":r.get::<_,String>(10)?
        }))
    })?.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn event_activity(event: &Value) -> Option<String> {
    let message = match event["type"].as_str()? {
        "thread.started" => "Chief conversation started".to_owned(),
        "item.started" | "item.completed" => {
            let item = &event["item"];
            match item["type"].as_str()? {
                "agent_message" | "reasoning" => item["text"].as_str()?.to_owned(),
                "command_execution" => format!(
                    "Running command: {}",
                    item["command"].as_str().unwrap_or("command")
                ),
                "mcp_tool_call" => format!(
                    "Using {}.{}",
                    item["server"].as_str().unwrap_or("tool"),
                    item["tool"].as_str().unwrap_or("call")
                ),
                _ => return None,
            }
        }
        "error" | "turn.failed" => event["message"]
            .as_str()
            .or(event["error"]["message"].as_str())?
            .to_owned(),
        _ => return None,
    };
    Some(message.chars().take(4000).collect())
}

/// The scheduler owns completion, using its already-open store even when the
/// launch cannot open another connection. A failed write retains the result.
pub(in crate::issues) struct Task {
    job: Job,
    handle: Option<thread::JoinHandle<Result<String>>>,
    outcome: Option<Result<String>>,
}

impl Task {
    pub(in crate::issues) fn start(path: PathBuf, job: Job, stop: Arc<AtomicBool>) -> Self {
        let launched = job.clone();
        match thread::Builder::new().spawn(move || execute(path, launched, stop)) {
            Ok(handle) => Self {
                job,
                handle: Some(handle),
                outcome: None,
            },
            Err(error) => Self {
                job,
                handle: None,
                outcome: Some(Err(error.into())),
            },
        }
    }

    pub(in crate::issues) fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.outcome = Some(handle.join().unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                Err(Error::new(
                    "worker_error",
                    format!("Chief launch thread panicked: {message}"),
                ))
            }));
        }
    }

    pub(in crate::issues) fn poll(&mut self, store: &Store) -> Result<bool> {
        if self
            .handle
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            self.join();
        }
        let Some(outcome) = &self.outcome else {
            return Ok(false);
        };
        let (state, summary) = match outcome {
            Ok(summary) => ("idle", summary.as_str()),
            Err(error) => (
                if error.code == "cancelled" {
                    "cancelled"
                } else {
                    "blocked"
                },
                error.message.as_str(),
            ),
        };
        store.chief_update(&self.job, state, summary)?;
        Ok(true)
    }
}

fn execute(path: PathBuf, job: Job, stop: Arc<AtomicBool>) -> Result<String> {
    let mut store = worker::retry_database_busy(|| Store::open(&path))?;
    run(&path, &mut store, &job, &stop)
}

fn run(path: &Path, store: &mut Store, job: &Job, stop: &AtomicBool) -> Result<String> {
    let mut session = job.session.clone();
    // A deleted conversation may be replaced once; ordinary failures keep its ID.
    for _ in 0..2 {
        let binary = worker::codex_binary()?;
        let mut paths = vec![
            std::env::current_exe()?.parent().unwrap().to_owned(),
            binary.parent().unwrap().to_owned(),
        ];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let mut command = Command::new(binary);
        command.arg("exec");
        if let Some(id) = &session {
            command.args(["resume", id]);
        }
        crate::codex_permissions::apply(&mut command);
        command
            .args(["--json", "--skip-git-repo-check"])
            .arg(worker::chief_instructions(&job.project, &job.prompt))
            .current_dir(&job.cwd)
            .process_group(0)
            .env("HEY_BOSS_ISSUE_DB", path)
            .env("HEY_BOSS_ISSUE_PROJECT", &job.project)
            .env(
                "PATH",
                std::env::join_paths(paths).map_err(|e| Error::invalid(e.to_string()))?,
            )
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_AGENT_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CLAUDE_SESSION_ID")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let pid = child.id();
        let start = crate::agents::process_identity(pid)
            .ok_or_else(|| Error::new("worker_error", "Chief exited during launch"));
        let outcome = (|| -> Result<String> {
            let start = start.as_ref().map_err(Clone::clone)?;
            store.db.execute("UPDATE project_chiefs SET pid=?3,process_start=?4 WHERE project_id=?1 AND machine=?2",params![job.project,job.machine,pid,start])?;
            let stdout = child.stdout.take().unwrap();
            let (send, receive) = mpsc::sync_channel(128);
            let reader = thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let mut line = Vec::new();
                    match Read::take(&mut reader, 1024 * 1024 + 1).read_until(b'\n', &mut line) {
                        Ok(0) => break,
                        Err(error) => {
                            let _ = send.send(Err(error.to_string()));
                            break;
                        }
                        Ok(_) => {
                            let value = if line.len() > 1024 * 1024 || line.last() != Some(&b'\n') {
                                Err("Chief returned an oversized or incomplete event".into())
                            } else {
                                serde_json::from_slice::<Value>(&line).map_err(|e| e.to_string())
                            };
                            let failed = value.is_err();
                            if send.send(value).is_err() || failed {
                                break;
                            }
                        }
                    }
                }
            });
            let mut thread_started = false;
            let mut completed = false;
            let mut failed = false;
            let mut summary = String::new();
            let mut missing = false;
            let mut last_control = Instant::now() - Duration::from_secs(1);
            loop {
                let disabled = if last_control.elapsed() >= Duration::from_secs(1) {
                    last_control = Instant::now();
                    store.db.query_row(
                        "SELECT NOT chief_enabled FROM project_settings WHERE project_id=?1",
                        [&job.project],
                        |r| r.get::<_, bool>(0),
                    )? || !crate::chief_ownership::allowed(&store.db, &job.project, &job.worker_id)?
                } else {
                    false
                };
                if stop.load(Ordering::Relaxed) || disabled {
                    return Err(Error::new(
                        "cancelled",
                        "Chief stopped; the saved thread will be resumed on its next scheduled pass",
                    ));
                }
                match receive.recv_timeout(Duration::from_millis(200)) {
                    Ok(Err(error)) => return Err(Error::new("worker_error", error)),
                    Ok(Ok(event)) => {
                        if let Some(activity) = event_activity(&event) {
                            store.db.execute("UPDATE project_chiefs SET last_event=?3 WHERE project_id=?1 AND machine=?2", params![job.project,job.machine,activity])?;
                        }
                        match event["type"].as_str().unwrap_or("") {
                            "thread.started" => {
                                let id = event["thread_id"].as_str().ok_or_else(|| {
                                    Error::new("worker_error", "Chief returned no thread ID")
                                })?;
                                if session.as_deref().is_some_and(|saved| saved != id) {
                                    return Err(Error::new(
                                        "worker_error",
                                        "Chief resumed a different thread",
                                    ));
                                }
                                thread_started = true;
                                store.db.execute("UPDATE project_chiefs SET session_id=?3 WHERE project_id=?1 AND machine=?2",params![job.project,job.machine,id])?;
                            }
                            "item.completed" if event["item"]["type"] == "agent_message" => {
                                summary = event["item"]["text"]
                                    .as_str()
                                    .unwrap_or("")
                                    .chars()
                                    .take(4000)
                                    .collect();
                            }
                            "turn.completed" => completed = true,
                            "error" | "turn.failed" => {
                                failed = true;
                                let message = event["message"]
                                    .as_str()
                                    .or(event["error"]["message"].as_str())
                                    .unwrap_or("Chief turn failed");
                                missing |=
                                    message.to_lowercase().contains("no saved session found")
                                        || message.to_lowercase().contains("thread not found");
                                summary = message.chars().take(4000).collect();
                            }
                            _ => {}
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if crate::agent_process::exited(pid)? {
                            return Err(Error::new(
                                "worker_error",
                                "Chief exited before closing its event stream",
                            ));
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        if child.try_wait()?.is_some() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(200));
                    }
                }
            }
            let _ = reader.join();
            let status = child.wait()?;
            if !thread_started && missing && session.is_some() {
                return Err(Error::new("missing_thread", summary));
            }
            if !status.success()
                || !completed
                || failed
                || !thread_started
                || summary.trim().is_empty()
            {
                return Err(Error::new(
                    "worker_error",
                    if summary.is_empty() {
                        "Chief ended without a completed turn".into()
                    } else {
                        summary
                    },
                ));
            }
            Ok(summary)
        })();
        if let Ok(start) = &start {
            let _ = worker::stop_group(pid, start);
        }
        let _ = child.kill();
        let _ = child.wait();
        if outcome.as_ref().is_err_and(|e| e.code == "missing_thread") {
            session = None;
            store.db.execute(
                "UPDATE project_chiefs SET session_id=NULL WHERE project_id=?1 AND machine=?2",
                params![job.project, job.machine],
            )?;
            continue;
        }
        return outcome;
    }
    Err(Error::new(
        "worker_error",
        "Chief could not start a replacement thread",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch_fixture() -> (PathBuf, Store, Job) {
        let root =
            std::env::temp_dir().join(format!("hb-chief-launch-{}", worker::random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let mut store = Store::open(&root.join("issues.db")).unwrap();
        store.db.execute_batch("CREATE TABLE issue_worker_runtime(worker_id TEXT PRIMARY KEY,owner_pid INTEGER,owner_start TEXT);
            INSERT INTO projects(id,name,next_number) VALUES('named:Chief','Chief',1);
            INSERT INTO project_settings(project_id,prompt,version,chief_enabled) VALUES('named:Chief','Work',1,1);").unwrap();
        let config = serde_json::to_string(&worker::Settings {
            enabled: true,
            projects: vec!["named:Chief".into()],
            directory: root.to_string_lossy().into(),
            ..Default::default()
        })
        .unwrap();
        store.db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES('owner','cli',?1,1,0)", [&config]).unwrap();
        let job = store.reserve_chief("unit", Some("owner")).unwrap().unwrap();
        store
            .db
            .execute("UPDATE project_chiefs SET session_id='saved-thread'", [])
            .unwrap();
        (root, store, job)
    }

    #[test]
    fn companion_stops_revoked_chief_without_stopping_the_worker() {
        let (root, store, job) = launch_fixture();
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let start = crate::agents::process_identity(pid).unwrap();
        store
            .db
            .execute(
                "UPDATE project_chiefs SET pid=?1,process_start=?2",
                params![pid, start],
            )
            .unwrap();
        store
            .db
            .execute("UPDATE fleet_meta SET role='agent',node='unit'", [])
            .unwrap();
        let assignment = crate::chief_ownership::Assignment {
            project_id: "named:Chief".into(),
            node: "unit".into(),
            worker_id: "owner".into(),
            generation: 1,
            revoking: false,
        };
        crate::chief_ownership::apply(&store.db, std::slice::from_ref(&assignment)).unwrap();
        crate::chief_ownership::stop_unassigned(&store.db).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        crate::chief_ownership::apply(
            &store.db,
            &[crate::chief_ownership::Assignment {
                generation: 2,
                revoking: true,
                ..assignment
            }],
        )
        .unwrap();
        crate::chief_ownership::stop_unassigned(&store.db).unwrap();
        assert!(!child.wait().unwrap().success());
        assert!(crate::agents::process_identity(std::process::id()).is_some());
        store.chief_update(&job, "blocked", "Revoked").unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fleet_assignment_retargets_only_finished_local_chiefs() {
        let (root, store, job) = launch_fixture();
        store.db.execute_batch("INSERT INTO issue_workers(id,kind,config,version,updated_at) SELECT 'selected',kind,config,1,0 FROM issue_workers WHERE id='owner'; UPDATE fleet_meta SET role='agent',node='unit';").unwrap();
        let assignment = crate::chief_ownership::Assignment {
            project_id: "named:Chief".into(),
            node: "unit".into(),
            worker_id: "selected".into(),
            generation: 1,
            revoking: false,
        };
        crate::chief_ownership::apply(&store.db, std::slice::from_ref(&assignment)).unwrap();
        crate::chief_ownership::stop_unassigned(&store.db).unwrap();
        assert_eq!(status(&store.db, Some("owner")).unwrap().len(), 1);
        store.chief_update(&job, "idle", "Completed pass").unwrap();
        let previous = status(&store.db, Some("owner")).unwrap()[0].clone();
        // The grant can arrive before the former owner records completion.
        // Older workers consult this local assignment before trying to reserve.
        crate::chief_ownership::stop_unassigned(&store.db).unwrap();
        let selected = status(&store.db, Some("selected")).unwrap();
        assert_eq!(
            selected.len(),
            1,
            "The supervisor's worker must inherit the idle Chief"
        );
        for key in ["next_at", "session_id", "summary", "state"] {
            assert_eq!(selected[0][key], previous[key]);
        }
        crate::chief_ownership::apply(&store.db, &[assignment]).unwrap();
        let writer = rusqlite::Connection::open(root.join("issues.db")).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        store.db.busy_timeout(std::time::Duration::ZERO).unwrap();
        crate::chief_ownership::stop_unassigned(&store.db).unwrap();
        drop(writer);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fleet_chief_requires_supervisors_selected_worker() {
        let (root, mut store, job) = launch_fixture();
        store.chief_update(&job, "idle", "Done").unwrap();
        store.db.execute_batch("UPDATE project_chiefs SET next_at=0; UPDATE fleet_meta SET role='agent',node='unit';").unwrap();
        assert!(
            store
                .reserve_chief("unit", Some("owner"))
                .unwrap()
                .is_none(),
            "A fleet worker must wait for the supervisor's assignment"
        );
        let assignment = crate::chief_ownership::Assignment {
            project_id: "named:Chief".into(),
            node: "unit".into(),
            worker_id: "owner".into(),
            generation: 1,
            revoking: false,
        };
        crate::chief_ownership::apply(&store.db, std::slice::from_ref(&assignment)).unwrap();
        let selected = store.reserve_chief("unit", Some("owner")).unwrap().unwrap();
        crate::chief_ownership::apply(
            &store.db,
            &[crate::chief_ownership::Assignment {
                generation: 2,
                revoking: true,
                ..assignment
            }],
        )
        .unwrap();
        assert!(!crate::chief_ownership::allowed(&store.db, "named:Chief", "owner").unwrap());
        assert!(
            store
                .db
                .execute("UPDATE project_chiefs SET last_event='legacy worker'", [])
                .is_err(),
            "Old workers must also obey revocation"
        );
        store
            .chief_update(&selected, "blocked", "Reassigned")
            .unwrap();
        assert!(
            store
                .reserve_chief("unit", Some("owner"))
                .unwrap()
                .is_none()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chief_database_open_failure_does_not_leave_a_live_reservation() {
        let (root, store, job) = launch_fixture();
        // An installer upgrades the schema while the existing worker stays alive.
        store.db.pragma_update(None, "user_version", 999).unwrap();
        let mut task = Task::start(
            root.join("issues.db"),
            job,
            Arc::new(AtomicBool::new(false)),
        );
        task.join();
        assert!(task.poll(&store).unwrap());
        let chiefs = status(&store.db, Some("owner")).unwrap();
        assert_eq!(chiefs[0]["state"], "blocked");
        assert!(
            chiefs[0]["summary"]
                .as_str()
                .unwrap()
                .contains("Incompatible issue database")
        );
        assert!(chiefs[0]["finished_at"].is_i64());
        assert_eq!(chiefs[0]["session_id"], "saved-thread");
        assert!(
            store
                .db
                .query_row(
                    "SELECT owner_pid IS NULL AND pid IS NULL FROM project_chiefs",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chief_failures_back_off_and_success_restores_the_hourly_schedule() {
        let (root, mut store, mut job) = launch_fixture();
        for delay in [30_000, 60_000, 120_000, 240_000, 300_000, 300_000] {
            store
                .chief_update(&job, "blocked", "Agent disconnected")
                .unwrap();
            let wait: i64 = store
                .db
                .query_row("SELECT next_at-finished_at FROM project_chiefs", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(wait, delay);
            assert!(
                store
                    .reserve_chief("unit", Some("owner"))
                    .unwrap()
                    .is_none()
            );
            store
                .db
                .execute("UPDATE project_chiefs SET next_at=0", [])
                .unwrap();
            job = store.reserve_chief("unit", Some("owner")).unwrap().unwrap();
        }
        store.chief_update(&job, "idle", "Verified pass").unwrap();
        let wait: i64 = store
            .db
            .query_row("SELECT next_at-finished_at FROM project_chiefs", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(wait, INTERVAL_MS);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chief_panics_and_failed_result_writes_are_supervised() {
        let (root, mut store, job) = launch_fixture();
        let handle = thread::spawn(|| panic!("synthetic launch failure"));
        let mut task = Task {
            job,
            handle: Some(handle),
            outcome: None,
        };
        task.join();
        store.db.execute_batch("CREATE TRIGGER fail_chief_result BEFORE UPDATE OF state ON project_chiefs BEGIN SELECT RAISE(ABORT,'result write unavailable'); END;").unwrap();
        assert!(task.poll(&store).is_err());
        assert!(
            store
                .reserve_chief("unit", Some("owner"))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            status(&store.db, Some("owner")).unwrap()[0]["state"],
            "running"
        );
        store
            .db
            .execute_batch("DROP TRIGGER fail_chief_result")
            .unwrap();
        assert!(task.poll(&store).unwrap());
        let chief = &status(&store.db, Some("owner")).unwrap()[0];
        assert_eq!(chief["state"], "blocked");
        assert!(
            chief["summary"]
                .as_str()
                .unwrap()
                .contains("synthetic launch failure")
        );
        assert_eq!(chief["last_event"], chief["summary"]);
        // A delayed completion must not overwrite the next pass.
        store
            .db
            .execute("UPDATE project_chiefs SET next_at=0", [])
            .unwrap();
        let replacement = store.reserve_chief("unit", Some("owner")).unwrap().unwrap();
        store
            .db
            .execute(
                "UPDATE project_chiefs SET started_at=?1",
                [replacement.started_at.max(task.job.started_at) + 1],
            )
            .unwrap();
        assert!(task.poll(&store).unwrap());
        assert_eq!(
            status(&store.db, Some("owner")).unwrap()[0]["state"],
            "running"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chief_reload_recovers_only_its_own_abandoned_attempts() {
        let (root, store, _job) = launch_fixture();
        store
            .recover_chiefs("unit", Some("another-worker"))
            .unwrap();
        assert_eq!(
            status(&store.db, Some("owner")).unwrap()[0]["state"],
            "running"
        );
        store
            .recover_chiefs("another-machine", Some("owner"))
            .unwrap();
        assert_eq!(
            status(&store.db, Some("owner")).unwrap()[0]["state"],
            "running"
        );
        store.recover_chiefs("unit", Some("owner")).unwrap();
        let chief = &status(&store.db, Some("owner")).unwrap()[0];
        assert_eq!(chief["state"], "blocked");
        assert_eq!(chief["session_id"], "saved-thread");
        assert!(chief["finished_at"].is_i64());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[expect(
        clippy::zombie_processes,
        reason = "The test abandons Child to verify that the supervisor stops and reaps it"
    )]
    fn chief_panic_stops_and_reaps_its_child_before_releasing_ownership() {
        let (root, store, job) = launch_fixture();
        let (send, receive) = mpsc::sync_channel(1);
        let handle = thread::spawn(move || {
            let child = Command::new("sleep")
                .arg("120")
                .process_group(0)
                .spawn()
                .unwrap();
            let pid = child.id();
            send.send((pid, crate::agents::process_identity(pid).unwrap()))
                .unwrap();
            panic!("synthetic failure after spawning Codex");
        });
        let (pid, start) = receive.recv().unwrap();
        store
            .db
            .execute(
                "UPDATE project_chiefs SET pid=?1,process_start=?2",
                params![pid, start],
            )
            .unwrap();
        let mut task = Task {
            job,
            handle: Some(handle),
            outcome: None,
        };
        task.join();
        let deadline = Instant::now() + Duration::from_secs(5);
        let saved = loop {
            match task.poll(&store) {
                Ok(done) => break done,
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                Err(error) => panic!("Chief result was not saved: {error}"),
            }
        };
        assert!(saved);
        assert!(crate::agents::process_identity(pid).is_none());
        assert_eq!(
            status(&store.db, Some("owner")).unwrap()[0]["state"],
            "blocked"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chief_status_reads_only_the_selected_workers_projects() {
        let root =
            std::env::temp_dir().join(format!("hb-chief-status-{}", worker::random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let store = Store::open(&root.join("issues.db")).unwrap();
            store.db.execute_batch("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES('selected','cli','{}',1,0),('other','cli','{}',1,0);
                WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000)
                INSERT INTO projects(id,name,next_number) SELECT 'project:'||x,'Project '||x,1 FROM n;
                UPDATE projects SET hidden_at=1 WHERE id='project:4';
                INSERT INTO project_settings(project_id,prompt,version,chief_enabled) VALUES('project:3','Work',1,1);
                WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000)
                INSERT INTO project_chiefs(project_id,machine,cwd,worker_id,state,started_at,finished_at,next_at)
                SELECT 'project:'||x,'unit','/workspace',CASE WHEN x<=4 THEN 'selected' ELSE 'other' END,
                    CASE WHEN x IN (2,3,4) THEN 'running' ELSE 'idle' END,
                    CASE x WHEN 1 THEN 30 WHEN 2 THEN 10 WHEN 3 THEN 20 WHEN 4 THEN 50 ELSE x END,40,3600040 FROM n;").unwrap();
            let chiefs = status(&store.db, Some("selected")).unwrap();
            assert_eq!(
                chiefs
                    .iter()
                    .map(|c| c["project_id"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["project:3", "project:2", "project:1"]
            );
            assert_eq!(chiefs[0]["enabled"], true);
            assert_eq!(chiefs[1]["enabled"], false);
            assert!(chiefs[0]["finished_at"].is_null());
            assert_eq!(chiefs[2]["finished_at"], 40);
            assert!(status(&store.db, None).unwrap().is_empty());
            let mut stmt = store.db.prepare(STATUS_QUERY).unwrap();
            let projects = stmt
                .query_map(["selected"], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(projects, ["project:3", "project:2", "project:1"]);
            let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
            eprintln!(
                "Selected Chief status used {steps} SQLite VM steps beside 9,996 unrelated project chiefs"
            );
            assert!(
                steps < 1000,
                "Chief status scanned unrelated projects: {steps} VM steps"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn another_worker_cannot_take_over_the_next_chief_pass() {
        let root =
            std::env::temp_dir().join(format!("hb-chief-owner-{}", worker::random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let mut store = Store::open(&root.join("issues.db")).unwrap();
            store.db.execute_batch("CREATE TABLE issue_worker_runtime(worker_id TEXT PRIMARY KEY,owner_pid INTEGER,owner_start TEXT);
                INSERT INTO projects(id,name,next_number) VALUES('named:Chief','Chief',1);
                INSERT INTO project_settings(project_id,prompt,version,chief_enabled) VALUES('named:Chief','Work',1,1);").unwrap();
            let config = serde_json::to_string(&worker::Settings {
                enabled: true,
                projects: vec!["named:Chief".into()],
                directory: root.to_string_lossy().into(),
                ..Default::default()
            })
            .unwrap();
            for id in ["first", "second"] {
                store.db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES(?1,'cli',?2,1,0)", params![id,config]).unwrap();
            }
            let first = store.reserve_chief("unit", Some("first")).unwrap().unwrap();
            assert!(
                store
                    .reserve_chief("unit", Some("second"))
                    .unwrap()
                    .is_none()
            );
            store.chief_update(&first, "idle", "Done").unwrap();
            store
                .db
                .execute("UPDATE project_chiefs SET next_at=0,session_id='saved'", [])
                .unwrap();
            assert!(
                store
                    .reserve_chief("unit", Some("second"))
                    .unwrap()
                    .is_none()
            );
            let resumed = store.reserve_chief("unit", Some("first")).unwrap().unwrap();
            assert_eq!(resumed.session.as_deref(), Some("saved"));
            store.chief_update(&resumed, "idle", "Done").unwrap();
            store.db.execute_batch("UPDATE issue_workers SET stop_requested=1 WHERE id='first'; UPDATE project_chiefs SET next_at=0;").unwrap();
            let transferred = store
                .reserve_chief("unit", Some("second"))
                .unwrap()
                .unwrap();
            assert_eq!(
                transferred.session.as_deref(),
                Some("saved"),
                "A stopped owner hands off the same conversation"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_live_chief_is_attached_to_its_existing_worker() {
        let root =
            std::env::temp_dir().join(format!("hb-chief-migrate-{}", worker::random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("issues.db");
        {
            let store = Store::open(&path).unwrap();
            store.db.execute_batch("DROP TABLE project_chiefs;
                CREATE TABLE project_chiefs(project_id TEXT,machine TEXT,cwd TEXT,session_id TEXT,next_at INTEGER,owner_pid INTEGER,owner_start TEXT,pid INTEGER,process_start TEXT,state TEXT,summary TEXT,PRIMARY KEY(project_id,machine));
                INSERT INTO projects(id,name,next_number) VALUES('named:Chief','Chief',1);
                INSERT INTO issue_workers(id,kind,config,version,owner_pid,owner_start,machine,updated_at) VALUES('owner','cli','{}',1,123,'start','unit',0);
                INSERT INTO project_chiefs VALUES('named:Chief','unit','/workspace','existing-thread',3600001,123,'start',456,'chief-start','running','');").unwrap();
        }
        let store = Store::open(&path).unwrap();
        let chiefs = status(&store.db, Some("owner")).unwrap();
        assert_eq!(chiefs[0]["session_id"], "existing-thread");
        assert_eq!(chiefs[0]["pid"], 456);
        assert_eq!(chiefs[0]["started_at"], 1);
        assert!(chiefs[0]["finished_at"].is_null());
        // An older installed worker can resume after the columns were added.
        store.db.execute_batch("INSERT INTO issue_workers(id,kind,config,version,owner_pid,owner_start,machine,updated_at) VALUES('new-owner','cli','{}',1,789,'new-start','unit',0);
            UPDATE project_chiefs SET owner_pid=789,owner_start='new-start';").unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(status(&store.db, Some("owner")).unwrap().is_empty());
        let chiefs = status(&store.db, Some("new-owner")).unwrap();
        assert_eq!(chiefs[0]["session_id"], "existing-thread");
        assert_eq!(chiefs[0]["started_at"], 1);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn chief_reservations_respect_scope_controls_and_idle_writer_contention() {
        let root =
            std::env::temp_dir().join(format!("hb-chief-store-{}", worker::random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("issues.db");
        {
            let mut store = Store::open(&path).unwrap();
            store.db.execute("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES('named:Chief','Chief',1,0,0),('named:Other','Other',1,0,0)",[]).unwrap();
            store.db.execute("INSERT INTO project_settings(project_id,prompt,version,chief_enabled) VALUES('named:Chief','Implement',1,1)",[]).unwrap();
            let id = store
                .register_worker(
                    None,
                    &worker::Settings {
                        projects: vec!["named:Chief".into()],
                        directory: root.to_string_lossy().into(),
                        ..Default::default()
                    },
                    "unit",
                )
                .unwrap();
            store.db.execute("UPDATE issue_workers SET config=json_set(config,'$.enabled',json('true')) WHERE id=?1",[&id]).unwrap();
            store
                .db
                .execute("INSERT INTO agents VALUES('agent','{}',0)", [])
                .unwrap();
            store.db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('named:Chief',1,'Busy','','open','agent',0,0,1,'[]')",[]).unwrap();
            store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id) VALUES('busy','named:Chief',1,'{}','agent','running',1,'busy','unit',0,0,?1)",[&id]).unwrap();
            let job = store.reserve_chief("unit", Some(&id)).unwrap().unwrap();
            assert!(
                store.reserve_chief("unit", Some(&id)).unwrap().is_none(),
                "Only one Chief can own the project"
            );
            store.chief_update(&job, "idle", "Done").unwrap();
            store.db.busy_timeout(Duration::from_millis(25)).unwrap();
            let mut other = crate::database::Connection::open(&path).unwrap();
            let tx = other
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(
                store.reserve_chief("unit", Some(&id)).unwrap().is_none(),
                "Waiting Chiefs do not acquire the writer lock"
            );
            tx.rollback().unwrap();
            store
                .db
                .execute("UPDATE project_chiefs SET next_at=0", [])
                .unwrap();
            store
                .db
                .execute(
                    "UPDATE issue_workers SET stop_requested=1 WHERE id=?1",
                    [&id],
                )
                .unwrap();
            assert!(
                store.reserve_chief("unit", Some(&id)).unwrap().is_none(),
                "Paused workers cannot launch Chiefs"
            );
            store.db.execute("UPDATE issue_workers SET stop_requested=0,config=json_set(config,'$.projects',json('[\"named:Other\"]')) WHERE id=?1",[&id]).unwrap();
            assert!(
                store.reserve_chief("unit", Some(&id)).unwrap().is_none(),
                "Project scope is respected"
            );
            store.db.execute("UPDATE issue_workers SET config=json_set(config,'$.projects',json('[\"named:Chief\"]')) WHERE id=?1",[&id]).unwrap();
            store.db.execute("UPDATE project_chiefs SET owner_pid=4294967295,owner_start='dead',session_id='saved-thread'",[]).unwrap();
            let recovered = store.reserve_chief("unit", Some(&id)).unwrap().unwrap();
            assert_eq!(
                recovered.session.as_deref(),
                Some("saved-thread"),
                "Dead owner recovery keeps the conversation"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
