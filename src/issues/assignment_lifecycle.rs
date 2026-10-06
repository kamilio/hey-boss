//! A watcher closes its task when no open supported PR remains.
use super::*;

fn has_open_pr(links: &[Value]) -> bool {
    links.iter().any(|pr| {
        pr["status"] != "merged"
            && pr["status"] != "closed"
            && pr["url"]
                .as_str()
                .and_then(hey_gh::watcher::pull_request_selector)
                .is_some()
    })
}

pub(super) fn linked_status(mut status: Value, links: &[Value], keep_removed: bool) -> Value {
    status["prs"]
        .as_object_mut()
        .unwrap()
        .retain(|url, snapshot| {
            let link = links.iter().find(|pr| pr["url"] == *url);
            if link.is_none() && !keep_removed {
                return false;
            }
            let lifecycle = link
                .map(|pr| pr["status"].as_str().unwrap_or("unknown"))
                .unwrap_or("removed");
            if matches!(lifecycle, "closed" | "merged" | "removed") {
                snapshot["lifecycle"] = json!(lifecycle);
            } else if let Some(fields) = snapshot.as_object_mut() {
                fields.remove("lifecycle");
            }
            true
        });
    status
}

pub(super) fn reconcile_issue(
    db: &Connection,
    project: &str,
    number: i64,
    actor: &Actor,
) -> Result<()> {
    if !is_watching(db, project, number)? {
        return Ok(());
    }
    let issue = get_issue(db, project, number, true)?;
    if issue.state == "closed" || issue.deleted_at.is_some() {
        return Ok(());
    }
    let links = registry::pull_requests(db, project, number)?;
    let has_open = has_open_pr(&links);
    let (_, previous) = saved(db, project, number)?;
    let mut status = linked_status(previous.clone(), &links, !has_open);
    db.execute("DELETE FROM issue_github_signals WHERE project_id=?1 AND issue_number=?2 AND rtrim(url,'/') NOT IN (SELECT rtrim(url,'/') FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2)", params![project,number])?;
    if has_open {
        if status != previous {
            db.execute(
                "UPDATE issue_github_watches SET status=?3 WHERE project_id=?1 AND issue_number=?2",
                params![project, number, status.to_string()],
            )?;
        }
        return Ok(());
    }
    let now = crate::issues::worker::now();
    let mut watcher = actor.clone();
    watcher.id = WATCHER.into();
    let task_project = Project {
        id: project.into(),
        name: db.query_row("SELECT name FROM projects WHERE id=?1", [project], |r| {
            r.get(0)
        })?,
    };
    mutate(
        db,
        &task_project,
        &watcher,
        &Operation::Close {
            number,
            comment: None,
            force: true,
            guard: None,
            allow_long_comment: false,
        },
        now,
    )?;
    db.execute(
        "UPDATE issues SET assignment_target=NULL WHERE project_id=?1 AND number=?2",
        params![project, number],
    )?;
    db.execute(
        "DELETE FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2",
        params![project, number],
    )?;
    status["stopped_reason"] = json!("no_open_pull_requests");
    db.execute("INSERT INTO issue_github_watches(project_id,issue_number,status) VALUES(?1,?2,?3) ON CONFLICT(project_id,issue_number) DO UPDATE SET status=excluded.status", params![project,number,status.to_string()])?;
    crate::issues::blockers::reconcile(db, project, Some(WATCHER), now)?;
    db.execute(
        "UPDATE projects SET activity_at=max(activity_at,?2) WHERE id=?1",
        params![project, now],
    )?;
    Ok(())
}

fn pending_reconciliations(db: &Connection) -> Result<Vec<(String, i64)>> {
    let tasks = db.prepare("SELECT i.project_id,i.number,w.status FROM issues i JOIN projects p ON p.id=i.project_id LEFT JOIN issue_github_watches w ON w.project_id=i.project_id AND w.issue_number=i.number WHERE i.assignment_target='github' AND i.state<>'closed' AND i.deleted_at IS NULL AND p.hidden_at IS NULL")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    // Lifecycle checks need only the linked URLs and states. Batch those keys
    // without loading attachment provenance or querying each task separately.
    let keys: Vec<_> = tasks
        .iter()
        .map(|(project, number, _)| (project, number))
        .collect();
    let mut links = std::collections::BTreeMap::<(String, i64), Vec<Value>>::new();
    let mut query = db.prepare("SELECT pr.project_id,pr.issue_number,pr.url,pr.status FROM json_each(?1) requested CROSS JOIN issue_pull_requests pr WHERE pr.project_id=json_extract(requested.value,'$[0]') AND pr.issue_number=json_extract(requested.value,'$[1]')")?;
    for row in query.query_map([serde_json::to_string(&keys)?], |r| {
        Ok((
            (r.get::<_, String>(0)?, r.get::<_, i64>(1)?),
            json!({"url":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?}),
        ))
    })? {
        let (key, link) = row?;
        links.entry(key).or_default().push(link);
    }
    tasks
        .into_iter()
        .filter_map(|(project, number, saved)| {
            let needs_update = (|| -> Result<bool> {
                let links = links.remove(&(project.clone(), number)).unwrap_or_default();
                if !has_open_pr(&links) {
                    return Ok(true);
                }
                let status = normalize_status(
                    saved
                        .map(|s| serde_json::from_str(&s))
                        .transpose()?
                        .unwrap_or(json!({"prs":{}})),
                );
                Ok(linked_status(status.clone(), &links, false) != status)
            })();
            match needs_update {
                Ok(false) => None,
                Ok(true) => Some(Ok((project, number))),
                Err(error) => Some(Err(error)),
            }
        })
        .collect()
}

impl Store {
    pub(crate) fn reconcile_github_assignments(&mut self, actor: &Actor) -> Result<()> {
        if pending_reconciliations(&self.db)?.is_empty() {
            return Ok(());
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (project, number) in pending_reconciliations(&tx)? {
            reconcile_issue(&tx, &project, number, actor)?;
        }
        tx.commit()?;
        Ok(())
    }
}
