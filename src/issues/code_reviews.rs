//! PR review drafts are independent of tasks and checkouts.
use super::{Error, Project, Result};
use crate::database::Connection;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const SCHEMA:&str = "
CREATE TABLE IF NOT EXISTS pr_code_reviews(
 id INTEGER PRIMARY KEY,project_id TEXT NOT NULL,number INTEGER NOT NULL,head TEXT NOT NULL,title TEXT NOT NULL,
 stage TEXT NOT NULL,verdict TEXT NOT NULL,body TEXT NOT NULL,version INTEGER NOT NULL,
 created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,receipt TEXT NOT NULL DEFAULT '',published_by TEXT NOT NULL DEFAULT '');
CREATE INDEX IF NOT EXISTS pr_code_reviews_queue ON pr_code_reviews(project_id,number,updated_at);
CREATE TABLE IF NOT EXISTS pr_code_comments(id INTEGER PRIMARY KEY,project_id TEXT NOT NULL,review_id INTEGER NOT NULL,
 ordinal INTEGER NOT NULL,anchor TEXT NOT NULL,snippet TEXT NOT NULL,body TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS pr_code_comment_review ON pr_code_comments(project_id,review_id,id);";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comment {
    pub path: String,
    pub line: u32,
    pub side: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_side: Option<String>,
    pub body: String,
    pub snippet: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    List,
    View {
        id: i64,
    },
    ForPr {
        number: i64,
    },
    Record {
        number: i64,
        head: String,
        title: String,
        comments: Vec<Comment>,
        verdict: Option<String>,
        body: Option<String>,
    },
    Edit {
        id: i64,
        version: i64,
        comment: Option<i64>,
        body: String,
    },
    ClaimPublication {
        id: i64,
        version: i64,
        digest: String,
    },
    RejectedPublication {
        id: i64,
        version: i64,
    },
    FinishPublication {
        id: i64,
        version: i64,
        receipt: String,
        author: String,
    },
}
impl Operation {
    pub fn writes(&self) -> bool {
        !matches!(self, Self::List | Self::View { .. } | Self::ForPr { .. })
    }
}
const SELECT: &str = "SELECT id,number,head,title,stage,verdict,body,version,updated_at,receipt,published_by FROM pr_code_reviews";
fn row(r: &crate::database::Row) -> rusqlite::Result<Value> {
    Ok(
        json!({"id":r.get::<_,i64>(0)?,"number":r.get::<_,i64>(1)?,"head":r.get::<_,String>(2)?,"title":r.get::<_,String>(3)?,"stage":r.get::<_,String>(4)?,"verdict":r.get::<_,String>(5)?,"body":r.get::<_,String>(6)?,"version":r.get::<_,i64>(7)?,"updated_at":r.get::<_,i64>(8)?,"receipt":r.get::<_,String>(9)?,"published_by":r.get::<_,String>(10)?}),
    )
}
fn view(db: &Connection, p: &str, id: i64) -> Result<Value> {
    let mut review = db
        .query_row(
            &format!("{SELECT} WHERE project_id=?1 AND id=?2"),
            params![p, id],
            row,
        )
        .optional()?
        .ok_or_else(|| Error::new("not_found", "Review not found"))?;
    review["comments"]=json!(db.query_collect("SELECT id,anchor,snippet,body FROM pr_code_comments WHERE project_id=?1 AND review_id=?2 ORDER BY ordinal,id",params![p,id],|r|Ok::<_,rusqlite::Error>(json!({"id":r.get::<_,i64>(0)?,"anchor":serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap_or(Value::Null),"snippet":r.get::<_,String>(2)?,"body":r.get::<_,String>(3)?})))?);
    use sha2::{Digest, Sha256};
    review["digest"] = json!(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&review)?)
    ));
    Ok(review)
}
fn valid_text(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 65536
}
pub(super) fn execute(db: &Connection, p: &Project, op: &Operation, now: i64) -> Result<Value> {
    if !db.query_row("SELECT coalesce((SELECT code_reviews_enabled FROM project_settings WHERE project_id=?1),0)",[&p.id],|r|r.get::<_,bool>(0))?{return Err(Error::invalid("Code Reviews is disabled for this project"))}
    let id = match op {
        Operation::List => {
            let sql = format!("SELECT r.*,
                (SELECT count(*) FROM pr_code_comments WHERE project_id=?1 AND review_id=r.id),
                (SELECT substr(body,1,240) FROM pr_code_comments WHERE project_id=?1 AND review_id=r.id ORDER BY ordinal,id LIMIT 1)
                FROM ({SELECT} WHERE project_id=?1 ORDER BY updated_at DESC,id DESC LIMIT 200) r
                ORDER BY r.updated_at DESC,r.id DESC");
            let reviews = db.query_collect(&sql, [&p.id], |r| {
                let mut item = row(r)?;
                item["comment_count"] = json!(r.get::<_, i64>(11)?);
                item["first_comment"] = json!(r.get::<_, Option<String>>(12)?);
                Ok::<_, rusqlite::Error>(item)
            })?;
            return Ok(json!({"ok":true,"reviews":reviews}));
        }
        Operation::ForPr { number } => {
            let ids=db.query_collect("SELECT id FROM pr_code_reviews WHERE project_id=?1 AND number=?2 ORDER BY updated_at DESC,id DESC",params![p.id,number],|r|r.get::<_,i64>(0))?;
            return Ok(
                json!({"ok":true,"reviews":ids.into_iter().map(|id|view(db,&p.id,id)).collect::<Result<Vec<_>>>()?}),
            );
        }
        Operation::View { id } => *id,
        Operation::Record {
            number,
            head,
            title,
            comments,
            verdict,
            body,
        } => {
            if *number < 1
                || head.len() != 40
                || !head.bytes().all(|c| c.is_ascii_hexdigit())
                || !valid_text(title)
                || comments.len() > 100
                || body.as_ref().is_some_and(|s| s.len() > 65536)
                || verdict
                    .as_deref()
                    .is_some_and(|v| !matches!(v, "COMMENT" | "APPROVE" | "REQUEST_CHANGES"))
            {
                return Err(Error::invalid("Invalid GitHub review"));
            }
            for c in comments {
                if !valid_text(&c.body)
                    || c.path.is_empty()
                    || c.path.len() > 4096
                    || c.line == 0
                    || !matches!(c.side.as_str(), "LEFT" | "RIGHT")
                    || c.snippet.len() > 65536
                    || c.start_line.is_some_and(|n| n == 0 || n > c.line)
                    || c.start_side.as_ref().is_some_and(|s| s != &c.side)
                {
                    return Err(Error::invalid("Invalid inline review comment"));
                }
            }
            let previous:Option<(i64,String,String)>=db.query_row("SELECT id,head,stage FROM pr_code_reviews WHERE project_id=?1 AND number=?2 AND stage IN ('draft','publishing') ORDER BY updated_at DESC LIMIT 1",params![p.id,number],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let mut reuse = None;
            if let Some((id, sha, stage)) = previous {
                if stage == "publishing" {
                    return Err(Error::conflict("This PR has an unconfirmed publication"));
                }
                if sha == *head {
                    reuse = Some(id)
                } else if verdict.is_some() {
                    return Err(Error::conflict(
                        "The PR changed since comments were written. Review the new diff first.",
                    ));
                } else {
                    db.execute("UPDATE pr_code_reviews SET stage='outdated',version=version+1,updated_at=?2 WHERE id=?1",params![id,now])?;
                }
            }
            let id = if let Some(id) = reuse {
                id
            } else {
                let id = new_id(db)?;
                db.execute("INSERT INTO pr_code_reviews(id,project_id,number,head,title,stage,verdict,body,version,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'draft','COMMENT','',0,?6,?6)",params![id,p.id,number,head,title,now])?;
                id
            };
            let ordinal:i64=db.query_row("SELECT coalesce(max(ordinal),0) FROM pr_code_comments WHERE project_id=?1 AND review_id=?2",params![p.id,id],|r|r.get(0))?;
            if verdict.as_deref() == Some("REQUEST_CHANGES")
                && body.as_ref().is_none_or(|s| s.trim().is_empty())
            {
                return Err(Error::invalid("A change request requires a summary"));
            }
            if verdict.as_deref() == Some("COMMENT")
                && body.as_ref().is_none_or(|s| s.trim().is_empty())
                && ordinal == 0
                && comments.is_empty()
            {
                return Err(Error::invalid(
                    "A comment review requires a summary or inline comment",
                ));
            }
            for (index, c) in comments.iter().enumerate() {
                let mut anchor = json!({"path":c.path,"side":c.side,"line":c.line});
                if let Some(n) = c.start_line {
                    anchor["start_line"] = json!(n);
                    anchor["start_side"] = json!(c.start_side.as_ref().unwrap_or(&c.side))
                }
                db.execute(
                    "INSERT INTO pr_code_comments VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        new_id(db)?,
                        p.id,
                        id,
                        ordinal + 1 + index as i64,
                        anchor.to_string(),
                        c.snippet,
                        c.body
                    ],
                )?;
            }
            db.execute("UPDATE pr_code_reviews SET stage=?2,verdict=coalesce(?3,verdict),body=coalesce(?4,body),updated_at=?5,version=version+1 WHERE id=?1",params![id,if verdict.is_some(){"submitted"}else{"draft"},verdict,body,now])?;
            id
        }
        Operation::Edit {
            id,
            version,
            comment,
            body,
        } => {
            if body.len() > 65536 || comment.is_some() && !valid_text(body) {
                return Err(Error::invalid("Invalid comment body"));
            }
            if db.execute("UPDATE pr_code_reviews SET body=CASE WHEN ?5 IS NULL THEN ?4 ELSE body END,version=version+1,updated_at=?6 WHERE project_id=?1 AND id=?2 AND version=?3 AND stage IN ('draft','submitted')",params![p.id,id,version,body,comment,now])?!=1{return Err(stale())}
            if let Some(c)=comment && db.execute("UPDATE pr_code_comments SET body=?4 WHERE project_id=?1 AND review_id=?2 AND id=?3",params![p.id,id,c,body])?!=1{return Err(Error::invalid("Comment not found"))}
            *id
        }
        Operation::ClaimPublication {
            id,
            version,
            digest,
        } => {
            if view(db, &p.id, *id)?["digest"] != digest.as_str() {
                return Err(stale());
            }
            if db.query_row("SELECT role='peer' FROM fleet_meta WHERE id=1", [], |r| {
                r.get::<_, bool>(0)
            })? && !db.query_row(
                "SELECT coalesce(controller=origin,0) FROM distributed_meta WHERE id=1",
                [],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Error::invalid("Publish from the controller's review queue"));
            }
            if db.execute("UPDATE pr_code_reviews SET stage='publishing',version=version+1,updated_at=?4 WHERE project_id=?1 AND id=?2 AND version=?3 AND stage='submitted'",params![p.id,id,version,now])?!=1{return Err(stale())}
            *id
        }
        Operation::RejectedPublication { id, version } => {
            if db.execute("UPDATE pr_code_reviews SET stage='submitted',version=version+1,updated_at=?4 WHERE project_id=?1 AND id=?2 AND version=?3 AND stage='publishing'",params![p.id,id,version,now])?!=1{return Err(stale())}
            *id
        }
        Operation::FinishPublication {
            id,
            version,
            receipt,
            author,
        } => {
            if !receipt.starts_with(&format!("https://{}/pull/", p.id)) || receipt.len() > 2048 {
                return Err(Error::invalid("Invalid review receipt"));
            }
            if db.execute("UPDATE pr_code_reviews SET stage='published',receipt=?4,published_by=?6,version=version+1,updated_at=?5 WHERE project_id=?1 AND id=?2 AND version=?3 AND stage='publishing'",params![p.id,id,version,receipt,now,author])?!=1{return Err(stale())}
            *id
        }
    };
    Ok(json!({"ok":true,"review":view(db,&p.id,id)?}))
}
// Stay within JavaScript's exact integer range on all replicas.
fn new_id(db: &Connection) -> Result<i64> {
    Ok(db.query_row("SELECT 1+(random() & 4503599627370495)", [], |r| r.get(0))?)
}
fn stale() -> Error {
    Error::conflict("Review changed or was already published; reload before continuing")
}
