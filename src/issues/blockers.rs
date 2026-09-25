//! Linked dependencies and subtasks share the Blocked lifecycle.
use super::{Error, Result};
use crate::database::Connection;
use rusqlite::params;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn migrate(db: &mut Connection) -> Result<()> {
    let present: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('issues') WHERE name='blockers')",
        [],
        |r| r.get(0),
    )?;
    let stale_capture: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name LIKE 'fleet_capture_issues_%' AND instr(sql,'''blockers'',')=0)",
        [], |r| r.get(0),
    )?;
    if present && !stale_capture {
        return Ok(());
    }
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let added = !tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('issues') WHERE name='blockers')",
        [],
        |r| r.get::<_, bool>(0),
    )?;
    if added {
        tx.execute_batch(
            "ALTER TABLE issues ADD COLUMN manual_blocked INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE issues ADD COLUMN blockers TEXT NOT NULL DEFAULT '[]';",
        )?;
    }
    // Existing capture triggers have a fixed column list. Replace them in
    // this transaction before normalization writes enter the sync journal.
    let triggers = tx.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND name LIKE 'fleet_capture_issues_%'")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    for (name, sql) in triggers {
        if sql.contains("'blockers',") {
            continue;
        }
        let sql = sql
                .replace("json_object('origin',NEW.", "json_object('manual_blocked',NEW.manual_blocked,'blockers',NEW.blockers,'origin',NEW.")
                .replace("json_object('origin',OLD.", "json_object('manual_blocked',OLD.manual_blocked,'blockers',OLD.blockers,'origin',OLD.")
                .replace("json_object('project_id',NEW.", "json_object('manual_blocked',NEW.manual_blocked,'blockers',NEW.blockers,'project_id',NEW.")
                .replace("json_object('project_id',OLD.", "json_object('manual_blocked',OLD.manual_blocked,'blockers',OLD.blockers,'project_id',OLD.")
                .replace(" AND NOT (", " AND NOT (OLD.manual_blocked IS NEW.manual_blocked AND OLD.blockers IS NEW.blockers AND ");
        tx.execute_batch(&format!("DROP TRIGGER {name}; {sql}"))?;
    }
    if added {
        tx.execute_batch("UPDATE issues SET manual_blocked=1 WHERE state='blocked';
            UPDATE issues SET state='blocked',manual_blocked=1,assignee=NULL,version=version+1
            WHERE state='open' AND deleted_at IS NULL
            AND NOT EXISTS(SELECT 1 FROM worker_runs live WHERE live.project_id=issues.project_id AND live.issue_number=issues.number AND live.finished_at IS NULL)
            AND EXISTS(SELECT 1 FROM worker_runs r WHERE r.id=(SELECT latest.id FROM worker_runs latest WHERE latest.project_id=issues.project_id AND latest.issue_number=issues.number AND latest.finished_at IS NOT NULL ORDER BY finished_at DESC,started_at DESC,id DESC LIMIT 1) AND r.retry_allowed=0 AND r.summary LIKE 'Codex needs input or approval:%');")?;
        reconcile_all(&tx)?;
    }
    tx.commit()?;
    Ok(())
}

struct Graph {
    satisfied: RefCell<BTreeMap<i64, bool>>,
    prs_enabled: bool,
    issues: BTreeMap<i64, Value>,
    children: BTreeMap<i64, Vec<i64>>,
    parents: BTreeMap<i64, i64>,
    previous: BTreeMap<i64, Vec<i64>>,
    links: BTreeMap<i64, Vec<i64>>,
}
impl Graph {
    fn load(db: &Connection, project: &str) -> Result<Self> {
        let mut graph = Self {
            satisfied: RefCell::new(BTreeMap::new()),
            prs_enabled: db.query_row("SELECT EXISTS(SELECT 1 FROM project_settings WHERE project_id=?1 AND prs_enabled=1)", [project], |r| r.get(0))?,
            issues: BTreeMap::new(),
            children: BTreeMap::new(),
            parents: BTreeMap::new(),
            previous: BTreeMap::new(),
            links: BTreeMap::new(),
        };
        let mut stmt = db.prepare("SELECT number,title,state,deleted_at,manual_blocked,blockers,created_by,draft,assignee,EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=issues.project_id AND r.issue_number=issues.number AND r.finished_at IS NULL),version FROM issues WHERE project_id=?1 ORDER BY sort_order,number")?;
        for row in stmt.query_map([project], |r| Ok((r.get::<_,i64>(0)?, json!({"number":r.get::<_,i64>(0)?,"title":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,"deleted_at":r.get::<_,Option<i64>>(3)?,"manual_blocked":r.get::<_,bool>(4)?,"created_by":r.get::<_,String>(6)?,"draft":r.get::<_,bool>(7)?,"assignee":r.get::<_,Option<String>>(8)?,"reserved":r.get::<_,bool>(9)?,"version":r.get::<_,i64>(10)?}), r.get::<_,String>(5)?)))? {
            let (n, issue, links) = row?;
            graph.links.insert(n, serde_json::from_str(&links)?);
            graph.issues.insert(n, issue);
        }
        let mut stmt = db.prepare("SELECT r.parent_number,r.child_number FROM issue_subtasks r JOIN issues i ON i.project_id=r.project_id AND i.number=r.child_number WHERE r.project_id=?1 ORDER BY i.sort_order,i.number")?;
        for row in stmt.query_map([project], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (parent, child) = row?;
            graph.parents.insert(child, parent);
            graph.children.entry(parent).or_default().push(child);
        }
        for (&parent, children) in &graph.children {
            if !graph.issues[&parent]["deleted_at"].is_null() {
                continue;
            }
            let mut previous = Vec::new();
            for &child in children {
                if graph.issues[&child]["deleted_at"].is_null() {
                    graph.previous.insert(child, previous.clone());
                    previous.push(child);
                }
            }
        }
        let mut prs = db.prepare("SELECT issue_number,url,purpose,status FROM issue_pull_requests WHERE project_id=?1 ORDER BY created_at,url")?;
        for row in prs.query_map([project], |r| Ok((r.get::<_,i64>(0)?, json!({"url":r.get::<_,String>(1)?,"purpose":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?}))))? {
            let (n, pr) = row?;
            if let Some(issue) = graph.issues.get_mut(&n) {
                if !issue["pull_requests"].is_array() { issue["pull_requests"] = json!([]); }
                issue["pull_requests"].as_array_mut().unwrap().push(pr);
            }
        }
        Ok(graph)
    }
    // Include preceding branches at every level, so a nested leaf cannot
    // overtake its parent's previous sibling. Deleted ancestors detach work.
    fn sequence_roots(&self, mut n: i64) -> Vec<i64> {
        let mut roots = Vec::new();
        loop {
            if self
                .issues
                .get(&n)
                .is_none_or(|i| !i["deleted_at"].is_null())
            {
                break;
            }
            roots.extend(self.previous.get(&n).into_iter().flatten());
            let Some(parent) = self.parents.get(&n) else {
                break;
            };
            n = *parent;
        }
        roots
    }
    fn unfinished(&self, n: i64) -> bool {
        if let Some(done) = self.satisfied.borrow().get(&n) {
            return !done;
        }
        // Legacy links may oppose newly introduced sibling order. Treat cycles
        // as unfinished so startup can preserve and display the work for repair.
        self.satisfied.borrow_mut().insert(n, false);
        let unfinished = self.issues.get(&n).is_none_or(|i| {
            i["deleted_at"].is_null()
                && i["state"] != "closed"
                && !(self.prs_enabled && i["state"] == "ready" && self.active(n).is_empty())
        });
        self.satisfied.borrow_mut().insert(n, !unfinished);
        unfinished
    }
    fn descendants(&self, n: i64) -> BTreeSet<i64> {
        let mut found = BTreeSet::new();
        let mut todo = self.children.get(&n).cloned().unwrap_or_default();
        while let Some(child) = todo.pop() {
            if self
                .issues
                .get(&child)
                .is_none_or(|i| !i["deleted_at"].is_null())
                || !found.insert(child)
            {
                continue;
            }
            todo.extend(self.children.get(&child).into_iter().flatten());
        }
        found
    }
    fn active(&self, n: i64) -> BTreeMap<i64, &'static str> {
        let mut result = BTreeMap::new();
        for child in self.descendants(n) {
            if self.unfinished(child) {
                result.insert(child, "subtask");
            }
        }
        for root in self.sequence_roots(n) {
            for previous in std::iter::once(root).chain(self.descendants(root)) {
                if self.unfinished(previous) {
                    result.insert(previous, "previous_subtask");
                }
            }
        }
        for &blocker in self.links.get(&n).into_iter().flatten() {
            if self.unfinished(blocker) {
                result.insert(blocker, "linked");
            }
        }
        result
    }
    fn validate_subtask_claims(&self) -> Result<()> {
        for (&number, issue) in &self.issues {
            if issue["assignee"].is_null()
                || !issue["deleted_at"].is_null()
                || issue["state"] == "closed"
            {
                continue;
            }
            let blocked = issue["manual_blocked"] == true
                || (issue["draft"] != true && !self.active(number).is_empty());
            if (issue["state"] == "blocked") != blocked {
                return Err(Error::new(
                    "subtask_claim_conflict",
                    format!(
                        "Cannot change subtasks: issue #{number} has an existing claim that this change would release. Use mindmap nesting for organization without changing ownership, or have the owner explicitly unassign the affected issue first."
                    ),
                ));
            }
        }
        Ok(())
    }
    fn reference(&self, n: i64, source: &str) -> Value {
        let mut issue = self.issues.get(&n).cloned().unwrap_or_else(
            || json!({"number":n,"title":"Issue unavailable","state":"blocked","deleted_at":null}),
        );
        issue.as_object_mut().unwrap().remove("manual_blocked");
        issue.as_object_mut().unwrap().remove("created_by");
        issue.as_object_mut().unwrap().remove("assignee");
        issue.as_object_mut().unwrap().remove("reserved");
        issue.as_object_mut().unwrap().remove("version");
        issue["source"] = json!(source);
        issue
    }
    fn validate_edges(&self) -> Result<()> {
        for (&number, links) in &self.links {
            for &target in links {
                self.validate_edge(number, target)?;
            }
        }
        Ok(())
    }
    fn validate_edge(&self, source: i64, target: i64) -> Result<()> {
        if source == target {
            return Err(Error::invalid("An issue cannot block itself"));
        }
        let mut seen = BTreeSet::new();
        let mut todo = vec![target];
        while let Some(n) = todo.pop() {
            if n == source {
                return Err(Error::invalid("This dependency would create a cycle"));
            }
            if !seen.insert(n) {
                continue;
            }
            todo.extend(self.children.get(&n).into_iter().flatten());
            todo.extend(self.sequence_roots(n));
            todo.extend(self.links.get(&n).into_iter().flatten());
        }
        Ok(())
    }
}

pub(crate) fn validate_links(
    db: &Connection,
    project: &str,
    number: i64,
    links: &[i64],
) -> Result<()> {
    if links.len() > 100
        || links.iter().any(|n| *n < 1)
        || links.iter().collect::<BTreeSet<_>>().len() != links.len()
    {
        return Err(Error::invalid(
            "Use up to 100 different positive blocker issue numbers",
        ));
    }
    let graph = Graph::load(db, project)?;
    for &target in links {
        if graph
            .issues
            .get(&target)
            .is_none_or(|i| !i["deleted_at"].is_null())
        {
            return Err(Error::new(
                "not_found",
                format!("Blocker issue #{target} was not found in this project"),
            ));
        }
        graph.validate_edge(number, target)?;
    }
    Ok(())
}
pub(super) fn validate_subtask(
    db: &Connection,
    project: &str,
    parent: i64,
    child: i64,
) -> Result<()> {
    Graph::load(db, project)?.validate_edge(parent, child)
}
pub(super) fn has_dependencies(db: &Connection, project: &str, number: i64) -> Result<bool> {
    Ok(!Graph::load(db, project)?.active(number).is_empty())
}

/// Validate an incoming fleet relationship inside its savepoint, before the
/// batch reconciler can release a claim acquired while the replica was offline.
pub(crate) fn validate_subtask_claims(db: &Connection, project: &str) -> Result<()> {
    Graph::load(db, project)?.validate_subtask_claims()
}

/// One graph snapshot per mutation. Reads never take a writer lock; only actual
/// transitions advance versions and enter the fleet journal.
pub(super) fn reconcile(
    db: &Connection,
    project: &str,
    actor: Option<&str>,
    now: i64,
) -> Result<()> {
    reconcile_graph(db, project, actor, now, false, false)
}

/// Reordering must not retroactively block a claimed or reserved branch.
pub(super) fn reconcile_sequence_change(
    db: &Connection,
    project: &str,
    actor: Option<&str>,
    now: i64,
) -> Result<()> {
    let graph = Graph::load(db, project)?;
    graph.validate_edges()?;
    for (&n, issue) in &graph.issues {
        if issue["state"] == "open"
            && issue["deleted_at"].is_null()
            && (!issue["assignee"].is_null() || issue["reserved"] == true)
            && graph
                .active(n)
                .values()
                .any(|source| *source == "previous_subtask")
        {
            return Err(Error::conflict(format!(
                "Cannot reorder subtasks: issue #{n} is claimed or reserved. Finish or release that work first."
            )));
        }
    }
    reconcile(db, project, actor, now)
}

/// Upstream rework pauses future pickups without taking running work away.
pub(super) fn reconcile_rework(
    db: &Connection,
    project: &str,
    actor: Option<&str>,
    now: i64,
) -> Result<()> {
    reconcile_graph(db, project, actor, now, false, true)
}

/// Subtask organization must preserve all existing parent and ancestor claims.
pub(super) fn reconcile_subtasks(
    db: &Connection,
    project: &str,
    actor: Option<&str>,
    now: i64,
) -> Result<()> {
    let graph = Graph::load(db, project)?;
    graph.validate_edges()?;
    graph.validate_subtask_claims()?;
    reconcile(db, project, actor, now)
}

fn reconcile_graph(
    db: &Connection,
    project: &str,
    actor: Option<&str>,
    now: i64,
    upgrading: bool,
    rework: bool,
) -> Result<()> {
    let graph = Graph::load(db, project)?;
    for (&number, issue) in &graph.issues {
        if !issue["deleted_at"].is_null() || issue["state"] == "closed" {
            continue;
        }
        let blockers = graph.active(number);
        let state = if issue["manual_blocked"] == true
            || (issue["draft"] != true && !blockers.is_empty())
        {
            "blocked"
        } else if issue["state"] == "ready" {
            "ready"
        } else {
            "open"
        };
        if issue["state"] == state {
            continue;
        }
        if (rework || graph.prs_enabled)
            && state == "blocked"
            && !blockers.is_empty()
            && (issue["state"] == "ready"
                || !issue["assignee"].is_null()
                || issue["reserved"] == true)
        {
            let dependencies = json!({"dependencies":blockers.keys().map(|n| json!([n,graph.issues.get(n).map(|i| &i["version"])])).collect::<Vec<_>>()});
            let signature = serde_json::to_string(&dependencies)?;
            let notified: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND issue_number=?2 AND action='dependency_rework' AND data=?3)", params![project,number,signature], |r| r.get(0))?;
            if !notified {
                let author = actor.unwrap_or(issue["created_by"].as_str().unwrap());
                let body = format!(
                    "Dependency rework: upstream tasks {:?} need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies.",
                    blockers.keys().collect::<Vec<_>>()
                );
                db.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?1,?2,?3,?4,?5)", params![project,number,author,body,now])?;
                let id = db.last_insert_rowid();
                super::store::event(
                    db,
                    project,
                    number,
                    author,
                    "commented",
                    now,
                    &json!({"comment_id":id,"body":body}),
                )?;
                super::store::event(
                    db,
                    project,
                    number,
                    author,
                    "dependency_rework",
                    now,
                    &dependencies,
                )?;
                db.execute("INSERT OR IGNORE INTO agent_steering(request_id,run_id,scope,text,created_at) SELECT ?1||':'||id,id,'dependency',?2,?3 FROM worker_runs WHERE project_id=?4 AND issue_number=?5 AND finished_at IS NULL", params![format!("dependency-rework-{id}"),body,now,project,number])?;
            }
        }
        if issue["state"] != "ready" && (!issue["assignee"].is_null() || issue["reserved"] == true)
        {
            // An upgrade lets existing work drain without stealing ownership.
            // The readiness view already excludes later branches from new pickup.
            if upgrading
                || rework
                || graph.prs_enabled
                || blockers.values().any(|s| *s == "previous_subtask")
            {
                continue;
            }
        }
        db.execute("UPDATE issues SET state=?3,assignee=NULL,version=version+1,updated_at=?4 WHERE project_id=?1 AND number=?2", params![project,number,state,now])?;
        super::store::event(
            db,
            project,
            number,
            actor.unwrap_or(issue["created_by"].as_str().unwrap()),
            if state == "blocked" {
                "blocked"
            } else {
                "reopened"
            },
            now,
            &json!({"blocked_by":blockers.keys().collect::<Vec<_>>()}),
        )?;
    }
    Ok(())
}
pub(crate) fn reconcile_all(db: &Connection) -> Result<()> {
    reconcile_projects(db, false)
}

pub(super) fn reconcile_sequence_upgrade(db: &Connection) -> Result<()> {
    reconcile_projects(db, true)
}

fn reconcile_projects(db: &Connection, upgrading: bool) -> Result<()> {
    let mut stmt = db.prepare("SELECT id FROM projects")?;
    let projects = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    for project in projects {
        reconcile_graph(db, &project, None, now, upgrading, false)?;
    }
    Ok(())
}
pub(super) fn enrich(db: &Connection, project: &str, result: &mut Value) -> Result<()> {
    if !["issue", "parent_issue", "child_issue", "issues", "subtasks"]
        .iter()
        .any(|key| {
            result
                .get(*key)
                .is_some_and(|v| v.is_object() || v.is_array())
        })
    {
        return Ok(());
    }
    let graph = Graph::load(db, project)?;
    let attach = |issue: &mut Value| {
        let Some(n) = issue["number"].as_i64() else {
            return;
        };
        let mut dependencies = BTreeMap::new();
        for root in graph.sequence_roots(n) {
            for prior in std::iter::once(root).chain(graph.descendants(root)) {
                dependencies.insert(prior, "previous_subtask");
            }
        }
        for &linked in graph.links.get(&n).into_iter().flatten() {
            dependencies.insert(linked, "linked");
        }
        issue["dependency_context"] = json!(
            dependencies
                .into_iter()
                .map(|(n, source)| graph.reference(n, source))
                .collect::<Vec<_>>()
        );
        issue["dependency_ready_state"] = json!(if graph.prs_enabled { "ready" } else { "closed" });
        issue["blocked_by"] = json!(
            graph
                .active(n)
                .into_iter()
                .map(|(n, source)| graph.reference(n, source))
                .collect::<Vec<_>>()
        );
        issue["blocker_links"] = json!(
            graph
                .links
                .get(&n)
                .into_iter()
                .flatten()
                .map(|&n| graph.reference(n, "linked"))
                .collect::<Vec<_>>()
        );
    };
    for key in ["issue", "parent_issue", "child_issue"] {
        if let Some(i) = result.get_mut(key) {
            attach(i);
        }
    }
    for key in ["issues", "subtasks"] {
        if let Some(issues) = result[key].as_array_mut() {
            for i in issues {
                attach(i);
            }
        }
    }
    Ok(())
}
