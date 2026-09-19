//! Independent worker instances and first-class project instructions/PR links.
use super::*;
use crate::issues::worker::{self, Job, ProjectConfig, Settings, now, random_id};
pub(super) const SCHEMA: &str = "
CREATE TABLE issue_workers(id TEXT PRIMARY KEY,kind TEXT NOT NULL,config TEXT NOT NULL,version INTEGER NOT NULL,owner_pid INTEGER,owner_start TEXT,machine TEXT,stop_requested INTEGER NOT NULL DEFAULT 0,updated_at INTEGER NOT NULL);
ALTER TABLE worker_runs ADD COLUMN worker_id TEXT REFERENCES issue_workers(id);
ALTER TABLE worker_runs ADD COLUMN reservation_expires INTEGER;
ALTER TABLE worker_runs ADD COLUMN claimed_at INTEGER;
CREATE INDEX worker_runs_worker ON worker_runs(worker_id,finished_at,started_at DESC);
CREATE TABLE project_settings(project_id TEXT PRIMARY KEY REFERENCES projects(id),prompt TEXT NOT NULL,prs_enabled INTEGER NOT NULL DEFAULT 0,version INTEGER NOT NULL);
CREATE TABLE issue_pull_requests(project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,url TEXT NOT NULL,added_by TEXT NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY(project_id,issue_number,url),FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number));
INSERT INTO issue_workers(id,kind,config,version,updated_at) SELECT 'legacy:'||w.project_id,'managed',json_object('name',p.name||' worker','projects',json_array(w.project_id),'directory',json_extract(w.config,'$.cwd'),'prompt',json_extract(w.config,'$.prompt'),'concurrency',json_extract(w.config,'$.concurrency'),'tags',json_extract(w.config,'$.labels'),'use_goal',json(CASE WHEN json_extract(w.config,'$.use_goal')=1 THEN 'true' ELSE 'false' END),'enabled',json(CASE WHEN json_extract(w.config,'$.enabled')=1 THEN 'true' ELSE 'false' END),'reservation_seconds',120),w.version,w.updated_at FROM project_workers w JOIN projects p ON p.id=w.project_id;
UPDATE worker_runs SET worker_id='legacy:'||project_id,claimed_at=started_at WHERE EXISTS(SELECT 1 FROM issue_workers WHERE id='legacy:'||worker_runs.project_id);
";
// Call while holding the caller's write transaction: old updaters can write
// this marker after Store::open, including while a supervisor is still alive.
pub(super) fn migrate_runtime(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS issue_worker_runtime(worker_id TEXT PRIMARY KEY REFERENCES issue_workers(id),owner_pid INTEGER NOT NULL,owner_start TEXT NOT NULL)")?;
    db.execute("INSERT OR IGNORE INTO issue_worker_runtime SELECT id,owner_pid,owner_start FROM issue_workers WHERE json_extract(config,'$.upgrading')=1 AND owner_pid IS NOT NULL AND owner_start IS NOT NULL", [])?;
    db.execute("UPDATE issue_workers SET config=json_remove(CASE WHEN json_extract(config,'$.upgrading')=1 AND stop_requested=0 THEN json_set(config,'$.enabled',json('true')) ELSE config END,'$.upgrading') WHERE json_type(config,'$.upgrading') IS NOT NULL", [])?;
    Ok(())
}
fn read_settings(db: &Connection, id: &str) -> Result<(Settings, i64, String)> {
    let row: Option<(String, i64, String)> = db
        .query_row(
            "SELECT config,version,kind FROM issue_workers WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (s, v, k) = row.ok_or_else(|| Error::new("not_found", "Worker was not found"))?;
    Ok((serde_json::from_str(&s)?, v, k))
}
pub(super) fn project_settings(db: &Connection, p: &Project) -> Result<Value> {
    let row: Option<(String, bool, i64, String)> = db
        .query_row(
            "SELECT prompt,prs_enabled,version,boss_name FROM project_settings WHERE project_id=?1",
            [&p.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (prompt, prs, version, _legacy_name) =
        row.unwrap_or((worker::DEFAULT_PROMPT.into(), false, 0, "Boss".into()));
    let boss_name = crate::issues::global_settings::read(db)?["boss_name"].clone();
    Ok(
        json!({"ok":true,"project":p,"prompt":prompt,"prs_enabled":prs,"version":version,"boss_name":boss_name}),
    )
}
fn directory(db: &Connection, p: &Project) -> Result<String> {
    let mut stmt=db.prepare("SELECT json_extract(metadata,'$.cwd') FROM agents WHERE EXISTS(SELECT 1 FROM issues i WHERE i.project_id=?1 AND (i.created_by=agents.id OR i.assignee=agents.id)) ORDER BY last_seen DESC LIMIT 50")?;
    let paths = stmt
        .query_map([&p.id], |r| r.get::<_, Option<String>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for path in paths.into_iter().flatten() {
        if Path::new(&path).is_dir()
            && (p.id.starts_with("named:")
                || crate::issues::identity::project(
                    Path::new(&path),
                    &crate::issues::identity::machine()?,
                )
                .is_ok_and(|actual| actual.id == p.id))
        {
            return Ok(path);
        }
    }
    // Local directory IDs retain the absolute path.
    if let Some(path) =
        p.id.strip_prefix("local:")
            .and_then(|s| s.split_once(':').map(|(_, p)| p))
        && Path::new(path).is_dir()
    {
        return Ok(path.into());
    }
    Ok(String::new())
}
fn runtime(db: &Connection, c: &Settings, p: &Project) -> Result<ProjectConfig> {
    let defaults = project_settings(db, p)?;
    Ok(ProjectConfig {
        prompt: c
            .prompt
            .clone()
            .unwrap_or_else(|| defaults["prompt"].as_str().unwrap().into()),
        cwd: if c.directory.is_empty() {
            directory(db, p)?
        } else {
            c.directory.clone()
        },
        concurrency: c.concurrency,
        labels: c.tags.clone(),
        use_goal: c.use_goal,
        enabled: true,
        prs_enabled: c.prs_enabled.unwrap_or(defaults["prs_enabled"] == true),
    })
}
const ELIGIBLE:&str="i.state='open' AND i.deleted_at IS NULL AND i.assignee IS NULL AND p.hidden_at IS NULL
 AND (json_array_length(?1)=0 OR i.project_id IN(SELECT value FROM json_each(?1)))
 AND NOT EXISTS(SELECT 1 FROM json_each(?2) wanted WHERE NOT EXISTS(SELECT 1 FROM json_each(i.labels) existing WHERE existing.value=wanted.value))";
// Finished attempts do not permanently exclude unfinished issues. Approval holds
// still need explicit retry; other failures back off from 30 seconds to 5 minutes.
pub(super) const PICKUP_READY: &str = "
 AND NOT EXISTS(SELECT 1 FROM fleet_allocations f WHERE f.project_id=i.project_id AND f.issue_number=i.number AND f.node<>(SELECT node FROM fleet_meta WHERE id=1))
 AND ((SELECT role FROM fleet_meta WHERE id=1)<>'agent' OR EXISTS(SELECT 1 FROM fleet_allocations f WHERE f.project_id=i.project_id AND f.issue_number=i.number AND f.node=(SELECT node FROM fleet_meta WHERE id=1)))
 AND EXISTS(SELECT 1 FROM issue_pickup_ready ready WHERE ready.project_id=i.project_id AND ready.number=i.number)";
fn candidates(db: &Connection, c: &Settings, limit: i64) -> Result<Vec<(Project, i64)>> {
    let mut stmt=db.prepare(&format!("SELECT p.id,p.name,i.number FROM issues i JOIN projects p ON p.id=i.project_id WHERE {ELIGIBLE} {PICKUP_READY} ORDER BY i.sort_order,i.created_at,i.project_id,i.number LIMIT ?3"))?;
    Ok(stmt
        .query_map(
            params![
                serde_json::to_string(&c.projects)?,
                serde_json::to_string(&c.tags)?,
                limit
            ],
            |r| {
                Ok((
                    Project {
                        id: r.get(0)?,
                        name: r.get(1)?,
                    },
                    r.get(2)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
fn status(db: &Connection, id: Option<&str>, p: &Project) -> Result<Value> {
    let mut stmt=db.prepare("SELECT id,config,version,kind,owner_pid,updated_at,(SELECT count(*) FROM worker_runs r WHERE r.worker_id=w.id AND r.finished_at IS NULL) FROM issue_workers w ORDER BY updated_at DESC,id LIMIT 100")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<u32>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, u32>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Runtime state is additive, so older CLIs can still read worker settings.
    let runtime_exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='issue_worker_runtime')", [], |r| r.get(0))?;
    let mut workers:Vec<Value>=rows.into_iter().map(|(id,c,v,k,pid,at,active)|Ok(json!({"id":id,"config":serde_json::from_str::<Settings>(&c)?,"upgrading":false,"version":v,"kind":k,"pid":pid,"updated_at":at,"active":active}))).collect::<Result<_>>()?;
    if runtime_exists {
        for w in &mut workers {
            w["upgrading"] = json!(db.query_row("SELECT EXISTS(SELECT 1 FROM issue_worker_runtime r JOIN issue_workers w ON w.id=r.worker_id WHERE w.id=?1 AND r.owner_pid=w.owner_pid AND r.owner_start=w.owner_start)", [w["id"].as_str().unwrap()], |r| r.get::<_,bool>(0))?);
        }
    }
    let selected = if id == Some("new") {
        None
    } else {
        id.map(str::to_owned).or_else(|| {
            workers
                .first()
                .and_then(|w| w["id"].as_str())
                .map(str::to_owned)
        })
    };
    let (config, version, kind) = if let Some(id) = &selected {
        read_settings(db, id)?
    } else {
        (
            Settings {
                projects: vec![p.id.clone()],
                directory: directory(db, p)?,
                name: format!("{} worker", p.name),
                ..Settings::default()
            },
            0,
            "managed".into(),
        )
    };
    let active: i64 = db.query_row(
        "SELECT count(*) FROM worker_runs WHERE worker_id=?1 AND finished_at IS NULL",
        [&selected],
        |r| r.get(0),
    )?;
    let upgrading = workers
        .iter()
        .any(|w| w["id"].as_str() == selected.as_deref() && w["upgrading"] == true);
    let mut stmt=db.prepare("SELECT r.id,r.project_id,p.name,r.issue_number,json_extract(r.job,'$.issue.title'),r.session_id,r.state,r.pid,r.started_at,r.finished_at,r.stop_requested,r.summary,r.last_event,r.goal,r.reservation_expires,r.claimed_at FROM worker_runs r JOIN projects p ON p.id=r.project_id WHERE r.worker_id=?1 AND (r.finished_at IS NULL OR r.id IN(SELECT id FROM worker_runs WHERE worker_id=?1 AND finished_at IS NOT NULL ORDER BY started_at DESC,id DESC LIMIT 20)) ORDER BY r.finished_at IS NOT NULL,r.started_at DESC,r.id DESC")?;
    let mut runs=stmt.query_map([&selected],|r|Ok(json!({"id":r.get::<_,String>(0)?,"project_id":r.get::<_,String>(1)?,"project_name":r.get::<_,String>(2)?,"number":r.get::<_,i64>(3)?,"title":r.get::<_,String>(4)?,"session_id":r.get::<_,Option<String>>(5)?,"state":r.get::<_,String>(6)?,"pid":r.get::<_,Option<u32>>(7)?,"started_at":r.get::<_,i64>(8)?,"finished_at":r.get::<_,Option<i64>>(9)?,"stop_requested":r.get::<_,bool>(10)?,"summary":r.get::<_,String>(11)?,"last_event":r.get::<_,String>(12)?,"goal":r.get::<_,Option<String>>(13)?,"reservation_expires":r.get::<_,Option<i64>>(14)?,"claimed_at":r.get::<_,Option<i64>>(15)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for run in &mut runs {
        if let Some(s) = run["goal"].as_str() {
            run["goal"] = serde_json::from_str(s)?;
        }
        let mut events = db.prepare(
            "SELECT created_at,text FROM worker_events WHERE run_id=?1 ORDER BY id DESC LIMIT 3",
        )?;
        run["events"] = json!(
            events
                .query_map([run["id"].as_str().unwrap()], |r| Ok(
                    json!({"at":r.get::<_,i64>(0)?,"text":r.get::<_,String>(1)?})
                ))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        );
    }
    let eligible: i64 = db.query_row(
        &format!(
            "SELECT count(*) FROM issues i JOIN projects p ON p.id=i.project_id WHERE {ELIGIBLE} {PICKUP_READY}"
        ),
        params![
            serde_json::to_string(&config.projects)?,
            serde_json::to_string(&config.tags)?
        ],
        |r| r.get(0),
    )?;
    let fleet: Value = db.query_row("SELECT role,node,(SELECT count(*) FROM fleet_outbox) FROM fleet_meta WHERE id=1", [], |r| Ok(json!({"role":r.get::<_,String>(0)?,"node":r.get::<_,String>(1)?,"pending_changes":r.get::<_,i64>(2)?})))?;
    Ok(
        json!({"ok":true,"workers":workers,"worker_id":selected,"config":config,"version":version,"kind":kind,"upgrading":upgrading,"fleet":fleet,"active":active,"free":(config.concurrency as i64-active).max(0),"eligible":eligible,"runs":runs,"project":p}),
    )
}
pub(super) fn execute(
    db: &Connection,
    p: &Project,
    op: &Operation,
    actor: Option<&Actor>,
) -> Result<Value> {
    migrate_runtime(db)?;
    match op {
        Operation::Workers { worker_id } => status(db, worker_id.as_deref(), p),
        Operation::ConfigureWorker {
            worker_id,
            config,
            if_version,
        } => {
            worker::validate_settings(config)?;
            let id = worker_id.clone().unwrap_or(random_id()?);
            let version = if worker_id.is_some() {
                read_settings(db, &id)?.1
            } else {
                0
            };
            if if_version.is_some_and(|v| v != version) {
                return Err(Error::conflict(
                    "Worker settings changed elsewhere. Reload saved settings before saving.",
                ));
            }
            db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES(?1,'managed',?2,?3,?4) ON CONFLICT(id) DO UPDATE SET config=excluded.config,version=excluded.version,stop_requested=0,updated_at=excluded.updated_at",params![id,serde_json::to_string(config)?,version+1,now()])?;
            status(db, Some(&id), p)
        }
        Operation::ControlWorker {
            worker_id,
            command,
            run_id,
        } => {
            let (mut c, v, _) = read_settings(db, worker_id)?;
            match command.as_str() {
                "start" | "pause" | "stop_worker" => {
                    c.enabled = command == "start";
                    worker::validate_settings(&c)?;
                    db.execute("UPDATE issue_workers SET config=?2,version=?3,stop_requested=?4,updated_at=?5 WHERE id=?1",params![worker_id,serde_json::to_string(&c)?,v+1,command=="stop_worker",now()])?;
                    if command == "stop_worker" {
                        db.execute("UPDATE worker_runs SET stop_requested=1 WHERE worker_id=?1 AND finished_at IS NULL",[worker_id])?;
                    }
                }
                "stop" => {
                    let changed=db.execute("UPDATE worker_runs SET stop_requested=1 WHERE worker_id=?1 AND id=?2 AND finished_at IS NULL",params![worker_id,run_id])?;
                    if changed == 0 {
                        return Err(Error::conflict("Run already stopped"));
                    }
                }
                "retry" => {
                    let row:Option<(String,i64)>=db.query_row("SELECT project_id,issue_number FROM worker_runs WHERE worker_id=?1 AND id=?2 AND finished_at IS NOT NULL AND state!='completed'",params![worker_id,run_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                    let (p, n) = row.ok_or_else(|| {
                        Error::conflict("Only finished unsuccessful runs can be retried")
                    })?;
                    db.execute("UPDATE worker_runs SET retry_allowed=1 WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NOT NULL",params![p,n])?;
                }
                _ => return Err(Error::invalid("Unknown worker action")),
            }
            status(db, Some(worker_id), p)
        }
        Operation::PreviewWorker { config, number } => {
            let candidate = if let Some(n) = number {
                Some((p.clone(), *n))
            } else {
                candidates(db, config, 1)?.into_iter().next()
            };
            let fallback = if config.projects.len() == 1 {
                let id = &config.projects[0];
                let name: Option<String> = db
                    .query_row("SELECT name FROM projects WHERE id=?1", [id], |r| r.get(0))
                    .optional()?;
                Project {
                    id: id.clone(),
                    name: name.ok_or_else(|| Error::invalid("Worker project was not found"))?,
                }
            } else {
                p.clone()
            };
            let (project, n) = candidate.clone().unwrap_or((fallback, 1));
            let issue = if candidate.is_some() {
                json!(get_issue(db, &project.id, n, false)?)
            } else {
                json!({"number":"<number>","title":"<issue title>","body":"<issue body>"})
            };
            let runtime = runtime(db, config, &project)?;
            let (prompt, goal, objective) = worker::preview(&runtime, &project, issue);
            Ok(
                json!({"ok":true,"prompt":prompt,"use_goal":goal,"objective":objective,"number":candidate.map(|(_,n)|n),"project":project,"template":runtime.prompt,"prs_enabled":runtime.prs_enabled}),
            )
        }
        Operation::ProjectSettings => project_settings(db, p),
        Operation::ConfigureProject {
            prompt,
            boss_name,
            prs_enabled,
            if_version,
        } => {
            let defaults = project_settings(db, p)?;
            let prompt = prompt
                .clone()
                .unwrap_or_else(|| defaults["prompt"].as_str().unwrap().into());
            let legacy_name = boss_name
                .clone()
                .unwrap_or_else(|| defaults["boss_name"].as_str().unwrap().into());
            super::identifier(&legacy_name, "Boss name", 64)?;
            let legacy_name = legacy_name.trim();
            let prs_enabled = prs_enabled.unwrap_or(defaults["prs_enabled"] == true);
            if prompt.trim().is_empty() || prompt.len() > 32000 {
                return Err(Error::invalid("Project prompt must contain 1–32000 bytes"));
            }
            let v = project_settings(db, p)?["version"].as_i64().unwrap();
            if if_version.is_some_and(|old| old != v) {
                return Err(Error::conflict("Project settings changed elsewhere"));
            }
            if let Some(name) = boss_name {
                crate::issues::global_settings::configure(db, name, None)?;
            }
            db.execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,boss_name) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(project_id) DO UPDATE SET prompt=excluded.prompt,prs_enabled=excluded.prs_enabled,version=excluded.version,boss_name=excluded.boss_name",params![p.id,prompt,prs_enabled,v+1,legacy_name])?;
            project_settings(db, p)
        }
        Operation::PullRequests { number }
        | Operation::AddPullRequest { number, .. }
        | Operation::RemovePullRequest { number, .. } => {
            get_issue(db, &p.id, *number, true)?;
            let mut changed = 0;
            if let Operation::AddPullRequest { url, .. }
            | Operation::RemovePullRequest { url, .. } = op
            {
                let after = url
                    .strip_prefix("https://")
                    .or_else(|| url.strip_prefix("http://"))
                    .ok_or_else(|| Error::invalid("PR links must be HTTP(S) URLs"))?;
                if url.len() > 2048
                    || after.split('/').next().unwrap_or("").is_empty()
                    || after.split('/').next().unwrap_or("").contains('@')
                    || url.chars().any(|c| c.is_control() || c.is_whitespace())
                {
                    return Err(Error::invalid("Invalid PR URL"));
                }
                let actor = actor.unwrap();
                changed = if matches!(op, Operation::AddPullRequest { .. }) {
                    db.execute("INSERT INTO issue_pull_requests VALUES(?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING",params![p.id,number,url,actor.id,now()])?
                } else {
                    db.execute("DELETE FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2 AND url=?3",params![p.id,number,url])?
                };
                if changed > 0 {
                    db.execute("UPDATE issues SET version=version+1,updated_at=?3 WHERE project_id=?1 AND number=?2",params![p.id,number,now()])?;
                    event(
                        db,
                        &p.id,
                        *number,
                        &actor.id,
                        if matches!(op, Operation::AddPullRequest { .. }) {
                            "pr_attached"
                        } else {
                            "pr_removed"
                        },
                        now(),
                        &json!({"url":url}),
                    )?;
                }
            }
            Ok(
                json!({"ok":true,"project":p,"number":number,"pull_requests":pull_requests(db,&p.id,*number)?,"changed":changed>0}),
            )
        }
        _ => unreachable!(),
    }
}
pub(super) fn pull_requests(db: &Connection, p: &str, n: i64) -> Result<Vec<Value>> {
    let mut stmt=db.prepare("SELECT url,added_by,created_at FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2 ORDER BY created_at,url")?;
    Ok(stmt.query_map(params![p,n],|r|Ok(json!({"url":r.get::<_,String>(0)?,"added_by":r.get::<_,String>(1)?,"created_at":r.get::<_,i64>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?)
}
pub(super) fn claim_lock(
    db: &Connection,
    p: &Project,
    n: i64,
    actor: &Actor,
    force: bool,
) -> Result<()> {
    crate::issues::fleet::check_claim(db, &p.id, n, &actor.machine, force)?;
    let expired: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 AND finished_at IS NULL AND claimed_at IS NULL AND reservation_expires<=?4)",params![p.id,n,actor.id,now()],|r|r.get(0))?;
    if expired {
        return Err(Error::conflict(
            "This worker's reservation expired; it must stop before this issue is retried",
        ));
    }
    let lock:Option<(String,i64)>=db.query_row("SELECT actor_id,reservation_expires FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL AND claimed_at IS NULL AND reservation_expires>?3",params![p.id,n,now()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((owner, expires)) = lock
        && owner != actor.id
        && !force
    {
        return Err(Error::conflict(format!(
            "Issue is reserved for another worker until {expires}; it has not been claimed yet"
        )));
    }
    db.execute("UPDATE worker_runs SET claimed_at=?4,reservation_expires=NULL,state='running',updated_at=?4 WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 AND finished_at IS NULL",params![p.id,n,actor.id,now()])?;
    Ok(())
}
pub(super) fn claim_instructions(
    db: &Connection,
    p: &Project,
    issue: &Value,
    actor: &Actor,
) -> Result<String> {
    let job:Option<String>=db.query_row("SELECT job FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 AND finished_at IS NULL",params![p.id,issue["number"].as_i64().unwrap(),actor.id],|r|r.get(0)).optional()?;
    let config = if let Some(job) = job {
        serde_json::from_str::<Job>(&job)?.config
    } else {
        runtime(db, &Settings::default(), p)?
    };
    Ok(worker::preview(&config, p, issue.clone()).0)
}
pub(super) fn reserve(
    store: &mut Store,
    machine: &str,
    worker_id: Option<&str>,
) -> Result<Option<Job>> {
    let tx = store
        .db
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    migrate_runtime(&tx)?;
    let mut stmt=tx.prepare("SELECT id,config FROM issue_workers w WHERE (?1 IS NOT NULL AND id=?1 OR ?1 IS NULL AND kind='managed') AND json_extract(config,'$.enabled')=1 AND stop_requested=0 AND NOT EXISTS(SELECT 1 FROM issue_worker_runtime runtime WHERE runtime.worker_id=w.id AND runtime.owner_pid=w.owner_pid AND runtime.owner_start=w.owner_start) AND (SELECT count(*) FROM worker_runs r WHERE r.worker_id=w.id AND r.finished_at IS NULL)<json_extract(config,'$.concurrency') ORDER BY updated_at,id")?;
    let defs = stmt
        .query_map([worker_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    for (worker_id, text) in defs {
        let settings: Settings = serde_json::from_str(&text)?;
        for (project, number) in candidates(&tx, &settings, 100)? {
            let config = runtime(&tx, &settings, &project)?;
            if config.cwd.is_empty() {
                continue;
            }
            // Reject a mismatched checkout before issuing a reservation.
            if worker::validate_config(&config, &project).is_err() {
                continue;
            }
            let id = random_id()?;
            let owner_pid = std::process::id();
            let owner_start = crate::agents::process_identity(owner_pid)
                .ok_or_else(|| Error::new("worker_error", "Cannot identify worker process"))?;
            let actor = Actor {
                id: format!("reservation:{id}"),
                kind: "worker".into(),
                session_id: None,
                machine: machine.into(),
                host: crate::issues::identity::host(),
                pid: None,
                process_start: None,
                cwd: config.cwd.clone().into(),
                source: "unclaimed worker reservation".into(),
            };
            let issue = json!(get_issue(&tx, &project.id, number, false)?);
            let job = Job {
                id: id.clone(),
                worker_id: worker_id.clone(),
                project: project.clone(),
                issue,
                comments: vec![],
                config,
                actor,
                owner_pid,
                owner_start: owner_start.clone(),
                machine: machine.into(),
            };
            tx.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,reservation_expires) VALUES(?1,?2,?3,?4,?5,'reserved',?6,?7,?8,?9,?9,?10,?11)",params![id,project.id,number,serde_json::to_string(&job)?,job.actor.id,owner_pid,owner_start,machine,now(),worker_id,now()+settings.reservation_seconds as i64*1000])?;
            tx.commit()?;
            return Ok(Some(job));
        }
    }
    Ok(None)
}
impl Store {
    pub fn register_worker(
        &mut self,
        id: Option<&str>,
        settings: &Settings,
        machine: &str,
    ) -> Result<String> {
        worker::validate_settings(settings)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = id.map(str::to_owned).unwrap_or(random_id()?);
        let prior: Option<(Option<u32>, Option<String>)> = tx
            .query_row(
                "SELECT owner_pid,owner_start FROM issue_workers WHERE id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((Some(pid), Some(start))) = prior
            && crate::agents::process_identity(pid).as_deref() == Some(&start)
        {
            return Err(Error::conflict("This worker is already running"));
        }
        let pid = std::process::id();
        let start = crate::agents::process_identity(pid)
            .ok_or_else(|| Error::new("worker_error", "Cannot identify worker process"))?;
        tx.execute("INSERT INTO issue_workers(id,kind,config,version,owner_pid,owner_start,machine,updated_at) VALUES(?1,'cli',?2,1,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET kind='cli',config=excluded.config,version=version+1,owner_pid=excluded.owner_pid,owner_start=excluded.owner_start,machine=excluded.machine,stop_requested=0,updated_at=excluded.updated_at",params![id,serde_json::to_string(settings)?,pid,start,machine,now()])?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS issue_worker_runtime(worker_id TEXT PRIMARY KEY REFERENCES issue_workers(id),owner_pid INTEGER NOT NULL,owner_start TEXT NOT NULL)")?;
        tx.execute("DELETE FROM issue_worker_runtime WHERE worker_id=?1", [&id])?;
        tx.commit()?;
        Ok(id)
    }
    pub fn unregister_worker(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE issue_workers SET owner_pid=NULL,owner_start=NULL,config=json_set(config,'$.enabled',json('false')),version=version+1,updated_at=?2 WHERE id=?1 AND owner_pid=?3",params![id,now(),std::process::id()])?;
        Ok(())
    }
    pub(crate) fn worker_shutdown_requested(&self, id: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT stop_requested FROM issue_workers WHERE id=?1",
            [id],
            |r| r.get(0),
        )?)
    }
    pub(crate) fn worker_reload_allowed(&self, id: &str) -> Result<bool> {
        Ok(self.db.query_row("SELECT (json_extract(config,'$.enabled')=1 OR coalesce(json_extract(config,'$.upgrading')=1,0)) AND stop_requested=0 FROM issue_workers WHERE id=?1", [id], |r| r.get(0))?)
    }
    pub(crate) fn worker_mark_upgrading(&self, id: &str) -> Result<()> {
        self.db.execute("INSERT OR IGNORE INTO issue_worker_runtime(worker_id,owner_pid,owner_start) SELECT id,owner_pid,owner_start FROM issue_workers WHERE id=?1 AND owner_pid=?2", params![id,std::process::id()])?;
        Ok(())
    }
}

impl Store {
    pub(crate) fn prune_workers(&self, machine: &str) -> Result<()> {
        let mut stmt = self.db.prepare("SELECT id,owner_pid,owner_start FROM issue_workers WHERE kind='cli' AND machine=?1 AND owner_pid IS NOT NULL")?;
        let rows = stmt
            .query_map([machine], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u32>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, pid, start) in rows {
            if crate::agents::process_identity(pid).as_deref() != Some(&start) {
                self.db.execute("UPDATE issue_workers SET owner_pid=NULL,owner_start=NULL,config=json_set(config,'$.enabled',json('false')),version=version+1,updated_at=?2 WHERE id=?1 AND owner_pid=?3 AND owner_start=?4",params![id,now(),pid,start])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_marker_written_after_open_preserves_session_and_blocks_new_pickup() {
        let root = std::env::temp_dir().join(format!("hb-legacy-worker-{}", random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let mut store = Store::open(&root.join("issues.db")).unwrap();
            let project = Project {
                id: "named:Legacy QA".into(),
                name: "Legacy QA".into(),
            };
            store.db.execute("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES(?1,?2,3,0,0)", params![project.id,project.name]).unwrap();
            store
                .db
                .execute("INSERT INTO agents VALUES('agent','{}',0)", [])
                .unwrap();
            for number in 1..=2 {
                store.db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) VALUES(?1,?2,'Task','','open','agent',0,0,1,'[]',?2)", params![project.id,number]).unwrap();
            }
            let config = Settings {
                concurrency: 2,
                directory: root.to_string_lossy().into(),
                projects: vec![project.id.clone()],
                ..Settings::default()
            };
            let id = store.register_worker(None, &config, "unit").unwrap();
            let pid = std::process::id();
            let start = crate::agents::process_identity(pid).unwrap();
            store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,session_id,claimed_at) VALUES('saved-run',?1,1,'{\"issue\":{\"title\":\"Task\"}}','agent','running',?2,?3,'unit',1,1,?4,'saved-session',1)", params![project.id,pid,start,id]).unwrap();
            store.db.execute("UPDATE issue_workers SET config=json_set(config,'$.enabled',json('false'),'$.upgrading',json('true')) WHERE id=?1", [&id]).unwrap();
            assert!(store.worker_reload_allowed(&id).unwrap());
            let request = Request {
                version: 1,
                project,
                project_override: None,
                actor: None,
                operation: Operation::Workers {
                    worker_id: Some(id.clone()),
                },
                request_id: None,
            };
            let status = store.execute(&request).unwrap();
            assert_eq!(status["active"], 1);
            assert_eq!(status["config"]["enabled"], true);
            assert_eq!(status["upgrading"], true);
            assert_eq!(status["runs"][0]["session_id"], "saved-session");
            assert!(reserve(&mut store, "unit", Some(&id)).unwrap().is_none());
            // Exercise the reservation path with a newly written legacy marker,
            // before any status/open path has had a chance to migrate it.
            store
                .db
                .execute("DELETE FROM issue_worker_runtime WHERE worker_id=?1", [&id])
                .unwrap();
            store.db.execute("UPDATE issue_workers SET config=json_set(config,'$.enabled',json('false'),'$.upgrading',json('true')) WHERE id=?1", [&id]).unwrap();
            assert!(reserve(&mut store, "unit", Some(&id)).unwrap().is_none());
            assert_eq!(
                store.execute(&request).unwrap()["runs"][0]["session_id"],
                "saved-session"
            );
            assert!(store.db.query_row("SELECT json_type(config,'$.upgrading') IS NULL FROM issue_workers WHERE id=?1", [&id], |row| row.get::<_,bool>(0)).unwrap());
            store.db.execute("UPDATE issue_workers SET stop_requested=1,config=json_set(config,'$.enabled',json('false'),'$.upgrading',json('true')) WHERE id=?1", [&id]).unwrap();
            assert!(!store.worker_reload_allowed(&id).unwrap());
            assert_eq!(store.execute(&request).unwrap()["config"]["enabled"], false);
            assert!(reserve(&mut store, "unit", Some(&id)).unwrap().is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn status_includes_every_active_codex_and_only_twenty_finished_runs() {
        let root = std::env::temp_dir().join(format!("hb-worker-status-{}", random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let store = Store::open(&root.join("issues.db")).unwrap();
            let db = &store.db;
            let p = Project {
                id: "named:Status QA".into(),
                name: "Status QA".into(),
            };
            db.execute("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES(?1,?2,100,0,0)",params![p.id,p.name]).unwrap();
            db.execute("INSERT INTO agents VALUES('agent','{}',0)", [])
                .unwrap();
            let c = Settings {
                concurrency: 30,
                ..Settings::default()
            };
            db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES('worker','managed',?1,1,0)",[serde_json::to_string(&c).unwrap()]).unwrap();
            for n in 1..=49 {
                db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES(?1,?2,'Task','','open','agent',0,0,1,'[]')",params![p.id,n]).unwrap();
                db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,finished_at) VALUES(?1,?2,?3,'{\"issue\":{\"title\":\"Task\"}}','agent','running',1,'start','machine',?3,0,'worker',?4)",params![format!("run-{n}"),p.id,n,if n<=24 {None}else{Some(n)}]).unwrap();
            }
            let s = status(db, Some("worker"), &p).unwrap();
            assert_eq!(s["active"], 24);
            assert_eq!(s["free"], 6);
            assert_eq!(s["runs"].as_array().unwrap().len(), 44);
            assert_eq!(
                s["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|r| r["finished_at"].is_null())
                    .count(),
                24
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
