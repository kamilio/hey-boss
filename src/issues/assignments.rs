//! Assignment is a destination; the assignee is the session currently doing work.
use super::*;
use crate::issues::ReviewedGithubEvidence;

const WATCHER: &str = "watcher:github";
#[path = "github_watch_comments.rs"]
mod comments;
#[path = "assignment_evidence.rs"]
mod evidence;
#[path = "github_fetch.rs"]
pub(super) mod fetch;
#[path = "github_handoff.rs"]
mod handoff;
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
    let (metadata_ready, summary_ready) = db.query_row("SELECT count(*)=4,EXISTS(SELECT 1 FROM sqlite_master WHERE name='issue_assignment_summary' AND type='index' AND instr(sql,'closed_by')>0) FROM sqlite_master WHERE name IN ('issue_github_watches','issue_github_signals','issue_github_destinations','github_fetch_status')",[],|r|Ok((r.get::<_,bool>(0)?,r.get::<_,bool>(1)?)))?;
    if !metadata_ready {
        db.execute_batch("CREATE TABLE IF NOT EXISTS issue_github_watches(project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,status TEXT NOT NULL CHECK(json_valid(status)),PRIMARY KEY(project_id,issue_number),FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number)); CREATE INDEX IF NOT EXISTS issue_github_destinations ON issues(project_id,number) WHERE assignment_target='github';")?;
        fetch::migrate(db)?;
        // The supervisor retains exact event identities locally. Peers need only
        // current evidence and its event generation, not an ever-growing history.
        db.execute_batch("CREATE TABLE IF NOT EXISTS issue_github_signals(project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,url TEXT NOT NULL,head TEXT NOT NULL,signal TEXT NOT NULL,PRIMARY KEY(project_id,issue_number,url,head,signal),FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number)) WITHOUT ROWID;")?;
    }
    if !summary_ready {
        // Ownership fields follow body in issue records. Cover both actor
        // metadata and worker polls without reading body overflow chains.
        let tx = db.unchecked_transaction()?;
        tx.execute_batch("DROP INDEX IF EXISTS issue_assignment_summary; CREATE INDEX issue_assignment_summary ON issues(project_id,number,assignment_target,assignee,state,deleted_at,closed_by)")?;
        tx.commit()?;
    }
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
    reviewed_evidence: Option<&[ReviewedGithubEvidence]>,
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
            "Assignment requires an open or Ready, non-draft issue",
        ));
    }
    let own_ready_handoff = target == "github"
        && issue.state == "ready"
        && issue.assignee.as_deref() == Some("human:boss")
        && db.query_row(
            "SELECT coalesce((SELECT action='ready' AND actor=?3 AND json_extract(data,'$.previous_assignee')=?3 FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('claimed','ready','assigned','unassigned','closed','reopened','blocked','deleted','restored') ORDER BY id DESC LIMIT 1),0)",
            params![project.id, issue.number, actor.id], |r| r.get::<_, bool>(0),
        )?;
    if !own_ready_handoff && ownership(issue, actor, actor.id == "human:boss").is_err() {
        return Err(Error::conflict(format!(
            "Issue #{} is claimed by {}; only its owner or Boss can change assignment. The agent that handed off its own claim may run `hey-boss issue assign {} github` after Ready. For another owner's Ready handoff, ask Boss to assign GitHub in the web UI; do not reopen or unassign it",
            issue.number,
            issue.assignee.as_deref().unwrap(),
            issue.number
        )));
    }
    if own_ready_handoff {
        // A Ready handoff delegates only this agent's completed claim. Never
        // release a later reservation or interrupt a foreign/unclaimed attempt.
        let reserved: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2 AND node<>?4) OR EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL AND (actor_id<>?3 OR claimed_at IS NULL))",
            params![project.id, issue.number, actor.id, actor.machine], |r| r.get(0),
        )?;
        if reserved {
            return Err(Error::conflict(
                "Ready handoff blocked by a fleet reservation or unfinished/unclaimed worker attempt; ownership and reservation preserved. Inspect `hey-boss issue allocation NUMBER --supervisor` before retrying",
            ));
        }
    }
    let other_run:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL AND actor_id<>?3)",params![project.id,issue.number,actor.id],|r|r.get(0))?;
    let claim_owner = if own_ready_handoff {
        Some(actor.id.as_str())
    } else {
        issue.assignee.as_deref()
    };
    let retain_claim = target == "github" && live_claim(db, claim_owner)?;
    let retained_owner = retain_claim.then(|| claim_owner.unwrap().to_owned());
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
    if let Some(evidence) = reviewed_evidence {
        if target != "github" {
            return Err(Error::invalid(
                "Reviewed evidence requires GitHub assignment",
            ));
        }
        handoff::acknowledge(db, &project.id, issue.number, evidence)?;
    }
    let previous = issue.assignee.clone();
    issue.assignee = match target {
        "github" if retain_claim => retained_owner,
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
    if target == "github" {
        let previous = status.clone();
        status = lifecycle::linked_status(
            status,
            &registry::pull_requests(db, &project.id, issue.number)?,
            false,
        );
        status.as_object_mut().unwrap().remove("stopped_reason");
        if status != previous {
            db.execute(
                "UPDATE issue_github_watches SET status=?3 WHERE project_id=?1 AND issue_number=?2",
                params![project.id, issue.number, status.to_string()],
            )?;
        }
    }
    if let Some(machine) = machine {
        db.execute(
            "INSERT INTO fleet_allocations(project_id,issue_number,node) VALUES(?1,?2,?3)",
            params![project.id, issue.number, machine],
        )?;
    }
    let mut data = json!({"target":target,"previous_assignee":previous,"assignee":issue.assignee});
    if let Some(evidence) = reviewed_evidence {
        data["reviewed_github"] = json!(
            evidence
                .iter()
                .map(|snapshot| json!({
                    "repository":snapshot.report.data.repository,
                    "number":snapshot.report.data.number,
                    "head":snapshot.report.data.ci.head_sha,
                    "observed_at_ms":snapshot.report.observed_at_ms
                }))
                .collect::<Vec<_>>()
        );
    }
    if retain_claim && (own_ready_handoff || previous.as_deref() == Some(&actor.id)) {
        // Assignment runs on the supervisor, including companion handoffs.
        // Scope the acknowledgement to this attempt, not a reusable session ID.
        if let Some(run) = live_issue_run(db, &project.id, issue.number, &actor.id)? {
            if reviewed_evidence.is_some()
                && let Some(id) = steering_id(&run, &status)
            {
                db.execute("INSERT INTO agent_steering(request_id,run_id,scope,text,state,created_at) VALUES(?1,?2,'session','','delivered',?3) ON CONFLICT(request_id) DO UPDATE SET state='delivered'",params![id,run,now])?;
            }
            data["github_handoff"] = json!({"run":run,"event":status["event"]});
        }
    }
    Ok(data)
}

fn live_issue_run(
    db: &Connection,
    project: &str,
    number: i64,
    actor: &str,
) -> Result<Option<String>> {
    let local = db.query_row("SELECT id FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 AND finished_at IS NULL",params![project,number,actor],|r|r.get(0)).optional()?;
    if local.is_some()
        || !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='fleet_state' AND type='table')",
            [],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Ok(local);
    }
    Ok(db.query_row("SELECT json_extract(r.value,'$.id') FROM fleet_state s,json_each(s.value) m,json_each(m.value,'$.workers') w,json_each(w.value,'$.runs') r WHERE s.key='machines' AND json_extract(r.value,'$.project_id')=?1 AND json_extract(r.value,'$.number')=?2 AND json_extract(r.value,'$.actor_id')=?3 AND json_extract(r.value,'$.finished_at') IS NULL LIMIT 1",params![project,number,actor],|r|r.get(0)).optional()?)
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
    Ok(db.query_collect::<_, _, rusqlite::Error>("SELECT i.number,i.assignment_target,a.node,json_extract(g.metadata,'$.machine'),json_extract(g.metadata,'$.host') FROM issues i LEFT JOIN fleet_allocations a ON a.project_id=i.project_id AND a.issue_number=i.number LEFT JOIN agents g ON g.id=i.assignee WHERE i.project_id=?1 AND i.number IN (SELECT value FROM json_each(?2))",params![project,serde_json::to_string(numbers)?],|r|Ok((r.get(0)?,AssignmentMetadata{target:r.get(1)?,reserved:r.get(2)?,actor_machine:r.get(3)?,actor_host:r.get(4)?})))?.into_iter().collect())
}

fn machines(db: &Connection, actor: Option<&Actor>) -> Result<Vec<Value>> {
    let mut machines = Vec::new();
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='fleet_state' AND type='table')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        machines=db.query_collect::<_, _, rusqlite::Error>("SELECT json_extract(m.value,'$.node'),json_extract(m.value,'$.hostname'),json_extract(m.value,'$.host'),json_extract(m.value,'$.state') FROM fleet_state s,json_each(s.value) m WHERE s.key='machines' AND json_extract(m.value,'$.node') IS NOT NULL",[],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,Option<String>>(1)?,"host":r.get::<_,Option<String>>(2)?,"state":r.get::<_,Option<String>>(3)?})))?;
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
            pr.remove("signals");
            pr.remove("review_signals");
        }
    }
    evidence::bounded(status)
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
            || status["prs"].as_object().is_some_and(|prs| !prs.is_empty())
        {
            let monitoring = meta.target.as_deref() == Some("github")
                && issue["state"] != "closed"
                && issue["deleted_at"].is_null();
            issue["github_status"] = public_status(status, monitoring);
            issue["github_status"]["fetches"] = fetch::status(db, project, number)?;
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
        let signal_url = url.trim_end_matches('/');
        for update in observation_updates(&tx, url, observation, checked_at)? {
            tx.execute("DELETE FROM issue_github_signals WHERE project_id=?1 AND issue_number=?2 AND url IN (?3,?3||'/') AND head<>?4",params![update.project,update.number,signal_url,observation.head])?;
            tx.execute("INSERT OR IGNORE INTO issue_github_signals SELECT ?1,?2,?3,?4,value FROM json_each(?5)",params![update.project,update.number,signal_url,observation.head,signals])?;
            tx.execute("INSERT INTO issue_github_watches(project_id,issue_number,status) VALUES(?1,?2,?3) ON CONFLICT(project_id,issue_number) DO UPDATE SET status=excluded.status",params![update.project,update.number,update.status.to_string()])?;
            if update.wake {
                comments::record(
                    &tx,
                    &update.project,
                    update.number,
                    url,
                    observation,
                    &update.new_signals,
                )?;
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
    new_signals: Vec<String>,
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
                new_signals: Vec::new(),
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
        // A trailing slash changes the attached link, not the GitHub event.
        // Read both spellings so existing durable histories remain effective.
        // The first policy identity establishes comparison state; it must not
        // invent a policy-change event during upgrade. Existing CI/review
        // signals still wake on a first scan, and reviewed handoffs seed policy.
        let new_signals: Vec<String> = db.prepare("SELECT DISTINCT s.value FROM json_each(?5) s WHERE NOT EXISTS(SELECT 1 FROM issue_github_signals seen WHERE seen.project_id=?1 AND seen.issue_number=?2 AND seen.url IN (?3,?3||'/') AND seen.head=?4 AND seen.signal=s.value) AND (s.value NOT LIKE 'policy:%' OR EXISTS(SELECT 1 FROM issue_github_signals baseline WHERE baseline.project_id=?1 AND baseline.issue_number=?2 AND baseline.url IN (?3,?3||'/') AND baseline.signal LIKE 'policy:%'))")?
            .query_map(params![project,number,url.trim_end_matches('/'),observation.head,signals], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let wake = !new_signals.is_empty();
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
        // Retain complete signal identities outside the bounded UI evidence.
        // CI-only passes must not discard review identities.
        next["review_signals"] = if evidence["complete"] == true {
            json!(observation.feedback)
        } else if previous["head"] == observation.head {
            previous.get("review_signals").cloned().unwrap_or(json!([]))
        } else {
            json!([])
        };
        next["signals"] = if same_evidence
            && !wake
            && evidence["complete"] != true
            && previous.get("signals").is_some()
        {
            previous["signals"].clone()
        } else if evidence["complete"] == true {
            json!(signal_keys(observation))
        } else {
            let mut known = next["review_signals"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            known.extend(signal_keys(observation).into_iter().map(|s| json!(s)));
            known.sort_by_cached_key(Value::to_string);
            known.dedup();
            json!(known)
        };
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
            new_signals,
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

fn signal_keys(observation: &hey_gh::watcher::Observation) -> Vec<&str> {
    observation
        .blocking
        .iter()
        .chain(&observation.completed)
        .chain(&observation.feedback)
        .map(String::as_str)
        .chain(
            observation
                .evidence
                .get("policy_fingerprint")
                .and_then(Value::as_str),
        )
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
    db.execute(
        "UPDATE issues SET github_ack_event=?3 WHERE project_id=?1 AND number=?2",
        params![project, number, status["event"].as_str()],
    )?;
    let run:Option<String>=db.query_row("SELECT id FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 AND finished_at IS NULL",params![project,number,actor.id],|r|r.get(0)).optional()?;
    if let Some(run) = run
        && let Some(id) = steering_id(&run, &status)
    {
        db.execute("INSERT OR IGNORE INTO agent_steering(request_id,run_id,scope,text,state,created_at) VALUES(?1,?2,'session','','delivered',?3)",params![id,run,crate::issues::worker::now()])?;
    }
    Ok(())
}

pub(super) fn ready_assignment(
    db: &Connection,
    project: &str,
    number: i64,
    assignee: Option<&str>,
    guarded: bool,
) -> Result<Option<String>> {
    // Managed workers retain ownership until their process finishes. A manual
    // Ready handoff has no worker completion callback to return it to watching.
    if live_claim(db, assignee)? {
        return Ok(assignee.map(str::to_owned));
    }
    let (_, status) = saved(db, project, number)?;
    let acknowledged: Option<String> = db.query_row(
        "SELECT github_ack_event FROM issues WHERE project_id=?1 AND number=?2",
        params![project, number],
        |r| r.get(0),
    )?;
    let pending = status["event"]
        .as_str()
        .is_some_and(|event| Some(event) != acknowledged.as_deref());
    if pending && !guarded {
        return Err(Error::conflict(
            "Ready blocked by unacknowledged GitHub findings; inspect the current watcher status before handing off. State, ownership and reservations were preserved",
        ));
    }
    // The Ready snapshot includes this event; a concurrent finding invalidates
    // it. Idle triage acknowledges exactly the reviewed status without a claim.
    if pending {
        db.execute(
            "UPDATE issues SET github_ack_event=?3 WHERE project_id=?1 AND number=?2",
            params![project, number, status["event"].as_str()],
        )?;
    }
    db.execute(
        "DELETE FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2",
        params![project, number],
    )?;
    Ok(Some(WATCHER.into()))
}

fn steering_update(db: &Connection, run: &str) -> Result<Option<(String, Value)>> {
    let status:Option<String>=db.query_row("SELECT w.status FROM issues i JOIN issue_github_watches w ON w.project_id=i.project_id AND w.issue_number=i.number JOIN worker_runs r ON r.project_id=i.project_id AND r.issue_number=i.number AND r.actor_id=i.assignee WHERE r.id=?1 AND r.finished_at IS NULL AND r.claimed_at IS NOT NULL AND r.stop_requested=0 AND i.assignment_target='github' AND i.state='open' AND i.deleted_at IS NULL AND json_type(w.status,'$.event')='text'",[run],|r|r.get(0)).optional()?;
    let Some(status) = status else {
        return Ok(None);
    };
    let status: Value = serde_json::from_str(&status)?;
    let Some(id) = steering_id(run, &status) else {
        return Ok(None);
    };
    let prefix = format!("github:{run}:");
    let needed: bool = db.query_row("SELECT NOT EXISTS(SELECT 1 FROM agent_steering WHERE request_id=?1) OR EXISTS(SELECT 1 FROM agent_steering WHERE run_id=?2 AND state='queued' AND request_id<>?1 AND substr(request_id,1,length(?3))=?3)",params![id,run,prefix],|r|r.get(0))?;
    Ok(needed.then_some((id, status)))
}

pub(super) fn queue_steering(db: &Connection, run: &str) -> Result<()> {
    if steering_update(db, run)?.is_none() {
        return Ok(());
    }
    let tx = crate::database::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    // Recheck the current event and ownership after acquiring the writer lock.
    // Only unsent watcher snapshots can be replaced; human instructions and
    // messages already handed to the transport retain their delivery state.
    let Some((id, _status)) = steering_update(&tx, run)? else {
        return tx.commit().map_err(Into::into);
    };
    let prefix = format!("github:{run}:");
    tx.execute("UPDATE agent_steering SET state='superseded',text='' WHERE run_id=?1 AND state='queued' AND request_id<>?2 AND substr(request_id,1,length(?3))=?3",params![run,id,prefix])?;
    let text = "";
    tx.execute("INSERT OR IGNORE INTO agent_steering(request_id,run_id,scope,text,created_at) VALUES(?1,?2,'session',?3,?4)",params![id,run,text,crate::issues::worker::now()])?;
    tx.commit()?;
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
    if !matches!(issue.state.as_str(), "open" | "ready")
        || issue.deleted_at.is_some()
        || issue.title != job.issue["title"]
        || issue.body != job.issue["body"]
        || !is_watching(db, &job.project.id, job.number())?
    {
        return Ok(false);
    }
    Ok(db.query_row("SELECT coalesce((SELECT actor=?3 AND json_extract(data,'$.target')='github' AND (json_extract(data,'$.previous_assignee')=?3 OR (json_extract(data,'$.previous_assignee')='human:boss' AND json_extract(data,'$.github_handoff.run')=?4)) FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('assigned','claimed','ready','unassigned','closed','reopened') ORDER BY id DESC LIMIT 1),0)",params![job.project.id,job.number(),job.actor.id,job.id],|r|r.get(0))?)
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
) -> Result<bool> {
    let (_, status) = saved(db, &job.project.id, job.number())?;
    // A deliberate handoff can end with a blocked/interrupted result while
    // waiting for external input. Consume only that run's exact snapshot;
    // later evidence and events never delivered to the worker stay runnable.
    let handed_off = db.query_row("SELECT coalesce((SELECT actor=?3 AND json_extract(data,'$.target')='github' AND json_extract(data,'$.previous_assignee') IN (?3,'human:boss') AND json_extract(data,'$.github_handoff.run')=?4 AND json_extract(data,'$.github_handoff.event') IS ?5 FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('assigned','claimed','ready','unassigned','closed','reopened') ORDER BY id DESC LIMIT 1),0)",params![job.project.id,job.number(),job.actor.id,job.id,status["event"].as_str()],|r|r.get::<_,bool>(0))?;
    let handed_off = handed_off
        && own_handoff(
            db,
            job,
            &get_issue(db, &job.project.id, job.number(), true)?,
        )?;
    let next = ((state == "completed" || handed_off) && delivered_to(db, &job.id, &status)?)
        .then_some(WATCHER);
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
    Ok(next.is_some())
}

pub(super) fn release_claim(
    db: &Connection,
    project: &str,
    number: i64,
    actor: &str,
    now: i64,
) -> Result<()> {
    let changed = db.execute("UPDATE issues SET assignee=NULL,version=version+1,updated_at=max(updated_at,?4) WHERE project_id=?1 AND number=?2 AND assignee=?3",params![project,number,actor,now])?;
    if changed > 0 {
        event(
            db,
            project,
            number,
            actor,
            "unassigned",
            now,
            &json!({"previous_assignee":actor,"forced":false}),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    include!("github_handoff_tests.rs");
    use super::*;
    #[test]
    fn unchanged_watcher_lifecycle_batches_reads() {
        let mut measurements = Vec::new();
        for count in [16, 128] {
            let mut f = Fixture::new();
            f.assign("github").unwrap();
            f.store.db.execute("WITH RECURSIVE n(id) AS (VALUES(2) UNION ALL SELECT id+1 FROM n WHERE id<?1)
                INSERT INTO issues(project_id,number,title,body,state,assignee,created_by,created_at,updated_at,version,labels,sort_order,assignment_target)
                SELECT 'named:test',id,'Task '||id,'','open','watcher:github','human:test',0,0,1,'[]',id,'github' FROM n", [count]).unwrap();
            f.store.db.execute_batch("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at)
                SELECT project_id,number,'https://github.com/o/r/pull/'||number,'human:test',0 FROM issues WHERE number>1;
                INSERT INTO issue_github_watches SELECT project_id,number,json_object('prs',json_object('https://github.com/o/r/pull/'||number,json_object())) FROM issues WHERE number%2=0;").unwrap();
            let path = f.root.join("issues.db");
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            let (db, transport) = crate::database::tests::measured_connection(&path);
            let mut reader = Store::open(&path).unwrap();
            reader.replace_connection_for_test(db);
            reader
                .reconcile_github_assignments(f.request.actor.as_ref().unwrap())
                .unwrap();
            drop(reader);
            let (commands, steps) = transport.join().unwrap();
            owner.stop();
            assert_eq!(
                f.store
                    .db
                    .query_row(
                        "SELECT count(*) FROM issues WHERE assignment_target='github'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                count
            );
            eprintln!("{count} unchanged watchers: {commands} RPCs, {steps} query steps");
            measurements.push(commands);
        }
        assert!(
            measurements.iter().all(|count| *count <= 8),
            "Lifecycle reads grew per watcher: {measurements:?}"
        );
    }

    #[test]
    fn watcher_lifecycle_keeps_identical_issue_and_pr_keys_scoped_to_projects() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.store.db.execute_batch("INSERT INTO projects(id,name,next_number,hidden_at) VALUES('named:Other','Other',2,NULL),('named:Hidden','Hidden',2,1);
            INSERT INTO issues(project_id,number,title,body,state,assignee,created_by,created_at,updated_at,version,labels,sort_order,assignment_target)
            SELECT id,1,'Task','','open','watcher:github','human:test',0,0,1,'[]',1,'github' FROM projects WHERE id IN ('named:Other','named:Hidden');
            INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,status)
            VALUES('named:Other',1,'https://github.com/o/r/pull/1','human:test',0,'closed');").unwrap();
        f.store
            .reconcile_github_assignments(f.request.actor.as_ref().unwrap())
            .unwrap();
        let assignments: Vec<_> = f
            .store
            .db
            .prepare("SELECT project_id,assignment_target,assignee FROM issues ORDER BY project_id")
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            assignments,
            vec![
                (
                    "named:Hidden".into(),
                    Some("github".into()),
                    Some("watcher:github".into())
                ),
                ("named:Other".into(), None, Some("human:boss".into())),
                (
                    "named:test".into(),
                    Some("github".into()),
                    Some("watcher:github".into())
                ),
            ]
        );
    }

    #[test]
    fn assignment_metadata_does_not_read_large_issue_body_pages() {
        let f = Fixture::new();
        let db = &f.store.db;
        db.execute_batch("DROP INDEX issue_assignment_summary; CREATE INDEX issue_assignment_summary ON issues(project_id,number,assignment_target);
            INSERT INTO agents VALUES('worker','{\"machine\":\"remote\",\"host\":\"worker-host\"}',0);
            WITH RECURSIVE n(id) AS (VALUES(2) UNION ALL SELECT id+1 FROM n WHERE id<16)
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
            SELECT 'named:test',id,'Task '||id,'','open','human:test',0,0,1,'[]',id FROM n;
            UPDATE issues SET assignee='worker',assignment_target='machine:remote';").unwrap();
        migrate(db).unwrap();
        let numbers: Vec<i64> = (1..=16).collect();
        let read_pages = || {
            db.execute_batch("PRAGMA cache_size=-64; PRAGMA shrink_memory")
                .unwrap();
            let mut pages = 0;
            let mut high = 0;
            unsafe {
                assert_eq!(
                    rusqlite::ffi::sqlite3_db_status(
                        db.handle(),
                        rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                        &mut pages,
                        &mut high,
                        1
                    ),
                    rusqlite::ffi::SQLITE_OK
                );
            }
            let found = metadata(db, "named:test", &numbers).unwrap();
            assert_eq!(found.len(), 16);
            for value in found.values() {
                assert_eq!(value.target.as_deref(), Some("machine:remote"));
                assert_eq!(value.actor_machine.as_deref(), Some("remote"));
                assert_eq!(value.actor_host.as_deref(), Some("worker-host"));
            }
            unsafe {
                assert_eq!(
                    rusqlite::ffi::sqlite3_db_status(
                        db.handle(),
                        rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                        &mut pages,
                        &mut high,
                        0
                    ),
                    rusqlite::ffi::SQLITE_OK
                );
            }
            pages
        };
        let small = read_pages();
        db.execute("UPDATE issues SET body=?1", ["x".repeat(1024 * 1024)])
            .unwrap();
        let large = read_pages();
        eprintln!("Assignment metadata cache misses for empty/1-MiB bodies: {small}/{large}");
        assert!(
            large <= small + 16,
            "Assignment lookup read body overflow pages: {small} -> {large}"
        );
    }
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
    fn watcher_comments_record_new_events_once_and_survive_restart() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.observation(Some("failed-build"));
        let comments = f.call(json!({"action":"view","number":1})).unwrap();
        let comments = comments["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["author"], WATCHER);
        let body = comments[0]["body"].as_str().unwrap();
        assert!(body.contains("Required checks failed"), "{body}");
        assert!(body.contains("[o/r#1](https://github.com/o/r/pull/1)"));
        assert!(body.contains("head"));

        f.observation(Some("failed-build"));
        f.observation(None);
        f.store = Store::open(&f.root.join("issues.db")).unwrap();
        f.observation(Some("failed-build"));
        assert_eq!(
            f.call(json!({"action":"view","number":1})).unwrap()["comments"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        f.observation(Some("rerun-failed"));
        assert_eq!(
            f.call(json!({"action":"view","number":1})).unwrap()["comments"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn watcher_comments_and_wakeup_roll_back_together() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        let observation = hey_gh::watcher::Observation {
            head: "head".into(),
            blocking: vec!["failed".into()],
            completed: None,
            feedback: vec![],
            evidence: json!({}),
        };
        f.store.db.execute_batch("CREATE TRIGGER reject_watch_comment BEFORE INSERT ON comments BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
        assert!(
            f.store
                .record_github_observation("https://github.com/o/r/pull/1", &observation, 100)
                .is_err()
        );
        assert_eq!(
            get_issue(&f.store.db, "named:test", 1, false)
                .unwrap()
                .assignee
                .as_deref(),
            Some(WATCHER)
        );
        assert_eq!(
            f.store
                .db
                .query_row("SELECT count(*) FROM issue_github_signals", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        f.store
            .db
            .execute_batch("DROP TRIGGER reject_watch_comment")
            .unwrap();
        f.observation(Some("failed"));
        assert_eq!(
            f.call(json!({"action":"view","number":1})).unwrap()["comments"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn watcher_comments_only_describe_new_reasons_and_ignore_stale_polls() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.observation(Some("failed"));
        let mut observation = hey_gh::watcher::Observation {
            head: "head".into(),
            blocking: vec!["failed".into()],
            completed: Some("finished".into()),
            feedback: vec!["review".into()],
            evidence: json!({"required":[{"context":"Build","state":"failure"}]}),
        };
        f.store
            .record_github_observation("https://github.com/o/r/pull/1", &observation, 200)
            .unwrap();
        let view = f.call(json!({"action":"view","number":1})).unwrap();
        let comments = view["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2);
        let body = comments
            .iter()
            .map(|c| c["body"].as_str().unwrap())
            .find(|body| body.contains("CI finished"))
            .unwrap();
        assert!(body.contains("New feedback"));
        assert!(!body.contains("Required checks failed"));
        observation.feedback.push("late".into());
        f.store
            .record_github_observation("https://github.com/o/r/pull/1", &observation, 150)
            .unwrap();
        assert_eq!(
            f.call(json!({"action":"view","number":1})).unwrap()["comments"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let events: i64 = f
            .store
            .db
            .query_row(
                "SELECT count(*) FROM events WHERE actor=?1 AND action='commented'",
                [WATCHER],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 2);
    }

    #[test]
    fn manual_fetch_preserves_ownership_and_survives_an_inflight_request() {
        let mut f = Fixture::new();
        assert!(
            f.call(json!({"action":"refresh_github","number":1}))
                .is_err()
        );
        f.assign("github").unwrap();
        f.store
            .db
            .execute("UPDATE issues SET assignee='human:test' WHERE number=1", [])
            .unwrap();
        f.observation(None);
        let evidence_before = saved(&f.store.db, "named:test", 1).unwrap().1;
        let before = f.call(json!({"action":"view","number":1})).unwrap();
        let result = f
            .call(json!({"action":"refresh_github","number":1}))
            .unwrap();
        assert_eq!(result["issue"]["version"], before["issue"]["version"]);
        assert_eq!(result["issue"]["assignee"], before["issue"]["assignee"]);
        let url = "https://github.com/o/r/pull/1";
        let now = crate::issues::worker::now();
        let (started, force) = f.store.begin_github_fetch(url, now).unwrap();
        assert!(force);
        f.call(json!({"action":"refresh_github","number":1}))
            .unwrap();
        f.store
            .finish_github_fetch(url, started, now + 1, now + 30000, None)
            .unwrap();
        assert!(f.store.requested_github_fetches().unwrap().contains(url));
        let (newer, force) = f.store.begin_github_fetch(url, now + 2).unwrap();
        assert!(force);
        f.store
            .finish_github_fetch(url, started, now + 3, 0, Some("old failure"))
            .unwrap();
        f.store
            .finish_github_fetch(url, newer, newer + 4, newer + 30000, None)
            .unwrap();
        f.observation(None);
        assert_eq!(
            saved(&f.store.db, "named:test", 1).unwrap().1,
            evidence_before
        );
        assert!(f.store.requested_github_fetches().unwrap().is_empty());
        let result = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(
            result["issue"]["github_status"]["fetches"][url]["finished_at"],
            newer + 4
        );
        assert!(result["issue"]["github_status"]["fetches"][url]["error"].is_null());
        assert_eq!(result["issue"]["version"], before["issue"]["version"]);
    }

    #[test]
    fn trailing_slash_aliases_share_event_history_after_either_link_is_removed() {
        let url = "https://github.com/o/r/pull/1";
        let alias = "https://github.com/o/r/pull/1/";
        for (removed, remaining) in [(url, alias), (alias, url)] {
            for legacy in [false, true] {
                let mut f = Fixture::new();
                f.call(json!({"action":"add_pull_request","number":1,"url":alias,"purpose":"fix"}))
                    .unwrap();
                f.assign("github").unwrap();
                let observation = hey_gh::watcher::Observation {
                    head: "head".into(),
                    blocking: vec!["failure".into()],
                    completed: None,
                    feedback: Vec::new(),
                    evidence: json!({"head":"head","complete":false}),
                };
                f.store
                    .record_github_observation(alias, &observation, 100)
                    .unwrap();
                if legacy {
                    f.store
                        .db
                        .execute("UPDATE issue_github_signals SET url=?1", [alias])
                        .unwrap();
                }
                let event = saved(&f.store.db, "named:test", 1).unwrap().1["event"].clone();
                f.assign("github").unwrap();
                f.store
                    .record_github_observation(url, &observation, 101)
                    .unwrap();
                assert_eq!(
                    saved(&f.store.db, "named:test", 1).unwrap().1["event"],
                    event
                );
                assert_eq!(
                    get_issue(&f.store.db, "named:test", 1, false)
                        .unwrap()
                        .assignee
                        .as_deref(),
                    Some(WATCHER)
                );
                f.call(json!({"action":"remove_pull_request","number":1,"url":removed}))
                    .unwrap();
                f.store
                    .record_github_observation(remaining, &observation, 102)
                    .unwrap();
                assert_eq!(
                    saved(&f.store.db, "named:test", 1).unwrap().1["event"],
                    event
                );
                assert_eq!(
                    get_issue(&f.store.db, "named:test", 1, false)
                        .unwrap()
                        .assignee
                        .as_deref(),
                    Some(WATCHER)
                );
            }
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
    fn many_prs_have_bounded_claim_evidence_with_trigger_and_failure_totals_retained() {
        let mut status =
            json!({"event":"new","trigger":{"url":"https://github.com/o/r/pull/40"},"prs":{}});
        for number in 1..=40 {
            status["prs"][format!("https://github.com/o/r/pull/{number}")] = json!({"head":"head","checked_at":123,
                "evidence":{"repository":"o/r","number":number,"complete":true,"required_state":"failure",
                    "required_counts":{"total":200,"failure":100},"required":[{"context":"test","state":"failure"}],
                    "reviews":(0..30).map(|id|json!({"id":id,"body":"x".repeat(1800)})).collect::<Vec<_>>()}});
        }
        let public = public_status(status.clone(), true);
        assert!(public.to_string().len() <= 128 * 1024);
        assert_eq!(public["prs"].as_object().unwrap().len(), 40);
        assert_eq!(
            public["prs"]["https://github.com/o/r/pull/40"]["evidence"]["reviews"],
            status["prs"]["https://github.com/o/r/pull/40"]["evidence"]["reviews"]
        );
        for snapshot in public["prs"].as_object().unwrap().values() {
            assert_eq!(snapshot["evidence"]["required_counts"]["failure"], 100);
        }
        assert!(
            public["prs"]
                .as_object()
                .unwrap()
                .values()
                .any(|snapshot| snapshot["evidence"]["truncated"] == true)
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
        f.observation(None);
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
            resumed["issue"]["github_status"]["prs"]
                .get("https://github.com/o/r/pull/1")
                .is_none()
        );
        assert!(
            resumed["issue"]["github_status"]
                .get("stopped_reason")
                .is_none()
        );
    }
    #[test]
    fn removing_last_pr_from_blocked_work_stops_monitoring_without_lifting_the_hold() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.observation(None);
        f.call(
            json!({"action":"block","number":1,"force":true,"comment":"Synthetic dependency hold"}),
        )
        .unwrap();
        f.call(json!({"action":"remove_pull_request","number":1,"url":"https://github.com/o/r/pull/1"})).unwrap();
        let view = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(view["issue"]["state"], "blocked");
        assert_eq!(view["issue"]["manual_blocked"], true);
        assert!(view["issue"]["assignee"].is_null());
        assert_eq!(view["issue"]["github_status"]["monitoring"], false);
        assert_eq!(
            view["issue"]["github_status"]["stopped_reason"],
            "no_open_pull_requests"
        );
        f.store
            .reconcile_github_assignments(f.request.actor.as_ref().unwrap())
            .unwrap();
    }

    #[test]
    fn removing_one_watched_pr_prunes_its_snapshot_and_signal_history() {
        let mut f = Fixture::new();
        let removed = "https://github.com/o/r/pull/1";
        f.assign("github").unwrap();
        f.observation(Some("first"));
        f.call(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/2","purpose":"fix"})).unwrap();
        f.call(json!({"action":"remove_pull_request","number":1,"url":removed}))
            .unwrap();
        let view = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(view["issue"]["assignment"]["kind"], "github");
        assert!(view["issue"]["github_status"]["prs"].get(removed).is_none());
        assert_eq!(
            f.store
                .db
                .query_row(
                    "SELECT count(*) FROM issue_github_signals WHERE url=?1",
                    [removed],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        f.observation(Some("late-removed"));
        assert!(
            saved(&f.store.db, "named:test", 1).unwrap().1["prs"]
                .get(removed)
                .is_none()
        );
    }

    #[test]
    fn manual_destination_keeps_previously_observed_status_without_a_wake_event() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.observation(None);
        f.assign("unassigned").unwrap();
        let claim = f
            .call(json!({"action":"claim","number":1,"force":false}))
            .unwrap();
        assert_eq!(claim["issue"]["github_status"]["monitoring"], false);
        assert_eq!(
            claim["issue"]["github_status"]["prs"]["https://github.com/o/r/pull/1"]["head"],
            "head"
        );
    }

    #[test]
    fn terminal_pr_reconciliation_preserves_an_active_claim_and_other_open_prs() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.observation(None);
        f.call(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/2","purpose":"fix"})).unwrap();
        f.store
            .record_pr_status("https://github.com/o/r/pull/1", Some("closed"), 100, None)
            .unwrap();
        f.store
            .reconcile_github_assignments(f.request.actor.as_ref().unwrap())
            .unwrap();
        assert_eq!(
            saved(&f.store.db, "named:test", 1).unwrap().1["prs"]["https://github.com/o/r/pull/1"]
                ["lifecycle"],
            "closed"
        );
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
    fn manual_ready_returns_to_watcher_unless_new_findings_have_not_been_claimed() {
        for late in [false, true] {
            let mut f = Fixture::new();
            f.assign("github").unwrap();
            f.observation(Some("first"));
            f.call(json!({"action":"claim","number":1,"force":false}))
                .unwrap();
            if late {
                f.observation(Some("later"));
                let before = f.call(json!({"action":"view","number":1})).unwrap();
                let error = f
                    .call(json!({"action":"ready","number":1,"force":false}))
                    .unwrap_err();
                assert!(error.message.contains("unacknowledged GitHub"));
                assert_eq!(f.call(json!({"action":"view","number":1})).unwrap(), before);
                let guard = before["ready_guard"].clone();
                f.request.request_id = Some("reviewed-ready".into());
                let ready = f
                    .call(json!({"action":"ready","number":1,"force":false,"guard":guard}))
                    .unwrap();
                assert_eq!(ready["issue"]["state"], "ready");
                assert_eq!(ready["issue"]["assignee"], WATCHER);
                f.request.request_id = None;
                continue;
            }
            let result = f
                .call(json!({"action":"ready","number":1,"force":false}))
                .unwrap();
            assert_eq!(result["issue"]["state"], "ready");
            assert_eq!(result["issue"]["assignee"], json!(WATCHER));
            assert_eq!(result["issue"]["assignment"]["kind"], "github");
            assert_eq!(result["prs_enabled"], false);
        }
    }
    #[test]
    fn guarded_idle_ready_acknowledges_only_the_reviewed_watcher_event() {
        let mut f = Fixture::new();
        f.assign("github").unwrap();
        f.observation(Some("first"));
        let before = f.call(json!({"action":"view","number":1})).unwrap();
        assert!(before["issue"]["assignee"].is_null());
        f.observation(Some("second"));
        let current = f.call(json!({"action":"view","number":1})).unwrap();
        assert_eq!(before["issue"]["version"], current["issue"]["version"]);
        f.request.request_id = Some("idle-ready".into());
        let error = f
            .call(json!({"action":"ready","number":1,"force":false,"guard":before["ready_guard"]}))
            .unwrap_err();
        assert!(error.message.contains("reservation guard mismatch"));
        let ready = f
            .call(json!({"action":"ready","number":1,"force":false,"guard":current["ready_guard"]}))
            .unwrap();
        assert_eq!(ready["issue"]["state"], "ready");
        assert_eq!(ready["issue"]["assignee"], WATCHER);
        assert_eq!(
            f.store
                .db
                .query_row(
                    "SELECT count(*) FROM events WHERE action='claimed'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
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
        let mut writer = Connection::open(f.root.join("issues.db")).unwrap();
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
        let mut writer = Connection::open(f.root.join("issues.db")).unwrap();
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
        let mut writer = Connection::open(f.root.join("issues.db")).unwrap();
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
