//! Linked dependencies and subtasks share the Blocked lifecycle.
use super::{Error, Result};
use crate::database::Connection;
use rusqlite::params;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
thread_local! {
    static DESCENDANT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static VALIDATION_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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
    links: BTreeMap<i64, Vec<i64>>,
    dependents: BTreeMap<i64, Vec<(i64, &'static str)>>,
    active_cache: RefCell<BTreeMap<i64, BTreeMap<i64, &'static str>>>,
}
impl Graph {
    fn load(db: &Connection, project: &str) -> Result<Self> {
        let mut graph = Self {
            satisfied: RefCell::new(BTreeMap::new()),
            prs_enabled: db.query_row("SELECT EXISTS(SELECT 1 FROM project_settings WHERE project_id=?1 AND prs_enabled=1)", [project], |r| r.get(0))?,
            issues: BTreeMap::new(),
            children: BTreeMap::new(),
            links: BTreeMap::new(),
            dependents: BTreeMap::new(),
            active_cache: RefCell::new(BTreeMap::new()),
        };
        let mut stmt = db.prepare("SELECT number,title,state,deleted_at,manual_blocked,blockers,created_by,draft,assignee,EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=issues.project_id AND r.issue_number=issues.number AND r.finished_at IS NULL),version FROM issues WHERE project_id=?1 ORDER BY sort_order,number")?;
        for row in stmt.query_map([project], |r| Ok((r.get::<_,i64>(0)?, json!({"number":r.get::<_,i64>(0)?,"title":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,"deleted_at":r.get::<_,Option<i64>>(3)?,"manual_blocked":r.get::<_,bool>(4)?,"created_by":r.get::<_,String>(6)?,"draft":r.get::<_,bool>(7)?,"assignee":r.get::<_,Option<String>>(8)?,"reserved":r.get::<_,bool>(9)?,"version":r.get::<_,i64>(10)?}), r.get::<_,String>(5)?)))? {
            let (n, issue, links) = row?;
            let parsed_links: Vec<i64> = serde_json::from_str(&links)?;
            if issue["deleted_at"].is_null() {
                for &target in &parsed_links {
                    graph.dependents.entry(target).or_default().push((n, "linked"));
                }
            }
            graph.links.insert(n, parsed_links);
            graph.issues.insert(n, issue);
        }
        let mut stmt = db.prepare("SELECT r.parent_number,r.child_number FROM issue_subtasks r JOIN issues i ON i.project_id=r.project_id AND i.number=r.child_number WHERE r.project_id=?1 ORDER BY i.sort_order,i.number")?;
        for row in stmt.query_map([project], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (parent, child) = row?;
            if graph
                .issues
                .get(&parent)
                .is_some_and(|i| i["deleted_at"].is_null())
                && graph
                    .issues
                    .get(&child)
                    .is_some_and(|i| i["deleted_at"].is_null())
            {
                graph
                    .dependents
                    .entry(child)
                    .or_default()
                    .push((parent, "subtask"));
            }
            graph.children.entry(parent).or_default().push(child);
        }
        Ok(graph)
    }
    fn load_prs(
        &mut self,
        db: &Connection,
        project: &str,
        references: &BTreeSet<i64>,
    ) -> Result<()> {
        if references.is_empty() {
            return Ok(());
        }
        // Drive indexed lookups from the referenced issues; a project's older
        // PR history must not add work to an unrelated dependency response.
        let mut prs = db.prepare("SELECT pr.issue_number,pr.url,pr.purpose,pr.status FROM json_each(?2) referenced CROSS JOIN issue_pull_requests pr WHERE pr.project_id=?1 AND pr.issue_number=referenced.value ORDER BY pr.created_at,pr.url")?;
        for row in prs.query_map(params![project, serde_json::to_string(references)?], |r| Ok((r.get::<_,i64>(0)?, json!({"url":r.get::<_,String>(1)?,"purpose":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?}))))? {
            let (n, pr) = row?;
            if let Some(issue) = self.issues.get_mut(&n) {
                if !issue["pull_requests"].is_array() { issue["pull_requests"] = json!([]); }
                issue["pull_requests"].as_array_mut().unwrap().push(pr);
            }
        }
        Ok(())
    }
    fn unfinished(&self, n: i64) -> bool {
        if let Some(done) = self.satisfied.borrow().get(&n) {
            return !done;
        }
        // Treat cycles as unfinished so damaged legacy data remains repairable.
        self.satisfied.borrow_mut().insert(n, false);
        let unfinished = self.issues.get(&n).is_none_or(|i| {
            i["deleted_at"].is_null()
                && i["state"] != "closed"
                && !(i["state"] == "ready" && !self.has_active(n))
        });
        self.satisfied.borrow_mut().insert(n, !unfinished);
        unfinished
    }
    fn has_active(&self, n: i64) -> bool {
        if let Some(cached) = self.active_cache.borrow().get(&n) {
            return !cached.is_empty();
        }
        let mut seen = BTreeSet::new();
        let mut todo = self.children.get(&n).cloned().unwrap_or_default();
        while let Some(child) = todo.pop() {
            #[cfg(test)]
            DESCENDANT_VISITS.with(|count| count.set(count.get() + 1));
            let Some(issue) = self.issues.get(&child) else {
                continue;
            };
            if !issue["deleted_at"].is_null() || !seen.insert(child) {
                continue;
            }
            if self.unfinished(child) {
                return true;
            }
            // A satisfied Ready child already checked its whole subtree.
            // Closed children satisfy links, but unfinished descendants still
            // block their ancestors. Keep traversing through those children.
            if issue["state"] == "closed" {
                todo.extend(self.children.get(&child).into_iter().flatten());
            }
        }
        self.links
            .get(&n)
            .into_iter()
            .flatten()
            .any(|&blocker| self.unfinished(blocker))
    }
    fn descendants(&self, n: i64) -> BTreeSet<i64> {
        let mut found = BTreeSet::new();
        let mut todo = self.children.get(&n).cloned().unwrap_or_default();
        while let Some(child) = todo.pop() {
            #[cfg(test)]
            DESCENDANT_VISITS.with(|count| count.set(count.get() + 1));
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
        if let Some(cached) = self.active_cache.borrow().get(&n) {
            return cached.clone();
        }
        let mut result = BTreeMap::new();
        for child in self.descendants(n) {
            if self.unfinished(child) {
                result.insert(child, "subtask");
            }
        }
        for &blocker in self.links.get(&n).into_iter().flatten() {
            if self.unfinished(blocker) {
                result.insert(blocker, "linked");
            }
        }
        self.active_cache.borrow_mut().insert(n, result.clone());
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
                || (issue["draft"] != true && self.has_active(number));
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
        if self.links.values().all(Vec::is_empty) {
            return Ok(());
        }
        // Two iterative passes find strongly connected components without
        // walking the same dependency chain once for every declared link.
        let mut seen = BTreeSet::new();
        let mut order = Vec::new();
        for &root in self.children.keys().chain(self.links.keys()) {
            if seen.contains(&root) {
                continue;
            }
            let mut todo = vec![(root, false)];
            while let Some((number, finished)) = todo.pop() {
                #[cfg(test)]
                VALIDATION_VISITS.with(|count| count.set(count.get() + 1));
                if finished {
                    order.push(number);
                } else if seen.insert(number) {
                    todo.push((number, true));
                    todo.extend(
                        self.children
                            .get(&number)
                            .into_iter()
                            .flatten()
                            .chain(self.links.get(&number).into_iter().flatten())
                            .map(|&target| (target, false)),
                    );
                }
            }
        }
        let mut reverse: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for (&source, targets) in self.children.iter().chain(&self.links) {
            for &target in targets {
                #[cfg(test)]
                VALIDATION_VISITS.with(|count| count.set(count.get() + 1));
                reverse.entry(target).or_default().push(source);
            }
        }
        let mut components = BTreeMap::new();
        for root in order.into_iter().rev() {
            if components.contains_key(&root) {
                continue;
            }
            let mut todo = vec![root];
            while let Some(number) = todo.pop() {
                #[cfg(test)]
                VALIDATION_VISITS.with(|count| count.set(count.get() + 1));
                if components.contains_key(&number) {
                    continue;
                }
                components.insert(number, root);
                todo.extend(reverse.get(&number).into_iter().flatten());
            }
        }
        // Keep the original linked-edge order and error wording. Subtask-only
        // cycles in damaged legacy data must not make an unrelated link fail.
        for (&number, links) in &self.links {
            for &target in links {
                #[cfg(test)]
                VALIDATION_VISITS.with(|count| count.set(count.get() + 1));
                if components.get(&number) == components.get(&target) {
                    return self.validate_edge(number, target);
                }
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
            #[cfg(test)]
            VALIDATION_VISITS.with(|count| count.set(count.get() + 1));
            if n == source {
                return Err(Error::invalid("This dependency would create a cycle"));
            }
            if !seen.insert(n) {
                continue;
            }
            todo.extend(self.children.get(&n).into_iter().flatten());
            todo.extend(self.links.get(&n).into_iter().flatten());
        }
        Ok(())
    }
}

pub(super) fn validate_new_links(db: &Connection, project: &str, links: &[i64]) -> Result<bool> {
    if links.is_empty() {
        return Ok(false);
    }
    if links.len() > 100
        || links.iter().any(|n| *n < 1)
        || links.iter().collect::<BTreeSet<_>>().len() != links.len()
    {
        return Err(Error::invalid(
            "Use up to 100 different positive blocker issue numbers",
        ));
    }
    let graph = Graph::load(db, project)?;
    let mut any_unfinished = false;
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
        if graph.unfinished(target) {
            any_unfinished = true;
        }
    }
    Ok(any_unfinished)
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
    Ok(Graph::load(db, project)?.has_active(number))
}

pub(super) fn reopen_blockers(db: &Connection, project: &str, number: i64) -> Result<Vec<Value>> {
    let mut graph = Graph::load(db, project)?;
    let active = graph.active(number);
    graph.load_prs(db, project, &active.keys().copied().collect())?;
    Ok(active
        .into_iter()
        .map(|(n, source)| graph.reference(n, source))
        .collect())
}

/// Validate an incoming fleet relationship inside its savepoint, before the
/// batch reconciler can release a claim acquired while the replica was offline.
pub(crate) fn validate_subtask_claims(db: &Connection, project: &str) -> Result<()> {
    Graph::load(db, project)?.validate_subtask_claims()
}

/// One graph snapshot per mutation. Reads never take a writer lock; only actual
/// transitions advance versions and enter the fleet journal.
pub(crate) fn reconcile(
    db: &Connection,
    project: &str,
    actor: Option<&str>,
    now: i64,
) -> Result<()> {
    reconcile_graph(db, project, actor, now, false, false)
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
        let has_blockers = graph.has_active(number);
        let state = if issue["manual_blocked"] == true || (issue["draft"] != true && has_blockers) {
            "blocked"
        } else if issue["state"] == "ready" {
            "ready"
        } else {
            "open"
        };
        if issue["state"] == state {
            continue;
        }
        // Full blocker identities are needed only for transition evidence.
        let blockers = graph.active(number);
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
                let inserted = db.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?1,?2,?3,?4,?5)", params![project,number,author,body,now])?;
                if inserted == 0 {
                    continue;
                }
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
            if upgrading || rework || graph.prs_enabled {
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

pub(super) fn reconcile_upgrade(db: &Connection) -> Result<()> {
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
    if ![
        "issue",
        "parent_issue",
        "child_issue",
        "issues",
        "subtasks",
        "created_chain",
    ]
    .iter()
    .any(|key| {
        result
            .get(*key)
            .is_some_and(|v| v.is_object() || v.is_array())
    }) {
        return Ok(());
    }
    let mut graph = Graph::load(db, project)?;
    let mut references = BTreeSet::new();
    for number in ["issue", "parent_issue", "child_issue"]
        .iter()
        .filter_map(|key| result[*key]["number"].as_i64())
        .chain(
            ["issues", "subtasks", "created_chain"]
                .iter()
                .flat_map(|key| result[*key].as_array().into_iter().flatten())
                .filter_map(|issue| issue["number"].as_i64()),
        )
    {
        references.extend(graph.links.get(&number).into_iter().flatten().copied());
        references.extend(graph.active(number).into_keys());
        references.extend(
            graph
                .dependents
                .get(&number)
                .into_iter()
                .flatten()
                .map(|(number, _)| *number),
        );
    }
    graph.load_prs(db, project, &references)?;
    let attach = |issue: &mut Value| {
        let Some(n) = issue["number"].as_i64() else {
            return;
        };
        issue["subtask_scheduling"] = json!("explicit");
        if let Some(context) = issue
            .get_mut("subtask_context")
            .and_then(Value::as_object_mut)
        {
            context.insert("scheduling".into(), json!("explicit"));
        }
        let mut dependencies = BTreeMap::new();
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
                .map(|&b| {
                    let mut r = graph.reference(b, "linked");
                    r["satisfied"] = json!(!graph.unfinished(b));
                    r
                })
                .collect::<Vec<_>>()
        );
        issue["blocking"] = json!(
            graph
                .dependents
                .get(&n)
                .into_iter()
                .flatten()
                .map(|&(dep, source)| {
                    let m_active = graph.active(dep);
                    let actively_blocked = m_active.contains_key(&n);
                    let unblocks_on_release = actively_blocked
                        && m_active.len() == 1
                        && graph
                            .issues
                            .get(&dep)
                            .is_some_and(|i| i["manual_blocked"] != true && i["draft"] != true);
                    let mut r = graph.reference(dep, source);
                    r["actively_blocked"] = json!(actively_blocked);
                    r["unblocks_on_release"] = json!(unblocks_on_release);
                    r["remaining_blocker_count"] = json!(m_active.len());
                    r
                })
                .collect::<Vec<_>>()
        );
    };
    for key in ["issue", "parent_issue", "child_issue"] {
        if let Some(i) = result.get_mut(key) {
            attach(i);
        }
    }
    for key in ["issues", "subtasks", "created_chain"] {
        if let Some(issues) = result[key].as_array_mut() {
            for i in issues {
                attach(i);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_cycle_validation_does_not_rewalk_every_dependency_chain() {
        let mut work = Vec::new();
        for count in [128, 1024] {
            let root = std::env::temp_dir().join(format!(
                "hb-dependency-validation-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            std::fs::create_dir(&root).unwrap();
            let path = root.join("issues.db");
            drop(crate::issues::Store::open(&path).unwrap());
            let db = Connection::open(&path).unwrap();
            db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:test','test',2000); INSERT INTO agents VALUES('human:test','{}',0)").unwrap();
            db.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?1) INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order,blockers) SELECT 'named:test',x,'Task','',CASE WHEN x=1 THEN 'open' ELSE 'blocked' END,'human:test',0,0,1,'[]',x,CASE WHEN x=1 THEN '[]' ELSE json_array(x-1) END FROM n", [count]).unwrap();
            db.execute_batch("BEGIN IMMEDIATE").unwrap();
            VALIDATION_VISITS.set(0);
            let started = std::time::Instant::now();
            reconcile_subtasks(&db, "named:test", Some("human:test"), 1).unwrap();
            let visits = VALIDATION_VISITS.get();
            eprintln!(
                "{count}-issue dependency chain validation: {visits} visits in {:?}",
                started.elapsed()
            );
            assert_eq!(
                db.query_row("SELECT sum(version) FROM issues", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                count
            );
            db.execute_batch("COMMIT").unwrap();
            work.push((count, visits));
            drop(db);
            std::fs::remove_dir_all(root).unwrap();
        }
        assert!(
            work.iter()
                .all(|(count, visits)| *visits <= *count as usize * 8),
            "Repeated cycle traversals: {work:?}"
        );
    }

    #[test]
    fn whole_graph_validation_matches_individual_link_validation() {
        for mask in 0..4096 {
            let mut graph = Graph {
                satisfied: RefCell::default(),
                prs_enabled: false,
                issues: BTreeMap::new(),
                children: BTreeMap::new(),
                links: BTreeMap::new(),
                dependents: BTreeMap::new(),
                active_cache: RefCell::default(),
            };
            let mut bit = 0;
            for source in 1..=4 {
                for target in 1..=4 {
                    if source == target {
                        continue;
                    }
                    if mask & (1 << bit) != 0 {
                        let edges = if (source + target) % 2 == 0 {
                            &mut graph.children
                        } else {
                            &mut graph.links
                        };
                        edges.entry(source).or_default().push(target);
                    }
                    bit += 1;
                }
            }
            for self_link in [None, Some(3)] {
                if let Some(n) = self_link {
                    graph.links.entry(n).or_default().push(n);
                }
                let expected = graph.links.iter().try_for_each(|(&source, links)| {
                    links
                        .iter()
                        .try_for_each(|&target| graph.validate_edge(source, target))
                });
                let error = |result: Result<()>| result.err().map(|e| (e.code, e.message));
                assert_eq!(
                    error(graph.validate_edges()),
                    error(expected),
                    "mask={mask}, self_link={self_link:?}"
                );
            }
        }
    }

    #[test]
    fn dependency_predicates_match_ready_completion_closure() {
        for mode in 0..8 {
            for combination in 0..1024 {
                let states: BTreeMap<_, _> = (1..=5)
                    .map(|n| {
                        (
                            n,
                            ["open", "blocked", "ready", "closed"]
                                [(combination >> ((n - 1) * 2)) & 3],
                        )
                    })
                    .collect();
                let deleted = mode & 1 != 0;
                let cycle = mode & 4 != 0;
                let descendants: BTreeMap<i64, Vec<i64>> = [
                    (1, if deleted { vec![] } else { vec![2, 3] }),
                    (2, vec![3]),
                    (3, vec![]),
                    (4, if cycle { vec![4, 5] } else { vec![5] }),
                    (5, if cycle { vec![4, 5] } else { vec![] }),
                ]
                .into();
                let links: BTreeMap<i64, Vec<i64>> =
                    [(3, vec![if mode & 2 != 0 { 6 } else { 4 }]), (5, vec![2])].into();
                // Least fixed point: Ready is satisfied only after all of its
                // prerequisites are satisfied. Cycles and missing nodes stay pending.
                let mut finished: BTreeSet<_> = states
                    .iter()
                    .filter_map(|(&n, &state)| {
                        (state == "closed" || (deleted && n == 2)).then_some(n)
                    })
                    .collect();
                loop {
                    let before = finished.len();
                    for (&n, &state) in &states {
                        if state == "ready"
                            && descendants[&n]
                                .iter()
                                .chain(links.get(&n).into_iter().flatten())
                                .all(|n| finished.contains(n))
                        {
                            finished.insert(n);
                        }
                    }
                    if before == finished.len() {
                        break;
                    }
                }
                let mut children: BTreeMap<_, _> =
                    [(1, vec![2]), (2, vec![3]), (4, vec![5])].into();
                if cycle {
                    children.insert(5, vec![4]);
                }
                let graph = Graph {
                    satisfied: RefCell::default(), prs_enabled:false,
                    issues: states.iter().map(|(&n,&state)| (n,json!({"state":state,"deleted_at":if deleted && n==2 {json!(1)} else {Value::Null}}))).collect(),
                    children, links:links.clone(), dependents:BTreeMap::new(), active_cache:RefCell::default(),
                };
                for n in 1..=5 {
                    assert_eq!(
                        graph.unfinished(n),
                        !finished.contains(&n),
                        "mode={mode}, states={states:?}, issue={n}"
                    );
                    let mut expected: BTreeMap<_, _> = descendants[&n]
                        .iter()
                        .filter(|n| !finished.contains(n))
                        .map(|&n| (n, "subtask"))
                        .collect();
                    for &linked in links.get(&n).into_iter().flatten() {
                        if !finished.contains(&linked) {
                            expected.insert(linked, "linked");
                        }
                    }
                    assert_eq!(
                        graph.has_active(n),
                        !expected.is_empty(),
                        "mode={mode}, states={states:?}, issue={n}"
                    );
                    assert_eq!(
                        graph.active(n),
                        expected,
                        "mode={mode}, states={states:?}, issue={n}"
                    );
                }
            }
        }
    }

    #[test]
    fn unchanged_nested_lifecycle_does_not_enumerate_every_transitive_blocker() {
        let mut work = Vec::new();
        for count in [128, 1024] {
            let root = std::env::temp_dir().join(format!(
                "hb-dependency-predicate-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            std::fs::create_dir(&root).unwrap();
            let path = root.join("issues.db");
            drop(crate::issues::Store::open(&path).unwrap());
            let db = Connection::open(&path).unwrap();
            db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:test','test',2000); INSERT INTO agents VALUES('human:test','{}',0)").unwrap();
            // A valid four-way tree stays inside the eight-level depth limit.
            db.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?1) INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) SELECT 'named:test',x,'Task','',CASE WHEN x<=(?1-2)/4+1 THEN 'blocked' ELSE 'open' END,'human:test',0,0,1,'[]',x FROM n", [count]).unwrap();
            db.execute("INSERT INTO issue_subtasks SELECT project_id,(number-2)/4+1,number,0,'human:test' FROM issues WHERE number>1", []).unwrap();
            DESCENDANT_VISITS.set(0);
            let started = std::time::Instant::now();
            reconcile(&db, "named:test", Some("human:test"), 1).unwrap();
            let visits = DESCENDANT_VISITS.get();
            eprintln!(
                "Unchanged {count}-issue nested graph: {visits} descendant visits in {:?}",
                started.elapsed()
            );
            assert_eq!(
                db.query_row("SELECT sum(version) FROM issues", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                count
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0
            );
            work.push((count, visits));
            drop(db);
            std::fs::remove_dir_all(root).unwrap();
        }
        assert!(
            work.iter()
                .all(|(count, visits)| *visits <= *count as usize),
            "Repeated transitive blocker enumeration remains: {work:?}"
        );
    }

    #[test]
    fn dependency_work_does_not_grow_with_unreferenced_pr_history() {
        let mut measurements = Vec::new();
        for unrelated in [0, 2048] {
            let root = std::env::temp_dir().join(format!(
                "hb-dependency-prs-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            std::fs::create_dir(&root).unwrap();
            let path = root.join("issues.db");
            drop(crate::issues::Store::open(&path).unwrap());
            let db = Connection::open(&path).unwrap();
            db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:test','test',6);
                INSERT INTO agents VALUES('human:test','{}',0);
                INSERT INTO project_settings(project_id,prompt,version,prs_enabled) VALUES('named:test','',1,1);
                WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<5)
                INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
                SELECT 'named:test',x,'Task','','open','human:test',0,0,1,'[]',x FROM n;
                UPDATE issues SET blockers='[1]',state='blocked' WHERE number=2;
                UPDATE issues SET state='blocked' WHERE number=4;
                INSERT INTO issue_subtasks VALUES('named:test',4,5,0,'human:test');
                INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose,status) VALUES
                ('named:test',1,'https://github.com/o/r/pull/11','human:test',20,'fix','open'),
                ('named:test',1,'https://github.com/o/r/pull/12','human:test',10,'supporting-evidence','merged'),
                ('named:test',2,'https://github.com/o/r/pull/2','human:test',0,'fix','open'),
                ('named:test',5,'https://github.com/o/r/pull/5','human:test',0,'prerequisite','open');").unwrap();
            db.execute("WITH RECURSIVE n(x) AS (SELECT 1 WHERE ?1>0 UNION ALL SELECT x+1 FROM n WHERE x<?1) INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at) SELECT 'named:test',3,'https://github.com/o/r/pull/'||(100+x),'human:test',x FROM n", [unrelated]).unwrap();
            drop(db);
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            let expected = json!([
                {"url":"https://github.com/o/r/pull/12","purpose":"supporting-evidence","status":"merged"},
                {"url":"https://github.com/o/r/pull/11","purpose":"fix","status":"open"}
            ]);
            for mode in ["lifecycle", "context", "reopen"] {
                let (db, transport) = crate::database::tests::measured_connection(&path);
                db.execute_batch("BEGIN IMMEDIATE").unwrap();
                match mode {
                    "lifecycle" => {
                        validate_links(&db, "named:test", 2, &[1]).unwrap();
                        validate_subtask_claims(&db, "named:test").unwrap();
                        assert!(has_dependencies(&db, "named:test", 2).unwrap());
                        reconcile(&db, "named:test", None, 1).unwrap();
                    }
                    "context" => {
                        let mut result =
                            json!({"issue":{"number":2},"issues":[{"number":1},{"number":4}]});
                        enrich(&db, "named:test", &mut result).unwrap();
                        for key in ["dependency_context", "blocked_by", "blocker_links"] {
                            assert_eq!(result["issue"][key][0]["pull_requests"], expected, "{key}");
                        }
                        assert_eq!(
                            result["issues"][0]["blocking"][0]["pull_requests"][0]["url"],
                            "https://github.com/o/r/pull/2"
                        );
                        assert_eq!(
                            result["issues"][1]["blocked_by"][0]["pull_requests"][0]["url"],
                            "https://github.com/o/r/pull/5"
                        );
                    }
                    "reopen" => assert_eq!(
                        reopen_blockers(&db, "named:test", 2).unwrap()[0]["pull_requests"],
                        expected
                    ),
                    _ => unreachable!(),
                }
                db.execute_batch("COMMIT").unwrap();
                drop(db);
                let (commands, steps) = transport.join().unwrap();
                eprintln!(
                    "{mode}, {unrelated} unrelated PRs: {commands} RPCs, {steps} query VM steps"
                );
                measurements.push((commands, steps));
            }
            owner.stop();
            std::fs::remove_dir_all(root).unwrap();
        }
        for index in 0..3 {
            assert_eq!(
                measurements[index],
                measurements[index + 3],
                "Unreferenced PRs added work: {measurements:?}"
            );
        }
    }
}
