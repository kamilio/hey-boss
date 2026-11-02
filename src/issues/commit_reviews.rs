//! Portable commit snapshots and review comments. No checkout is needed to read a review.
use super::{Actor, Error, Project, Result};
use crate::database::Connection;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Command};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS commit_reviews(
 project_id TEXT NOT NULL, issue_number INTEGER NOT NULL, sha TEXT NOT NULL,
 snapshot TEXT NOT NULL CHECK(json_valid(snapshot)), message TEXT NOT NULL,
 PRIMARY KEY(project_id,issue_number,sha)
);
CREATE TABLE IF NOT EXISTS commit_review_comments(
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, issue_number INTEGER NOT NULL, sha TEXT NOT NULL,
 location TEXT NOT NULL CHECK(json_valid(location)), source TEXT NOT NULL, body TEXT NOT NULL,
 author TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS commit_review_comments_scope ON commit_review_comments(project_id,issue_number,sha,created_at,id);
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub repository: String,
    pub parent: String,
    pub message: String,
    pub author: String,
    pub email: String,
    pub authored_at: String,
    pub patch: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub file: String,
    pub side: String,
    pub line: u32,
    #[serde(rename = "endLine", default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    View,
    Patch,
    Comments,
    Comment {
        location: Location,
        body: String,
    },
    EditComment {
        id: String,
        body: String,
        previous_body: String,
    },
    DeleteComment {
        id: String,
        previous_body: String,
    },
    Message {
        message: String,
        previous_message: String,
    },
}
impl Operation {
    pub fn writes(&self) -> bool {
        !matches!(self, Self::View | Self::Patch | Self::Comments)
    }
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    use std::io::Read;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(12 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > 12 * 1024 * 1024 {
        let _ = child.kill();
        let _ = child.wait();
        read?;
        return Err(Error::invalid(
            "Commit patch exceeds 12 MiB; split it before attaching",
        ));
    }
    if !child.wait()?.success() {
        return Err(Error::invalid("Git could not read the commit snapshot"));
    }
    String::from_utf8(bytes).map_err(|_| Error::invalid("Commit contains non-UTF-8 text; convert it to UTF-8 or mark the file binary in Git before attaching"))
}
/// Capture on the caller, before a request is routed to another machine.
pub fn capture(cwd: &Path, sha: &str, repository: &str) -> Result<Option<Snapshot>> {
    if !super::commits::is_hex_sha(sha) {
        return Err(Error::invalid("Invalid commit SHA"));
    }
    if !Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .output()?
        .status
        .success()
    {
        return Ok(None);
    }
    let metadata = git(
        cwd,
        &["show", "-s", "--format=%P%x00%an%x00%ae%x00%aD%x00%B", sha],
    )?;
    let parts: Vec<_> = metadata.splitn(5, '\0').collect();
    if parts.len() != 5 {
        return Err(Error::invalid("Incomplete commit metadata"));
    }
    let parents: Vec<_> = parts[0].split_whitespace().collect();
    if parents.len() > 1 {
        return Err(Error::invalid(
            "Attach individual work commits; merge commits cannot be exported as a single patch",
        ));
    }
    let snapshot = Snapshot {
        repository: crate::agents::git_repository_info(cwd.to_str().unwrap_or(""))
            .and_then(|info| info.origin)
            .unwrap_or_else(|| repository.into()),
        parent: parents.first().copied().unwrap_or("").into(),
        author: parts[1].into(),
        email: parts[2].into(),
        authored_at: parts[3].into(),
        message: parts[4].strip_suffix('\n').unwrap_or(parts[4]).into(),
        patch: git(
            cwd,
            &[
                "-c",
                "core.quotePath=true",
                "show",
                "--format=",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                sha,
                "--",
            ],
        )?,
    };
    validate(&snapshot)?;
    Ok(Some(snapshot))
}
fn validate(snapshot: &Snapshot) -> Result<()> {
    // Leave room for JSON escaping and the task envelope in the 16 MiB transport.
    if serde_json::to_vec(snapshot)?.len() > 12 * 1024 * 1024 {
        return Err(Error::invalid(
            "Commit review exceeds the 12 MiB snapshot limit; split the commit before attaching",
        ));
    }
    for header in [&snapshot.author, &snapshot.email, &snapshot.authored_at] {
        if header.chars().any(|c| c.is_control()) {
            return Err(Error::invalid("Invalid commit mail header"));
        }
    }
    message(&snapshot.message)?;
    if !snapshot.patch.is_empty() && !snapshot.patch.starts_with("diff --git ") {
        return Err(Error::invalid("Expected a complete Git patch"));
    }
    Ok(())
}
fn message(text: &str) -> Result<()> {
    if text.trim().is_empty() || text.len() > 65536 || text.contains('\0') {
        return Err(Error::invalid(
            "Commit message must contain 1–65536 bytes and no NUL",
        ));
    }
    Ok(())
}
fn body(text: &str) -> Result<()> {
    if text.trim().is_empty() || text.len() > 32000 {
        return Err(Error::invalid("Review comment must contain 1–32000 bytes"));
    }
    Ok(())
}
fn resolve(db: &Connection, project: &str, number: i64, commit: &str) -> Result<String> {
    if commit.trim().is_empty() {
        return Err(Error::invalid("Commit reference cannot be empty"));
    }
    let target = super::commits::parse_url_commit(commit)
        .map(|(_, sha)| sha)
        .unwrap_or_else(|| commit.to_ascii_lowercase());
    let rows = super::commits::list(db, project, number)?;
    let matches: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["sha"].as_str().is_some_and(|s| s.starts_with(&target)) || row["url"] == commit
        })
        .collect();
    if matches.len() != 1 {
        return Err(Error::invalid(
            "Commit must identify exactly one attachment by SHA, prefix, or URL",
        ));
    }
    Ok(matches[0]["sha"].as_str().unwrap().into())
}
pub(crate) fn attach(
    db: &Connection,
    project: &str,
    number: i64,
    commit: &str,
    snapshot: &Snapshot,
) -> Result<bool> {
    validate(snapshot)?;
    let sha = resolve(db, project, number, commit)?;
    let previous: Option<String> = db.query_row("SELECT snapshot FROM commit_reviews WHERE project_id=?1 AND issue_number=?2 AND sha=?3", params![project,number,sha], |r| r.get(0)).optional()?;
    let encoded = serde_json::to_string(snapshot)?;
    if let Some(previous) = previous {
        if previous != encoded {
            return Err(Error::conflict(
                "This commit already has a different snapshot; attach the new commit instead",
            ));
        }
        return Ok(false);
    }
    db.execute(
        "INSERT INTO commit_reviews VALUES(?1,?2,?3,?4,?5)",
        params![project, number, sha, encoded, snapshot.message],
    )?;
    Ok(true)
}

fn decode_path(path: &str) -> Result<String> {
    let path = path.trim_end_matches('\t');
    if !path.starts_with('"') {
        return Ok(path.into());
    }
    let raw = path
        .strip_prefix('"')
        .and_then(|p| p.strip_suffix('"'))
        .ok_or_else(|| Error::invalid("Invalid quoted Git path"))?
        .as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != b'\\' {
            out.push(raw[i]);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&c) = raw.get(i) else {
            return Err(Error::invalid("Invalid Git path escape"));
        };
        if (b'0'..=b'7').contains(&c) {
            let mut n = 0u16;
            let mut count = 0;
            while i < raw.len() && count < 3 && (b'0'..=b'7').contains(&raw[i]) {
                n = n * 8 + (raw[i] - b'0') as u16;
                i += 1;
                count += 1;
            }
            if n > 255 {
                return Err(Error::invalid("Invalid Git path byte"));
            }
            out.push(n as u8);
        } else {
            out.push(match c {
                b't' => b'\t',
                b'n' => b'\n',
                b'r' => b'\r',
                b'b' => 8,
                b'f' => 12,
                b'v' => 11,
                b'a' => 7,
                b'\\' => b'\\',
                b'"' => b'"',
                _ => return Err(Error::invalid("Invalid Git path escape")),
            });
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| Error::invalid("Git path is not UTF-8"))
}
/// Resolve only lines present in the snapshot, never an unchecked repository path.
fn source(patch: &str, location: &Location) -> Result<String> {
    let end = location.end_line.unwrap_or(location.line);
    if !matches!(location.side.as_str(), "old" | "new")
        || location.line == 0
        || end < location.line
        || end - location.line > 10000
    {
        return Err(Error::invalid("Invalid review line range"));
    }
    let mut file = String::new();
    let mut old_file = String::new();
    let (mut old, mut new, mut old_left, mut new_left) = (0u32, 0u32, 0u32, 0u32);
    let mut lines = BTreeMap::new();
    for line in patch.split_terminator('\n') {
        if line.starts_with("diff --git ") {
            file.clear();
            old_file.clear();
            old_left = 0;
            new_left = 0;
        } else if old_left == 0 && new_left == 0 && line.starts_with("--- ") {
            let path = decode_path(&line[4..])?;
            old_file = path.strip_prefix("a/").unwrap_or(&path).into();
        } else if old_left == 0 && new_left == 0 && line.starts_with("+++ ") {
            let path = decode_path(&line[4..])?;
            file = if path == "/dev/null" {
                old_file.clone()
            } else {
                path.strip_prefix("b/").unwrap_or(&path).into()
            };
        } else if line.starts_with("@@ ") {
            let fields: Vec<_> = line.split_whitespace().collect();
            let range = |s: &str| -> Result<(u32, u32)> {
                let s = &s[1..];
                let (start, count) = s.split_once(',').unwrap_or((s, "1"));
                let start: u32 = start
                    .parse()
                    .map_err(|_| Error::invalid("Invalid patch hunk"))?;
                let count: u32 = count
                    .parse()
                    .map_err(|_| Error::invalid("Invalid patch hunk"))?;
                if start.checked_add(count).is_none() {
                    return Err(Error::invalid("Patch line number is too large"));
                }
                Ok((start, count))
            };
            if fields.len() < 4 {
                return Err(Error::invalid("Invalid patch hunk"));
            }
            (old, old_left) = range(fields[1])?;
            (new, new_left) = range(fields[2])?;
        } else if (old_left > 0 || new_left > 0) && !line.starts_with('\\') {
            let (old_line, new_line) = match line.as_bytes().first() {
                Some(b' ') if old_left > 0 && new_left > 0 => {
                    old_left -= 1;
                    new_left -= 1;
                    let pair = (Some(old), Some(new));
                    old += 1;
                    new += 1;
                    pair
                }
                Some(b'-') if old_left > 0 => {
                    old_left -= 1;
                    old += 1;
                    (Some(old - 1), None)
                }
                Some(b'+') if new_left > 0 => {
                    new_left -= 1;
                    new += 1;
                    (None, Some(new - 1))
                }
                _ => continue,
            };
            let number = if location.side == "old" {
                old_line
            } else {
                new_line
            };
            if file == location.file
                && let Some(n) = number
                && n >= location.line
                && n <= end
            {
                lines.insert(n, line[1..].to_owned());
            }
        }
    }
    if lines.len() != (end - location.line + 1) as usize {
        return Err(Error::invalid(
            "Comment lines are not present in this commit patch",
        ));
    }
    Ok(lines.into_values().collect::<Vec<_>>().join("\n"))
}
fn comments(db: &Connection, project: &str, number: i64, sha: &str) -> Result<Vec<Value>> {
    Ok(db.query_collect("SELECT id,location,source,body,author,created_at,updated_at FROM commit_review_comments WHERE project_id=?1 AND issue_number=?2 AND sha=?3 ORDER BY created_at,id",params![project,number,sha],|r| {
        let location: String=r.get(1)?;
        Ok::<_,rusqlite::Error>(json!({"id":r.get::<_,String>(0)?,"location":serde_json::from_str::<Value>(&location).unwrap_or(Value::Null),"source":r.get::<_,String>(2)?,"body":r.get::<_,String>(3)?,"author":r.get::<_,String>(4)?,"createdAt":r.get::<_,i64>(5)?,"updatedAt":r.get::<_,i64>(6)?}))
    })?)
}
pub(crate) fn execute(
    db: &Connection,
    project: &Project,
    number: i64,
    commit: &str,
    operation: &Operation,
    actor: Option<&Actor>,
    now: i64,
) -> Result<Value> {
    let sha = resolve(db, &project.id, number, commit)?;
    let (encoded, current): (String,String) = db.query_row("SELECT snapshot,message FROM commit_reviews WHERE project_id=?1 AND issue_number=?2 AND sha=?3",params![project.id,number,sha],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or_else(|| Error::invalid("No stored patch; attach this commit from its Git checkout with issue commit add"))?;
    let snapshot: Snapshot = serde_json::from_str(&encoded)?;
    let mut result =
        json!({"ok":true,"project":project,"number":number,"sha":sha,"changed":operation.writes()});
    match operation {
        Operation::View => { let mut review=serde_json::to_value(&snapshot)?; review["message"]=json!(current); review["sha"]=json!(sha); result["review"]=review; result["comments"]=json!(comments(db,&project.id,number,&sha)?); },
        Operation::Comments => result["comments"]=json!(comments(db,&project.id,number,&sha)?),
        Operation::Patch => {
            let trimmed=current.trim_end(); let (subject,rest)=trimmed.split_once('\n').unwrap_or((trimmed,""));
            result["patch"]=json!(format!("From {sha} Mon Sep 17 00:00:00 2001\nFrom: {} <{}>\nDate: {}\nSubject: [PATCH] {subject}\n\n{}\n---\n{}",snapshot.author,snapshot.email,snapshot.authored_at,rest.trim_start_matches('\n'),snapshot.patch));
        },
        Operation::Message { message: next, previous_message } => {
            message(next)?;
            if &current != previous_message { return Err(Error::conflict("Commit message changed; reload before editing")); }
            db.execute("UPDATE commit_reviews SET message=?4 WHERE project_id=?1 AND issue_number=?2 AND sha=?3",params![project.id,number,sha,next])?;
            result["message"]=json!(next);
        },
        Operation::Comment { location, body: text } => {
            body(text)?; let source=source(&snapshot.patch,location)?; let id=super::worker::random_id()?;
            db.execute("INSERT INTO commit_review_comments VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",params![id,project.id,number,sha,serde_json::to_string(location)?,source,text,actor.unwrap().id,now])?;
            result["comment"]=comments(db,&project.id,number,&sha)?.into_iter().find(|c|c["id"]==id).unwrap();
        },
        Operation::EditComment { id, body: text, previous_body } => {
            body(text)?;
            let changed=db.execute("UPDATE commit_review_comments SET body=?5,updated_at=?6 WHERE project_id=?1 AND issue_number=?2 AND sha=?3 AND id=?4 AND body=?7",params![project.id,number,sha,id,text,now,previous_body])?;
            if changed==0 { return Err(Error::conflict("Comment changed or was deleted; reload before editing")); }
            result["comment"]=comments(db,&project.id,number,&sha)?.into_iter().find(|c|c["id"]==*id).unwrap();
        },
        Operation::DeleteComment { id, previous_body } => {
            if db.execute("DELETE FROM commit_review_comments WHERE project_id=?1 AND issue_number=?2 AND sha=?3 AND id=?4 AND body=?5",params![project.id,number,sha,id,previous_body])?==0 { return Err(Error::conflict("Comment changed or was deleted; reload before deleting")); }
        },
    }
    if operation.writes() {
        db.execute(
            "UPDATE issues SET version=version+1,updated_at=?3 WHERE project_id=?1 AND number=?2",
            params![project.id, number, now],
        )?;
        super::store::event(
            db,
            &project.id,
            number,
            &actor.unwrap().id,
            "commit_reviewed",
            now,
            &json!({"sha":sha,"command":serde_json::to_value(operation)?["command"]}),
        )?;
    }
    Ok(result)
}
