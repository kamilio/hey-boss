//! One short organizing turn per hour, independent of issue reservations.
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

pub(in crate::issues) const DEFAULT_PROMPT: &str = "You are this project's Chief. Keep its work organized and moving. In this turn:\n- Check every open hey-boss issue and attached PR.\n- For confirmed new conflicts or review blockers, add the label 'rework needed' and unassign the issue so a worker can pick it up. Never release a live agent's claim; report the blocker instead.\n- Add 'PR ready' only when CI passes and required reviews are complete and clean. Remove stale readiness labels.\n- Maintain the mindmap through the CLI, using simple labels and nesting confirmed follow-ups under their source.\n- Report missing capabilities or CLI friction as issues in the hey-boss project; avoid duplicate reports.\nWorkers handle code changes. Do not implement issues, edit repository files, commit, merge PRs, or deploy. Inspect current state before changing project metadata. Finish this pass and stop; you will be resumed in about an hour. Do not create a goal or wait in a monitoring loop.";
pub(in crate::issues) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS project_chiefs(
 project_id TEXT NOT NULL REFERENCES projects(id),machine TEXT NOT NULL,cwd TEXT NOT NULL,
 session_id TEXT,next_at INTEGER NOT NULL DEFAULT 0,owner_pid INTEGER,owner_start TEXT,
 pid INTEGER,process_start TEXT,state TEXT NOT NULL DEFAULT 'idle',summary TEXT NOT NULL DEFAULT '',
 PRIMARY KEY(project_id,machine));";
const INTERVAL_MS: i64 = 60 * 60 * 1000;

#[derive(Clone)]
pub(in crate::issues) struct Job {
    project: String,
    machine: String,
    cwd: String,
    prompt: String,
    session: Option<String>,
}

impl Store {
    pub(in crate::issues) fn reserve_chief(
        &mut self,
        machine: &str,
        worker_id: Option<&str>,
    ) -> Result<Option<Job>> {
        let candidates = self.chief_candidates(worker_id)?;
        for (project, cwd, prompt) in candidates {
            if !Path::new(&cwd).is_dir() {
                continue;
            }
            // Empty/disabled/not-due projects do not acquire a writer lock.
            let old: Option<(i64, Option<u32>, Option<String>, Option<u32>, Option<String>)> = self.db.query_row(
                "SELECT next_at,owner_pid,owner_start,pid,process_start FROM project_chiefs WHERE project_id=?1 AND machine=?2",
                params![project,machine], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
            if let Some((next, owner, start, pid, process_start)) = &old {
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
                if owner.is_none() && *next > worker::now() {
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
            let current: Option<(i64,Option<u32>,Option<String>,Option<u32>,Option<String>)> = tx.query_row(
                "SELECT next_at,owner_pid,owner_start,pid,process_start FROM project_chiefs WHERE project_id=?1 AND machine=?2",
                params![project,machine], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
            if current != old {
                continue;
            }
            let enabled: bool = tx.query_row(
                "SELECT chief_enabled FROM project_settings WHERE project_id=?1",
                [&project],
                |r| r.get(0),
            )?;
            if !enabled {
                continue;
            }
            tx.execute("INSERT INTO project_chiefs(project_id,machine,cwd,owner_pid,owner_start,next_at,state) VALUES(?1,?2,?3,?4,?5,?6,'running') ON CONFLICT(project_id,machine) DO UPDATE SET session_id=CASE WHEN cwd=excluded.cwd THEN session_id ELSE NULL END,cwd=excluded.cwd,owner_pid=excluded.owner_pid,owner_start=excluded.owner_start,pid=NULL,process_start=NULL,next_at=excluded.next_at,state='running',summary=''",params![project,machine,cwd,owner,start,worker::now()+INTERVAL_MS])?;
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
            }));
        }
        Ok(None)
    }
    fn chief_update(&self, job: &Job, state: &str, summary: &str) -> Result<()> {
        self.db.execute("UPDATE project_chiefs SET owner_pid=NULL,owner_start=NULL,pid=NULL,process_start=NULL,next_at=?3,state=?4,summary=?5 WHERE project_id=?1 AND machine=?2",params![job.project,job.machine,worker::now()+INTERVAL_MS,state,summary])?;
        Ok(())
    }
}

pub(in crate::issues) fn execute(path: PathBuf, job: Job, stop: Arc<AtomicBool>) {
    let result = worker::retry_database_busy(|| Store::open(&path)).and_then(|mut store| {
        let outcome = run(&path, &mut store, &job, &stop);
        let (state, summary) = match &outcome {
            Ok(summary) => ("idle", summary.as_str()),
            Err(error) => ("blocked", error.message.as_str()),
        };
        worker::retry_database_busy(|| store.chief_update(&job, state, summary))?;
        outcome
    });
    if let Err(error) = result {
        eprintln!("Chief {}: {error}", job.project);
    }
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
        command.args(["--json","--skip-git-repo-check","-c","approval_policy=\"never\""])
            .arg(format!("Project: {}. Use hey-boss issue and mm commands in this project.\n\n{}\n\nRun one organizing pass, then stop. Workers handle all code changes; do not start a goal.",job.project,job.prompt))
            .current_dir(&job.cwd).process_group(0)
            .env("HEY_BOSS_ISSUE_DB",path).env("HEY_BOSS_ISSUE_PROJECT",&job.project)
            .env("PATH",std::env::join_paths(paths).map_err(|e| Error::invalid(e.to_string()))?)
            .env_remove("HEY_BOSS_ISSUE_HOST").env_remove("HEY_BOSS_AGENT_ID")
            .env_remove("CODEX_THREAD_ID").env_remove("CODEX_SESSION_ID").env_remove("CLAUDE_SESSION_ID")
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
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
                        Ok(0) | Err(_) => break,
                        Ok(_) if line.len() > 1024 * 1024 => break,
                        _ => {
                            if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                                if send.send(value).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            });
            let deadline = Instant::now() + Duration::from_secs(30 * 60);
            let mut thread_started = false;
            let mut completed = false;
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
                    )?
                } else {
                    false
                };
                if stop.load(Ordering::Relaxed) || disabled || Instant::now() >= deadline {
                    return Err(Error::new(
                        "cancelled",
                        "Chief stopped; the saved thread will be resumed on its next scheduled pass",
                    ));
                }
                match receive.recv_timeout(Duration::from_millis(200)) {
                    Ok(event) => match event["type"].as_str().unwrap_or("") {
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
                            let message = event["message"]
                                .as_str()
                                .or(event["error"]["message"].as_str())
                                .unwrap_or("Chief turn failed");
                            missing |= message.to_lowercase().contains("no saved session found")
                                || message.to_lowercase().contains("thread not found");
                            summary = message.chars().take(4000).collect();
                        }
                        _ => {}
                    },
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
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
            if !status.success() || !completed || !thread_started {
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
            let mut other = rusqlite::Connection::open(&path).unwrap();
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
