//! Transfers run in the issue store's immediate transaction. A source tombstone
//! keeps old links and finished worker records valid without duplicating pickup.
use super::*;

pub(super) const INDEX: &str = "CREATE INDEX IF NOT EXISTS issue_redirect ON events(project_id,issue_number,id DESC) WHERE action='moved_to';";

pub(super) fn destination(db: &Connection, project: &str, number: i64) -> Result<Option<Value>> {
    let data: Option<String> = db.query_row(
        "SELECT data FROM events WHERE project_id=?1 AND issue_number=?2 AND action='moved_to' ORDER BY id DESC LIMIT 1",
        params![project, number], |row| row.get(0)).optional()?;
    data.map(|data| serde_json::from_str(&data).map_err(Error::from))
        .transpose()
}

pub(super) fn execute(
    db: &Connection,
    source: &Project,
    actor: &Actor,
    number: i64,
    target: &str,
    version: i64,
    now: i64,
) -> Result<Value> {
    identifier(target, "destination project", 8192)?;
    if version < 1 {
        return Err(Error::invalid("Issue version must be positive"));
    }
    let issue = get_issue(db, &source.id, number, false)?;
    if issue.version != version {
        return Err(Error::conflict(
            "Issue changed; load its latest revision before moving",
        ));
    }
    let target = resolve_project(db, source, Some(target))?;
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1 AND hidden_at IS NULL)",
        [&target.id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(Error::new(
            "not_found",
            "Choose an existing visible destination project",
        ));
    }
    if target.id == source.id {
        return Err(Error::invalid("Choose a different destination project"));
    }
    let replica: bool =
        db.query_row("SELECT role='agent' FROM fleet_meta WHERE id=1", [], |r| {
            r.get(0)
        })?;
    if replica {
        return Err(Error::conflict(
            "Move issues on the fleet supervisor so both projects update together",
        ));
    }
    let active: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL)",params![source.id,number],|r|r.get(0))?;
    if active
        || issue
            .assignee
            .as_deref()
            .is_some_and(|id| id != "human:boss")
    {
        return Err(Error::conflict(
            "Stop active work and release the agent claim before moving this issue",
        ));
    }
    let linked: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM issue_subtasks WHERE project_id=?1 AND (parent_number=?2 OR child_number=?2))",params![source.id,number],|r|r.get(0))?;
    if linked {
        return Err(Error::conflict(
            "Unlink the parent and subtasks before moving this issue to another project",
        ));
    }
    let documents: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM artifact_links WHERE project_id=?1 AND kind='issue' AND target=CAST(?2 AS TEXT))",params![source.id,number],|r|r.get(0))?;
    if documents {
        return Err(Error::conflict(
            "Unlink project documents before moving this issue",
        ));
    }
    if issue.plan.is_some() {
        return Err(Error::conflict(
            "Issues with a file-synced plan cannot move between projects",
        ));
    }
    if issue.draft {
        crate::issues::planning::drafts_allowed(db, &target)?;
    }
    let next: i64 = db.query_row(
        "SELECT next_number FROM projects WHERE id=?1",
        [&target.id],
        |r| r.get(0),
    )?;
    db.execute(
        "UPDATE projects SET next_number=next_number+1 WHERE id=?1",
        [&target.id],
    )?;
    db.execute("INSERT INTO issues(project_id,number,title,body,state,assignee,created_by,closed_by,created_at,updated_at,closed_at,version,labels,sort_order,draft) SELECT ?3,?4,title,body,state,assignee,created_by,closed_by,created_at,?5,closed_at,version+1,labels,(SELECT coalesce(max(sort_order),0)+1 FROM issues WHERE project_id=?3),draft FROM issues WHERE project_id=?1 AND number=?2",params![source.id,number,target.id,next,now])?;
    // Fleet history is append-only. Copy history with fresh IDs rather than
    // relocating existing rows that replicas have already acknowledged.
    db.execute("INSERT INTO issue_agent_launches(run_id,project_id,issue_number,launched_at) SELECT run_id,?3,?4,launched_at FROM issue_agent_launches WHERE project_id=?1 AND issue_number=?2",params![source.id,number,target.id,next])?;
    let mut comments = db.prepare("SELECT id,author,body,created_at FROM comments WHERE project_id=?1 AND issue_number=?2 ORDER BY id")?;
    let mut comment_ids = std::collections::BTreeMap::new();
    for row in comments.query_map(params![source.id, number], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })? {
        let (id, author, body, created) = row?;
        db.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?1,?2,?3,?4,?5)",params![target.id,next,author,body,created])?;
        comment_ids.insert(id, db.last_insert_rowid());
    }
    let mut events = db.prepare("SELECT actor,action,created_at,data FROM events WHERE project_id=?1 AND issue_number=?2 ORDER BY id")?;
    for row in events.query_map(params![source.id, number], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
        ))
    })? {
        let (author, action, created, data) = row?;
        let mut data: Value = serde_json::from_str(&data)?;
        if let Some(id) = data["comment_id"]
            .as_i64()
            .and_then(|id| comment_ids.get(&id))
        {
            data["comment_id"] = json!(id);
            if let Some(object) = data.as_object_mut() {
                object.remove("comment_origin");
                object.remove("comment_origin_id");
            }
        }
        event(db, &target.id, next, &author, &action, created, &data)?;
    }
    db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose) SELECT ?3,?4,url,added_by,created_at,purpose FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2",params![source.id,number,target.id,next])?;
    db.execute("UPDATE mindmaps SET version=version+1 WHERE project_id IN (SELECT project_id FROM mindmap_nodes WHERE kind='issue' AND reference_project=?1 AND reference=CAST(?2 AS TEXT))",params![source.id,number])?;
    db.execute("UPDATE mindmap_nodes SET reference_project=?3,reference=CAST(?4 AS TEXT),updated_at=?5 WHERE kind='issue' AND reference_project=?1 AND reference=CAST(?2 AS TEXT)",params![source.id,number,target.id,next,now])?;
    db.execute("UPDATE issues SET deleted_at=?3,assignee=NULL,updated_at=?3,version=version+1 WHERE project_id=?1 AND number=?2",params![source.id,number,now])?;
    db.execute(
        "DELETE FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2",
        params![source.id, number],
    )?;
    let moved_to = json!({"project":target,"number":next});
    event(
        db, &source.id, number, &actor.id, "moved_to", now, &moved_to,
    )?;
    event(
        db,
        &target.id,
        next,
        &actor.id,
        "transferred",
        now,
        &json!({"from":{"project":source,"number":number},"to":moved_to}),
    )?;
    db.execute("UPDATE projects SET issue_order_version=issue_order_version+1,activity_at=max(activity_at,?3) WHERE id IN (?1,?2)",params![source.id,target.id,now])?;
    Ok(
        json!({"ok":true,"project":target,"issue":get_issue(db,&target.id,next,false)?,"changed":true}),
    )
}
