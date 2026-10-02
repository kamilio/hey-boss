//! Scheduling relationships, reconciled transactionally without releasing claims.
use super::{Actor, Error, Operation, Project, Result, create_issue, event, get_issue};
use crate::database::Connection;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const SCHEMA: &str = include_str!("subtasks.sql");
pub(super) fn migrate_sync(db: &Connection) -> Result<()> {
    db.execute_batch("DROP TRIGGER subtask_graph_insert; DROP TRIGGER subtask_graph_update;")?;
    let triggers = SCHEMA
        .split_once("CREATE TRIGGER")
        .unwrap()
        .1
        .split_once("-- Shared")
        .unwrap()
        .0;
    db.execute_batch(&format!("CREATE TRIGGER{triggers}"))?;
    db.execute_batch(include_str!("subtask-readiness.sql"))?;
    Ok(())
}

fn expected(actual: i64, version: Option<i64>) -> Result<()> {
    if version.is_some_and(|v| v != actual) {
        return Err(Error::conflict(
            "Issue changed. Refresh before changing its subtasks.",
        ));
    }
    Ok(())
}
fn relationship_error(error: rusqlite::Error) -> Error {
    if let rusqlite::Error::SqliteFailure(_, Some(message)) = &error
        && message.starts_with("Subtasks:")
    {
        return Error::invalid(message.trim_start_matches("Subtasks: "));
    }
    error.into()
}

pub(super) fn execute(
    db: &Connection,
    project: &Project,
    operation: &Operation,
    actor: Option<&Actor>,
    now: i64,
) -> Result<Value> {
    let number = operation.number().unwrap();
    match operation {
        Operation::Subtasks {
            include_deleted, ..
        } => {
            let parent = get_issue(db, &project.id, number, true)?;
            let mut statement = db.prepare(&format!("SELECT {} FROM issues WHERE project_id=?1 AND number IN(SELECT child_number FROM issue_subtasks WHERE project_id=?1 AND parent_number=?2) AND (?3 OR deleted_at IS NULL) ORDER BY sort_order,number", super::COLUMNS))?;
            let issues = statement
                .query_map(
                    params![project.id, number, include_deleted],
                    super::row_issue,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({"ok":true,"project":project,"parent_issue":parent,"issues":issues}))
        }
        Operation::CreateSubtask {
            if_version,
            labels,
            then_titles,
            ..
        } => {
            let parent = get_issue(db, &project.id, number, false)?;
            expected(parent.version, *if_version)?;
            let actor = actor.unwrap();
            let child = create_issue(db, project, actor, operation, now)?;
            change(db, project, actor, operation, child.number, true, now)?;
            let mut created_chain = vec![serde_json::to_value(get_issue(
                db,
                &project.id,
                child.number,
                false,
            )?)?];
            let mut prev_number = child.number;
            for next_title in then_titles {
                let next_op = Operation::CreateSubtask {
                    number,
                    title: next_title.clone(),
                    body: String::new(),
                    labels: labels.clone(),
                    at_top: false,
                    if_version: None,
                    blockers: vec![prev_number],
                    then_titles: Vec::new(),
                };
                let next_child = create_issue(db, project, actor, &next_op, now)?;
                change(db, project, actor, &next_op, next_child.number, true, now)?;
                prev_number = next_child.number;
                created_chain.push(serde_json::to_value(get_issue(
                    db,
                    &project.id,
                    next_child.number,
                    false,
                )?)?);
            }
            let mut out = json!({"ok":true,"project":project,"issue":get_issue(db,&project.id,child.number,false)?,"parent_issue":get_issue(db,&project.id,number,false)?,"changed":true});
            if !then_titles.is_empty() {
                out["created_chain"] = json!(created_chain);
            }
            Ok(out)
        }
        Operation::AddSubtask { child, .. } | Operation::RemoveSubtask { child, .. } => {
            let changed = change(
                db,
                project,
                actor.unwrap(),
                operation,
                *child,
                matches!(operation, Operation::AddSubtask { .. }),
                now,
            )?;
            Ok(
                json!({"ok":true,"project":project,"issue":get_issue(db,&project.id,number,true)?,"child_issue":get_issue(db,&project.id,*child,true)?,"changed":changed}),
            )
        }
        _ => unreachable!(),
    }
}
fn change(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    operation: &Operation,
    child: i64,
    add: bool,
    now: i64,
) -> Result<bool> {
    let number = operation.number().unwrap();
    let parent_issue = get_issue(db, &project.id, number, !add)?;
    let child_issue = get_issue(db, &project.id, child, !add)?;
    let (parent_version, child_version) = match operation {
        Operation::CreateSubtask { if_version, .. } => (*if_version, None),
        Operation::AddSubtask {
            if_version,
            if_child_version,
            ..
        }
        | Operation::RemoveSubtask {
            if_version,
            if_child_version,
            ..
        } => (*if_version, *if_child_version),
        _ => unreachable!(),
    };
    expected(parent_issue.version, parent_version)?;
    expected(child_issue.version, child_version)?;
    let current: Option<i64> = db
        .query_row(
            "SELECT parent_number FROM issue_subtasks WHERE project_id=?1 AND child_number=?2",
            params![project.id, child],
            |r| r.get(0),
        )
        .optional()?;
    if current.is_some_and(|p| p != number) {
        return Err(Error::conflict(format!(
            "Issue #{child} already belongs to issue #{}. Unlink it there before adding it here.",
            current.unwrap()
        )));
    }
    if current.is_some() == add {
        return Ok(false);
    }
    if add {
        super::super::blockers::validate_subtask(db, &project.id, number, child)?;
        db.execute("INSERT INTO issue_subtasks(project_id,parent_number,child_number,created_at,created_by) VALUES(?1,?2,?3,?4,?5)",params![project.id,number,child,now,actor.id]).map_err(relationship_error)?;
    } else {
        db.execute("DELETE FROM issue_subtasks WHERE project_id=?1 AND parent_number=?2 AND child_number=?3",params![project.id,number,child])?;
    }
    db.execute("UPDATE issues SET version=version+1,updated_at=?4 WHERE project_id=?1 AND number IN(?2,?3)",params![project.id,number,child,now])?;
    let data = json!({"parent":number,"child":child});
    event(
        db,
        &project.id,
        number,
        &actor.id,
        if add {
            "subtask_added"
        } else {
            "subtask_removed"
        },
        now,
        &data,
    )?;
    event(
        db,
        &project.id,
        child,
        &actor.id,
        if add {
            "parent_added"
        } else {
            "parent_removed"
        },
        now,
        &data,
    )?;
    Ok(true)
}

struct Graph {
    issues: BTreeMap<i64, Value>,
    parents: BTreeMap<i64, i64>,
    children: BTreeMap<i64, Vec<i64>>,
    open: BTreeMap<i64, i64>,
}
impl Graph {
    fn context_reference(issue: &Value) -> Value {
        json!({"number":issue["number"],"title":issue["title"],"state":issue["state"],
            "deleted_at":issue["deleted_at"],"pull_requests":issue["pull_requests"]})
    }
    fn load(db: &Connection, project: &str) -> Result<Self> {
        let mut graph = Self {
            issues: BTreeMap::new(),
            parents: BTreeMap::new(),
            children: BTreeMap::new(),
            open: BTreeMap::new(),
        };
        for (child, parent) in db.query_collect::<_, _, rusqlite::Error>(
            "SELECT child_number,parent_number FROM issue_subtasks WHERE project_id=?1",
            [project],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )? {
            graph.parents.insert(child, parent);
        }
        if graph.parents.is_empty() {
            return Ok(graph);
        }
        let nodes: BTreeSet<_> = graph
            .parents
            .keys()
            .chain(graph.parents.values())
            .copied()
            .collect();
        let nodes = serde_json::to_string(&nodes)?;
        let issues = db.query_collect::<_, _, Error>("SELECT number,title,state,assignee,deleted_at,version,sort_order,closed_at,labels FROM json_each(?2) requested CROSS JOIN issues WHERE project_id=?1 AND number=requested.value ORDER BY sort_order,number",params![project,nodes], |r| {
            let labels: String = r.get(8)?;
            Ok(json!({"number":r.get::<_,i64>(0)?,"title":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,"assignee":r.get::<_,Option<String>>(3)?,"deleted_at":r.get::<_,Option<i64>>(4)?,"version":r.get::<_,i64>(5)?,"sort_order":r.get::<_,i64>(6)?,"closed_at":r.get::<_,Option<i64>>(7)?,"labels":serde_json::from_str::<Value>(&labels)?,"pull_requests":[]}))
        })?;
        for issue in issues {
            let number = issue["number"].as_i64().unwrap();
            if let Some(parent) = graph.parents.get(&number) {
                graph.children.entry(*parent).or_default().push(number);
            }
            graph.issues.insert(number, issue);
        }
        let prs = db.query_collect::<_, _, rusqlite::Error>("SELECT issue_number,url,added_by,created_at,purpose,status,checked_at,error FROM json_each(?2) requested CROSS JOIN issue_pull_requests WHERE project_id=?1 AND issue_number=requested.value ORDER BY created_at,url",params![project,nodes],|r|Ok((r.get::<_,i64>(0)?,json!({"url":r.get::<_,String>(1)?,"added_by":r.get::<_,String>(2)?,"created_at":r.get::<_,i64>(3)?,"purpose":r.get::<_,String>(4)?,"status":r.get::<_,String>(5)?,"checked_at":r.get::<_,Option<i64>>(6)?,"error":r.get::<_,Option<String>>(7)?}))))?;
        for (number, pr) in prs {
            if let Some(issue) = graph.issues.get_mut(&number) {
                issue["pull_requests"].as_array_mut().unwrap().push(pr);
            }
        }
        Ok(graph)
    }
    fn open_subtree(&mut self, number: i64) -> i64 {
        if let Some(count) = self.open.get(&number) {
            return *count;
        }
        let Some(issue) = self.issues.get(&number) else {
            return 0;
        };
        let count = if !issue["deleted_at"].is_null() {
            0
        } else {
            let own = i64::from(issue["state"] == "open" || issue["state"] == "blocked");
            own + self
                .children
                .get(&number)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|n| self.open_subtree(*n))
                .sum::<i64>()
        };
        self.open.insert(number, count);
        count
    }
    fn attach(&mut self, issue: &mut Value) {
        let Some(number) = issue["number"].as_i64() else {
            return;
        };
        issue["parent"] = self
            .parents
            .get(&number)
            .and_then(|p| self.issues.get(p))
            .cloned()
            .unwrap_or(Value::Null);
        issue["subtask_context"] = Value::Null;
        if let Some(parent) = self.parents.get(&number) {
            let siblings: Vec<_> = self
                .children
                .get(parent)
                .into_iter()
                .flatten()
                .filter(|n| self.issues[n]["deleted_at"].is_null())
                .collect();
            if let Some(position) = siblings.iter().position(|n| **n == number) {
                issue["subtask_context"] = json!({
                    "parent":self.issues.get(parent).map(Self::context_reference),
                    "position":position+1,
                    "total":siblings.len(),
                    "previous":position.checked_sub(1).map(|p| Self::context_reference(&self.issues[siblings[p]])),
                    "next":siblings.get(position+1).map(|n| Self::context_reference(&self.issues[n])),
                });
            }
        }
        let children = self.children.get(&number).cloned().unwrap_or_default();
        if children.is_empty() {
            issue["subtasks"] = Value::Null;
            return;
        }
        let visible: Vec<_> = children
            .iter()
            .filter(|n| self.issues[n]["deleted_at"].is_null())
            .collect();
        let closed = visible
            .iter()
            .filter(|n| self.issues[n]["state"] == "closed")
            .count();
        let total = visible.len();
        let open_descendants = children.iter().map(|n| self.open_subtree(*n)).sum::<i64>();
        issue["subtasks"] = json!({"total":total,"closed":closed,"deleted":children.len()-total,"open_descendants":open_descendants});
    }
    fn child_list(&self, number: i64) -> Vec<Value> {
        self.children
            .get(&number)
            .into_iter()
            .flatten()
            .filter_map(|n| self.issues.get(n).cloned())
            .collect()
    }
}
/// Use the same relationship snapshot for claim responses, previews and jobs.
pub(super) fn worker_issue(db: &Connection, project: &str, number: i64) -> Result<Value> {
    let mut issue = json!(get_issue(db, project, number, false)?);
    super::assignments::enrich(db, project, &mut issue)?;
    super::attempts::enrich(db, project, &mut issue)?;
    Graph::load(db, project)?.attach(&mut issue);
    let mut result = json!({"issue":issue});
    super::super::blockers::enrich(db, project, &mut result)?;
    Ok(result["issue"].take())
}
/// One graph snapshot enriches an entire page; no relationship query per row.
pub(super) fn enrich(db: &Connection, project: &str, result: &mut Value) -> Result<()> {
    let has_issue = ["issue", "parent_issue", "child_issue"]
        .iter()
        .any(|key| result[*key]["number"].as_i64().is_some())
        || result["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|issue| issue["number"].as_i64().is_some());
    if !has_issue && result.get("comments").is_none() {
        return Ok(());
    }
    let mut graph = Graph::load(db, project)?;
    for key in ["issue", "parent_issue", "child_issue"] {
        if let Some(issue) = result.get_mut(key) {
            graph.attach(issue);
        }
    }
    if let Some(issues) = result["issues"].as_array_mut() {
        for issue in issues {
            graph.attach(issue);
        }
    }
    if result.get("comments").is_some() {
        result["subtasks"] = json!(graph.child_list(result["issue"]["number"].as_i64().unwrap()));
        for child in result["subtasks"].as_array_mut().unwrap() {
            graph.attach(child);
        }
        result["order_version"] = json!(db.query_row(
            "SELECT issue_order_version FROM projects WHERE id=?1",
            [project],
            |r| r.get::<_, i64>(0)
        )?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtask_response_enrichment_avoids_unrelated_metadata() {
        let root = std::env::temp_dir().join(format!(
            "hb-subtask-response-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        let path = root.join("issues.db");
        let store = crate::issues::Store::open(&path).unwrap();
        store.db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:Graph','Graph',10000);
            INSERT INTO agents VALUES('creator','{}',0);
            WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<8192)
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
            SELECT 'named:Graph',id,'Task '||id,'','open','creator',0,0,1,'[]',id FROM n;
            INSERT INTO issue_subtasks VALUES('named:Graph',1,2,0,'creator');
            INSERT INTO issue_subtasks VALUES('named:Graph',2,3,0,'creator'),('named:Graph',1,4,0,'creator'),('named:Graph',1,5,0,'creator'),('named:Graph',5,6,0,'creator'),('named:Graph',4,7,0,'creator');
            UPDATE issues SET deleted_at=1 WHERE number=4;
            UPDATE issues SET state='closed' WHERE number=5;
            INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at)
            SELECT project_id,number,'https://github.com/o/r/pull/'||number,'creator',0 FROM issues;").unwrap();
        let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
        let (db, transport) = crate::database::tests::measured_connection(&path);
        let samples = [
            json!({"ok":true,"issues":null}),
            json!({"ok":true,"issues":[]}),
            json!({"ok":true,"issues":[{"other":0}]}),
        ];
        let started = std::time::Instant::now();
        let outputs: Vec<_> = samples
            .iter()
            .map(|sample| {
                let mut result = sample.clone();
                enrich(&db, "named:Graph", &mut result).unwrap();
                result
            })
            .collect();
        let elapsed = started.elapsed();
        drop(db);
        let (commands, steps) = transport.join().unwrap();
        let mut issue_response = json!({"issue":{"number":1},"parent_issue":{"number":1},"child_issue":{"number":2},"issues":[{"number":1},{"number":2},{"number":8192}],"comments":[]});
        let (db, transport) = crate::database::tests::measured_connection(&path);
        let issue_started = std::time::Instant::now();
        enrich(&db, "named:Graph", &mut issue_response).unwrap();
        let issue_elapsed = issue_started.elapsed();
        drop(db);
        let (issue_commands, issue_steps) = transport.join().unwrap();
        owner.stop();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
        eprintln!(
            "Three metadata responses beside 8192 issues: {commands} RPCs, {steps} query VM steps in {elapsed:?}"
        );
        eprintln!(
            "Seven related nodes beside 8192 issues: {issue_commands} RPCs, {issue_steps} query VM steps in {issue_elapsed:?}"
        );
        assert_eq!(outputs, samples);
        assert_eq!(issue_response["issue"]["subtasks"]["total"], 2);
        assert_eq!(issue_response["issue"]["subtasks"]["closed"], 1);
        assert_eq!(issue_response["issue"]["subtasks"]["deleted"], 1);
        assert_eq!(
            issue_response["parent_issue"]["subtasks"]["open_descendants"],
            3
        );
        assert_eq!(issue_response["child_issue"]["parent"]["number"], 1);
        assert_eq!(
            issue_response["issues"][1]["subtask_context"]["position"],
            1
        );
        assert_eq!(issue_response["subtasks"][0]["number"], 2);
        assert_eq!(issue_response["subtasks"][1]["number"], 4);
        assert_eq!(issue_response["subtasks"].as_array().unwrap().len(), 3);
        assert_eq!(
            issue_response["issues"][1]["subtask_context"]["next"]["number"],
            5
        );
        assert!(issue_response["issues"][2]["subtasks"].is_null());
        assert_eq!(
            issue_response["subtasks"][0]["parent"]["pull_requests"][0]["url"],
            "https://github.com/o/r/pull/1"
        );
        assert_eq!(
            commands, 0,
            "Responses without issues must not load a graph"
        );
        assert_eq!(steps, 0);
        assert!(
            issue_commands <= 4,
            "Subtask reads repeated statement metadata requests: {issue_commands} RPCs"
        );
        assert!(
            issue_steps < 5000,
            "Subtask metadata scanned unrelated issues: {issue_steps} query VM steps"
        );
    }
}
