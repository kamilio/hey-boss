//! Assignment is a destination; the assignee is the session currently doing work.
use super::*;

const WATCHER: &str = "watcher:github";
#[path = "assignment_lifecycle.rs"]
mod lifecycle;

pub(super) fn links_changed(
    db: &Connection,
    project: &str,
    number: i64,
    actor: &Actor,
) -> Result<()> {
    lifecycle::reconcile_issue(db, project, number, actor)
}

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row("SELECT count(*)=4 FROM sqlite_master WHERE name IN ('issue_github_watches','issue_github_signals','issue_assignment_summary','issue_github_destinations')",[],|r|r.get::<_,bool>(0))? { return Ok(()); }
    db.execute_batch("CREATE TABLE IF NOT EXISTS issue_github_watches(project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,status TEXT NOT NULL CHECK(json_valid(status)),PRIMARY KEY(project_id,issue_number),FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number)); CREATE INDEX IF NOT EXISTS issue_assignment_summary ON issues(project_id,number,assignment_target); CREATE INDEX IF NOT EXISTS issue_github_destinations ON issues(project_id,number) WHERE assignment_target='github';")?;
    // The supervisor retains exact event identities locally. Peers need only
    // current evidence and its event generation, not an ever-growing history.
    db.execute_batch("CREATE TABLE IF NOT EXISTS issue_github_signals(project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,url TEXT NOT NULL,head TEXT NOT NULL,signal TEXT NOT NULL,PRIMARY KEY(project_id,issue_number,url,head,signal),FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number)) WITHOUT ROWID;")?;
    Ok(())
}

fn saved(db: &Connection, project: &str, number: i64) -> Result<(Option<String>, Value)> {
    let target = db.query_row(
        "SELECT assignment_target FROM issues WHERE project_id=?1 AND number=?2",
        params![project, number],
        |r| r.get(0),
    )?;
    let status: Option<String> = db
        .query_row(
            "SELECT status FROM issue_github_watches WHERE project_id=?1 AND issue_number=?2",
            params![project, number],
            |r| r.get(0),
        )
        .optional()?;
    Ok((
        target,
        normalize_status(
            status
                .map(|s| serde_json::from_str(&s))
                .transpose()?
                .unwrap_or(json!({"prs":{}})),
        ),
    ))
}

pub(super) fn assign(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    issue: &mut Issue,
    target: &str,
    version: i64,
    now: i64,
) -> Result<Value> {
    if issue.version != version {
        return Err(Error::conflict("Issue changed; refresh before assigning"));
    }
    if issue.draft
        || issue.deleted_at.is_some()
        || !matches!(issue.state.as_str(), "open" | "ready")
    {
        return Err(Error::conflict(
            "Assignment requires an open, non-draft issue",
        ));
    }
    if issue.assignee.as_deref() != Some(WATCHER) {
        ownership(issue, actor, actor.id == "human:boss")?;
    }
    let other_run:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL AND actor_id<>?3)",params![project.id,issue.number,actor.id],|r|r.get(0))?;
    let retain_claim = target == "github" && live_claim(db, issue.assignee.as_deref())?;
    if other_run && !retain_claim {
        return Err(Error::conflict(
            "An agent is still running; stop it before changing its assignment",
        ));
    }
    let machine = target.strip_prefix("machine:");
    if let Some(machine) = machine {
        identifier(machine, "machine ID", 256)?;
    } else if !["github", "boss", "unassigned"].contains(&target) {
        return Err(Error::invalid(
            "Assignment must be github, boss, unassigned, or machine:MACHINE_ID",
        ));
    }
    if target == "github" {
        let links = registry::pull_requests(db, &project.id, issue.number)?;
        if !links.iter().any(|pr| {
            pr["status"] != "merged"
                && pr["status"] != "closed"
                && pr["url"]
                    .as_str()
                    .and_then(hey_gh::watcher::pull_request_selector)
                    .is_some()
        }) {
            return Err(Error::conflict(
                "Attach an open GitHub pull request before assigning its watcher",
            ));
        }
        let mut watcher = actor.clone();
        watcher.id = WATCHER.into();
        watcher.kind = "system".into();
        watcher.session_id = None;
        db.execute(
            "INSERT OR IGNORE INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3)",
            params![WATCHER, serde_json::to_string(&watcher)?, now],
        )?;
    }
    if target == "boss" {
        ready::register_boss(db, actor, now)?;
    }
    let previous = issue.assignee.clone();
    issue.assignee = match target {
        "github" if retain_claim => previous.clone(),
        "github" => Some(WATCHER.into()),
        "boss" => Some("human:boss".into()),
        _ => None,
    };
    if target != "github" {
        issue.state = "open".into();
    }
    db.execute(
        "UPDATE issues SET assignment_target=?3 WHERE project_id=?1 AND number=?2",
        params![
            project.id,
            issue.number,
            if target == "github" || machine.is_some() {
                Some(target)
            } else {
                None
            }
        ],
    )?;
    let (_, mut status) = saved(db, &project.id, issue.number)?;
    if !retain_claim {
        db.execute(
            "UPDATE issues SET github_ack_event=?3 WHERE project_id=?1 AND number=?2",
            params![project.id, issue.number, status["event"].as_str()],
        )?;
        db.execute(
            "DELETE FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2",
            params![project.id, issue.number],
        )?;
    }
    if target == "github"
        && status
            .as_object_mut()
            .unwrap()
            .remove("stopped_reason")
            .is_some()
    {
        db.execute(
            "UPDATE issue_github_watches SET status=?3 WHERE project_id=?1 AND issue_number=?2",
            params![project.id, issue.number, status.to_string()],
        )?;
    }
    if let Some(machine) = machine {
        db.execute(
            "INSERT INTO fleet_allocations(project_id,issue_number,node) VALUES(?1,?2,?3)",
            params![project.id, issue.number, machine],
        )?;
    }
    Ok(json!({"target":target,"previous_assignee":previous,"assignee":issue.assignee}))
}

fn live_claim(db: &Connection, actor: Option<&str>) -> Result<bool> {
    let Some(actor) = actor.filter(|a| *a != WATCHER && *a != "human:boss") else {
        return Ok(false);
    };
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM worker_runs WHERE actor_id=?1 AND finished_at IS NULL)",
        [actor],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(true);
    }
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='fleet_state' AND type='table')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(false);
    }
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM fleet_state s,json_each(s.value) m,json_each(m.value,'$.workers') w,json_each(w.value,'$.runs') r WHERE s.key='machines' AND json_extract(r.value,'$.actor_id')=?1 AND json_extract(r.value,'$.finished_at') IS NULL)",[actor],|r|r.get(0))?)
}

#[derive(Default)]
struct AssignmentMetadata {
    target: Option<String>,
    reserved: Option<String>,
    actor_machine: Option<String>,
    actor_host: Option<String>,
}

fn metadata(
    db: &Connection,
    project: &str,
    numbers: &[i64],
) -> Result<std::collections::BTreeMap<i64, AssignmentMetadata>> {
    Ok(db.prepare("SELECT i.number,i.assignment_target,a.node,json_extract(g.metadata,'$.machine'),json_extract(g.metadata,'$.host') FROM issues i LEFT JOIN fleet_allocations a ON a.project_id=i.project_id AND a.issue_number=i.number LEFT JOIN agents g ON g.id=i.assignee WHERE i.project_id=?1 AND i.number IN (SELECT value FROM json_each(?2))")?.query_map(params![project,serde_json::to_string(numbers)?],|r|Ok((r.get(0)?,AssignmentMetadata{target:r.get(1)?,reserved:r.get(2)?,actor_machine:r.get(3)?,actor_host:r.get(4)?})))?.collect::<rusqlite::Result<_>>()?)
}

fn machines(db: &Connection, actor: Option<&Actor>) -> Result<Vec<Value>> {
    let mut machines = Vec::new();
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='fleet_state' AND type='table')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        machines=db.prepare("SELECT json_extract(m.value,'$.node'),json_extract(m.value,'$.hostname'),json_extract(m.value,'$.host'),json_extract(m.value,'$.state') FROM fleet_state s,json_each(s.value) m WHERE s.key='machines' AND json_extract(m.value,'$.node') IS NOT NULL")?.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,Option<String>>(1)?,"host":r.get::<_,Option<String>>(2)?,"state":r.get::<_,Option<String>>(3)?})))?.collect::<rusqlite::Result<_>>()?;
    }
    let local: String = db.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    let local = if local.is_empty() {
        actor.map(|a| a.machine.as_str()).unwrap_or("")
    } else {
        &local
    };
    if !local.is_empty() && !machines.iter().any(|m| m["id"] == local) {
        machines.push(json!({"id":local,"name":actor.filter(|a|a.machine==local).map(|a|a.host.clone()).unwrap_or_else(super::super::identity::host),"host":"local","state":"connected"}));
    }
    Ok(machines)
}

fn attach(issue: &mut Value, meta: &AssignmentMetadata, machines: &[Value]) {
    let assignee = issue["assignee"].as_str();
    let active = assignee.filter(|id| *id != WATCHER && *id != "human:boss");
    let machine = if active.is_some() {
        meta.actor_machine.as_deref().or(meta.reserved.as_deref())
    } else {
        meta.reserved.as_deref().or_else(|| {
            meta.target
                .as_deref()
                .and_then(|t| t.strip_prefix("machine:"))
        })
    };
    let name = machine
        .and_then(|id| {
            machines
                .iter()
                .find(|m| m["id"] == id)
                .and_then(|m| m["name"].as_str())
        })
        .or_else(|| active.and(meta.actor_host.as_deref()))
        .or(machine);
    issue["assignment"] = if assignee == Some("human:boss") {
        json!({"kind":"boss","actor":"human:boss"})
    } else if meta.target.as_deref() == Some("github") {
        json!({"kind":"github","actor":active,"waiting":assignee==Some(WATCHER),"machine":machine,"machine_name":name})
    } else if let Some(id) = active {
        json!({"kind":"agent","actor":id,"machine":machine,"machine_name":name})
    } else if machine.is_some() {
        json!({"kind":"machine","machine":machine,"machine_name":name})
    } else {
        json!({"kind":"unassigned"})
    };
}

fn normalize_status(mut status: Value) -> Value {
    if !status.is_object() {
        status = json!({});
    }
    if !status["prs"].is_object() {
        status["prs"] = json!({});
    }
    status["prs"]
        .as_object_mut()
        .unwrap()
        .retain(|_, pr| pr.is_object());
    status
}

fn public_status(status: Value, monitoring: bool) -> Value {
    let mut status = normalize_status(status);
    status["monitoring"] = json!(monitoring);
    for pr in status["prs"]
        .as_object_mut()
        .into_iter()
        .flat_map(|prs| prs.values_mut())
    {
        if let Some(pr) = pr.as_object_mut() {
            pr.remove("seen");
        }
    }
    status
}

pub(super) fn enrich_result(
    db: &Connection,
    project: &str,
    result: &mut Value,
    actor: Option<&Actor>,
) -> Result<()> {
    let mut numbers = Vec::new();
    if let Some(number) = result["issue"]["number"].as_i64() {
        numbers.push(number);
    }
    numbers.extend(
        result["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["number"].as_i64()),
    );
    if numbers.is_empty() {
        return Ok(());
    }
    let metadata = metadata(db, project, &numbers)?;
    let machines = machines(db, actor)?;
    for issue in result["issues"].as_array_mut().into_iter().flatten() {
        if let Some(meta) = issue["number"]
            .as_i64()
            .and_then(|number| metadata.get(&number))
        {
            attach(issue, meta, &machines);
        }
    }
    if let Some(issue) = result.get_mut("issue")
        && let Some(number) = issue["number"].as_i64()
        && let Some(meta) = metadata.get(&number)
    {
        attach(issue, meta, &machines);
        let (_, status) = saved(db, project, number)?;
        if meta.target.as_deref() == Some("github")
            || status.get("event").is_some()
            || status.get("error").is_some()
            || status.get("stopped_reason").is_some()
        {
            let monitoring = meta.target.as_deref() == Some("github")
                && issue["state"] != "closed"
                && issue["deleted_at"].is_null();
            issue["github_status"] = public_status(status, monitoring);
        }
    }
    result["assignment_machines"] = json!(machines);
    Ok(())
}

pub(super) fn enrich(db: &Connection, project: &str, issue: &mut Value) -> Result<()> {
    let mut result = json!({"issue":issue.take()});
    enrich_result(db, project, &mut result, None)?;
    *issue = result["issue"].take();
    Ok(())
}

pub(super) fn clear(db: &Connection, project: &str, number: i64) -> Result<()> {
    db.execute(
        "UPDATE issues SET assignment_target=NULL WHERE project_id=?1 AND number=?2",
        params![project, number],
    )?;
    db.execute(
        "DELETE FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2",
        params![project, number],
    )?;
    Ok(())
}

impl Store {
    pub(crate) fn record_github_error(
        &mut self,
        url: &str,
        source: &str,
        error: &str,
    ) -> Result<()> {
        let error: String = error.chars().take(1000).collect();
        if error_updates(&self.db, url, source, &error)?.is_empty() {
            return Ok(());
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for update in error_updates(&tx, url, source, &error)? {
            tx.execute("INSERT INTO issue_github_watches(project_id,issue_number,status) VALUES(?1,?2,?3) ON CONFLICT(project_id,issue_number) DO UPDATE SET status=excluded.status", params![update.project,update.number,update.status.to_string()])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn github_watch_urls(&self) -> Result<Vec<String>> {
        Ok(self.db.prepare("SELECT DISTINCT pr.url FROM issue_pull_requests pr JOIN issues i ON i.project_id=pr.project_id AND i.number=pr.issue_number JOIN projects p ON p.id=i.project_id WHERE i.assignment_target='github' AND i.state<>'closed' AND i.deleted_at IS NULL AND i.draft=0 AND p.hidden_at IS NULL AND pr.status NOT IN ('merged','closed') ORDER BY pr.url")?.query_map([],|r|r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn record_github_observation(
        &mut self,
        url: &str,
        observation: &hey_gh::watcher::Observation,
        checked_at: i64,
    ) -> Result<()> {
        // Most polls see identical evidence. Never queue those reads behind a
        // worker's write transaction or append their timestamps to the journal.
        if observation_updates(&self.db, url, observation, checked_at)?.is_empty() {
            return Ok(());
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Assignment or link edits during the read must win before publishing.
        let mut reopened = BTreeSet::new();
        let signals = serde_json::to_string(&signal_keys(observation))?;
        for update in observation_updates(&tx, url, observation, checked_at)? {
            tx.execute("DELETE FROM issue_github_signals WHERE project_id=?1 AND issue_number=?2 AND url=?3 AND head<>?4",params![update.project,update.number,url,observation.head])?;
            tx.execute("INSERT OR IGNORE INTO issue_github_signals SELECT ?1,?2,?3,?4,value FROM json_each(?5)",params![update.project,update.number,url,observation.head,signals])?;
            tx.execute("INSERT INTO issue_github_watches(project_id,issue_number,status) VALUES(?1,?2,?3) ON CONFLICT(project_id,issue_number) DO UPDATE SET status=excluded.status",params![update.project,update.number,update.status.to_string()])?;
            if update.wake {
                let was_ready: bool = tx.query_row(
                    "SELECT state='ready' FROM issues WHERE project_id=?1 AND number=?2",
                    params![update.project, update.number],
                    |r| r.get(0),
                )?;
                tx.execute("UPDATE issues SET assignee=CASE WHEN assignee=?3 THEN NULL ELSE assignee END,state=CASE WHEN state='ready' THEN 'open' ELSE state END,version=version+1,updated_at=max(updated_at,?4) WHERE project_id=?1 AND number=?2 AND (assignee=?3 OR state='ready')",params![update.project,update.number,WATCHER,crate::issues::worker::now()])?;
                tx.execute("UPDATE worker_runs SET retry_allowed=1 WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NOT NULL",params![update.project,update.number])?;
                if was_ready {
                    reopened.insert(update.project);
                }
            }
        }
        for project in reopened {
            super::super::blockers::reconcile_rework(
                &tx,
                &project,
                Some(WATCHER),
                crate::issues::worker::now(),
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

struct ObservationUpdate {
    project: String,
    number: i64,
    status: Value,
    wake: bool,
}

fn watch_tasks(db: &Connection, url: &str) -> Result<Vec<(String, i64, Value)>> {
    let tasks=db.prepare("SELECT i.project_id,i.number,w.status FROM issues i JOIN projects p ON p.id=i.project_id JOIN issue_pull_requests pr ON pr.project_id=i.project_id AND pr.issue_number=i.number LEFT JOIN issue_github_watches w ON w.project_id=i.project_id AND w.issue_number=i.number WHERE pr.url=?1 AND pr.status NOT IN ('closed','merged') AND i.assignment_target='github' AND i.state<>'closed' AND i.deleted_at IS NULL AND i.draft=0 AND p.hidden_at IS NULL")?.query_map([url],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    tasks
        .into_iter()
        .map(|(project, number, stored)| {
            let status = stored
                .map(|s| serde_json::from_str(&s))
                .transpose()?
                .unwrap_or(json!({"prs":{}}));
            Ok((project, number, normalize_status(status)))
        })
        .collect()
}

fn source_errors(pr: &Value) -> serde_json::Map<String, Value> {
    if let Some(errors) = pr["errors"].as_object() {
        return errors.clone();
    }
    pr["error"]
        .as_str()
        .map(|error| serde_json::Map::from_iter([("github".into(), json!(error))]))
        .unwrap_or_default()
}

fn attach_errors(pr: &mut Value, errors: serde_json::Map<String, Value>) {
    if errors.is_empty() {
        pr.as_object_mut().unwrap().remove("errors");
        pr.as_object_mut().unwrap().remove("error");
    } else {
        pr["error"] = json!(
            errors
                .values()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        );
        pr["errors"] = json!(errors);
    }
}

fn error_updates(
    db: &Connection,
    url: &str,
    source: &str,
    error: &str,
) -> Result<Vec<ObservationUpdate>> {
    Ok(watch_tasks(db, url)?
        .into_iter()
        .filter_map(|(project, number, mut status)| {
            let mut errors = source_errors(&status["prs"][url]);
            if errors.get(source).is_some_and(|previous| previous == error) {
                return None;
            }
            errors.insert(source.into(), json!(error));
            if !status["prs"][url].is_object() {
                status["prs"][url] = json!({});
            }
            attach_errors(&mut status["prs"][url], errors);
            Some(ObservationUpdate {
                project,
                number,
                status,
                wake: false,
            })
        })
        .collect())
}

fn observation_updates(
    db: &Connection,
    url: &str,
    observation: &hey_gh::watcher::Observation,
    checked_at: i64,
) -> Result<Vec<ObservationUpdate>> {
    let mut updates = Vec::new();
    let signals = serde_json::to_string(&signal_keys(observation))?;
    for (project, number, mut status) in watch_tasks(db, url)? {
        let previous = &status["prs"][url];
        if previous["checked_at"]
            .as_i64()
            .is_some_and(|old| old > checked_at)
        {
            continue;
        }
        let wake: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM json_each(?5) s WHERE NOT EXISTS(SELECT 1 FROM issue_github_signals seen WHERE seen.project_id=?1 AND seen.issue_number=?2 AND seen.url=?3 AND seen.head=?4 AND seen.signal=s.value))",params![project,number,url,observation.head,signals],|r|r.get(0))?;
        let mut errors = source_errors(previous);
        errors.remove("required_checks");
        let evidence = &observation.evidence;
        if evidence.get("checks").is_some() {
            errors.remove("ci");
        }
        if evidence.get("reviews").is_some() || evidence.get("required").is_none() {
            errors.clear();
        }
        let same_evidence = previous["head"] == observation.head
            && (previous["evidence"] == *evidence
                || same_ci_evidence(&previous["evidence"], evidence));
        let mut next = json!({"head":observation.head,"checked_at":previous["checked_at"],"evidence":if same_evidence {&previous["evidence"]} else {evidence}});
        attach_errors(&mut next, errors);
        if !wake && next == *previous {
            continue;
        }
        next["checked_at"] = json!(checked_at);
        status.as_object_mut().unwrap().remove("error");
        status["prs"][url] = next;
        if wake {
            status["event"] = json!(crate::issues::worker::random_id()?);
            status["trigger"] = json!({"url":url,"blocking":!observation.blocking.is_empty(),"completed":observation.completed.is_some()});
        }
        updates.push(ObservationUpdate {
            project,
            number,
            status,
            wake,
        });
    }
    Ok(updates)
}

fn same_ci_evidence(previous: &Value, next: &Value) -> bool {
    next.get("reviews").is_none()
        && (next.get("ci_complete").is_none() || next["ci_complete"] == previous["ci_complete"])
        && next.as_object().is_some_and(|fields| {
            fields.iter().all(|(key, value)| {
                key == "complete"
                    || (key == "omitted"
                        && value.as_object().is_some_and(|counts| {
                            counts
                                .iter()
                                .all(|(name, count)| previous["omitted"].get(name) == Some(count))
                        }))
                    || previous.get(key) == Some(value)
            })
        })
}

fn signal_keys(observation: &hey_gh::watcher::Observation) -> Vec<&String> {
    observation
        .blocking
        .iter()
        .chain(&observation.completed)
        .chain(&observation.feedback)
        .collect()
}

fn steering_id(run: &str, status: &Value) -> Option<String> {
    status["event"]
        .as_str()
        .map(|event| format!("github:{run}:{event}"))
}

pub(super) fn acknowledge_claim(
    db: &Connection,
    project: &str,
    number: i64,
    actor: &Actor,
) -> Result<()> {
    let (target, status) = saved(db, project, number)?;
    if target.as_deref() != Some("github") {
        return Ok(());
    }
    let run:Option<String>=db.query_row("SELECT id FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 AND finished_at IS NULL",params![project,number,actor.id],|r|r.get(0)).optional()?;
    if let Some(run) = run
        && let Some(id) = steering_id(&run, &status)
    {
        db.execute("INSERT OR IGNORE INTO agent_steering(request_id,run_id,scope,text,state,created_at) VALUES(?1,?2,'session','','delivered',?3)",params![id,run,crate::issues::worker::now()])?;
    }
    Ok(())
}

pub(super) fn queue_steering(db: &Connection, run: &str) -> Result<()> {
    let status:Option<String>=db.query_row("SELECT w.status FROM issues i JOIN issue_github_watches w ON w.project_id=i.project_id AND w.issue_number=i.number JOIN worker_runs r ON r.project_id=i.project_id AND r.issue_number=i.number AND r.actor_id=i.assignee WHERE r.id=?1 AND r.finished_at IS NULL AND r.claimed_at IS NOT NULL AND r.stop_requested=0 AND i.assignment_target='github' AND i.state='open' AND i.deleted_at IS NULL AND json_type(w.status,'$.event')='text' AND NOT EXISTS(SELECT 1 FROM agent_steering s WHERE s.request_id='github:'||r.id||':'||json_extract(w.status,'$.event'))",[run],|r|r.get(0)).optional()?;
    let Some(status) = status else {
        return Ok(());
    };
    let status: Value = serde_json::from_str(&status)?;
    let Some(id) = steering_id(run, &status) else {
        return Ok(());
    };
    let text = serde_json::to_string(&json!({"github_status":public_status(status,true)}))?;
    db.execute("INSERT OR IGNORE INTO agent_steering(request_id,run_id,scope,text,created_at) VALUES(?1,?2,'session',?3,?4)",params![id,run,text,crate::issues::worker::now()])?;
    Ok(())
}

pub(super) fn is_watching(db: &Connection, project: &str, number: i64) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT assignment_target='github' FROM issues WHERE project_id=?1 AND number=?2",
            params![project, number],
            |r| r.get::<_, Option<bool>>(0),
        )?
        .unwrap_or(false))
}

pub(super) fn own_handoff(
    db: &Connection,
    job: &crate::issues::worker::Job,
    issue: &Issue,
) -> Result<bool> {
    if issue.state != "open"
        || issue.deleted_at.is_some()
        || issue.title != job.issue["title"]
        || issue.body != job.issue["body"]
        || !is_watching(db, &job.project.id, job.number())?
    {
        return Ok(false);
    }
    Ok(db.query_row("SELECT coalesce((SELECT actor=?3 AND json_extract(data,'$.target')='github' AND json_extract(data,'$.previous_assignee')=?3 FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('assigned','claimed','ready','unassigned','closed','reopened') ORDER BY id DESC LIMIT 1),0)",params![job.project.id,job.number(),job.actor.id],|r|r.get(0))?)
}

fn delivered_to(db: &Connection, run: &str, status: &Value) -> Result<bool> {
    if let Some(id) = steering_id(run, status) {
        Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_steering WHERE request_id=?1 AND state='delivered')",
            [id],
            |r| r.get(0),
        )?)
    } else {
        Ok(true)
    }
}

pub(super) fn release_worker(
    db: &Connection,
    job: &crate::issues::worker::Job,
    state: &str,
) -> Result<()> {
    let (_, status) = saved(db, &job.project.id, job.number())?;
    let next = (state == "completed" && delivered_to(db, &job.id, &status)?).then_some(WATCHER);
    if next.is_none() {
        db.execute("UPDATE issues SET state='open' WHERE project_id=?1 AND number=?2 AND assignee=?3 AND state='ready'",params![job.project.id,job.number(),job.actor.id])?;
    }
    if next.is_some() {
        db.execute(
            "DELETE FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2",
            params![job.project.id, job.number()],
        )?;
    }
    let now = crate::issues::worker::now();
    db.execute("UPDATE issues SET assignee=?4,github_ack_event=?6,version=version+1,updated_at=max(updated_at,?5) WHERE project_id=?1 AND number=?2 AND assignee=?3",params![job.project.id,job.number(),job.actor.id,next,now,next.and_then(|_|status["event"].as_str())])?;
    event(
        db,
        &job.project.id,
        job.number(),
        &job.actor.id,
        "assigned",
        now,
        &json!({"target":"github","previous_assignee":job.actor.id,"assignee":next}),
    )?;
    Ok(())
}

pub(super) fn release_claim(db: &Connection, job: &crate::issues::worker::Job) -> Result<()> {
    let now = crate::issues::worker::now();
    let changed = db.execute("UPDATE issues SET assignee=NULL,version=version+1,updated_at=max(updated_at,?4) WHERE project_id=?1 AND number=?2 AND assignee=?3",params![job.project.id,job.number(),job.actor.id,now])?;
    if changed > 0 {
        event(
            db,
            &job.project.id,
            job.number(),
            &job.actor.id,
            "unassigned",
            now,
            &json!({"previous_assignee":job.actor.id,"forced":false}),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        store: Store,
        request: Request,
        root: std::path::PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "hb-assignment-{}",
                super::super::super::worker::random_id().unwrap()
            ));
            std::fs::create_dir(&root).unwrap();
            let store = Store::open(&root.join("issues.db")).unwrap();
            let actor =
                crate::issues::identity::resolve(Some("human:test"), "test", &root).unwrap();
            let request = Request {
                version: 1,
                project: Project {
                    id: "named:test".into(),
                    name: "test".into(),
                },
                project_override: None,
                actor: Some(actor),
                operation: Operation::View { number: 1 },
                request_id: None,
            };
            let mut f = Self {
                store,
                request,
                root,
            };
            f.call(json!({"action":"create","title":"Task","body":"","labels":[]}))
                .unwrap();
            f.call(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/1","purpose":"fix"})).unwrap();
            f
        }
        fn call(&mut self, value: Value) -> Result<Value> {
            self.request.operation = serde_json::from_value(value).unwrap();
            self.store.execute(&self.request)
        }
        fn assign(&mut self, target: &str) -> Result<Value> {
            let version = get_issue(&self.store.db, "named:test", 1, false)?.version;
            self.call(json!({"action":"assign","number":1,"target":target,"if_version":version}))
        }
        fn observation(&mut self, key: Option<&str>) {
            self.store
                .record_github_observation(
                    "https://github.com/o/r/pull/1",
                    &hey_gh::watcher::Observation {
                        head: "head".into(),
                        blocking: key.into_iter().map(str::to_owned).collect(),
                        completed: None,
                        feedback: Vec::new(),
                        evidence: json!({"head":"head","complete":false}),
                    },
                    100,
                )
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn assignment_waits_then_a_new_failure_releases_exactly_once() {
        let mut f = Fixture::new();
        let assigned = f.assign("github").unwrap();
        assert_eq!(assigned["issue"]["assignment"]["kind"], "github");
        assert_eq!(assigned["issue"]["assignee"], WATCHER);
        f.observation(None);
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .assignee
                .as_deref(),
            Some(WATCHER)
        );
        f.observation(Some("fail-1"));
        assert!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .assignee
                .is_none()
        );
        f.call(json!({"action":"claim","number":1,"force":false}))
            .unwrap();
        let claim = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(
            claim["issue"]["github_status"]["prs"]["https://github.com/o/r/pull/1"]["evidence"]["head"],
            "head"
        );
        f.assign("github").unwrap();
        f.observation(Some("fail-1"));
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .assignee
                .as_deref(),
            Some(WATCHER)
        );
        f.observation(Some("fail-2"));
        assert!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .assignee
                .is_none()
        );
    }
    #[test]
    fn manual_takeover_unsubscribes_and_stale_assignment_is_rejected() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        assert!(
            f.call(json!({"action":"assign","number":1,"target":"unassigned","if_version":1}))
                .is_err()
        );
        f.assign("boss").unwrap();
        f.observation(Some("new"));
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .assignee
                .as_deref(),
            Some("human:boss")
        );
    }

    #[test]
    fn parked_watcher_is_not_another_agents_claim_and_closed_tasks_stop_monitoring() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        let closed = f
            .call(json!({"action":"close","number":1,"force":false,"comment":null}))
            .unwrap();
        assert_eq!(closed["issue"]["state"], "closed");
        let view = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(view["issue"]["github_status"]["monitoring"], false);
        assert!(f.store.github_watch_urls().unwrap().is_empty());
    }

    #[test]
    fn removing_the_last_watchable_pr_returns_waiting_work_to_boss() {
        let mut f = Fixture::new();
        f.call(json!({"action":"add_pull_request","number":1,"url":"https://example.com/not-github","purpose":"supporting-evidence"})).unwrap();
        f.assign("github").unwrap();
        f.call(json!({"action":"remove_pull_request","number":1,"url":"https://github.com/o/r/pull/1"})).unwrap();
        let view = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(view["issue"]["assignee"], "human:boss");
        assert_eq!(view["issue"]["github_status"]["monitoring"], false);
        assert_eq!(
            view["issue"]["github_status"]["stopped_reason"],
            "no_open_pull_requests"
        );
        let version = view["issue"]["version"].clone();
        f.store
            .reconcile_github_assignments(f.request.actor.as_ref().unwrap())
            .unwrap();
        assert_eq!(
            f.call(json!({"action":"view","number":1})).unwrap()["issue"]["version"],
            version
        );
        f.request.actor.as_mut().unwrap().id = "human:boss".into();
        f.call(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/2","purpose":"fix"})).unwrap();
        f.assign("github").unwrap();
        let resumed = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(resumed["issue"]["github_status"]["monitoring"], true);
        assert!(
            resumed["issue"]["github_status"]
                .get("stopped_reason")
                .is_none()
        );
    }

    #[test]
    fn terminal_pr_reconciliation_preserves_an_active_claim_and_other_open_prs() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.call(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/2","purpose":"fix"})).unwrap();
        f.store
            .record_pr_status("https://github.com/o/r/pull/1", Some("closed"), 100, None)
            .unwrap();
        f.store
            .reconcile_github_assignments(f.request.actor.as_ref().unwrap())
            .unwrap();
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .assignee
                .as_deref(),
            Some(WATCHER)
        );
        f.call(json!({"action":"claim","number":1,"force":false}))
            .unwrap();
        f.store
            .record_pr_status("https://github.com/o/r/pull/2", Some("closed"), 100, None)
            .unwrap();
        f.store
            .reconcile_github_assignments(f.request.actor.as_ref().unwrap())
            .unwrap();
        let view = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(
            view["issue"]["assignee"],
            f.request.actor.as_ref().unwrap().id
        );
        assert_eq!(view["issue"]["github_status"]["monitoring"], false);
    }

    #[test]
    fn github_subscription_preserves_ready_until_actionable_work_reopens_it() {
        let mut f = Fixture::new();
        f.call(json!({"action":"configure_project","prs_enabled":true}))
            .unwrap();
        f.request.actor.as_mut().unwrap().id = "human:boss".into();
        f.call(json!({"action":"ready","number":1,"force":false}))
            .unwrap();
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .state,
            "ready"
        );
        f.assign("github").unwrap();
        f.observation(None);
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .state,
            "ready"
        );
        f.observation(Some("failure"));
        let issue = get_issue(&f.store.db, "named:test", 1, false).unwrap();
        assert_eq!(issue.state, "open");
        assert!(issue.assignee.is_none());
    }
    #[test]
    fn watcher_requires_a_link_and_cannot_steal_a_claim() {
        let mut f = Fixture::new();
        f.call(json!({"action":"remove_pull_request","number":1,"url":"https://github.com/o/r/pull/1"})).unwrap();
        assert!(f.assign("github").is_err());
        f.call(json!({"action":"add_pull_request","number":1,"url":"https://example.com/o/r/pull/1","purpose":"fix"})).unwrap();
        assert!(
            f.assign("github").is_err(),
            "Unsupported links must not create a watcher that cannot poll"
        );
        f.call(json!({"action":"claim","number":1,"force":false}))
            .unwrap();
        f.request.actor.as_mut().unwrap().id = "human:someone-else".into();
        assert!(f.assign("unassigned").is_err());
    }
    #[test]
    fn allocation_is_shown_as_assignment_and_machine_target_is_durable() {
        let mut f = Fixture::new();
        f.assign("machine:devbox").unwrap();
        let view = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(view["issue"]["assignment"]["kind"], "machine");
        assert_eq!(view["issue"]["assignment"]["machine"], "devbox");
        f.store
            .db
            .execute("DELETE FROM fleet_allocations", [])
            .unwrap();
        assert!(
            crate::issues::fleet::check_claim(
                &f.store.db,
                "named:test",
                1,
                "another-machine",
                false
            )
            .is_err()
        );
        f.assign("unassigned").unwrap();
        assert!(
            !f.store
                .db
                .query_row("SELECT EXISTS(SELECT 1 FROM fleet_allocations)", [], |r| {
                    r.get::<_, bool>(0)
                })
                .unwrap()
        );
    }

    #[test]
    fn unchanged_poll_does_not_write_or_wait_for_a_writer() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.observation(None);
        f.store.db.busy_timeout(Duration::from_millis(20)).unwrap();
        let mut writer = Connection::open(&f.root.join("issues.db")).unwrap();
        let _lock = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        f.observation(None);
        f.store
            .reconcile_github_assignments(f.request.actor.as_ref().unwrap())
            .unwrap();
    }

    #[test]
    fn repeated_ci_pass_does_not_erase_reviews_or_rewrite_complete_evidence() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        let mut observation = hey_gh::watcher::Observation {
            head: "head".into(),
            blocking: vec![],
            completed: Some("done".into()),
            feedback: vec![],
            evidence: json!({"head":"head","ci_complete":true,"complete":true,"checks":[],"reviews":[{"body":"Finding"}],"source_errors":[],"omitted":{"checks":5,"reviews":9},"truncated":true}),
        };
        let url = "https://github.com/o/r/pull/1";
        f.store
            .record_github_observation(url, &observation, 100)
            .unwrap();
        let before = saved(&f.store.db, "named:test", 1).unwrap().1;
        observation.completed = None;
        observation.evidence = json!({"head":"head","ci_complete":true,"complete":false,"checks":[],"omitted":{"checks":5},"truncated":true});
        f.store.db.busy_timeout(Duration::from_millis(20)).unwrap();
        let mut writer = Connection::open(&f.root.join("issues.db")).unwrap();
        let lock = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        f.store
            .record_github_observation(url, &observation, 200)
            .unwrap();
        assert_eq!(saved(&f.store.db, "named:test", 1).unwrap().1, before);
        drop(lock);
        observation.evidence["checks"] = json!([{"id":2,"status":"in_progress"}]);
        observation.evidence["ci_complete"] = json!(false);
        f.store
            .record_github_observation(url, &observation, 300)
            .unwrap();
        assert_eq!(
            saved(&f.store.db, "named:test", 1).unwrap().1["prs"][url]["evidence"]["complete"],
            false
        );
    }

    #[test]
    fn malformed_saved_status_recovers_without_panicking_or_hiding_new_work() {
        for malformed in [
            json!([]),
            json!("bad"),
            json!({"prs":[]}),
            json!({"prs":{"https://github.com/o/r/pull/1":"bad"}}),
        ] {
            let mut f = Fixture::new();
            f.assign("github").unwrap();
            f.store.db.execute("INSERT INTO issue_github_watches(project_id,issue_number,status) VALUES('named:test',1,?1)", [malformed.to_string()]).unwrap();
            let view = f.call(json!({"action":"view","number":1})).unwrap();
            assert!(view["issue"]["github_status"]["prs"].is_object());
            f.store
                .record_github_error(
                    "https://github.com/o/r/pull/1",
                    "required_checks",
                    "Temporary error",
                )
                .unwrap();
            f.observation(Some("new-failure"));
            let view = f.call(json!({"action":"view","number":1})).unwrap();
            assert!(view["issue"]["assignee"].is_null());
            assert!(view["issue"]["github_status"]["event"].is_string());
        }
    }

    #[test]
    fn partial_recovery_preserves_independent_source_errors_without_rewriting_them() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        let url = "https://github.com/o/r/pull/1";
        let mut observation = hey_gh::watcher::Observation {
            head: "head".into(),
            blocking: vec![],
            completed: None,
            feedback: vec![],
            evidence: json!({"head":"head","required":[],"required_state":"not_required","complete":false}),
        };
        f.store
            .record_github_observation(url, &observation, 100)
            .unwrap();
        f.store
            .record_github_error(url, "reviews", "Review access denied")
            .unwrap();
        f.store
            .record_github_error(url, "ci", "Jobs unavailable")
            .unwrap();
        let before = saved(&f.store.db, "named:test", 1).unwrap().1;
        f.store.db.busy_timeout(Duration::from_millis(20)).unwrap();
        let mut writer = Connection::open(&f.root.join("issues.db")).unwrap();
        let lock = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        f.store
            .record_github_observation(url, &observation, 200)
            .unwrap();
        assert_eq!(saved(&f.store.db, "named:test", 1).unwrap().1, before);
        drop(lock);
        observation.evidence["checks"] = json!([]);
        f.store
            .record_github_observation(url, &observation, 300)
            .unwrap();
        let status = saved(&f.store.db, "named:test", 1).unwrap().1;
        assert_eq!(status["prs"][url]["error"], "Review access denied");
        observation.evidence["reviews"] = json!([]);
        f.store
            .record_github_observation(url, &observation, 400)
            .unwrap();
        assert!(
            saved(&f.store.db, "named:test", 1).unwrap().1["prs"][url]
                .get("error")
                .is_none()
        );
    }

    #[test]
    fn repeated_reruns_keep_replica_status_small_without_forgetting_old_events() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        for i in 0..256 {
            f.observation(Some(&format!("failure-{i:064}")));
        }
        let status = saved(&f.store.db, "named:test", 1).unwrap().1;
        assert!(
            status.to_string().len() < 2000,
            "Event history must not grow the replicated status: {} bytes",
            status.to_string().len()
        );
        f.store = Store::open(&f.root.join("issues.db")).unwrap();
        f.assign("github").unwrap();
        f.observation(Some(&format!("failure-{:064}", 0)));
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .assignee
                .as_deref(),
            Some(WATCHER)
        );
    }

    #[test]
    fn polling_updates_never_rewrite_issue_content() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.store.db.execute_batch("CREATE TRIGGER reject_content_poll BEFORE UPDATE ON issues BEGIN SELECT RAISE(ABORT,'poll rewrote issue'); END;").unwrap();
        f.observation(None);
        let status = saved(&f.store.db, "named:test", 1).unwrap().1;
        assert_eq!(
            status["prs"]["https://github.com/o/r/pull/1"]["head"],
            "head"
        );
    }

    #[test]
    fn first_poll_failure_is_durable_and_pr_recovery_is_independent() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        let first = "https://github.com/o/r/pull/1";
        let second = "https://github.com/o/r/pull/2";
        f.call(json!({"action":"add_pull_request","number":1,"url":second,"purpose":"fix"}))
            .unwrap();
        f.store
            .record_github_error(first, "required_checks", "rate limited")
            .unwrap();
        f.store
            .record_github_error(second, "required_checks", "unavailable")
            .unwrap();
        let status = saved(&f.store.db, "named:test", 1).unwrap().1;
        assert_eq!(status["prs"][first]["error"], "rate limited");
        assert_eq!(status["prs"][second]["error"], "unavailable");
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .assignee
                .as_deref(),
            Some(WATCHER)
        );
        f.observation(None);
        let status = saved(&f.store.db, "named:test", 1).unwrap().1;
        assert!(status["prs"][first]["error"].is_null());
        assert_eq!(status["prs"][second]["error"], "unavailable");
        f.store.db.busy_timeout(Duration::from_millis(20)).unwrap();
        let mut writer = Connection::open(f.root.join("issues.db")).unwrap();
        let _lock = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        f.store
            .record_github_error(second, "required_checks", "unavailable")
            .unwrap();
    }

    #[test]
    fn assignment_filters_include_queued_and_running_watchers() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        for step in 0..3 {
            if step == 1 {
                f.observation(Some("failure"));
            }
            if step == 2 {
                f.call(json!({"action":"claim","number":1,"force":false}))
                    .unwrap();
            }
            let listed=f.call(json!({"action":"list","state":"all","mine":false,"unassigned":false,"assignee":"watcher:github","labels":[],"search":null,"limit":50,"offset":0})).unwrap();
            assert_eq!(listed["issues"].as_array().unwrap().len(), 1);
            let listed=f.call(json!({"action":"list","state":"all","mine":false,"unassigned":true,"labels":[],"search":null,"limit":50,"offset":0})).unwrap();
            assert!(listed["issues"].as_array().unwrap().is_empty());
        }
    }

    #[test]
    fn project_unassigned_count_agrees_with_assignment_filter() {
        let mut f = Fixture::new();
        for target in ["machine:devbox", "github", "unassigned"] {
            f.assign(target).unwrap();
            if target == "github" {
                f.observation(Some("new"));
            }
            let projects = f
                .call(json!({"action":"projects","include_hidden":false}))
                .unwrap();
            assert_eq!(
                projects["projects"][0]["unassigned"],
                if target == "unassigned" { 1 } else { 0 }
            );
        }
    }

    #[test]
    fn transferring_a_waiting_watcher_preserves_subscription_and_deduplication() {
        let mut f = Fixture::new();
        f.request.project_override = Some("Destination".into());
        f.call(json!({"action":"create","title":"Other","body":"","labels":[]}))
            .unwrap();
        f.request.project_override = None;
        f.assign("github").unwrap();
        f.observation(Some("failure"));
        f.assign("github").unwrap();
        let version = get_issue(&f.store.db, "named:test", 1, false)
            .unwrap()
            .version;
        let moved = f.call(json!({"action":"transfer","number":1,"destination":"named:Destination","if_version":version})).unwrap();
        assert_eq!(moved["issue"]["assignment"]["kind"], "github");
        assert_eq!(
            moved["issue"]["github_status"]["prs"]["https://github.com/o/r/pull/1"]["head"],
            "head"
        );
        f.observation(Some("failure"));
        assert_eq!(
            get_issue(&f.store.db, "named:Destination", 2, false)
                .unwrap()
                .assignee
                .as_deref(),
            Some(WATCHER)
        );
        assert!(saved(&f.store.db, "named:test", 1).unwrap().0.is_none());
    }
}
