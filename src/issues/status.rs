//! Small, immutable progress updates. They never edit issue content or comments.
use super::*;

const SCHEMA: &str = "
CREATE TABLE issue_status_updates(
 id TEXT PRIMARY KEY,project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,
 author TEXT NOT NULL REFERENCES agents(id),
 level TEXT NOT NULL CHECK(level IN ('green','orange','red')),
 comment TEXT NOT NULL CHECK(length(comment) BETWEEN 1 AND 500),
 created_at INTEGER NOT NULL,
 FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number));
CREATE INDEX issue_status_latest ON issue_status_updates(project_id,issue_number,created_at DESC,id DESC);
";

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if exists(db)? {
        return Ok(());
    }
    let tx = rusqlite::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    if !exists(&tx)? {
        tx.execute_batch(SCHEMA)?;
    }
    tx.commit()?;
    Ok(())
}
fn exists(db: &Connection) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='issue_status_updates' AND type='table')", [], |r| r.get(0))?)
}

pub(super) fn validate(comment: &str) -> Result<()> {
    let controls = comment.chars().any(char::is_control);
    let comment = comment.trim();
    if comment.is_empty() || comment.chars().count() > 500 || controls {
        return Err(Error::invalid(
            "Status comment must be one line of plain text, between 1 and 500 characters",
        ));
    }
    Ok(())
}

pub(super) fn update(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    number: i64,
    level: super::super::StatusLevel,
    comment: &str,
    now: i64,
) -> Result<Value> {
    let issue = get_issue(db, &project.id, number, false)?;
    if issue.state != "open" || issue.draft || issue.assignee.as_deref() != Some(&actor.id) {
        return Err(Error::conflict(
            "Only the owner of an open, non-draft issue can update its status. Claim the issue first.",
        ));
    }
    super::super::fleet::check_claim(db, &project.id, number, &actor.machine, false)?;
    // A monotonic timestamp orders same-millisecond updates and clock corrections
    // identically on replicas. IDs stay stable across machines.
    db.execute("INSERT INTO issue_status_updates(id,project_id,issue_number,author,level,comment,created_at)
        VALUES(lower(hex(randomblob(16))),?1,?2,?3,?4,?5,max(?6,coalesce((SELECT max(created_at)+1 FROM issue_status_updates WHERE project_id=?1 AND issue_number=?2),?6)))",
        params![project.id,number,actor.id,level.as_str(),comment.trim(),now])?;
    Ok(
        json!({"ok":true,"project":project,"issue":get_issue(db,&project.id,number,false)?,"changed":true}),
    )
}

pub(super) fn history(
    db: &Connection,
    project: &Project,
    number: i64,
    limit: u32,
    offset: u32,
) -> Result<Value> {
    get_issue(db, &project.id, number, true)?;
    let mut query = db.prepare("SELECT id,author,level,comment,created_at FROM issue_status_updates WHERE project_id=?1 AND issue_number=?2 ORDER BY created_at DESC,id DESC LIMIT ?3 OFFSET ?4")?;
    let mut updates = query.query_map(params![project.id,number,limit+1,offset], |r| Ok(json!({"id":r.get::<_,String>(0)?,"author":r.get::<_,String>(1)?,"level":r.get::<_,String>(2)?,"comment":r.get::<_,String>(3)?,"created_at":r.get::<_,i64>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let more = updates.len() > limit as usize;
    updates.truncate(limit as usize);
    Ok(
        json!({"ok":true,"project":project,"updates":updates,"next_offset":if more {Some(offset+limit)} else {None}}),
    )
}

pub(super) fn current(db: &Connection, project: &Project, number: i64) -> Result<Value> {
    // Polling a phone must not transfer the description or scan status history.
    let assignee: Option<Option<String>> = db
        .query_row(
            "SELECT assignee FROM issues WHERE project_id=?1 AND number=?2",
            params![project.id, number],
            |r| r.get(0),
        )
        .optional()?;
    let assignee = assignee
        .ok_or_else(|| Error::new("not_found", format!("Issue #{number} was not found")))?;
    let status = db.query_row("SELECT id,author,level,comment,created_at FROM issue_status_updates WHERE project_id=?1 AND issue_number=?2 ORDER BY created_at DESC,id DESC LIMIT 1",params![project.id,number],|r|Ok(json!({"id":r.get::<_,String>(0)?,"author":r.get::<_,String>(1)?,"level":r.get::<_,String>(2)?,"comment":r.get::<_,String>(3)?,"created_at":r.get::<_,i64>(4)?}))).optional()?;
    Ok(json!({"ok":true,"project":project,"status":status,"assignee":assignee}))
}
