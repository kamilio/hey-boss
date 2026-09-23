//! Independent worker instances and first-class project instructions/PR links.
use super::*;
use crate::issues::worker::{self, Job, ProjectConfig, Settings, now, random_id};
use std::collections::HashMap;
pub(super) const FINISHED_HISTORY_INDEX: &str = "CREATE INDEX IF NOT EXISTS worker_finished_history ON worker_runs(worker_id,started_at DESC,id DESC) WHERE finished_at IS NOT NULL;";

pub(super) fn stale_pr_capture(db: &Connection) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name IN ('fleet_capture_issue_pull_requests_INSERT','fleet_capture_issue_pull_requests_UPDATE','fleet_capture_issue_pull_requests_DELETE') AND (instr(sql,'''purpose'',')=0 OR (name='fleet_capture_issue_pull_requests_UPDATE' AND instr(sql,'OLD.\"purpose\" IS NEW.\"purpose\"')=0)))", [], |r| r.get(0))?)
}

// Run inside the additive migration's write transaction. Existing fleet
// triggers retain their fixed column lists across ALTER TABLE and restart.
pub(super) fn repair_pr_capture(db: &Connection) -> Result<()> {
    let triggers = db.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND name IN ('fleet_capture_issue_pull_requests_INSERT','fleet_capture_issue_pull_requests_UPDATE','fleet_capture_issue_pull_requests_DELETE')")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (name, original) in triggers {
        let mut sql = original.clone();
        if !sql.contains("'purpose',") {
            sql = sql
                .replace(
                    "json_object('project_id',NEW.",
                    "json_object('purpose',NEW.\"purpose\",'project_id',NEW.",
                )
                .replace(
                    "json_object('project_id',OLD.",
                    "json_object('purpose',OLD.\"purpose\",'project_id',OLD.",
                );
        }
        if name.ends_with("_UPDATE") && !sql.contains("OLD.\"purpose\" IS NEW.\"purpose\"") {
            sql = sql.replace(
                " AND NOT (",
                " AND NOT (OLD.\"purpose\" IS NEW.\"purpose\" AND ",
            );
        }
        if sql != original {
            db.execute_batch(&format!("DROP TRIGGER {name}; {sql}"))?;
        }
    }
    Ok(())
}
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
// this marker after Store::open, including while a worker is still alive.
pub(super) fn migrate_runtime(db: &Connection) -> Result<()> {
    if !db.query_row("SELECT EXISTS(SELECT 1 FROM issue_workers WHERE json_type(config,'$.upgrading') IS NOT NULL)", [], |row| row.get::<_, bool>(0))? {
        return Ok(());
    }
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
struct ProjectSettingsRow {
    chief_enabled: bool,
    chief_prompt: Option<String>,
    prompt: String,
    prs: bool,
    version: i64,
    drafts_enabled: bool,
    plan_template: String,
    worktree_enabled: bool,
    overrides: String,
}
pub(super) fn project_settings(db: &Connection, p: &Project) -> Result<Value> {
    let row = db.query_row(
        "SELECT prompt,prs_enabled,version,drafts_enabled,plan_template,worktree_enabled,prompt_overrides,chief_enabled,chief_prompt FROM project_settings WHERE project_id=?1",
        [&p.id],
        |r| Ok(ProjectSettingsRow {
            chief_enabled: r.get(7)?, chief_prompt: r.get(8)?,
            prompt: r.get(0)?, prs: r.get(1)?, version: r.get(2)?,
            drafts_enabled: r.get(3)?, plan_template: r.get(4)?,
            worktree_enabled: r.get(5)?, overrides: r.get(6)?,
        }),
    ).optional()?;
    let ProjectSettingsRow {
        chief_enabled,
        chief_prompt,
        prompt,
        prs,
        version,
        drafts_enabled,
        plan_template,
        worktree_enabled,
        overrides,
    } = row.unwrap_or_else(|| ProjectSettingsRow {
        chief_enabled: false,
        chief_prompt: None,
        prompt: worker::DEFAULT_PROMPT.into(),
        prs: false,
        version: 0,
        drafts_enabled: true,
        plan_template: "plans/{timestamp}-{number}.md".into(),
        worktree_enabled: false,
        overrides: "{}".into(),
    });
    let prompt_overrides: worker::PromptOverrides = serde_json::from_str(&overrides)?;
    let prompt = worker::base_prompt(&prompt);
    let boss_name = crate::issues::global_settings::read(db)?["boss_name"].clone();
    Ok(
        json!({"ok":true,"project":p,"prompt":prompt,"chief_enabled":chief_enabled,"chief_prompt":chief_prompt.as_deref().unwrap_or(super::super::chief::DEFAULT_PROMPT),"chief_default_prompt":super::super::chief::DEFAULT_PROMPT,"prs_enabled":prs,"worktree_enabled":worktree_enabled,"prompt_overrides":prompt_overrides,"prompt_defaults":{"plan":worker::DEFAULT_PLAN_PROMPT,"worktree":worker::DEFAULT_WORKTREE_PROMPT,"checkout":worker::DEFAULT_CHECKOUT_PROMPT,"prs":worker::DEFAULT_PRS_PROMPT,"main":worker::DEFAULT_MAIN_PROMPT},"drafts_enabled":drafts_enabled,"plan_template":plan_template,"version":version,"boss_name":boss_name}),
    )
}
// Discover the project's agents first instead of rescanning its issues for
// every agent in the store. Both legacy and independent workers use this.
pub(super) const PROJECT_DIRECTORIES: &str = "SELECT json_extract(metadata,'$.cwd') FROM agents
 WHERE id IN(SELECT created_by FROM issues WHERE project_id=?1
 UNION SELECT assignee FROM issues WHERE project_id=?1 AND assignee IS NOT NULL)
 ORDER BY last_seen DESC LIMIT ?2";
// Resolve the bounded set of IDs first. An outer worker_id/OR filter scans all
// attempts even when the history subquery is indexed.
const STATUS_RUNS: &str = "SELECT r.id,r.project_id,p.name,r.issue_number,json_extract(r.job,'$.issue.title'),r.session_id,r.state,r.pid,r.started_at,r.finished_at,r.stop_requested,r.summary,r.last_event,r.goal,r.reservation_expires,r.claimed_at,r.actor_id
 FROM worker_runs r JOIN projects p ON p.id=r.project_id
 WHERE r.id IN(SELECT id FROM worker_runs WHERE worker_id=?1 AND finished_at IS NULL
 UNION ALL SELECT id FROM(SELECT id FROM worker_runs WHERE worker_id=?1 AND finished_at IS NOT NULL ORDER BY started_at DESC,id DESC LIMIT 20))
 ORDER BY r.finished_at IS NOT NULL,r.started_at DESC,r.id DESC";
fn directory(db: &Connection, p: &Project) -> Result<String> {
    let mut stmt = db.prepare(PROJECT_DIRECTORIES)?;
    let paths = stmt
        .query_map(params![p.id, 50], |r| r.get::<_, Option<String>>(0))?
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
        cwd: checkout(db, c, p)?,
        concurrency: c.concurrency,
        labels: c.tags.clone(),
        use_goal: c.use_goal,
        enabled: true,
        prs_enabled: c.prs_enabled.unwrap_or(defaults["prs_enabled"] == true),
        // Project permission is a ceiling, not a worker's workspace choice.
        worktree_enabled: defaults["worktree_enabled"] == true && c.worktree_enabled == Some(true),
        prompt_overrides: c.prompt_overrides.clone().unwrap_or(serde_json::from_value(
            defaults["prompt_overrides"].clone(),
        )?),
    })
}

fn checkout(db: &Connection, c: &Settings, p: &Project) -> Result<String> {
    if let Some(path) = c.directories.get(&p.id) {
        Ok(path.clone())
    } else if !c.directory.is_empty() {
        Ok(c.directory.clone())
    } else {
        directory(db, p)
    }
}

impl Store {
    /// Return machine activity from one snapshot without repeating the overview.
    pub(crate) fn fleet_workers(&self) -> Result<Vec<Value>> {
        retry_contention(Instant::now() + CONTENTION_BUDGET, || {
            let legacy_runtime: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_workers WHERE json_type(config,'$.upgrading') IS NOT NULL)", [], |r| r.get(0))?;
            let tx = rusqlite::Transaction::new_unchecked(
                &self.db,
                if legacy_runtime {
                    TransactionBehavior::Immediate
                } else {
                    TransactionBehavior::Deferred
                },
            )?;
            migrate_runtime(&tx)?;
            let mut workers = worker_overview(&tx)?;
            // Queue counts depend on these sets, not the worker ID or capacity.
            // Cache only within this transaction so each poll sees fresh data.
            let mut eligible_counts = HashMap::new();
            for worker in &mut workers {
                let config: Settings = serde_json::from_value(worker["config"].clone())?;
                let mut projects = config.projects.clone();
                let mut tags = config.tags.clone();
                projects.sort_unstable();
                projects.dedup();
                tags.sort_unstable();
                tags.dedup();
                let eligible = match eligible_counts.entry((projects, tags)) {
                    std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        *entry.insert(worker_queue(&tx, &config)?["eligible"].as_i64().unwrap())
                    }
                };
                let Value::Object(mut activity) =
                    worker_activity(&tx, worker["id"].as_str(), &config)?
                else {
                    unreachable!("worker activity is an object")
                };
                // The machine protocol exposes capacity and activity, while
                // queue diagnostics belong to the public status response.
                activity.insert("eligible".into(), json!(eligible));
                worker.as_object_mut().unwrap().extend(activity);
            }
            tx.commit()?;
            Ok(workers)
        })
    }

    /// Refresh text dependencies only. Workspace/delivery choices are fixed for
    /// an active task: changing them halfway through would invalidate its work.
    pub(crate) fn worker_prompt_config(&self, job: &Job) -> Result<ProjectConfig> {
        let mut config = job.config.clone();
        if job.worker_id.is_empty() {
            return Ok(config);
        }
        let (settings, _, _) = read_settings(&self.db, &job.worker_id)?;
        let defaults = project_settings(&self.db, &job.project)?;
        config.prompt = settings
            .prompt
            .unwrap_or_else(|| defaults["prompt"].as_str().unwrap().into());
        config.prompt_overrides = settings.prompt_overrides.unwrap_or(serde_json::from_value(
            defaults["prompt_overrides"].clone(),
        )?);
        Ok(config)
    }

    pub(crate) fn chief_candidates(
        &self,
        worker_id: Option<&str>,
    ) -> Result<Vec<(String, String, String, String)>> {
        let mut stmt = self.db.prepare("SELECT id,config FROM issue_workers w WHERE (?1 IS NOT NULL AND id=?1 OR ?1 IS NULL AND kind='managed') AND json_extract(config,'$.enabled')=1 AND stop_requested=0 AND NOT EXISTS(SELECT 1 FROM issue_worker_runtime runtime WHERE runtime.worker_id=w.id AND runtime.owner_pid=w.owner_pid AND runtime.owner_start=w.owner_start)")?;
        let settings = stmt
            .query_map([worker_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut projects_stmt = self.db.prepare("SELECT p.id,p.name FROM projects p JOIN project_settings s ON s.project_id=p.id WHERE s.chief_enabled=1 AND p.hidden_at IS NULL ORDER BY p.id")?;
        let projects = projects_stmt
            .query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut result = Vec::new();
        for (worker_id, text) in settings {
            let config: Settings = serde_json::from_str(&text)?;
            for project in &projects {
                if !config.projects.is_empty() && !config.projects.contains(&project.id) {
                    continue;
                }
                let cwd = checkout(&self.db, &config, project)?;
                if cwd.is_empty() {
                    continue;
                }
                let prompt = project_settings(&self.db, project)?["chief_prompt"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                result.push((project.id.clone(), cwd, prompt, worker_id.clone()));
            }
        }
        Ok(result)
    }
}
const ELIGIBLE:&str="i.state='open' AND i.deleted_at IS NULL AND i.assignee IS NULL AND p.hidden_at IS NULL
 AND (json_array_length(?1)=0 OR i.project_id IN(SELECT value FROM json_each(?1)))
 AND NOT EXISTS(SELECT 1 FROM json_each(?2) wanted WHERE NOT EXISTS(SELECT 1 FROM json_each(i.labels) existing WHERE existing.value=wanted.value))";
// Finished attempts do not permanently exclude unfinished issues. Approval holds
// still need explicit retry; other failures back off from 30 seconds to 5 minutes.
pub(super) const PICKUP_READY: &str = "
 AND i.draft=0
 AND NOT EXISTS(SELECT 1 FROM fleet_allocation_deadlines d WHERE d.project_id=i.project_id AND d.issue_number=i.number AND d.expires_at<=CAST(strftime('%s','now') AS INTEGER)*1000)
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
fn worker_overview(db: &Connection) -> Result<Vec<Value>> {
    let builds_exist: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='issue_worker_builds')", [], |r| r.get(0))?;
    let build = if builds_exist {
        "(SELECT build FROM issue_worker_builds b WHERE b.worker_id=w.id AND b.owner_pid=w.owner_pid AND b.owner_start=w.owner_start)"
    } else {
        "NULL"
    };
    let mut stmt=db.prepare(&format!("SELECT id,config,version,kind,owner_pid,updated_at,(SELECT count(*) FROM worker_runs r WHERE r.worker_id=w.id AND r.finished_at IS NULL),{build},owner_start FROM issue_workers w ORDER BY updated_at DESC,id LIMIT 100"))?;
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
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Runtime state is additive, so older CLIs can still read worker settings.
    let runtime_exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='issue_worker_runtime')", [], |r| r.get(0))?;
    let mut workers: Vec<Value> = rows.into_iter().map(|(id,c,v,k,pid,at,active,build,start)| {
        let build = build.filter(|_| pid.zip(start.as_deref()).is_some_and(|(pid,start)| crate::agents::process_identity(pid).as_deref() == Some(start)));
        Ok(json!({"id":id,"config":serde_json::from_str::<Settings>(&c)?,"upgrading":false,"version":v,"kind":k,"pid":pid,"updated_at":at,"active":active,"build":build}))
    }).collect::<Result<_>>()?;
    if runtime_exists {
        for w in &mut workers {
            w["upgrading"] = json!(db.query_row("SELECT EXISTS(SELECT 1 FROM issue_worker_runtime r JOIN issue_workers w ON w.id=r.worker_id WHERE w.id=?1 AND r.owner_pid=w.owner_pid AND r.owner_start=w.owner_start)", [w["id"].as_str().unwrap()], |r| r.get::<_,bool>(0))?);
        }
    }
    Ok(workers)
}
fn worker_activity(db: &Connection, selected: Option<&str>, config: &Settings) -> Result<Value> {
    let active: i64 = db.query_row(
        "SELECT count(*) FROM worker_runs WHERE worker_id=?1 AND finished_at IS NULL",
        [&selected],
        |r| r.get(0),
    )?;
    let mut stmt = db.prepare(STATUS_RUNS)?;
    let mut runs=stmt.query_map([&selected],|r|Ok(json!({"id":r.get::<_,String>(0)?,"project_id":r.get::<_,String>(1)?,"project_name":r.get::<_,String>(2)?,"number":r.get::<_,i64>(3)?,"title":r.get::<_,String>(4)?,"session_id":r.get::<_,Option<String>>(5)?,"state":r.get::<_,String>(6)?,"pid":r.get::<_,Option<u32>>(7)?,"started_at":r.get::<_,i64>(8)?,"finished_at":r.get::<_,Option<i64>>(9)?,"stop_requested":r.get::<_,bool>(10)?,"summary":r.get::<_,String>(11)?,"last_event":r.get::<_,String>(12)?,"goal":r.get::<_,Option<String>>(13)?,"reservation_expires":r.get::<_,Option<i64>>(14)?,"claimed_at":r.get::<_,Option<i64>>(15)?,"actor_id":r.get::<_,String>(16)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for run in &mut runs {
        if let Some(s) = run["goal"].as_str() {
            run["goal"] = serde_json::from_str(s)?;
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
    let chiefs = super::super::chief::status(db, selected)?;
    Ok(
        json!({"active":active,"free":(config.concurrency as i64-active).max(0),"runs":runs,"chiefs":chiefs}),
    )
}

fn worker_queue(db: &Connection, config: &Settings) -> Result<Value> {
    let (open, assigned, tag_filtered, eligible): (i64, i64, i64, i64) = db.query_row(
        &format!(
            "SELECT count(*),
             coalesce(sum(i.assignee IS NOT NULL),0),
             coalesce(sum(i.assignee IS NULL AND EXISTS(SELECT 1 FROM json_each(?2) wanted WHERE NOT EXISTS(SELECT 1 FROM json_each(i.labels) existing WHERE existing.value=wanted.value))),0),
             coalesce(sum(CASE WHEN {ELIGIBLE} {PICKUP_READY} THEN 1 ELSE 0 END),0)
             FROM issues i JOIN projects p ON p.id=i.project_id
             WHERE i.state='open' AND i.deleted_at IS NULL AND p.hidden_at IS NULL
             AND (json_array_length(?1)=0 OR i.project_id IN(SELECT value FROM json_each(?1)))"
        ),
        params![
            serde_json::to_string(&config.projects)?,
            serde_json::to_string(&config.tags)?
        ],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    Ok(
        json!({"open":open,"assigned":assigned,"tag_filtered":tag_filtered,"waiting":open-assigned-tag_filtered-eligible,"eligible":eligible}),
    )
}
fn status(db: &Connection, id: Option<&str>, p: &Project) -> Result<Value> {
    let workers = worker_overview(db)?;
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
    let mut stmt =
        db.prepare("SELECT id,name FROM projects WHERE id IN (SELECT value FROM json_each(?1))")?;
    let projects = stmt
        .query_map([serde_json::to_string(&config.projects)?], |r| {
            Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?}))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let upgrading = workers
        .iter()
        .any(|w| w["id"].as_str() == selected.as_deref() && w["upgrading"] == true);
    let mut fleet: Value = db.query_row("SELECT role,node,(SELECT count(*) FROM fleet_outbox) FROM fleet_meta WHERE id=1", [], |r| Ok(json!({"role":r.get::<_,String>(0)?,"node":r.get::<_,String>(1)?,"pending_changes":r.get::<_,i64>(2)?})))?;
    fleet["supervisor_connection"] =
        crate::fleet::worker_connection(fleet["role"].as_str().unwrap_or_default());
    // Older dashboards read this key; retain it during mixed-version upgrades.
    fleet["controller_connection"] = fleet["supervisor_connection"].clone();
    if fleet["role"] == crate::fleet::SUPERVISOR_ROLE {
        fleet["role"] = json!("supervisor");
    } else if fleet["role"] == crate::fleet::COMPANION_ROLE {
        fleet["role"] = json!("companion");
    }
    let mut result = worker_activity(db, selected.as_deref(), &config)?;
    let queue = worker_queue(db, &config)?;
    result["eligible"] = queue["eligible"].clone();
    result["queue"] = queue;
    result.as_object_mut().unwrap().extend(json!({"ok":true,"workers":workers,"worker_id":selected,"config":config,"version":version,"kind":kind,"upgrading":upgrading,"fleet":fleet,"project":p,"projects":projects}).as_object().unwrap().clone());
    Ok(result)
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
                    let state: String = db.query_row(
                        "SELECT state FROM issues WHERE project_id=?1 AND number=?2",
                        params![p, n],
                        |r| r.get(0),
                    )?;
                    if state != "open" {
                        return Err(Error::conflict(
                            "Reopen the issue before retrying its agent",
                        ));
                    }
                    db.execute("UPDATE worker_runs SET retry_allowed=1 WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NOT NULL",params![p,n])?;
                }
                _ => return Err(Error::invalid("Unknown worker action")),
            }
            status(db, Some(worker_id), p)
        }
        Operation::PreviewWorker {
            config,
            number,
            worktree_allowed,
            task_kind,
        } => {
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
            let mut issue = if candidate.is_some() {
                json!(get_issue(db, &project.id, n, false)?)
            } else {
                json!({"number":"<number>","title":"<issue title>","body":"<issue body>"})
            };
            if let Some(kind) = task_kind {
                if !["implement", "plan"].contains(&kind.as_str()) {
                    return Err(Error::invalid("Task preview must be Implement or Plan"));
                }
                // Settings previews select a task without altering saved issue metadata.
                issue["labels"] = if kind == "plan" {
                    json!(["task:plan"])
                } else {
                    json!([])
                };
            }
            let mut runtime = runtime(db, config, &project)?;
            // Settings previews may inspect an unsaved permission without changing
            // the project or affecting worker registration and pickup.
            if let Some(allowed) = worktree_allowed {
                runtime.worktree_enabled = *allowed && config.worktree_enabled == Some(true);
            }
            let prs_enabled = runtime.prs_enabled && worker::artifact_task(&issue).is_none();
            let worktree_enabled =
                runtime.worktree_enabled && worker::artifact_task(&issue).is_none();
            let (prompt, goal, objective) = worker::preview(&runtime, &project, issue);
            Ok(
                json!({"ok":true,"prompt":prompt,"use_goal":goal,"objective":objective,"number":candidate.map(|(_,n)|n),"project":project,"template":runtime.prompt,"prs_enabled":prs_enabled,"worktree_enabled":worktree_enabled}),
            )
        }
        Operation::ProjectSettings => project_settings(db, p),
        Operation::ConfigureProject {
            prompt,
            chief_enabled,
            chief_prompt,
            boss_name,
            prs_enabled,
            worktree_enabled,
            prompt_overrides,
            drafts_enabled,
            plan_template,
            if_version,
        } => {
            let defaults = project_settings(db, p)?;
            let prompt = prompt
                .clone()
                .unwrap_or_else(|| defaults["prompt"].as_str().unwrap().into());
            let prompt = worker::base_prompt(&prompt);
            let worktree_enabled = worktree_enabled.unwrap_or(defaults["worktree_enabled"] == true);
            let prompt_overrides = prompt_overrides.clone().unwrap_or(serde_json::from_value(
                defaults["prompt_overrides"].clone(),
            )?);
            prompt_overrides.validate()?;
            let legacy_name = boss_name
                .clone()
                .unwrap_or_else(|| defaults["boss_name"].as_str().unwrap().into());
            super::identifier(&legacy_name, "Boss name", 64)?;
            let legacy_name = legacy_name.trim();
            let drafts_enabled = drafts_enabled.unwrap_or(defaults["drafts_enabled"] == true);
            let plan_template = plan_template
                .as_deref()
                .unwrap_or(defaults["plan_template"].as_str().unwrap());
            crate::issues::planning::validate_template(plan_template)?;
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
            db.execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,boss_name,drafts_enabled,plan_template,worktree_enabled,prompt_overrides) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(project_id) DO UPDATE SET prompt=excluded.prompt,prs_enabled=excluded.prs_enabled,version=excluded.version,boss_name=excluded.boss_name,drafts_enabled=excluded.drafts_enabled,plan_template=excluded.plan_template,worktree_enabled=excluded.worktree_enabled,prompt_overrides=excluded.prompt_overrides",params![p.id,prompt,prs_enabled,v+1,legacy_name,drafts_enabled,plan_template,worktree_enabled,serde_json::to_string(&prompt_overrides)?])?;
            let chief_prompt = chief_prompt
                .as_deref()
                .unwrap_or(defaults["chief_prompt"].as_str().unwrap());
            if chief_prompt.trim().is_empty()
                || chief_prompt.len() > 32000
                || chief_prompt.trim_start().starts_with("/goal")
            {
                return Err(Error::invalid(
                    "Chief prompt must contain 1–32000 bytes and cannot start with /goal",
                ));
            }
            db.execute(
                "UPDATE project_settings SET chief_enabled=?2,chief_prompt=?3 WHERE project_id=?1",
                params![
                    p.id,
                    chief_enabled.unwrap_or(defaults["chief_enabled"] == true),
                    chief_prompt
                ],
            )?;
            project_settings(db, p)
        }
        Operation::PullRequests { number }
        | Operation::AddPullRequest { number, .. }
        | Operation::ClassifyPullRequest { number, .. }
        | Operation::RemovePullRequest { number, .. } => {
            get_issue(db, &p.id, *number, true)?;
            let mut changed = 0;
            if let Operation::AddPullRequest { url, .. }
            | Operation::ClassifyPullRequest { url, .. }
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
                let (action, data) = match op {
                    Operation::AddPullRequest { purpose, .. } => {
                        changed = db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT DO NOTHING",params![p.id,number,url,actor.id,now(),purpose.as_str()])?;
                        ("pr_attached", json!({"url":url,"purpose":purpose}))
                    }
                    Operation::ClassifyPullRequest { purpose, .. } => {
                        let previous: String = db.query_row("SELECT purpose FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2 AND url=?3",params![p.id,number,url],|r|r.get(0)).optional()?.ok_or_else(|| Error::new("not_found", "PR is not attached to this issue"))?;
                        changed = db.execute("UPDATE issue_pull_requests SET purpose=?4 WHERE project_id=?1 AND issue_number=?2 AND url=?3 AND purpose<>?4",params![p.id,number,url,purpose.as_str()])?;
                        (
                            "pr_classified",
                            json!({"url":url,"purpose":purpose,"previous_purpose":previous}),
                        )
                    }
                    _ => {
                        changed = db.execute("DELETE FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2 AND url=?3",params![p.id,number,url])?;
                        ("pr_removed", json!({"url":url}))
                    }
                };
                if changed > 0 {
                    db.execute("UPDATE issues SET version=version+1,updated_at=?3 WHERE project_id=?1 AND number=?2",params![p.id,number,now()])?;
                    event(db, &p.id, *number, &actor.id, action, now(), &data)?;
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
    let mut stmt=db.prepare("SELECT url,added_by,created_at,purpose FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2 ORDER BY created_at,url")?;
    Ok(stmt.query_map(params![p,n],|r|Ok(json!({"url":r.get::<_,String>(0)?,"added_by":r.get::<_,String>(1)?,"created_at":r.get::<_,i64>(2)?,"purpose":r.get::<_,String>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?)
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
fn ready_workers(db: &Connection, worker_id: Option<&str>) -> Result<Vec<(String, String)>> {
    let mut stmt=db.prepare("SELECT id,config FROM issue_workers w WHERE (?1 IS NOT NULL AND id=?1 OR ?1 IS NULL AND kind='managed') AND json_extract(config,'$.enabled')=1 AND stop_requested=0 AND NOT EXISTS(SELECT 1 FROM issue_worker_runtime runtime WHERE runtime.worker_id=w.id AND runtime.owner_pid=w.owner_pid AND runtime.owner_start=w.owner_start) AND (SELECT count(*) FROM worker_runs r WHERE r.worker_id=w.id AND r.finished_at IS NULL)<json_extract(config,'$.concurrency') ORDER BY updated_at,id")?;
    Ok(stmt
        .query_map([worker_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

struct PreparedProject {
    project: Project,
    config: ProjectConfig,
    defaults: Value,
    valid: bool,
}

pub(super) fn reserve(
    store: &mut Store,
    machine: &str,
    worker_id: Option<&str>,
) -> Result<Option<Job>> {
    // Legacy updates can arrive after open; migrate only when one is present.
    if store.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_workers WHERE json_type(config,'$.upgrading') IS NOT NULL)", [], |r| r.get::<_, bool>(0))? {
        let tx = store.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        migrate_runtime(&tx)?;
        tx.commit()?;
    }
    // Empty/full queues never request a write lock. Discover checkouts in a WAL
    // snapshot, then run Git/filesystem/process validation before reserving.
    let tx = store
        .db
        .transaction_with_behavior(TransactionBehavior::Deferred)?;
    let mut prepared = HashMap::new();
    for (id, text) in ready_workers(&tx, worker_id)? {
        let settings: Settings = serde_json::from_str(&text)?;
        let mut projects = HashMap::new();
        for (project, _) in candidates(&tx, &settings, 100)? {
            if !projects.contains_key(&project.id) {
                let config = runtime(&tx, &settings, &project)?;
                let defaults = project_settings(&tx, &project)?;
                projects.insert(
                    project.id.clone(),
                    PreparedProject {
                        project,
                        config,
                        defaults,
                        valid: false,
                    },
                );
            }
        }
        prepared.insert(id, (text, projects));
    }
    tx.commit()?;
    let mut any_valid = false;
    for (_, projects) in prepared.values_mut() {
        for project in projects.values_mut() {
            project.valid = !project.config.cwd.is_empty()
                && worker::validate_config(&project.config, &project.project).is_ok();
            any_valid |= project.valid;
        }
    }
    if !any_valid {
        return Ok(None);
    }
    let id = random_id()?;
    let owner_pid = std::process::id();
    let owner_start = crate::agents::process_identity(owner_pid)
        .ok_or_else(|| Error::new("worker_error", "Cannot identify worker process"))?;
    let host = crate::issues::identity::host();
    let tx = store
        .db
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    migrate_runtime(&tx)?;
    // Recheck capacity, controls, filters and queue order atomically. A new
    // project/configuration needs a fresh preflight on the next scheduler poll.
    for (worker_id, text) in ready_workers(&tx, worker_id)? {
        let Some((old_text, projects)) = prepared.get(&worker_id) else {
            return Ok(None);
        };
        if old_text != &text {
            return Ok(None);
        }
        let settings: Settings = serde_json::from_str(&text)?;
        for (project, number) in candidates(&tx, &settings, 100)? {
            let Some(prepared) = projects.get(&project.id) else {
                return Ok(None);
            };
            if prepared.defaults != project_settings(&tx, &project)? {
                return Ok(None);
            }
            if !prepared.valid {
                continue;
            }
            let config = prepared.config.clone();
            let actor = Actor {
                id: format!("reservation:{id}"),
                kind: "worker".into(),
                session_id: None,
                machine: machine.into(),
                host: host.clone(),
                pid: None,
                process_start: None,
                cwd: config.cwd.clone().into(),
                source: "unclaimed worker reservation".into(),
                invocation: None,
                creation_run: None,
            };
            let issue = json!(get_issue(&tx, &project.id, number, false)?);
            // Thread rollouts and unfinished checkout edits belong to this host
            // and directory. A completed latest attempt starts fresh when reopened.
            // A rejected startup schema cannot be recovered by resuming the same
            // session again. Its server has stopped before worker_finish; retain
            // its history and checkout, but let the next attempt start fresh.
            let resume_session: Option<String> = tx.query_row(
                "SELECT coalesce(session_id,json_extract(job,'$.resume_session')) FROM worker_runs
                 WHERE id=(SELECT id FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND machine=?3 AND finished_at IS NOT NULL
                  ORDER BY finished_at DESC,started_at DESC,id DESC LIMIT 1)
                 AND state!='completed' AND json_extract(job,'$.config.cwd')=?4
                 AND NOT (state='failed' AND summary LIKE 'Codex turn/start:%ActiveTurnOutputSchemaMismatch%')",
                params![project.id, number, machine, config.cwd], |r| r.get::<_, Option<String>>(0),
            ).optional()?.flatten();
            let job = Job {
                id: id.clone(),
                worker_id: worker_id.clone(),
                resume_session,
                project: project.clone(),
                issue,
                comments: vec![],
                config,
                actor,
                owner_pid,
                owner_start: owner_start.clone(),
                machine: machine.into(),
            };
            tx.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,reservation_expires,session_id) VALUES(?1,?2,?3,?4,?5,'reserved',?6,?7,?8,?9,?9,?10,?11,?12)",params![id,project.id,number,serde_json::to_string(&job)?,job.actor.id,owner_pid,owner_start,machine,now(),worker_id,now()+settings.reservation_seconds as i64*1000,job.resume_session])?;
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
        // Optional metadata identifies the binary running in the owner process.
        // Legacy owner registration must clear even an unchanged PID/start pair.
        tx.execute_batch("CREATE TABLE IF NOT EXISTS issue_worker_builds(worker_id TEXT PRIMARY KEY REFERENCES issue_workers(id) ON DELETE CASCADE,owner_pid INTEGER NOT NULL,owner_start TEXT NOT NULL,build TEXT NOT NULL);
            CREATE TRIGGER IF NOT EXISTS issue_worker_build_owner_changed AFTER UPDATE OF owner_pid,owner_start ON issue_workers BEGIN DELETE FROM issue_worker_builds WHERE worker_id=NEW.id; END;")?;
        tx.execute(
            "INSERT OR REPLACE INTO issue_worker_builds VALUES(?1,?2,?3,?4)",
            params![id, pid, start, env!("HEY_BOSS_BUILD_ID")],
        )?;
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
    pub(crate) fn worker_set_upgrading(&self, id: &str, draining: bool) -> Result<()> {
        if draining {
            self.db.execute("INSERT OR IGNORE INTO issue_worker_runtime(worker_id,owner_pid,owner_start) SELECT id,owner_pid,owner_start FROM issue_workers WHERE id=?1 AND owner_pid=?2", params![id,std::process::id()])?;
        } else {
            self.db.execute(
                "DELETE FROM issue_worker_runtime WHERE worker_id=?1 AND owner_pid=?2",
                params![id, std::process::id()],
            )?;
        }
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
    fn multi_checkout_runtime_and_chief_use_the_project_mapping() {
        let root = std::env::temp_dir().join(format!("hb-multi-checkout-{}", random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let store = Store::open(&root.join("issues.db")).unwrap();
            store.db.execute_batch("CREATE TABLE issue_worker_runtime(worker_id TEXT PRIMARY KEY,owner_pid INTEGER,owner_start TEXT)").unwrap();
            let mut directories = serde_json::Map::new();
            for name in ["Atlas", "Beacon"] {
                let path = root.join(name);
                std::fs::create_dir(&path).unwrap();
                let id = format!("named:{name}");
                directories.insert(id.clone(), json!(path));
                store.db.execute("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES(?1,?2,1,0,0)", params![id, name]).unwrap();
                store.db.execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,chief_enabled) VALUES(?1,'Work',0,1,1)", [&id]).unwrap();
            }
            let settings: Settings = serde_json::from_value(json!({
                "projects": ["named:Atlas", "named:Beacon"], "directories": directories, "enabled": true
            })).unwrap();
            for name in ["Atlas", "Beacon"] {
                let p = Project {
                    id: format!("named:{name}"),
                    name: name.into(),
                };
                assert_eq!(
                    runtime(&store.db, &settings, &p).unwrap().cwd,
                    root.join(name).to_string_lossy()
                );
            }
            store.db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES('multi','cli',?1,1,0)", [serde_json::to_string(&settings).unwrap()]).unwrap();
            let chiefs = store.chief_candidates(Some("multi")).unwrap();
            assert_eq!(chiefs.len(), 2);
            for (id, cwd, _, _) in chiefs {
                assert_eq!(cwd, directories[&id].as_str().unwrap());
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn directory_discovery_scales_with_project_associations() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE agents(id TEXT PRIMARY KEY,metadata TEXT,last_seen INTEGER);
            CREATE TABLE issues(project_id TEXT,created_by TEXT,assignee TEXT);
            CREATE INDEX project_issues ON issues(project_id);
            WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000)
            INSERT INTO agents SELECT 'unrelated-'||x,'{}',x FROM n;
            INSERT INTO agents VALUES('creator','{\"cwd\":\"/creator\"}',1),('assignee','{\"cwd\":\"/assignee\"}',2),('missing-cwd','{}',3);
            WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100)
            INSERT INTO issues SELECT 'project','creator',CASE WHEN x=1 THEN 'assignee' WHEN x=2 THEN 'missing-cwd' ELSE NULL END FROM n;
            INSERT INTO issues VALUES('other','unrelated-1000',NULL);").unwrap();
        let mut stmt = db.prepare(PROJECT_DIRECTORIES).unwrap();
        let paths = stmt
            .query_map(params!["project", 50], |r| r.get::<_, Option<String>>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            paths,
            vec![None, Some("/assignee".into()), Some("/creator".into())]
        );
        // Count VM work rather than wall time: unrelated agents must not each
        // rescan the project's issues, regardless of machine speed.
        let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
        assert!(steps < 10_000, "directory discovery used {steps} VM steps");
    }

    #[test]
    fn idle_and_invalid_checkout_polls_do_not_wait_for_a_writer() {
        let root = std::env::temp_dir().join(format!("hb-idle-worker-{}", random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let path = root.join("issues.db");
            let mut store = Store::open(&path).unwrap();
            store
                .register_worker(
                    None,
                    &Settings {
                        enabled: false,
                        ..Settings::default()
                    },
                    "unit",
                )
                .unwrap();
            store
                .db
                .busy_timeout(std::time::Duration::from_millis(25))
                .unwrap();
            let mut other = rusqlite::Connection::open(&path).unwrap();
            let tx = other
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(reserve(&mut store, "unit", None).unwrap().is_none());
            tx.rollback().unwrap();

            store.db.execute("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES('named:Idle','Idle',2,0,0)", []).unwrap();
            store
                .db
                .execute("INSERT INTO agents VALUES('agent','{}',0)", [])
                .unwrap();
            store.db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('named:Idle',1,'Task','','open','agent',0,0,1,'[]')", []).unwrap();
            let settings = Settings {
                directory: root.to_string_lossy().into(),
                projects: vec!["named:Idle".into()],
                ..Settings::default()
            };
            let id = store.register_worker(None, &settings, "unit").unwrap();
            store
                .db
                .execute(
                    "UPDATE issue_workers SET config=json_set(config,'$.directory',?2) WHERE id=?1",
                    params![id, root.join("missing-checkout").to_string_lossy()],
                )
                .unwrap();
            let tx = other
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(reserve(&mut store, "unit", Some(&id)).unwrap().is_none());
            tx.rollback().unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

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
            assert_eq!(status["runs"][0]["actor_id"], "agent");
            store
                .db
                .execute("DELETE FROM issue_worker_runtime WHERE worker_id=?1", [&id])
                .unwrap();
            store.db.execute("UPDATE issue_workers SET config=json_set(config,'$.enabled',json('false'),'$.upgrading',json('true')) WHERE id=?1", [&id]).unwrap();
            let fleet = store.fleet_workers().unwrap();
            assert_eq!(fleet[0]["upgrading"], true);
            assert_eq!(fleet[0]["config"]["enabled"], true);
            assert_eq!(fleet[0]["runs"][0]["session_id"], "saved-session");
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
    fn worker_build_provenance_belongs_to_the_registered_process_owner() {
        let root = std::env::temp_dir().join(format!("hb-worker-build-{}", random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let mut store = Store::open(&root.join("issues.db")).unwrap();
            let id = store
                .register_worker(None, &Settings::default(), "unit")
                .unwrap();
            let workers = worker_overview(&store.db).unwrap();
            assert_eq!(workers[0]["build"], env!("HEY_BOSS_BUILD_ID"));
            store.db.execute("UPDATE issue_workers SET owner_pid=owner_pid,owner_start=owner_start WHERE id=?1", [&id]).unwrap();
            assert!(worker_overview(&store.db).unwrap()[0]["build"].is_null());
            // An older CLI can register a new owner without writing provenance.
            // Its status must not inherit a previous owner's recorded build.
            store
                .db
                .execute(
                    "UPDATE issue_workers SET owner_start='legacy-owner' WHERE id=?1",
                    [&id],
                )
                .unwrap();
            store
                .register_worker(Some(&id), &Settings::default(), "unit")
                .unwrap();
            assert_eq!(
                worker_overview(&store.db).unwrap()[0]["build"],
                env!("HEY_BOSS_BUILD_ID")
            );
            store.db.execute("UPDATE issue_worker_builds SET owner_start='previous-owner' WHERE worker_id=?1", [&id]).unwrap();
            assert!(worker_overview(&store.db).unwrap()[0]["build"].is_null());
            // Even internally matching saved records cannot identify a process
            // whose PID has been reused with a different actual start time.
            store
                .db
                .execute(
                    "UPDATE issue_workers SET owner_start='previous-owner' WHERE id=?1",
                    [&id],
                )
                .unwrap();
            store
                .db
                .execute(
                    "INSERT INTO issue_worker_builds VALUES(?1,?2,'previous-owner',?3)",
                    params![id, std::process::id(), env!("HEY_BOSS_BUILD_ID")],
                )
                .unwrap();
            assert!(worker_overview(&store.db).unwrap()[0]["build"].is_null());
            store.unregister_worker(&id).unwrap();
            assert!(worker_overview(&store.db).unwrap()[0]["build"].is_null());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fleet_poll_work_is_linear_in_the_number_of_workers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        unsafe extern "C" fn count_steps(context: *mut std::ffi::c_void) -> std::ffi::c_int {
            unsafe { &*context.cast::<AtomicUsize>() }.fetch_add(100, Ordering::Relaxed);
            0
        }
        let root = std::env::temp_dir().join(format!("hb-fleet-poll-{}", random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let store = Store::open(&root.join("issues.db")).unwrap();
            let config = serde_json::to_string(&Settings {
                concurrency: 2,
                ..Settings::default()
            })
            .unwrap();
            for n in 1..=100 {
                store.db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES(?1,'managed',?2,1,?3)", params![format!("worker-{n:03}"),config,n]).unwrap();
            }
            store.db.execute_batch("CREATE TABLE issue_worker_runtime(worker_id TEXT PRIMARY KEY REFERENCES issue_workers(id),owner_pid INTEGER NOT NULL,owner_start TEXT NOT NULL);
                UPDATE issue_workers SET owner_pid=123,owner_start='same-owner' WHERE id='worker-050';
                INSERT INTO issue_worker_runtime VALUES('worker-050',123,'same-owner');").unwrap();
            let steps = AtomicUsize::new(0);
            unsafe {
                rusqlite::ffi::sqlite3_progress_handler(
                    store.db.handle(),
                    100,
                    Some(count_steps),
                    (&steps as *const AtomicUsize).cast_mut().cast(),
                );
            }
            let started = std::time::Instant::now();
            let result = store.fleet_workers();
            unsafe {
                rusqlite::ffi::sqlite3_progress_handler(
                    store.db.handle(),
                    0,
                    None,
                    std::ptr::null_mut(),
                );
            }
            let elapsed = started.elapsed();
            let workers = result.unwrap();
            assert_eq!(workers.len(), 100);
            assert_eq!(workers[0]["id"], "worker-100");
            assert_eq!(workers[99]["id"], "worker-001");
            for worker in &workers {
                assert_eq!(worker["active"], 0);
                assert_eq!(worker["free"], 2);
                assert_eq!(worker["eligible"], 0);
                assert!(worker["runs"].as_array().unwrap().is_empty());
                assert!(worker["chiefs"].as_array().unwrap().is_empty());
                assert_eq!(worker["upgrading"], worker["id"] == "worker-050");
            }
            let steps = steps.load(Ordering::Relaxed);
            eprintln!(
                "100-worker fleet poll: fewer than {} VM steps in {elapsed:?}",
                steps + 100
            );
            assert!(
                steps < 100_000,
                "Fleet poll repeatedly scanned unrelated workers: {steps} VM steps"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fleet_poll_reuses_equivalent_queue_filters_within_one_snapshot() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        unsafe extern "C" fn count_steps(context: *mut std::ffi::c_void) -> std::ffi::c_int {
            unsafe { &*context.cast::<AtomicUsize>() }.fetch_add(100, Ordering::Relaxed);
            0
        }
        let root = std::env::temp_dir().join(format!("hb-fleet-queue-{}", random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let store = Store::open(&root.join("issues.db")).unwrap();
            store.db.execute_batch("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES('named:A','A',1001,0,0),('named:B','B',1001,0,0);
                INSERT INTO agents VALUES('agent','{}',0);
                WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000)
                INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels)
                SELECT p.id,x,'Task','','open','agent',0,0,1,CASE WHEN x%2=0 THEN '[\"ready\",\"urgent\"]' ELSE '[\"ready\"]' END FROM n CROSS JOIN projects p WHERE p.id IN ('named:A','named:B');").unwrap();
            for n in 0..30 {
                let (mut projects, mut tags): (Vec<String>, Vec<String>) = match n % 3 {
                    0 => (vec!["named:A".into()], vec!["ready".into()]),
                    1 => (
                        vec!["named:A".into(), "named:B".into()],
                        vec!["ready".into(), "urgent".into()],
                    ),
                    _ => (vec!["named:B".into()], vec!["urgent".into()]),
                };
                if n % 2 == 0 && !tags.is_empty() {
                    projects.reverse();
                    tags.reverse();
                    tags.push(tags[0].clone());
                }
                let config = Settings {
                    projects,
                    tags,
                    concurrency: n + 1,
                    ..Settings::default()
                };
                store.db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES(?1,'managed',?2,1,?3)", params![format!("worker-{n:02}"), serde_json::to_string(&config).unwrap(), n]).unwrap();
            }
            let steps = AtomicUsize::new(0);
            unsafe {
                rusqlite::ffi::sqlite3_progress_handler(
                    store.db.handle(),
                    100,
                    Some(count_steps),
                    (&steps as *const AtomicUsize).cast_mut().cast(),
                );
            }
            let result = store.fleet_workers();
            unsafe {
                rusqlite::ffi::sqlite3_progress_handler(
                    store.db.handle(),
                    0,
                    None,
                    std::ptr::null_mut(),
                );
            }
            let workers = result.unwrap();
            assert_eq!(workers.len(), 30);
            for w in &workers {
                let n: u32 = w["id"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("worker-")
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(w["eligible"], if n % 3 == 2 { 500 } else { 1000 });
                assert_eq!(w["free"], w["config"]["concurrency"]);
                assert!(w.get("queue").is_none());
                let id = w["id"].as_str().unwrap();
                let public = status(
                    &store.db,
                    Some(id),
                    &Project {
                        id: "named:A".into(),
                        name: "A".into(),
                    },
                )
                .unwrap();
                for key in ["active", "free", "eligible", "runs", "chiefs"] {
                    assert_eq!(w[key], public[key], "{id} {key}");
                }
            }
            let steps = steps.load(Ordering::Relaxed);
            eprintln!(
                "30-worker, 2000-issue fleet poll: fewer than {} VM steps",
                steps + 100
            );
            assert!(
                steps < 1_000_000,
                "Repeated queue scans used {steps} VM steps"
            );
            store.db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('named:A',1001,'New task','','open','agent',0,0,1,'[\"ready\",\"urgent\"]')", []).unwrap();
            for w in store.fleet_workers().unwrap() {
                let n: u32 = w["id"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("worker-")
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(
                    w["eligible"],
                    if n % 3 == 2 { 500 } else { 1001 },
                    "next poll must refresh counts"
                );
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fleet_poll_capacity_and_activity_share_one_wal_snapshot() {
        struct ConcurrentStart {
            writer: Connection,
            started: std::cell::Cell<bool>,
        }
        unsafe extern "C" fn start_run(
            kind: u32,
            context: *mut std::ffi::c_void,
            statement: *mut std::ffi::c_void,
            _: *mut std::ffi::c_void,
        ) -> std::ffi::c_int {
            if kind != rusqlite::ffi::SQLITE_TRACE_STMT {
                return 0;
            }
            let state = unsafe { &*context.cast::<ConcurrentStart>() };
            let sql =
                unsafe { std::ffi::CStr::from_ptr(rusqlite::ffi::sqlite3_sql(statement.cast())) }
                    .to_string_lossy();
            if sql.starts_with("SELECT count(*) FROM worker_runs WHERE worker_id")
                && !state.started.replace(true)
            {
                // Commit from a different WAL connection between overview and details.
                let result = state.writer.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id) VALUES('concurrent','named:Snapshot QA',1,'{\"issue\":{\"title\":\"Task\"}}','agent','running',1,'start','unit',1,1,'worker-b')", []);
                if result.is_err() {
                    state.started.set(false);
                }
            }
            0
        }
        let root = std::env::temp_dir().join(format!("hb-fleet-snapshot-{}", random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        {
            let store = Store::open(&root.join("issues.db")).unwrap();
            store.db.execute_batch("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES('named:Snapshot QA','Snapshot QA',2,0,0);
                INSERT INTO agents VALUES('agent','{}',0);
                INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('named:Snapshot QA',1,'Task','','open','agent',0,0,1,'[]');").unwrap();
            let config = serde_json::to_string(&Settings::default()).unwrap();
            for id in ["worker-a", "worker-b"] {
                store.db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES(?1,'managed',?2,1,0)", params![id,config]).unwrap();
            }
            let mut concurrent = ConcurrentStart {
                writer: Connection::open(root.join("issues.db")).unwrap(),
                started: std::cell::Cell::new(false),
            };
            concurrent
                .writer
                .pragma_update(None, "foreign_keys", true)
                .unwrap();
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(
                    store.db.handle(),
                    rusqlite::ffi::SQLITE_TRACE_STMT,
                    Some(start_run),
                    (&mut concurrent as *mut ConcurrentStart).cast(),
                );
            }
            let result = store.fleet_workers();
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(store.db.handle(), 0, None, std::ptr::null_mut());
            }
            assert!(
                concurrent.started.get(),
                "The concurrent writer did not commit during polling"
            );
            let workers = result.unwrap();
            assert!(
                workers
                    .iter()
                    .all(|w| w["active"] == 0 && w["runs"].as_array().unwrap().is_empty()),
                "Polling combined capacity and activity from different database snapshots"
            );
            let next = store.fleet_workers().unwrap();
            let updated = next.iter().find(|w| w["id"] == "worker-b").unwrap();
            assert_eq!(updated["active"], 1);
            assert_eq!(updated["runs"][0]["id"], "concurrent");
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
            db.execute("UPDATE worker_runs SET started_at=48 WHERE id='run-49'", [])
                .unwrap();
            for n in 1..=20 {
                db.execute(
                    "INSERT INTO worker_events(run_id,created_at,text) VALUES('run-24',?1,?2)",
                    params![n, format!("Activity {n}")],
                )
                .unwrap();
            }
            let s = status(db, Some("worker"), &p).unwrap();
            let events = s["runs"][0]["events"].as_array().unwrap();
            assert_eq!(
                events.len(),
                12,
                "Status must retain a bounded, useful activity log"
            );
            assert_eq!(events[0]["text"], "Activity 20");
            assert_eq!(events[11]["text"], "Activity 9");
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
            let expected: Vec<String> = (1..=24)
                .rev()
                .chain((30..=49).rev())
                .map(|n| format!("run-{n}"))
                .collect();
            let ids: Vec<&str> = s["runs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap())
                .collect();
            assert_eq!(ids, expected);
            let fleet = store.fleet_workers().unwrap();
            assert_eq!(fleet.len(), 1);
            let mut expected_worker = s["workers"][0].clone();
            for key in ["active", "free", "eligible", "runs", "chiefs"] {
                expected_worker[key] = s[key].clone();
            }
            assert_eq!(fleet[0], expected_worker);
            // Large old history must not change the result or make status
            // scan every finished attempt. Use nonmonotonic finish times.
            db.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000)
                INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,finished_at)
                SELECT 'archived-'||x,?1,49,'{}','agent','completed',1,'start','machine',-x,0,'worker',(x*7919)%10000 FROM n", [&p.id]).unwrap();
            let mut stmt = db.prepare(STATUS_RUNS).unwrap();
            let ids = stmt
                .query_map(["worker"], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(ids, expected);
            let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
            assert!(steps < 5_000, "status history used {steps} VM steps");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
