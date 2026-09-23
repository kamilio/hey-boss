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

pub(in crate::issues) const DEFAULT_PROMPT: &str = "You are this project's Chief. Keep its work organized and moving. In this turn:\n- Check every open hey-boss issue and attached PR.\n- For confirmed new conflicts or review blockers, add the label 'rework needed' and unassign the issue so a worker can pick it up. Never release a live agent's claim; report the blocker instead.\n- Add 'PR ready' only when CI passes and required reviews are complete and clean. Remove stale readiness labels.\n- Maintain the mindmap through the CLI, using simple labels and nesting confirmed follow-ups under their source.\n- Report missing capabilities or CLI friction as issues in the hey-boss project; avoid duplicate reports.\nWorkers handle code changes. Do not implement issues, edit repository files, commit, merge PRs, or deploy. Inspect current state before changing project metadata. Finish this pass and stop; you will be resumed in about an hour. Do not create a goal or wait in a monitoring loop.";
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
        return Ok(());
    }
    let tx = crate::database::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
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
    tx.execute(&format!("UPDATE project_chiefs SET worker_id=({matching_owner}),started_at=COALESCE(started_at,next_at-?1) WHERE state='running' AND ({matching_owner}) IS NOT NULL AND (worker_id IS NOT ({matching_owner}) OR started_at IS NULL)"), [INTERVAL_MS])?;
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
}

impl Store {
    pub(in crate::issues) fn reserve_chief(
        &mut self,
        machine: &str,
        worker_id: Option<&str>,
    ) -> Result<Option<Job>> {
        let candidates = self.chief_candidates(worker_id)?;
        for (project, cwd, prompt, worker_id) in candidates {
            if !Path::new(&cwd).is_dir() {
                continue;
            }
            // Empty/disabled/not-due projects do not acquire a writer lock.
            let old: Option<Reservation> = self.db.query_row(
                "SELECT next_at,owner_pid,owner_start,pid,process_start,worker_id FROM project_chiefs WHERE project_id=?1 AND machine=?2",
                params![project,machine], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
            if let Some((next, owner, start, pid, process_start, assigned_worker)) = &old {
                if let Some(assigned) = assigned_worker.as_deref().filter(|assigned| *assigned != worker_id)
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
            if !enabled {
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
            }));
        }
        Ok(None)
    }
    fn chief_update(&self, job: &Job, state: &str, summary: &str) -> Result<()> {
        let finished = worker::now();
        self.db.execute("UPDATE project_chiefs SET owner_pid=NULL,owner_start=NULL,pid=NULL,process_start=NULL,next_at=?3,state=?4,summary=?5,finished_at=?6 WHERE project_id=?1 AND machine=?2",params![job.project,job.machine,finished+INTERVAL_MS,state,summary,finished])?;
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
        crate::worker_tui::diagnostics::report(format_args!("Chief {}: {error}", job.project));
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
        crate::codex_permissions::apply(&mut command);
        command.args(["--json","--skip-git-repo-check"])
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
                            if let Ok(value) = serde_json::from_slice::<Value>(&line)
                                && send.send(value).is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            });
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
                if stop.load(Ordering::Relaxed) || disabled {
                    return Err(Error::new(
                        "cancelled",
                        "Chief stopped; the saved thread will be resumed on its next scheduled pass",
                    ));
                }
                match receive.recv_timeout(Duration::from_millis(200)) {
                    Ok(event) => {
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
