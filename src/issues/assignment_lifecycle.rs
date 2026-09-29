//! A watcher without an open supported PR cannot do useful work.
use super::*;

fn has_open_pr(db: &Connection, project: &str, number: i64) -> Result<bool> {
    Ok(registry::pull_requests(db, project, number)?
        .iter()
        .any(|pr| {
            pr["status"] != "merged"
                && pr["status"] != "closed"
                && pr["url"]
                    .as_str()
                    .and_then(hey_gh::watcher::pull_request_selector)
                    .is_some()
        }))
}

pub(super) fn reconcile_issue(
    db: &Connection,
    project: &str,
    number: i64,
    actor: &Actor,
) -> Result<()> {
    if !is_watching(db, project, number)? || has_open_pr(db, project, number)? {
        return Ok(());
    }
    let issue = get_issue(db, project, number, true)?;
    if issue.state == "closed" || issue.deleted_at.is_some() {
        return Ok(());
    }
    let now = crate::issues::worker::now();
    let active = issue
        .assignee
        .as_deref()
        .filter(|id| *id != WATCHER && *id != "human:boss");
    if active.is_none() {
        ready::register_boss(db, actor, now)?;
    }
    let assignee = active.unwrap_or("human:boss");
    db.execute("UPDATE issues SET assignment_target=NULL,assignee=?3,version=version+1,updated_at=max(updated_at,?4) WHERE project_id=?1 AND number=?2",params![project,number,assignee,now])?;
    if active.is_none() {
        db.execute(
            "DELETE FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2",
            params![project, number],
        )?;
    }
    let (_, mut status) = saved(db, project, number)?;
    status["stopped_reason"] = json!("no_open_pull_requests");
    let links = registry::pull_requests(db, project, number)?;
    for (url, snapshot) in status["prs"].as_object_mut().unwrap() {
        snapshot["lifecycle"] = links
            .iter()
            .find(|pr| pr["url"] == *url)
            .map(|pr| pr["status"].clone())
            .unwrap_or(json!("removed"));
    }
    db.execute("INSERT INTO issue_github_watches(project_id,issue_number,status) VALUES(?1,?2,?3) ON CONFLICT(project_id,issue_number) DO UPDATE SET status=excluded.status", params![project,number,status.to_string()])?;
    event(
        db,
        project,
        number,
        WATCHER,
        "assigned",
        now,
        &json!({"target":if active.is_some() {"agent"} else {"boss"},"assignee":assignee,"previous_assignee":issue.assignee,"reason":"no_open_pull_requests"}),
    )?;
    Ok(())
}

fn waiting_without_pr(db: &Connection) -> Result<Vec<(String, i64)>> {
    let tasks = db.prepare("SELECT i.project_id,i.number FROM issues i JOIN projects p ON p.id=i.project_id WHERE i.assignment_target='github' AND i.state<>'closed' AND i.deleted_at IS NULL AND p.hidden_at IS NULL")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    tasks
        .into_iter()
        .filter_map(
            |(project, number)| match has_open_pr(db, &project, number) {
                Ok(true) => None,
                Ok(false) => Some(Ok((project, number))),
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

impl Store {
    pub(crate) fn reconcile_github_assignments(&mut self, actor: &Actor) -> Result<()> {
        if waiting_without_pr(&self.db)?.is_empty() {
            return Ok(());
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (project, number) in waiting_without_pr(&tx)? {
            reconcile_issue(&tx, &project, number, actor)?;
        }
        tx.commit()?;
        Ok(())
    }
}
