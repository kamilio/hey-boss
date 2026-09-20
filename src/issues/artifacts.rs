use super::{body, get_issue};
use crate::artifacts::Operation;
use crate::issues::{Error, Project, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::io::Read;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS artifacts(project_id TEXT NOT NULL REFERENCES projects(id),id TEXT NOT NULL,title TEXT NOT NULL,body TEXT NOT NULL,version INTEGER NOT NULL DEFAULT 1,archived INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(project_id,id));
CREATE INDEX IF NOT EXISTS artifacts_recent ON artifacts(project_id,archived,updated_at DESC,id);
CREATE TABLE IF NOT EXISTS artifact_comments(id INTEGER PRIMARY KEY AUTOINCREMENT,project_id TEXT NOT NULL,artifact_id TEXT NOT NULL,parent INTEGER REFERENCES artifact_comments(id),author TEXT NOT NULL,body TEXT NOT NULL,quote TEXT,prefix TEXT,suffix TEXT,resolved INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL,FOREIGN KEY(project_id,artifact_id) REFERENCES artifacts(project_id,id));
CREATE INDEX IF NOT EXISTS artifact_comment_document ON artifact_comments(project_id,artifact_id,id);
CREATE TABLE IF NOT EXISTS artifact_links(project_id TEXT NOT NULL,artifact_id TEXT NOT NULL,kind TEXT NOT NULL,target TEXT NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY(project_id,artifact_id,kind,target),FOREIGN KEY(project_id,artifact_id) REFERENCES artifacts(project_id,id));
CREATE INDEX IF NOT EXISTS artifact_link_target ON artifact_links(project_id,kind,target);
";
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(
        json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"body":r.get::<_,String>(2)?,"version":r.get::<_,i64>(3)?,"archived":r.get::<_,bool>(4)?,"created_at":r.get::<_,i64>(5)?,"updated_at":r.get::<_,i64>(6)?}),
    )
}
pub(super) fn get(db: &Connection, p: &str, id: &str) -> Result<Value> {
    db.query_row("SELECT id,title,body,version,archived,created_at,updated_at FROM artifacts WHERE project_id=?1 AND id=?2",params![p,id],row).optional()?.ok_or_else(||Error::new("not_found","Artifact not found in this project"))
}
fn target(
    db: &Connection,
    p: &Project,
    issue: Option<i64>,
    node: Option<&str>,
    check: bool,
) -> Result<(String, String)> {
    if let Some(n) = issue {
        if check {
            get_issue(db, &p.id, n, false)?;
        }
        return Ok(("issue".into(), n.to_string()));
    }
    let n = node.unwrap();
    let id:Option<String>=db.query_row("SELECT id FROM mindmap_nodes WHERE project_id=?1 AND (id=?2 OR alias=?2 OR (kind='issue' AND 'issue:'||reference=?2)) LIMIT 1",params![p.id,n],|r|r.get(0)).optional()?;
    if !check {
        return Ok(("node".into(), id.unwrap_or_else(|| n.into())));
    }
    Ok((
        "node".into(),
        id.ok_or_else(|| Error::new("not_found", "Mindmap node not found in this project"))?,
    ))
}
pub(super) fn links(
    db: &Connection,
    p: &Project,
    issue: Option<i64>,
    node: Option<&str>,
) -> Result<Value> {
    let (kind, target) = target(db, p, issue, node, false)?;
    let mut stmt=db.prepare("SELECT a.id,a.title,'',a.version,a.archived,a.created_at,a.updated_at FROM artifact_links l JOIN artifacts a ON a.project_id=l.project_id AND a.id=l.artifact_id WHERE l.project_id=?1 AND l.kind=?2 AND l.target=?3 ORDER BY a.updated_at DESC,a.id LIMIT 1001")?;
    let mut rows = stmt
        .query_map(params![p.id, kind, target], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() > 1000;
    rows.truncate(1000);
    Ok(json!({"ok":true,"project":p,"artifacts":rows,"more":more}))
}

/// A project scan enriches all map cards without a query for every node.
pub(super) fn enrich_nodes(db: &Connection, nodes: &mut [Value]) -> Result<()> {
    use std::collections::{BTreeSet, HashMap};
    let projects: BTreeSet<_> = nodes
        .iter()
        .filter_map(|n| n["project_id"].as_str())
        .collect();
    let mut attached: HashMap<String, Vec<Value>> = HashMap::new();
    let mut stmt=db.prepare("SELECT l.target,a.id,a.title,'',a.version,a.archived,a.created_at,a.updated_at FROM artifact_links l JOIN artifacts a ON a.project_id=l.project_id AND a.id=l.artifact_id JOIN mindmap_nodes n ON n.id=l.target AND n.project_id=l.project_id WHERE l.kind='node' AND l.project_id=?1 ORDER BY a.updated_at DESC,a.id")?;
    for project in projects {
        let mut rows = stmt.query([project])?;
        while let Some(r) = rows.next()? {
            let target: String = r.get(0)?;
            let items = attached.entry(target).or_default();
            if items.len() >= 1000 {
                return Err(Error::invalid(
                    "Too many artifacts on a map node; unlink a reference using the CLI",
                ));
            }
            items.push(json!({"id":r.get::<_,String>(1)?,"title":r.get::<_,String>(2)?,"version":r.get::<_,i64>(4)?,"archived":r.get::<_,bool>(5)?,"updated_at":r.get::<_,i64>(7)?}));
        }
    }
    for node in nodes {
        node["artifacts"] = json!(
            attached
                .remove(node["id"].as_str().unwrap())
                .unwrap_or_default()
        );
    }
    Ok(())
}
fn view(db: &Connection, p: &Project, id: &str) -> Result<Value> {
    let mut artifact = get(db, &p.id, id)?;
    let source = artifact["body"].as_str().unwrap();
    let rendered = crate::markdown::render_fragment(source);
    let mut stmt=db.prepare("SELECT id,parent,author,body,quote,prefix,suffix,resolved,created_at FROM artifact_comments WHERE project_id=?1 AND artifact_id=?2 ORDER BY id LIMIT 1001")?;
    let mut comments=stmt.query_map(params![p.id,id],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"parent":r.get::<_,Option<i64>>(1)?,"author":r.get::<_,String>(2)?,"body":r.get::<_,String>(3)?,"quote":r.get::<_,Option<String>>(4)?,"prefix":r.get::<_,Option<String>>(5)?,"suffix":r.get::<_,Option<String>>(6)?,"resolved":r.get::<_,bool>(7)?,"created_at":r.get::<_,i64>(8)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let plain = rendered_text(&rendered);
    for c in &mut comments {
        c["outdated"] = json!(c["quote"].as_str().is_some_and(|q| !anchor_matches(
            &plain,
            q,
            c["prefix"].as_str().unwrap_or(""),
            c["suffix"].as_str().unwrap_or("")
        )));
        c["body_html"] = json!(crate::markdown::render_fragment(
            c["body"].as_str().unwrap()
        ));
    }
    let more = comments.len() > 1000;
    comments.truncate(1000);
    let mut stmt=db.prepare("SELECT l.kind,l.target,CASE l.kind WHEN 'issue' THEN (SELECT title FROM issues WHERE project_id=l.project_id AND number=CAST(l.target AS INTEGER)) ELSE (SELECT coalesce(display_label,title) FROM mindmap_nodes WHERE project_id=l.project_id AND id=l.target) END FROM artifact_links l WHERE l.project_id=?1 AND l.artifact_id=?2 ORDER BY l.created_at LIMIT 1000")?;
    let backlinks=stmt.query_map(params![p.id,id],|r|Ok(json!({"kind":r.get::<_,String>(0)?,"target":r.get::<_,String>(1)?,"title":r.get::<_,Option<String>>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    artifact["body_html"] = json!(rendered);
    let result = json!({"ok":true,"project":p,"artifact":artifact,"comments":comments,"more_comments":more,"backlinks":backlinks});
    if serde_json::to_vec(&result)?.len() > crate::issues::WIRE_LIMIT - 65536 {
        return Err(Error::invalid(
            "Rendered artifact exceeds the response budget; shorten the document or comment",
        ));
    }
    Ok(result)
}
// The browser anchors against the rendered reading surface, not Markdown syntax.
fn rendered_text(html: &str) -> String {
    let mut text = String::new();
    let mut tag = false;
    let mut quote = None;
    // This is our escaped renderer output. Match DOM text without inventing
    // separators between adjacent cells or dropping generated footnote numbers.
    for c in html.chars() {
        if tag {
            if quote == Some(c) {
                quote = None;
            } else if quote.is_none() {
                match c {
                    '\'' | '"' => quote = Some(c),
                    '>' => tag = false,
                    _ => {}
                }
            }
        } else if c == '<' {
            tag = true;
        } else {
            text.push(c);
        }
    }
    html_escape::decode_html_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn anchor_matches(text: &str, quote: &str, prefix: &str, suffix: &str) -> bool {
    let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let quote = normalize(quote);
    let prefix = normalize(prefix);
    let suffix = normalize(suffix);
    text.match_indices(&quote).any(|(i, q)| {
        text[..i].trim_end().ends_with(&prefix)
            && text[i + q.len()..].trim_start().starts_with(&suffix)
    })
}
pub(super) fn execute(
    db: &Connection,
    p: &Project,
    op: &Operation,
    author: &str,
    now: i64,
) -> Result<Value> {
    match op {
        Operation::Preview { body } => {
            return Ok(json!({"ok":true,"html":crate::markdown::render_fragment(body)}));
        }
        Operation::List {
            query,
            archived,
            offset,
        } => {
            let mut stmt=db.prepare("SELECT id,title,'',version,archived,created_at,updated_at FROM artifacts WHERE project_id=?1 AND archived=?2 AND (?3='' OR instr(lower(title),lower(?3))>0 OR instr(lower(body),lower(?3))>0) ORDER BY updated_at DESC,id LIMIT 51 OFFSET ?4")?;
            let mut rows = stmt
                .query_map(
                    params![
                        p.id,
                        archived,
                        query.as_deref().unwrap_or(""),
                        *offset as i64
                    ],
                    row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let more = rows.len() > 50;
            rows.truncate(50);
            return Ok(json!({"ok":true,"project":p,"artifacts":rows,"more":more}));
        }
        Operation::Links { issue, node } => return links(db, p, *issue, node.as_deref()),
        Operation::Create {
            title,
            body: text,
            issue,
            node,
        } => {
            let mut bytes = [0u8; 16];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
            let id = format!(
                "a-{}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            );
            db.execute("INSERT INTO artifacts(project_id,id,title,body,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)",params![p.id,id,title,text,now])?;
            if issue.is_some() || node.is_some() {
                let (kind, target) = target(db, p, *issue, node.as_deref(), true)?;
                db.execute(
                    "INSERT INTO artifact_links VALUES(?1,?2,?3,?4,?5)",
                    params![p.id, id, kind, target, now],
                )?;
            }
            return view(db, p, &id);
        }
        _ => {}
    }
    let id = match op {
        Operation::View { id }
        | Operation::Edit { id, .. }
        | Operation::Archive { id, .. }
        | Operation::Comment { id, .. }
        | Operation::Resolve { id, .. }
        | Operation::Link { id, .. }
        | Operation::Unlink { id, .. } => id,
        _ => unreachable!(),
    };
    let artifact = get(db, &p.id, id)?;
    match op {
        Operation::Edit {
            title,
            body: text,
            if_version,
            ..
        } => {
            if artifact["version"] != *if_version {
                return Err(Error::conflict(
                    "Artifact changed on another device. Your draft is preserved; reload the latest revision before merging.",
                ));
            }
            db.execute("UPDATE artifacts SET title=?3,body=?4,version=version+1,updated_at=?5 WHERE project_id=?1 AND id=?2",params![p.id,id,title.as_deref().unwrap_or(artifact["title"].as_str().unwrap()),text.as_deref().unwrap_or(artifact["body"].as_str().unwrap()),now])?;
        }
        Operation::Archive {
            archived,
            if_version,
            ..
        } => {
            if artifact["version"] != *if_version {
                return Err(Error::conflict(
                    "Artifact changed; reload before archiving/restoring",
                ));
            }
            db.execute("UPDATE artifacts SET archived=?3,version=version+1,updated_at=?4 WHERE project_id=?1 AND id=?2",params![p.id,id,archived,now])?;
        }
        Operation::Comment {
            body: text,
            quote,
            prefix,
            suffix,
            parent,
            ..
        } => {
            body(text, true)?;
            // Bound full threads to the read budget; never omit saved replies silently.
            let (count,size):(i64,i64)=db.query_row("SELECT count(*),coalesce(sum(length(CAST(body AS BLOB))),0) FROM artifact_comments WHERE project_id=?1 AND artifact_id=?2",params![p.id,id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if count >= 1000 || size + text.len() as i64 > 8 * 1024 * 1024 {
                return Err(Error::invalid(
                    "This document has reached its comment limit",
                ));
            }
            if let Some(parent) = parent {
                let valid:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM artifact_comments WHERE project_id=?1 AND artifact_id=?2 AND id=?3 AND parent IS NULL)",params![p.id,id,parent],|r|r.get(0))?;
                if !valid {
                    return Err(Error::invalid(
                        "Reply must reference a thread in this artifact",
                    ));
                }
            }
            db.execute("INSERT INTO artifact_comments(project_id,artifact_id,parent,author,body,quote,prefix,suffix,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![p.id,id,parent,author,text,quote,prefix,suffix,now])?;
            db.execute(
                "UPDATE artifacts SET updated_at=?3 WHERE project_id=?1 AND id=?2",
                params![p.id, id, now],
            )?;
        }
        Operation::Resolve {
            comment_id,
            resolved,
            ..
        } => {
            if db.execute("UPDATE artifact_comments SET resolved=?4 WHERE project_id=?1 AND artifact_id=?2 AND id=?3 AND parent IS NULL",params![p.id,id,comment_id,resolved])?==0 {return Err(Error::new("not_found","Comment thread not found"));}
            db.execute(
                "UPDATE artifacts SET updated_at=?3 WHERE project_id=?1 AND id=?2",
                params![p.id, id, now],
            )?;
        }
        Operation::Link { issue, node, .. } | Operation::Unlink { issue, node, .. } => {
            let linking = matches!(op, Operation::Link { .. });
            let (kind, target) = target(db, p, *issue, node.as_deref(), linking)?;
            if linking {
                db.execute(
                    "INSERT OR IGNORE INTO artifact_links VALUES(?1,?2,?3,?4,?5)",
                    params![p.id, id, kind, target, now],
                )?;
            } else {
                db.execute("DELETE FROM artifact_links WHERE project_id=?1 AND artifact_id=?2 AND kind=?3 AND target=?4",params![p.id,id,kind,target])?;
            }
        }
        _ => {}
    }
    view(db, p, id)
}
