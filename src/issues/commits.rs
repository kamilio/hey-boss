//! Deduplicated issue commit attachments and agent trace provenance.
use super::{Actor, Error, Project, Result};
use crate::database::Connection;
use rusqlite::params;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS issue_commits(
    project_id TEXT NOT NULL,
    issue_number INTEGER NOT NULL,
    sha TEXT NOT NULL,
    url TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    added_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    origin TEXT CHECK(origin IS NULL OR json_valid(origin)),
    PRIMARY KEY(project_id, issue_number, sha)
);
CREATE INDEX IF NOT EXISTS issue_commits_lookup ON issue_commits(project_id, issue_number, created_at, sha);
CREATE INDEX IF NOT EXISTS issue_commits_origin_session ON issue_commits(json_extract(origin,'$.session_id'), json_extract(origin,'$.host'));
CREATE INDEX IF NOT EXISTS issue_commits_origin_run ON issue_commits(json_extract(origin,'$.run.id'), json_extract(origin,'$.host'));

CREATE TRIGGER IF NOT EXISTS issue_commits_event_sync_insert
AFTER INSERT ON events
WHEN NEW.action = 'commit_attached' AND json_valid(NEW.data) AND json_extract(NEW.data, '$.sha') IS NOT NULL
BEGIN
    DELETE FROM issue_commits
    WHERE project_id = NEW.project_id
      AND issue_number = NEW.issue_number
      AND (
          sha = lower(json_extract(NEW.data, '$.sha'))
          OR (length(sha) < length(json_extract(NEW.data, '$.sha')) AND substr(lower(json_extract(NEW.data, '$.sha')), 1, length(sha)) = sha)
      );
    INSERT INTO issue_commits(project_id, issue_number, sha, url, title, added_by, created_at, origin)
    SELECT
        NEW.project_id,
        NEW.issue_number,
        lower(json_extract(NEW.data, '$.sha')),
        coalesce(json_extract(NEW.data, '$.url'), ''),
        coalesce(json_extract(NEW.data, '$.title'), ''),
        NEW.actor,
        coalesce(json_extract(NEW.data, '$.created_at'), NEW.created_at),
        CASE WHEN json_type(NEW.data, '$.origin') = 'object' THEN json_extract(NEW.data, '$.origin')
             WHEN json_type(NEW.data, '$.origin') = 'text' AND json_valid(json_extract(NEW.data, '$.origin')) THEN json_extract(NEW.data, '$.origin')
             ELSE NULL END
    WHERE NOT EXISTS (
        SELECT 1 FROM issue_commits existing
        WHERE existing.project_id = NEW.project_id
          AND existing.issue_number = NEW.issue_number
          AND length(existing.sha) > length(json_extract(NEW.data, '$.sha'))
          AND substr(existing.sha, 1, length(json_extract(NEW.data, '$.sha'))) = lower(json_extract(NEW.data, '$.sha'))
    )
    ON CONFLICT(project_id, issue_number, sha) DO UPDATE SET
        url = CASE WHEN excluded.url <> '' THEN excluded.url ELSE issue_commits.url END,
        title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE issue_commits.title END,
        origin = coalesce(issue_commits.origin, excluded.origin);
END;

CREATE TRIGGER IF NOT EXISTS issue_commits_event_sync_delete
AFTER INSERT ON events
WHEN NEW.action = 'commit_removed' AND json_valid(NEW.data) AND json_extract(NEW.data, '$.sha') IS NOT NULL
BEGIN
    DELETE FROM issue_commits
    WHERE project_id = NEW.project_id
      AND issue_number = NEW.issue_number
      AND (
          sha = lower(json_extract(NEW.data, '$.sha'))
          OR substr(sha, 1, length(json_extract(NEW.data, '$.sha'))) = lower(json_extract(NEW.data, '$.sha'))
          OR substr(lower(json_extract(NEW.data, '$.sha')), 1, length(sha)) = sha
      );
END;
";

pub(crate) fn migrate(db: &Connection) -> Result<()> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='issue_commits' AND type='table')
         AND EXISTS(SELECT 1 FROM sqlite_master WHERE name='issue_commits_event_sync_insert' AND type='trigger')",
        [],
        |r| r.get(0),
    )?;
    if exists {
        return Ok(());
    }
    db.execute_batch("SAVEPOINT issue_commits_migrate;")?;
    let res = (|| -> Result<()> {
        let already: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='issue_commits' AND type='table')",
            [],
            |r| r.get(0),
        )?;
        db.execute_batch(SCHEMA)?;
        if !already {
            // Replay any replicated commit events if bootstrapping an existing store.
            let has_events: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='events' AND type='table')",
                [],
                |r| r.get(0),
            )?;
            if has_events {
                let mut stmt = db.prepare(
                    "SELECT project_id, issue_number, actor, action, data, created_at FROM events WHERE action IN ('commit_attached', 'commit_removed') ORDER BY created_at, id",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, i64>(5)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(stmt);
                for (project_id, issue_number, actor, action, data, created_at) in rows {
                    let Ok(val) = serde_json::from_str::<Value>(&data) else {
                        continue;
                    };
                    let Some(sha) = val["sha"].as_str().map(|s| s.to_ascii_lowercase()) else {
                        continue;
                    };
                    if action == "commit_removed" {
                        db.execute(
                            "DELETE FROM issue_commits WHERE project_id=?1 AND issue_number=?2 AND (sha=?3 OR substr(sha,1,length(?3))=?3 OR substr(?3,1,length(sha))=sha)",
                            params![project_id, issue_number, sha],
                        )?;
                    } else {
                        let url = val["url"].as_str().unwrap_or("");
                        let title = val["title"].as_str().unwrap_or("");
                        let origin = if val["origin"].is_object() {
                            Some(val["origin"].to_string())
                        } else {
                            val["origin"].as_str().map(str::to_owned)
                        };
                        upsert_row(db, &project_id, issue_number, &sha, url, title, &actor, created_at, origin.as_deref())?;
                    }
                }
            }
        }
        Ok(())
    })();
    match res {
        Ok(()) => {
            db.execute_batch("RELEASE SAVEPOINT issue_commits_migrate;")?;
            Ok(())
        }
        Err(err) => {
            let _ = db.execute_batch("ROLLBACK TO SAVEPOINT issue_commits_migrate; RELEASE SAVEPOINT issue_commits_migrate;");
            Err(err)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCommit {
    pub sha: String,
    pub url: String,
    pub title: String,
}

pub fn is_hex_sha(value: &str) -> bool {
    (7..=40).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn is_commit_url(url: &str) -> bool {
    parse_url_commit(url).is_some()
}

fn parse_url_commit(url: &str) -> Option<(Option<(String, String)>, String)> {
    let trimmed = url.trim();
    let after = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))?;
    let path_end = after.find(['?', '#']).unwrap_or(after.len());
    let clean = after[..path_end].trim_end_matches('/');
    let parts: Vec<&str> = clean.split('/').collect();
    // Match:
    // github.com/<owner>/<repo>/commit/<sha>[.patch|.diff]
    // github.com/<owner>/<repo>/commits/<sha>
    // github.com/<owner>/<repo>/pull/<n>/commits/<sha>
    for idx in 0..parts.len() {
        if matches!(parts[idx], "commit" | "commits") && idx + 1 < parts.len() {
            let raw_sha = parts[idx + 1]
                .trim_end_matches(".patch")
                .trim_end_matches(".diff");
            if is_hex_sha(raw_sha) {
                let repo = if parts.len() >= 3 && parts[0].eq_ignore_ascii_case("github.com") {
                    Some((parts[1].to_owned(), parts[2].to_owned()))
                } else {
                    None
                };
                return Some((repo, raw_sha.to_ascii_lowercase()));
            }
        }
    }
    None
}

fn git_resolve_in(cwd: &Path, rev: &str) -> Option<(String, String, Option<(String, String)>)> {
    let sha_out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")])
        .output()
        .ok()?;
    if !sha_out.status.success() {
        return None;
    }
    let full_sha = String::from_utf8_lossy(&sha_out.stdout)
        .trim()
        .to_ascii_lowercase();
    if !is_hex_sha(&full_sha) {
        return None;
    }
    let title = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["log", "-1", "--format=%s", &full_sha])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let repo = crate::agents::git_info(cwd.to_str().unwrap_or(""))
        .and_then(|g| g.origin)
        .and_then(|origin| {
            let rest = origin.strip_prefix("github.com/")?;
            let (owner, name) = rest.split_once('/')?;
            Some((owner.to_owned(), name.to_owned()))
        });
    Some((full_sha, title, repo))
}

fn github_repo_from_project(project_id: &str) -> Option<(String, String)> {
    let rest = project_id.strip_prefix("github.com/")?;
    let (owner, repo) = rest.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty()).then(|| (owner.to_owned(), repo.to_owned()))
}

pub fn canonical_commit_url(
    project_id: &str,
    url_repo: Option<(String, String)>,
    git_repo: Option<(String, String)>,
    sha: &str,
    fallback_url: Option<&str>,
) -> String {
    if let Some((owner, repo)) = url_repo
        .or_else(|| github_repo_from_project(project_id))
        .or(git_repo)
    {
        return format!("https://github.com/{owner}/{repo}/commit/{sha}");
    }
    if let Some(raw) = fallback_url {
        let trimmed = raw.trim().trim_end_matches('/');
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            return trimmed.to_owned();
        }
    }
    format!("https://github.com/commit/{sha}")
}

pub fn resolve_commit_input(
    input: &str,
    project_id: &str,
    repo_cwd: Option<&Path>,
    title_override: Option<&str>,
) -> Result<ResolvedCommit> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::invalid("Commit reference cannot be empty"));
    }
    let cwd_candidates = |primary: Option<&Path>| -> Vec<PathBuf> {
        match primary {
            Some(p) => vec![p.to_path_buf()],
            None => std::env::current_dir().into_iter().collect(),
        }
    };
    if let Some((url_repo, url_sha)) = parse_url_commit(trimmed) {
        for dir in cwd_candidates(repo_cwd) {
            if let Some((full_sha, git_title, git_repo)) = git_resolve_in(&dir, &url_sha) {
                let title = title_override
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or(git_title);
                let url = canonical_commit_url(project_id, url_repo, git_repo, &full_sha, Some(trimmed));
                return Ok(ResolvedCommit {
                    sha: full_sha,
                    url,
                    title,
                });
            }
        }
        let title = title_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_owned();
        let url = canonical_commit_url(project_id, url_repo, None, &url_sha, Some(trimmed));
        return Ok(ResolvedCommit {
            sha: url_sha,
            url,
            title,
        });
    }
    for dir in cwd_candidates(repo_cwd) {
        if let Some((full_sha, git_title, git_repo)) = git_resolve_in(&dir, trimmed) {
            let title = title_override
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or(git_title);
            let url = canonical_commit_url(project_id, None, git_repo, &full_sha, None);
            return Ok(ResolvedCommit {
                sha: full_sha,
                url,
                title,
            });
        }
    }
    if is_hex_sha(trimmed) {
        let sha = trimmed.to_ascii_lowercase();
        let title = title_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_owned();
        let url = canonical_commit_url(project_id, None, None, &sha, None);
        return Ok(ResolvedCommit { sha, url, title });
    }
    Err(Error::invalid(
        "Commit must be a valid Git commit SHA, revision (such as HEAD), or GitHub commit URL",
    ))
}

fn sha_matches(a: &str, b: &str) -> bool {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    a == b || a.starts_with(&b) || b.starts_with(&a)
}

fn origin_score(origin: Option<&str>) -> u8 {
    let Some(raw) = origin else {
        return 0;
    };
    let Ok(val) = serde_json::from_str::<Value>(raw) else {
        return 0;
    };
    let has_session = val["session_id"].as_str().is_some_and(|s| !s.is_empty());
    let has_offset = val["invocation"]["offset"].is_u64() || val["invocation"]["offset"].is_i64();
    let has_run = val["run"].is_object();
    match (has_session, has_offset, has_run) {
        (true, true, true) => 4,
        (true, true, false) => 3,
        (true, false, _) => 2,
        _ => 1,
    }
}

#[allow(clippy::too_many_arguments)]
fn upsert_row(
    db: &Connection,
    project_id: &str,
    number: i64,
    sha: &str,
    url: &str,
    title: &str,
    added_by: &str,
    created_at: i64,
    origin: Option<&str>,
) -> Result<(bool, String, String, String, i64, Option<String>)> {
    let sha = sha.to_ascii_lowercase();
    let mut stmt = db.prepare(
        "SELECT sha, url, title, added_by, created_at, origin FROM issue_commits WHERE project_id=?1 AND issue_number=?2 ORDER BY created_at, sha",
    )?;
    let existing_rows = stmt
        .query_map(params![project_id, number], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let matched: Vec<_> = existing_rows
        .into_iter()
        .filter(|(ex_sha, ex_url, _, _, _, _)| {
            sha_matches(ex_sha, &sha)
                || (!url.is_empty()
                    && ex_url.trim_end_matches('/').eq_ignore_ascii_case(url.trim_end_matches('/')))
        })
        .collect();

    if matched.is_empty() {
        db.execute(
            "INSERT INTO issue_commits(project_id, issue_number, sha, url, title, added_by, created_at, origin) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![project_id, number, sha, url, title, added_by, created_at, origin],
        )?;
        return Ok((true, sha, url.to_owned(), title.to_owned(), created_at, origin.map(str::to_owned)));
    }

    let mut final_sha = sha.clone();
    let mut final_url = url.to_owned();
    let mut final_title = title.to_owned();
    let mut final_added_by = added_by.to_owned();
    let mut final_created_at = created_at;
    let mut final_origin = origin.map(str::to_owned);

    for (ex_sha, ex_url, ex_title, ex_added_by, ex_created_at, ex_origin) in &matched {
        if ex_sha.len() >= final_sha.len() {
            final_sha = ex_sha.clone();
            if !ex_url.is_empty() {
                final_url = ex_url.clone();
            }
        } else if final_url.is_empty() && !ex_url.is_empty() {
            final_url = ex_url.clone();
        }
        // Always ensure canonical URL uses the longest resolved SHA if it's a GitHub commit link.
        if let Some((repo, _)) = parse_url_commit(&final_url) {
            final_url = canonical_commit_url(project_id, repo, None, &final_sha, Some(&final_url));
        }
        if final_title.is_empty() && !ex_title.is_empty() {
            final_title = ex_title.clone();
        }
        if *ex_created_at <= final_created_at {
            final_created_at = *ex_created_at;
            final_added_by = ex_added_by.clone();
        }
        if origin_score(ex_origin.as_deref()) >= origin_score(final_origin.as_deref()) && ex_origin.is_some() {
            final_origin = ex_origin.clone();
        }
    }

    let identical = matched.len() == 1
        && matched[0].0 == final_sha
        && matched[0].1 == final_url
        && matched[0].2 == final_title
        && matched[0].5 == final_origin;
    if identical {
        return Ok((false, final_sha, final_url, final_title, final_created_at, final_origin));
    }

    for (ex_sha, _, _, _, _, _) in &matched {
        if ex_sha != &final_sha {
            db.execute(
                "DELETE FROM issue_commits WHERE project_id=?1 AND issue_number=?2 AND sha=?3",
                params![project_id, number, ex_sha],
            )?;
        }
    }
    db.execute(
        "INSERT INTO issue_commits(project_id, issue_number, sha, url, title, added_by, created_at, origin)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(project_id, issue_number, sha) DO UPDATE SET
             url=excluded.url,
             title=excluded.title,
             origin=excluded.origin",
        params![
            project_id,
            number,
            final_sha,
            final_url,
            final_title,
            final_added_by,
            final_created_at,
            final_origin
        ],
    )?;
    Ok((true, final_sha, final_url, final_title, final_created_at, final_origin))
}

pub(crate) fn add(
    db: &Connection,
    project: &Project,
    number: i64,
    raw_commit: &str,
    title_override: Option<&str>,
    actor: &Actor,
    now: i64,
) -> Result<(bool, Vec<Value>)> {
    migrate(db)?;
    let resolved = resolve_commit_input(raw_commit, &project.id, Some(&actor.cwd), title_override)?;
    let origin = super::provenance::capture(db, actor, now)?;
    let (changed, final_sha, final_url, final_title, final_created_at, final_origin) = upsert_row(
        db,
        &project.id,
        number,
        &resolved.sha,
        &resolved.url,
        &resolved.title,
        &actor.id,
        now,
        Some(&origin),
    )?;
    if changed {
        db.execute(
            "UPDATE issues SET version=version+1, updated_at=?3 WHERE project_id=?1 AND number=?2",
            params![project.id, number, now],
        )?;
        let origin_val = final_origin
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or(Value::Null);
        super::store::event(
            db,
            &project.id,
            number,
            &actor.id,
            "commit_attached",
            now,
            &json!({
                "sha": final_sha,
                "url": final_url,
                "title": final_title,
                "created_at": final_created_at,
                "origin": origin_val,
            }),
        )?;
    }
    Ok((changed, list(db, &project.id, number)?))
}

pub(crate) fn remove(
    db: &Connection,
    project: &Project,
    number: i64,
    raw_commit: &str,
    actor: &Actor,
    now: i64,
) -> Result<(bool, Vec<Value>)> {
    migrate(db)?;
    let target_sha = parse_url_commit(raw_commit)
        .map(|(_, sha)| sha)
        .unwrap_or_else(|| raw_commit.trim().to_ascii_lowercase());
    if target_sha.is_empty() {
        return Err(Error::invalid("Commit reference cannot be empty"));
    }
    let mut stmt = db.prepare(
        "SELECT sha, url FROM issue_commits WHERE project_id=?1 AND issue_number=?2",
    )?;
    let rows = stmt
        .query_map(params![project.id, number], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    let mut removed_shas = Vec::new();
    for (sha, url) in rows {
        if sha_matches(&sha, &target_sha)
            || url.trim_end_matches('/').eq_ignore_ascii_case(raw_commit.trim().trim_end_matches('/'))
        {
            db.execute(
                "DELETE FROM issue_commits WHERE project_id=?1 AND issue_number=?2 AND sha=?3",
                params![project.id, number, sha],
            )?;
            removed_shas.push(sha);
        }
    }
    let changed = !removed_shas.is_empty();
    if changed {
        db.execute(
            "UPDATE issues SET version=version+1, updated_at=?3 WHERE project_id=?1 AND number=?2",
            params![project.id, number, now],
        )?;
        for sha in removed_shas {
            super::store::event(
                db,
                &project.id,
                number,
                &actor.id,
                "commit_removed",
                now,
                &json!({"sha": sha}),
            )?;
        }
    }
    Ok((changed, list(db, &project.id, number)?))
}

pub(crate) fn list_by_project(
    db: &Connection,
    project_id: &str,
) -> Result<std::collections::HashMap<i64, Vec<Value>>> {
    migrate(db)?;
    let mut stmt = db.prepare(
        "SELECT issue_number, sha, url, title, added_by, created_at, origin FROM issue_commits WHERE project_id=?1 ORDER BY created_at, sha",
    )?;
    let mut map: std::collections::HashMap<i64, Vec<Value>> = std::collections::HashMap::new();
    let rows = stmt.query_map([project_id], |r| {
        let num: i64 = r.get(0)?;
        let sha: String = r.get(1)?;
        let short_sha: String = sha.chars().take(7).collect();
        let origin_raw: Option<String> = r.get(6)?;
        let origin = origin_raw
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or(Value::Null);
        Ok((
            num,
            json!({
                "sha": sha,
                "short_sha": short_sha,
                "url": r.get::<_, String>(2)?,
                "title": r.get::<_, String>(3)?,
                "added_by": r.get::<_, String>(4)?,
                "created_at": r.get::<_, i64>(5)?,
                "origin": origin,
            }),
        ))
    })?;
    for row in rows {
        let (num, item) = row?;
        let list = map.entry(num).or_default();
        let sha = item["sha"].as_str().unwrap_or("");
        if let Some(existing) = list
            .iter_mut()
            .find(|e| sha_matches(e["sha"].as_str().unwrap_or(""), sha))
        {
            if sha.len() > existing["sha"].as_str().unwrap_or("").len() {
                let created_at = existing["created_at"].clone();
                let prev_origin = existing["origin"].clone();
                *existing = item;
                existing["created_at"] = created_at;
                if existing["origin"].is_null() && !prev_origin.is_null() {
                    existing["origin"] = prev_origin;
                }
            }
        } else {
            list.push(item);
        }
    }
    Ok(map)
}

pub(crate) fn list(db: &Connection, project_id: &str, number: i64) -> Result<Vec<Value>> {
    migrate(db)?;
    let mut stmt = db.prepare(
        "SELECT sha, url, title, added_by, created_at, origin FROM issue_commits WHERE project_id=?1 AND issue_number=?2 ORDER BY created_at, sha",
    )?;
    let rows = stmt
        .query_map(params![project_id, number], |r| {
            let sha: String = r.get(0)?;
            let short_sha: String = sha.chars().take(7).collect();
            let origin_raw: Option<String> = r.get(5)?;
            let origin = origin_raw
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or(Value::Null);
            Ok(json!({
                "sha": sha,
                "short_sha": short_sha,
                "url": r.get::<_, String>(1)?,
                "title": r.get::<_, String>(2)?,
                "added_by": r.get::<_, String>(3)?,
                "created_at": r.get::<_, i64>(4)?,
                "origin": origin,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Defensive read-time prefix deduplication.
    let mut deduped: Vec<Value> = Vec::with_capacity(rows.len());
    for item in rows {
        let sha = item["sha"].as_str().unwrap_or("");
        if let Some(existing) = deduped
            .iter_mut()
            .find(|e| sha_matches(e["sha"].as_str().unwrap_or(""), sha))
        {
            if sha.len() > existing["sha"].as_str().unwrap_or("").len() {
                let created_at = existing["created_at"].clone();
                let prev_origin = existing["origin"].clone();
                *existing = item;
                existing["created_at"] = created_at;
                if existing["origin"].is_null() && !prev_origin.is_null() {
                    existing["origin"] = prev_origin;
                }
            }
        } else {
            deduped.push(item);
        }
    }
    Ok(deduped)
}

/// Extract issue numbers referenced as `#123` from a commit message.
pub fn extract_issue_numbers(message: &str) -> Vec<i64> {
    let mut numbers = Vec::new();
    let bytes = message.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let valid_prefix = i == 0
                || (!bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'/' && bytes[i - 1] != b'&');
            if valid_prefix {
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    end += 1;
                }
                if end > start
                    && (end == bytes.len() || !bytes[end].is_ascii_alphanumeric())
                    && let Ok(num) = message[start..end].parse::<i64>()
                    && num > 0
                    && !numbers.contains(&num)
                {
                    numbers.push(num);
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    numbers
}

/// Resolve the issue numbers in `project_id` that should receive an automatic commit hook attachment.
pub fn target_issues_for_hook_path(
    path: &std::path::Path,
    project_id: &str,
    actor: &Actor,
    explicit_issue: Option<i64>,
    commit_message: &str,
) -> Result<Vec<i64>> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let _ = db.busy_timeout(std::time::Duration::from_millis(500));
    target_issues_for_hook(
        &db,
        project_id,
        actor,
        explicit_issue,
        commit_message,
        super::worker::now(),
    )
}

pub fn target_issues_for_hook(
    db: &Connection,
    project_id: &str,
    actor: &Actor,
    explicit_issue: Option<i64>,
    commit_message: &str,
    now: i64,
) -> Result<Vec<i64>> {
    let mut targets = Vec::new();
    let mut add_if_exists = |num: i64| -> Result<()> {
        if num > 0 && !targets.contains(&num) {
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM issues WHERE project_id=?1 AND number=?2 AND deleted_at IS NULL)",
                params![project_id, num],
                |r| r.get(0),
            )?;
            if exists {
                targets.push(num);
            }
        }
        Ok(())
    };

    if let Some(num) = explicit_issue {
        add_if_exists(num)?;
    }
    if let Some(run) = &actor.creation_run
        && run.project_id == project_id
    {
        add_if_exists(run.number)?;
    }
    if let Some(run) = super::provenance::source_run(db, actor, now)?
        && run.project_id == project_id
    {
        add_if_exists(run.number)?;
    }

    // Active claims by this agent session in this project.
    if !actor.id.is_empty() && !actor.id.starts_with("human:") {
        let mut stmt = db.prepare(
            "SELECT number FROM issues WHERE project_id=?1 AND assignee=?2 AND state IN ('open','ready') AND deleted_at IS NULL ORDER BY updated_at DESC",
        )?;
        let claimed = stmt
            .query_map(params![project_id, actor.id], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for num in claimed {
            add_if_exists(num)?;
        }
    }

    // Commit message `#123` references when committed by an agent or matching an open issue.
    for num in extract_issue_numbers(commit_message) {
        let eligible: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM issues WHERE project_id=?1 AND number=?2 AND deleted_at IS NULL AND (assignee=?3 OR state IN ('open','ready')))",
            params![project_id, num, actor.id],
            |r| r.get(0),
        )?;
        if eligible {
            add_if_exists(num)?;
        }
    }

    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE projects(id TEXT PRIMARY KEY, name TEXT, hidden_at INTEGER);
             INSERT INTO projects VALUES('github.com/kamilio/hey-boss', 'hey-boss', NULL);
             CREATE TABLE issues(project_id TEXT, number INTEGER, title TEXT, state TEXT, assignee TEXT, deleted_at INTEGER, version INTEGER DEFAULT 1, updated_at INTEGER DEFAULT 0, origin TEXT, PRIMARY KEY(project_id, number));
             INSERT INTO issues VALUES('github.com/kamilio/hey-boss', 704, 'Attach commits', 'open', 'codex:01a0ed5c-be46-7eb1-bf98-0cf62f47e3d3', NULL, 1, 0, NULL);
             CREATE TABLE worker_runs(id TEXT, project_id TEXT, issue_number INTEGER, session_id TEXT, machine TEXT, started_at INTEGER, finished_at INTEGER, job TEXT, state TEXT, actor_id TEXT);
             CREATE TABLE events(id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, issue_number INTEGER NOT NULL, actor TEXT NOT NULL, action TEXT NOT NULL, data TEXT NOT NULL, created_at INTEGER NOT NULL);",
        )
        .unwrap();
        migrate(&db).unwrap();
        db
    }

    fn test_actor() -> Actor {
        Actor {
            id: "codex:01a0ed5c-be46-7eb1-bf98-0cf62f47e3d3".into(),
            kind: "codex".into(),
            session_id: Some("01a0ed5c-be46-7eb1-bf98-0cf62f47e3d3".into()),
            machine: "m1".into(),
            host: "mac.local".into(),
            pid: None,
            process_start: None,
            cwd: PathBuf::from("/tmp"),
            source: "CODEX_THREAD_ID".into(),
            invocation: Some(crate::issues::Invocation {
                offset: 123456,
                call_id: Some("call_commit_1".into()),
            }),
            creation_run: None,
            model: Some("gpt-6-astra".into()),
        }
    }

    #[test]
    fn deduplicates_short_full_and_url_commit_attachments_and_preserves_trace_origin() {
        let db = test_db();
        let project = Project {
            id: "github.com/kamilio/hey-boss".into(),
            name: "hey-boss".into(),
        };
        let actor = test_actor();

        // 1. Attach short 7-char SHA first.
        let (changed1, list1) = add(
            &db,
            &project,
            704,
            "71cb79c",
            Some("Initial short commit"),
            &actor,
            1000,
        )
        .unwrap();
        assert!(changed1);
        assert_eq!(list1.len(), 1);
        assert_eq!(list1[0]["sha"], "71cb79c");
        assert_eq!(
            list1[0]["url"],
            "https://github.com/kamilio/hey-boss/commit/71cb79c"
        );
        assert_eq!(list1[0]["origin"]["invocation"]["offset"], 123456);

        // 2. Re-attaching the exact same short SHA is a no-op (changed = false).
        let (changed_dup, list_dup) = add(&db, &project, 704, "71CB79C", None, &actor, 1005).unwrap();
        assert!(!changed_dup);
        assert_eq!(list_dup.len(), 1);

        // 3. Attaching the full 40-char SHA via a GitHub URL upgrades the existing row in-place!
        let full_sha = "71cb79c4a1b2c3d4e5f60123456789abcdef0123";
        let url_input = format!("https://github.com/kamilio/hey-boss/commit/{full_sha}/");
        let (changed2, list2) = add(&db, &project, 704, &url_input, None, &actor, 1010).unwrap();
        assert!(changed2);
        assert_eq!(list2.len(), 1);
        assert_eq!(list2[0]["sha"], full_sha);
        assert_eq!(list2[0]["short_sha"], "71cb79c");
        assert_eq!(
            list2[0]["url"],
            format!("https://github.com/kamilio/hey-boss/commit/{full_sha}")
        );
        assert_eq!(list2[0]["title"], "Initial short commit");
        assert_eq!(list2[0]["created_at"], 1000);
        assert_eq!(list2[0]["origin"]["invocation"]["offset"], 123456);

        // 4. Subsequent re-attachment by short SHA, full SHA, or .patch URL is a no-op!
        for input in [
            "71cb79c",
            full_sha,
            &format!("https://github.com/kamilio/hey-boss/commit/71cb79c.patch"),
        ] {
            let (changed_again, list_again) =
                add(&db, &project, 704, input, None, &actor, 1020).unwrap();
            assert!(!changed_again, "expected {input} to be deduplicated as no-op");
            assert_eq!(list_again.len(), 1);
            assert_eq!(list_again[0]["sha"], full_sha);
        }
    }

    #[test]
    fn event_sync_triggers_replicate_and_deduplicate_commits_on_peers() {
        let db = test_db();
        // Simulate a replicated commit_attached event arriving from fleet outbox.
        let origin = json!({"session_id":"01a0ed5c-be46-7eb1-bf98-0cf62f47e3d3","host":"mac.local","kind":"codex","invocation":{"offset":98765}});
        db.execute(
            "INSERT INTO events(project_id, issue_number, actor, action, data, created_at) VALUES(?1, ?2, ?3, 'commit_attached', ?4, 2000)",
            params![
                "github.com/kamilio/hey-boss",
                704,
                "codex:01a0ed5c-be46-7eb1-bf98-0cf62f47e3d3",
                json!({"sha":"abcdef1","url":"https://github.com/kamilio/hey-boss/commit/abcdef1","title":"Short","origin":origin}).to_string()
            ],
        )
        .unwrap();
        let commits = list(&db, "github.com/kamilio/hey-boss", 704).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0]["sha"], "abcdef1");

        // Replicating the full 40-char SHA replaces the short SHA row automatically.
        let full = "abcdef1234567890abcdef1234567890abcdef12";
        db.execute(
            "INSERT INTO events(project_id, issue_number, actor, action, data, created_at) VALUES(?1, ?2, ?3, 'commit_attached', ?4, 2010)",
            params![
                "github.com/kamilio/hey-boss",
                704,
                "codex:01a0ed5c-be46-7eb1-bf98-0cf62f47e3d3",
                json!({"sha":full,"url":format!("https://github.com/kamilio/hey-boss/commit/{full}"),"title":"Full commit title","origin":origin}).to_string()
            ],
        )
        .unwrap();
        let commits = list(&db, "github.com/kamilio/hey-boss", 704).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0]["sha"], full);
        assert_eq!(commits[0]["title"], "Full commit title");
        assert_eq!(commits[0]["origin"]["invocation"]["offset"], 98765);
    }

    #[test]
    fn hook_targets_claimed_and_message_referenced_issues() {
        let db = test_db();
        let actor = test_actor();
        let targets = target_issues_for_hook(
            &db,
            "github.com/kamilio/hey-boss",
            &actor,
            None,
            "Implement commit deduplication (#704)",
            1000,
        )
        .unwrap();
        assert_eq!(targets, vec![704]);
    }
}

const MANAGED_HOOKS: &[&str] = &[
    "pre-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "post-rewrite",
    "pre-push",
    "post-checkout",
    "post-merge",
];

pub fn hooks_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".config").join("hey-boss").join("git-hooks"))
}

fn hook_script(hook: &str) -> String {
    let post_action = if matches!(hook, "post-commit" | "post-rewrite") {
        "\nexport PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH\"\nhey-boss issue commit hook >/dev/null 2>&1 || true\n"
    } else {
        ""
    };
    format!(
        "#!/bin/sh
# Managed by hey-boss: chains repo-local and previous global hooks before commit provenance capture.
SELF_DIR=$(cd \"$(dirname \"$0\")\" 2>/dev/null && pwd)
HOOK_NAME=\"{hook}\"

run_chained() {{
    target=\"$1\"
    shift
    if [ -n \"$target\" ] && [ -x \"$target\" ] && [ ! \"$target\" -ef \"$0\" ]; then
        \"$target\" \"$@\"
        status=$?
        if [ $status -ne 0 ]; then
            exit $status
        fi
        return 0
    fi
    return 1
}}

local_hooks=$(git config --local --get core.hooksPath 2>/dev/null || true)
if [ -n \"$local_hooks\" ]; then
    case \"$local_hooks\" in
        /*) candidate=\"$local_hooks/$HOOK_NAME\" ;;
        *)
            top=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
            candidate=\"$top/$local_hooks/$HOOK_NAME\"
            ;;
    esac
    run_chained \"$candidate\" \"$@\" || true
else
    common_dir=$(git rev-parse --git-common-dir 2>/dev/null || git rev-parse --git-dir 2>/dev/null || true)
    if [ -n \"$common_dir\" ]; then
        run_chained \"$common_dir/hooks/$HOOK_NAME\" \"$@\" || true
    fi
fi

if [ -f \"$SELF_DIR/.previous-global-hooks-path\" ]; then
    prev_dir=$(cat \"$SELF_DIR/.previous-global-hooks-path\" 2>/dev/null || true)
    if [ -n \"$prev_dir\" ] && [ \"$prev_dir\" != \"$SELF_DIR\" ]; then
        run_chained \"$prev_dir/$HOOK_NAME\" \"$@\" || true
    fi
fi
{post_action}exit 0
"
    )
}

pub fn ensure_git_hooks() -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let dir = hooks_dir().ok_or_else(|| Error::invalid("HOME is not set"))?;
    std::fs::create_dir_all(&dir)?;
    for &hook in MANAGED_HOOKS {
        let path = dir.join(hook);
        let content = hook_script(hook);
        let unchanged = std::fs::read_to_string(&path).ok().as_deref() == Some(content.as_str());
        if !unchanged {
            std::fs::write(&path, content)?;
        }
        let mut perms = std::fs::metadata(&path)?.permissions();
        if perms.mode() & 0o111 == 0 {
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms)?;
        }
    }
    Ok(dir)
}

pub fn install_global_git_hooks() -> Result<PathBuf> {
    let dir = ensure_git_hooks()?;
    let existing = Command::new("git")
        .args(["config", "--global", "--get", "core.hooksPath"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty());
    if let Some(prev) = existing {
        let prev_path = PathBuf::from(&prev);
        if prev_path != dir && !prev.ends_with("hey-boss/git-hooks") {
            let _ = std::fs::write(dir.join(".previous-global-hooks-path"), format!("{prev}\n"));
        }
    }
    let status = Command::new("git")
        .args([
            "config",
            "--global",
            "core.hooksPath",
            dir.to_str().unwrap_or(""),
        ])
        .status()?;
    if !status.success() {
        return Err(Error::invalid("Failed to set git global core.hooksPath"));
    }
    Ok(dir)
}
