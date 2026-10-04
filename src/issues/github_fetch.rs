//! Small, shared fetch records; unchanged evidence must not be copied each poll.
use super::*;

pub(super) fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS github_fetch_status(url TEXT PRIMARY KEY,requested_at INTEGER,started_at INTEGER,finished_at INTEGER,next_at INTEGER,error TEXT);")?;
    Ok(())
}

pub(in crate::issues::store) fn request(
    db: &Connection,
    project: &Project,
    number: i64,
    now: i64,
) -> Result<Value> {
    let issue = get_issue(db, &project.id, number, false)?;
    let active: bool = db.query_row("SELECT coalesce(i.assignment_target='github',0) AND i.draft=0 AND i.state<>'closed' AND i.deleted_at IS NULL AND p.hidden_at IS NULL FROM issues i JOIN projects p ON p.id=i.project_id WHERE i.project_id=?1 AND i.number=?2", params![project.id,number], |r| r.get(0))?;
    let urls = registry::pull_requests(db, &project.id, number)?
        .into_iter()
        .filter(|pr| pr["status"] != "closed" && pr["status"] != "merged")
        .filter_map(|pr| {
            pr["url"]
                .as_str()
                .filter(|url| hey_gh::watcher::pull_request_selector(url).is_some())
                .map(|url| url.trim_end_matches('/').to_owned())
        })
        .collect::<BTreeSet<_>>();
    if !active || urls.is_empty() {
        return Err(Error::conflict(
            "Fetch requires an active GitHub PR watcher with an open pull request",
        ));
    }
    for url in urls {
        // Coalesce queued clicks. A click during an attempt belongs to the next
        // attempt, even when both occur within the same millisecond.
        db.execute("INSERT INTO github_fetch_status(url,requested_at) VALUES(?1,?2) ON CONFLICT(url) DO UPDATE SET requested_at=CASE WHEN requested_at>coalesce(started_at,0) THEN requested_at ELSE max(?2,coalesce(started_at,0)+1) END",params![url,now])?;
    }
    Ok(json!({"ok":true,"project":project,"issue":issue,"changed":true}))
}

pub(super) fn status(db: &Connection, project: &str, number: i64) -> Result<Value> {
    let mut fetches = serde_json::Map::new();
    let mut query = db.prepare("SELECT pr.url,f.requested_at,f.started_at,f.finished_at,f.next_at,f.error FROM issue_pull_requests pr JOIN github_fetch_status f ON f.url=rtrim(pr.url,'/') WHERE pr.project_id=?1 AND pr.issue_number=?2 ORDER BY pr.url")?;
    for row in query.query_map(params![project,number], |r| Ok((r.get::<_,String>(0)?,json!({"requested_at":r.get::<_,Option<i64>>(1)?,"started_at":r.get::<_,Option<i64>>(2)?,"finished_at":r.get::<_,Option<i64>>(3)?,"next_at":r.get::<_,Option<i64>>(4)?,"error":r.get::<_,Option<String>>(5)?}))))? {
        let (url, value) = row?;
        fetches.insert(url, value);
    }
    Ok(Value::Object(fetches))
}

impl Store {
    pub(crate) fn requested_github_fetches(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .db
            .prepare("SELECT url FROM github_fetch_status WHERE requested_at IS NOT NULL")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn begin_github_fetch(&mut self, url: &str, now: i64) -> Result<(i64, bool)> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = tx.query_row("INSERT INTO github_fetch_status(url,started_at) VALUES(?1,?2) ON CONFLICT(url) DO UPDATE SET started_at=max(?2,coalesce(started_at,0)+1,coalesce(finished_at,0)+1,coalesce(requested_at,0)) RETURNING started_at,requested_at IS NOT NULL",params![url.trim_end_matches('/'),now], |r| Ok((r.get(0)?,r.get(1)?)))?;
        tx.commit()?;
        Ok(result)
    }

    pub(crate) fn finish_github_fetch(
        &self,
        url: &str,
        started: i64,
        finished: i64,
        next: i64,
        error: Option<&str>,
    ) -> Result<()> {
        let error = error.map(|e| e.chars().take(1000).collect::<String>());
        self.db.execute("UPDATE github_fetch_status SET finished_at=max(?3,started_at),next_at=?4,error=?5,requested_at=CASE WHEN requested_at<=started_at THEN NULL ELSE requested_at END WHERE url=?1 AND started_at=?2",params![url.trim_end_matches('/'),started,finished,next,error])?;
        Ok(())
    }

    pub(crate) fn github_fetch_cooldown(&self, until: i64) -> Result<()> {
        self.db.execute(
            "UPDATE github_fetch_status SET next_at=?1 WHERE coalesce(next_at,0)<?1",
            [until],
        )?;
        Ok(())
    }
}
