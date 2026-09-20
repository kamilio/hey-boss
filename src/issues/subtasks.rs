//! Issue relationships remain separate from issue ownership and lifecycle.
use super::{Actor, Error, Operation, Project, Result, create_issue, event, get_issue};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;

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
        Operation::CreateSubtask { if_version, .. } => {
            let parent = get_issue(db, &project.id, number, false)?;
            expected(parent.version, *if_version)?;
            let actor = actor.unwrap();
            let child = create_issue(db, project, actor, operation, now)?;
            change(db, project, actor, operation, child.number, true, now)?;
            Ok(
                json!({"ok":true,"project":project,"issue":get_issue(db,&project.id,child.number,false)?,"parent_issue":get_issue(db,&project.id,number,false)?,"changed":true}),
            )
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
    fn load(db: &Connection, project: &str) -> Result<Self> {
        let mut graph = Self {
            issues: BTreeMap::new(),
            parents: BTreeMap::new(),
            children: BTreeMap::new(),
            open: BTreeMap::new(),
        };
        let mut statement = db
            .prepare("SELECT child_number,parent_number FROM issue_subtasks WHERE project_id=?1")?;
        for pair in statement.query_map([project], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (child, parent) = pair?;
            graph.parents.insert(child, parent);
        }
        if graph.parents.is_empty() {
            return Ok(graph);
        }
        let mut statement=db.prepare("SELECT number,title,state,assignee,deleted_at,version,sort_order,closed_at,labels FROM issues WHERE project_id=?1 ORDER BY sort_order,number")?;
        for issue in statement.query_map([project], |r|Ok(json!({"number":r.get::<_,i64>(0)?,"title":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,"assignee":r.get::<_,Option<String>>(3)?,"deleted_at":r.get::<_,Option<i64>>(4)?,"version":r.get::<_,i64>(5)?,"sort_order":r.get::<_,i64>(6)?,"closed_at":r.get::<_,Option<i64>>(7)?,"labels":r.get::<_,String>(8)?,"pull_requests":[]})))? {
            let mut issue=issue?;let number=issue["number"].as_i64().unwrap();
            issue["labels"]=serde_json::from_str(issue["labels"].as_str().unwrap())?;
            if let Some(parent)=graph.parents.get(&number) {graph.children.entry(*parent).or_default().push(number);}
            graph.issues.insert(number,issue);
        }
        let mut statement=db.prepare("SELECT issue_number,url,added_by,created_at,purpose FROM issue_pull_requests WHERE project_id=?1 ORDER BY created_at,url")?;
        for pr in statement.query_map([project],|r|Ok((r.get::<_,i64>(0)?,json!({"url":r.get::<_,String>(1)?,"added_by":r.get::<_,String>(2)?,"created_at":r.get::<_,i64>(3)?,"purpose":r.get::<_,String>(4)?}))))? {
            let (number,pr)=pr?;
            if let Some(issue)=graph.issues.get_mut(&number){issue["pull_requests"].as_array_mut().unwrap().push(pr);}
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
            let own = i64::from(issue["state"] == "open");
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
/// One graph snapshot enriches an entire page; no relationship query per row.
pub(super) fn enrich(db: &Connection, project: &str, result: &mut Value) -> Result<()> {
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
