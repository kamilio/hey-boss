//! Project shelves share the task review tables. Scope 0 is an unlinked review.
use super::{Actor, Error, Project, Result, commit_reviews, commits};
use crate::database::Connection;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    List,
    Save {
        sha: String,
        raw_commit: String,
        task: Option<i64>,
        snapshot: Option<commit_reviews::Snapshot>,
    },
    Open {
        sha: String,
    },
    Link {
        sha: String,
        task: i64,
    },
    Comment {
        sha: String,
        location: commit_reviews::Location,
        body: String,
    },
}

impl Operation {
    pub fn writes(&self) -> bool {
        !matches!(self, Self::List | Self::Open { .. })
    }
}

fn find(db: &Connection, project: &Project, prefix: &str) -> Result<(String, i64)> {
    if prefix.len() < 4 || !prefix.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::invalid(
            "Use at least four hexadecimal digits of the shelved commit",
        ));
    }
    let rows = db.query_collect(
        "SELECT sha, issue_number FROM commit_shelf WHERE project_id=?1 AND sha GLOB ?2 LIMIT 2",
        params![project.id, format!("{}*", prefix.to_ascii_lowercase())],
        |r| Ok::<_, rusqlite::Error>((r.get(0)?, r.get(1)?)),
    )?;
    match rows.as_slice() {
        [entry] => Ok(entry.clone()),
        [] => Err(Error::invalid("Commit not found on this project shelf")),
        _ => Err(Error::invalid("Ambiguous shelf SHA; use more digits")),
    }
}

fn link(
    db: &Connection,
    p: &Project,
    sha: &str,
    scope: i64,
    task: i64,
    actor: &Actor,
    now: i64,
) -> Result<()> {
    super::store::get_issue(db, &p.id, task, false)?;
    if scope != 0 && scope != task {
        return Err(Error::conflict(
            "This shelf entry is already linked to another task",
        ));
    }
    let (snapshot,message):(String,String)=db.query_row("SELECT snapshot,message FROM commit_reviews WHERE project_id=?1 AND issue_number=?2 AND sha=?3",params![p.id,scope,sha],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let url = commits::canonical_commit_url(&p.id, None, None, sha, None);
    commits::add(db, p, task, &url, message.lines().next(), actor, now)?;
    if scope != task {
        commit_reviews::attach(db, &p.id, task, &url, &serde_json::from_str(&snapshot)?)?;
        db.execute("UPDATE commit_review_comments SET issue_number=?2 WHERE project_id=?1 AND issue_number=0 AND sha=?3",params![p.id,task,sha])?;
        db.execute(
            "DELETE FROM commit_reviews WHERE project_id=?1 AND issue_number=0 AND sha=?2",
            params![p.id, sha],
        )?;
        db.execute(
            "UPDATE commit_shelf SET issue_number=?2 WHERE project_id=?1 AND sha=?3",
            params![p.id, task, sha],
        )?;
    }
    Ok(())
}

pub(super) fn execute(
    db: &Connection,
    p: &Project,
    op: &Operation,
    actor: Option<&Actor>,
    now: i64,
) -> Result<Value> {
    match op {
        Operation::List => {
            let entries=db.query_collect("SELECT s.sha,s.issue_number,substr(r.message,1,instr(r.message||char(10),char(10))-1) FROM commit_shelf s JOIN commit_reviews r USING(project_id,issue_number,sha) WHERE s.project_id=?1 ORDER BY s.saved_at DESC,s.sha", [&p.id], |r|Ok::<_,rusqlite::Error>(json!({"sha":r.get::<_,String>(0)?,"task":r.get::<_,i64>(1)?,"title":r.get::<_,String>(2)?})))?;
            Ok(json!({"ok":true,"project":p,"entries":entries}))
        }
        Operation::Save {
            sha,
            raw_commit,
            task,
            snapshot,
        } => {
            let snapshot = snapshot
                .as_ref()
                .ok_or_else(|| Error::invalid("Commit must exist in the invoking checkout"))?;
            commit_reviews::validate(snapshot)?;
            if !matches!(sha.len(), 40 | 64)
                || !sha.bytes().all(|c| c.is_ascii_hexdigit())
                || snapshot.parent.is_empty()
                || raw_commit.is_empty()
                || raw_commit.len() > 256 * 1024
            {
                return Err(Error::invalid("Expected an ordinary single-parent commit"));
            }
            let inserted=db.execute("INSERT INTO commit_shelf(project_id,sha,raw_commit,saved_at) VALUES(?1,?2,?3,?4) ON CONFLICT(project_id,sha) DO NOTHING",params![p.id,sha,raw_commit,now])?;
            if inserted != 0 {
                db.execute(
                    "INSERT INTO commit_reviews VALUES(?1,0,?2,?3,?4)",
                    params![
                        p.id,
                        sha,
                        serde_json::to_string(snapshot)?,
                        snapshot.message
                    ],
                )?;
            }
            let (_, scope) = find(db, p, sha)?;
            if let Some(task) = task {
                link(db, p, sha, scope, *task, actor.unwrap(), now)?;
            }
            Ok(json!({"ok":true,"sha":sha}))
        }
        Operation::Link { sha, task } => {
            let (sha, scope) = find(db, p, sha)?;
            link(db, p, &sha, scope, *task, actor.unwrap(), now)?;
            Ok(json!({"ok":true,"sha":sha,"task":task}))
        }
        Operation::Open { sha } => {
            let (sha, scope) = find(db, p, sha)?;
            let mut data = commit_reviews::execute_sha(
                db,
                p,
                scope,
                &sha,
                &commit_reviews::Operation::View,
                None,
                now,
            )?;
            data["raw_commit"] = json!(db.query_row(
                "SELECT raw_commit FROM commit_shelf WHERE project_id=?1 AND sha=?2",
                params![p.id, sha],
                |r| r.get::<_, String>(0)
            )?);
            Ok(data)
        }
        Operation::Comment {
            sha,
            location,
            body,
        } => {
            let (sha, scope) = find(db, p, sha)?;
            commit_reviews::execute_sha(
                db,
                p,
                scope,
                &sha,
                &commit_reviews::Operation::Comment {
                    location: location.clone(),
                    body: body.clone(),
                },
                actor,
                now,
            )
        }
    }
}

// An ordinary unlink keeps the commit and its discussion available on the shelf.
pub(super) fn detach(db: &Connection, p: &str, task: i64, sha: &str) -> Result<()> {
    let entry = db
        .query_row(
            "SELECT 1 FROM commit_shelf WHERE project_id=?1 AND issue_number=?2 AND sha=?3",
            params![p, task, sha],
            |_| Ok(()),
        )
        .optional()?;
    if entry.is_some() {
        for table in ["commit_shelf", "commit_reviews", "commit_review_comments"] {
            db.execute(&format!("UPDATE {table} SET issue_number=0 WHERE project_id=?1 AND issue_number=?2 AND sha=?3"),params![p,task,sha])?;
        }
    }
    Ok(())
}
