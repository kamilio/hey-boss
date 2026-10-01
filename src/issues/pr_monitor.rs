//! Atomic completion of tasks whose explicitly classified fix PRs have merged.
use super::*;

pub(crate) struct TrackedPullRequest {
    pub url: String,
    pub checked_at: Option<i64>,
    pub closed: bool,
    pub backfill: bool,
}

fn merged_tasks(db: &Connection) -> Result<Vec<(Project, i64)>> {
    if super::super::global_settings::read(db)?["auto_close_merged_prs"] != true {
        return Ok(Vec::new());
    }
    Ok(db.prepare("SELECT i.project_id,p.name,i.number FROM issues i JOIN projects p ON p.id=i.project_id WHERE i.state<>'closed' AND i.deleted_at IS NULL AND i.draft=0 AND EXISTS(SELECT 1 FROM issue_pull_requests pr WHERE pr.project_id=i.project_id AND pr.issue_number=i.number AND pr.purpose='fix') AND NOT EXISTS(SELECT 1 FROM issue_pull_requests pr WHERE pr.project_id=i.project_id AND pr.issue_number=i.number AND pr.purpose='fix' AND pr.status<>'merged')")?
        .query_map([], |r| Ok((Project { id:r.get(0)?, name:r.get(1)? },r.get::<_,i64>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn merged_history(
    db: &Connection,
    project: &str,
    limit: u32,
    offset: u32,
) -> Result<Value> {
    let viewer: Option<i64> = db.query_row(
        "SELECT github_user_id FROM global_settings WHERE id=1",
        [],
        |row| row.get(0),
    )?;
    // Materialize the PR selection once, then drive indexed URL lookups from it.
    // CROSS JOIN prevents SQLite from rescanning the selection per task link.
    let mut query = db.prepare(
        "WITH history AS MATERIALIZED (
            SELECT pr.url,max(pr.pr_title) AS title,min(pr.merged_at) AS merged_at,min(pr.checked_at) AS observed_at
            FROM issue_pull_requests pr
            JOIN issues i ON i.project_id=pr.project_id AND i.number=pr.issue_number
            WHERE pr.project_id=?1 AND pr.status='merged' AND pr.purpose='fix' AND pr.author_id=?4 AND i.deleted_at IS NULL
            GROUP BY pr.url
            ORDER BY coalesce(min(pr.merged_at),0) DESC,pr.url LIMIT ?2 OFFSET ?3
        )
        SELECT h.url,h.title,h.merged_at,h.observed_at,i.number,i.title
        FROM history h
        CROSS JOIN issue_pull_requests pr ON pr.project_id=?1 AND pr.url=h.url AND pr.purpose='fix'
        CROSS JOIN issues i ON i.project_id=pr.project_id AND i.number=pr.issue_number
        WHERE i.deleted_at IS NULL
        ORDER BY coalesce(h.merged_at,0) DESC,h.url,i.number",
    )?;
    let mut rows = query.query(params![project, i64::from(limit) + 1, offset, viewer])?;
    let mut prs: Vec<Value> = Vec::new();
    while let Some(row) = rows.next()? {
        let url: String = row.get(0)?;
        if prs.last().is_none_or(|pr| pr["url"] != url) {
            prs.push(json!({
                "url":url,"title":row.get::<_,Option<String>>(1)?,
                "merged_at":row.get::<_,Option<i64>>(2)?,
                "observed_at":row.get::<_,Option<i64>>(3)?,"issues":[]
            }));
        }
        prs.last_mut().unwrap()["issues"]
            .as_array_mut()
            .unwrap()
            .push(json!({"number":row.get::<_,i64>(4)?,"title":row.get::<_,String>(5)?}));
    }
    let more = prs.len() > limit as usize;
    prs.truncate(limit as usize);
    let authorship_pending: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM issue_pull_requests pr JOIN issues i ON i.project_id=pr.project_id AND i.number=pr.issue_number WHERE pr.project_id=?1 AND pr.status='merged' AND pr.purpose='fix' AND i.deleted_at IS NULL AND (?2 IS NULL OR pr.author_id IS NULL))", params![project,viewer], |row| row.get(0))?;
    Ok(
        json!({"ok":true,"pull_requests":prs,"authorship_pending":authorship_pending,"next_offset":more.then_some(u64::from(offset)+u64::from(limit))}),
    )
}

impl Store {
    pub(crate) fn record_github_user(&mut self, id: i64) -> Result<()> {
        let current: Option<i64> = self.db.query_row(
            "SELECT github_user_id FROM global_settings WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        if id > 0 && current != Some(id) {
            self.db.execute("UPDATE global_settings SET github_user_id=?1 WHERE id=1 AND github_user_id IS NOT ?1", [id])?;
        }
        Ok(())
    }

    pub(crate) fn record_pr_author(&mut self, url: &str, id: i64) -> Result<()> {
        if id > 0 {
            self.db.execute(
                "UPDATE issue_pull_requests SET author_id=?2 WHERE url=?1 AND author_id IS NOT ?2",
                params![url, id],
            )?;
        }
        Ok(())
    }

    pub(crate) fn tracked_pull_requests(&self) -> Result<Vec<TrackedPullRequest>> {
        let mut query = self.db.prepare("SELECT pr.url,CASE WHEN count(pr.checked_at)=count(*) AND sum(pr.status='merged' AND pr.purpose='fix' AND pr.author_id IS NULL)=0 THEN min(pr.checked_at) END,min(pr.status='closed'),min(pr.status='merged') FROM issue_pull_requests pr JOIN issues i ON i.project_id=pr.project_id AND i.number=pr.issue_number WHERE i.deleted_at IS NULL AND ((i.state<>'closed' AND pr.status<>'merged') OR (pr.status='merged' AND (pr.merged_at IS NULL OR (pr.purpose='fix' AND pr.author_id IS NULL)))) GROUP BY pr.url ORDER BY pr.url")?;
        Ok(query
            .query_map([], |r| {
                Ok(TrackedPullRequest {
                    url: r.get(0)?,
                    checked_at: r.get(1)?,
                    closed: r.get(2)?,
                    backfill: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn record_pr_status(
        &mut self,
        url: &str,
        status: Option<&str>,
        checked_at: i64,
        error: Option<&str>,
    ) -> Result<()> {
        self.db.execute("UPDATE issue_pull_requests SET status=coalesce(?2,status),checked_at=CASE WHEN ?2 IS NULL THEN checked_at ELSE ?3 END,error=?4 WHERE url=?1 AND status<>'merged' AND (checked_at IS NULL OR checked_at<=?3)", params![url,status,checked_at,error])?;
        Ok(())
    }

    pub(crate) fn record_pr_merge_details(
        &mut self,
        url: &str,
        title: &str,
        merged_at: Option<&str>,
        checked_at: i64,
    ) -> Result<()> {
        // Keep terminal status immutable, but allow historical merges to acquire
        // their GitHub timestamp. Stale metadata never overwrites a newer read.
        self.db.execute("UPDATE issue_pull_requests SET pr_title=?2,checked_at=max(coalesce(checked_at,0),?4),merged_at=coalesce(merged_at,CAST(strftime('%s',?3) AS INTEGER)*1000) WHERE url=?1 AND status='merged' AND (checked_at IS NULL OR checked_at<=?4) AND (pr_title IS NOT ?2 OR (merged_at IS NULL AND strftime('%s',?3) IS NOT NULL))",params![url,title,merged_at,checked_at])?;
        Ok(())
    }

    pub(crate) fn record_open_pr_if_changed(&mut self, url: &str, checked_at: i64) -> Result<()> {
        // The watcher owns lifecycle checks for its PRs. Reconfirming an open
        // PR every 30 seconds must not append idle writes to the fleet journal.
        let changed: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_pull_requests WHERE url=?1 AND status<>'merged' AND (status<>'open' OR error IS NOT NULL) AND (checked_at IS NULL OR checked_at<=?2))", params![url,checked_at], |row| row.get(0))?;
        if changed {
            self.record_pr_status(url, Some("open"), checked_at, None)?;
        }
        Ok(())
    }

    pub(crate) fn close_merged_pull_requests(&mut self, actor: &Actor) -> Result<usize> {
        // Idle polling is a WAL read, so it cannot queue behind normal writes.
        if merged_tasks(&self.db)?.is_empty() {
            return Ok(0);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Re-evaluate links and state while holding the writer lock. A new fix
        // attached during the network read must prevent premature completion.
        let tasks = merged_tasks(&tx)?;
        if tasks.is_empty() {
            return Ok(0);
        }
        let now = crate::issues::worker::now();
        tx.execute("INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET last_seen=excluded.last_seen",params![actor.id,serde_json::to_string(actor)?,now])?;
        for (project, number) in &tasks {
            let urls = registry::pull_requests(&tx, &project.id, *number)?
                .into_iter()
                .filter(|pr| pr["purpose"] == "fix")
                .map(|pr| pr["url"].as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            mutate(
                &tx,
                project,
                actor,
                &Operation::Close {
                    allow_long_comment: false,
                    number: *number,
                    comment: Some(format!(
                        "Automatically closed: all fix PRs merged.\n\n{}",
                        urls.join("\n")
                    )),
                    force: true,
                },
                now,
            )?;
            super::super::blockers::reconcile(&tx, &project.id, Some(&actor.id), now)?;
            tx.execute(
                "UPDATE projects SET activity_at=max(activity_at,?2) WHERE id=?1",
                params![project.id, now],
            )?;
        }
        tx.commit()?;
        Ok(tasks.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merged_history_loads_task_links_without_per_pr_queries() {
        let (store, actor, root) = fixture();
        for number in 10..522 {
            let url = format!("https://github.com/o/r/pull/{number}");
            store.db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('named:test',?1,'Task','','open',?2,0,0,1,'[]')", params![number,actor.id]).unwrap();
            for issue in [5, number] {
                store.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose,status,author_id,merged_at) VALUES('named:test',?1,?2,?3,0,'fix','merged',42,?4)", params![issue,url,actor.id,number]).unwrap();
            }
        }
        drop(store);
        let path = root.join("issues.db");
        let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
        let mut work = Vec::new();
        for limit in [16, 128, 512] {
            let (db, transport) = crate::database::tests::measured_connection(&path);
            let result = merged_history(&db, "named:test", limit, 0).unwrap();
            drop(db);
            let (commands, steps) = transport.join().unwrap();
            let prs = result["pull_requests"].as_array().unwrap();
            assert_eq!(prs.len(), limit as usize);
            for (index, pr) in prs.iter().enumerate() {
                assert_eq!(
                    pr["url"],
                    format!("https://github.com/o/r/pull/{}", 521 - index)
                );
                assert_eq!(
                    pr["issues"],
                    json!([{"number":5,"title":"Task"},{"number":521-index,"title":"Task"}])
                );
            }
            assert_eq!(
                result["next_offset"],
                if limit == 512 {
                    Value::Null
                } else {
                    json!(limit)
                }
            );
            eprintln!("{limit} merged PRs: {commands} RPCs, {steps} VM steps");
            work.push((limit, commands, steps));
        }
        owner.stop();
        std::fs::remove_dir_all(root).unwrap();
        for (limit, commands, steps) in work {
            assert!(
                commands <= 4,
                "History must fetch all task links together; got {commands} RPCs for {limit} PRs"
            );
            assert!(
                steps < 110_000,
                "History must not rescan PRs per task link: {steps} VM steps for {limit} PRs"
            );
        }
    }

    #[test]
    fn merged_history_requires_confirmed_current_account_authorship() {
        let (mut store, _, root) = fixture();
        for number in 1..=2 {
            store
                .record_pr_status(
                    &format!("https://github.com/o/r/pull/{number}"),
                    Some("merged"),
                    2000,
                    None,
                )
                .unwrap();
        }
        store
            .record_pr_author("https://github.com/o/r/pull/1", 42)
            .unwrap();
        store
            .record_pr_author("https://github.com/o/r/pull/2", 99)
            .unwrap();
        store.record_github_user(42).unwrap();
        let result = merged_history(&store.db, "named:test", 1, 0).unwrap();
        assert_eq!(result["pull_requests"].as_array().unwrap().len(), 1);
        assert_eq!(
            result["pull_requests"][0]["url"],
            "https://github.com/o/r/pull/1"
        );
        assert!(result["next_offset"].is_null());
        assert_eq!(result["authorship_pending"], false);
        store.db.execute("UPDATE issue_pull_requests SET author_id=NULL WHERE url='https://github.com/o/r/pull/2'", []).unwrap();
        let pending = merged_history(&store.db, "named:test", 10, 0).unwrap();
        assert_eq!(pending["pull_requests"].as_array().unwrap().len(), 1);
        assert_eq!(pending["authorship_pending"], true);
        let backfill = store
            .tracked_pull_requests()
            .unwrap()
            .into_iter()
            .find(|pr| pr.url.ends_with("/2"))
            .unwrap();
        assert!(backfill.backfill);
        assert!(backfill.checked_at.is_none());
        store
            .record_pr_author("https://github.com/o/r/pull/2", 99)
            .unwrap();
        store.record_github_user(99).unwrap();
        assert_eq!(
            merged_history(&store.db, "named:test", 10, 0).unwrap()["pull_requests"][0]["url"],
            "https://github.com/o/r/pull/2"
        );
        store
            .db
            .execute("UPDATE global_settings SET github_user_id=NULL", [])
            .unwrap();
        assert!(
            merged_history(&store.db, "named:test", 10, 0).unwrap()["pull_requests"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn merged_history_backfills_closed_tasks_and_repairs_fleet_capture() {
        let (mut store, _, root) = fixture();
        let url = "https://github.com/o/r/pull/1";
        store
            .record_pr_status(url, Some("merged"), 2000, None)
            .unwrap();
        store
            .db
            .execute("UPDATE issues SET state='closed'", [])
            .unwrap();
        assert!(
            store
                .tracked_pull_requests()
                .unwrap()
                .iter()
                .any(|pr| pr.url == url)
        );
        store.db.execute_batch("CREATE TABLE capture_log(value TEXT); CREATE TRIGGER fleet_capture_issue_pull_requests_UPDATE AFTER UPDATE ON issue_pull_requests WHEN 1 AND NOT (OLD.project_id IS NEW.project_id AND OLD.url IS NEW.url) BEGIN INSERT INTO capture_log VALUES(json_object('project_id',NEW.project_id,'url',NEW.url)); END;").unwrap();
        registry::repair_pr_capture(&store.db).unwrap();
        store
            .record_pr_merge_details(url, "Historical merge", Some("2026-09-29T23:30:00Z"), 3000)
            .unwrap();
        let captured: String = store
            .db
            .query_row("SELECT value FROM capture_log LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let captured: Value = serde_json::from_str(&captured).unwrap();
        assert_eq!(captured["pr_title"], "Historical merge");
        assert_eq!(captured["merged_at"], 1790724600000_i64);
        store.record_pr_author(url, 99).unwrap();
        let captured: String = store
            .db
            .query_row(
                "SELECT value FROM capture_log ORDER BY rowid DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&captured).unwrap()["author_id"],
            99
        );
        store.record_pr_author(url, 42).unwrap();
        assert!(
            !store
                .tracked_pull_requests()
                .unwrap()
                .iter()
                .any(|pr| pr.url == url)
        );
        assert_eq!(
            merged_history(&store.db, "named:test", 10, 0).unwrap()["pull_requests"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn merged_history_excludes_reference_only_prs_before_pagination() {
        let (mut store, actor, root) = fixture();
        for (issue, pr, purpose) in [(2, 4, "prerequisite"), (3, 5, "unspecified"), (4, 6, "fix")] {
            store.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose) VALUES('named:test',?1,?2,?3,0,?4)",params![issue,format!("https://github.com/o/r/pull/{pr}"),actor.id,purpose]).unwrap();
        }
        store
            .db
            .execute("UPDATE issue_pull_requests SET author_id=42", [])
            .unwrap();
        for pr in 1..=6 {
            let url = format!("https://github.com/o/r/pull/{pr}");
            store
                .record_pr_status(&url, Some("merged"), 2000, None)
                .unwrap();
            store
                .record_pr_merge_details(
                    &url,
                    "Merged",
                    Some(if pr > 2 {
                        "2026-09-30T12:00:00Z"
                    } else {
                        "2026-09-29T12:00:00Z"
                    }),
                    2000,
                )
                .unwrap();
        }
        let page = merged_history(&store.db, "named:test", 1, 0).unwrap();
        assert_eq!(
            page["pull_requests"][0]["url"],
            "https://github.com/o/r/pull/1"
        );
        assert_eq!(
            page["pull_requests"][0]["issues"],
            json!([
                {"number":1,"title":"Task"}, {"number":5,"title":"Task"}
            ])
        );
        assert_eq!(page["next_offset"], 1);
        let next = merged_history(&store.db, "named:test", 1, 1).unwrap();
        assert_eq!(
            next["pull_requests"][0]["url"],
            "https://github.com/o/r/pull/2"
        );
        assert!(next["next_offset"].is_null());
        // A reference can enter delivered history when explicitly reclassified.
        store.db.execute("UPDATE issue_pull_requests SET purpose='fix' WHERE url='https://github.com/o/r/pull/3'", []).unwrap();
        assert_eq!(
            merged_history(&store.db, "named:test", 10, 0).unwrap()["pull_requests"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merged_history_deduplicates_and_preserves_github_dates() {
        let (mut store, _, root) = fixture();
        let url = "https://github.com/o/r/pull/1";
        store
            .record_pr_status(url, Some("merged"), 2000, None)
            .unwrap();
        store
            .record_pr_merge_details(url, "Ship it", Some("2026-09-29T23:30:00Z"), 2000)
            .unwrap();
        // A stale response cannot replace the title or move a merge to another day.
        store
            .record_pr_merge_details(url, "Old title", Some("2026-09-28T00:00:00Z"), 1000)
            .unwrap();
        store
            .record_pr_status("https://github.com/o/r/pull/2", Some("merged"), 3000, None)
            .unwrap();
        let page = merged_history(&store.db, "named:test", 1, 0).unwrap();
        assert_eq!(page["ok"], true);
        assert_eq!(page["pull_requests"].as_array().unwrap().len(), 1);
        assert_eq!(page["pull_requests"][0]["url"], url);
        assert_eq!(page["pull_requests"][0]["title"], "Ship it");
        assert_eq!(page["pull_requests"][0]["merged_at"], 1790724600000_i64);
        assert_eq!(
            page["pull_requests"][0]["issues"].as_array().unwrap().len(),
            2
        );
        assert_eq!(page["next_offset"], 1);
        let older = merged_history(&store.db, "named:test", 1, 1).unwrap();
        assert!(older["pull_requests"][0]["merged_at"].is_null());
        assert_eq!(older["pull_requests"][0]["observed_at"], 3000);
        assert!(older["next_offset"].is_null());
        assert!(
            merged_history(&store.db, "another", 10, 0).unwrap()["pull_requests"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    fn fixture() -> (Store, Actor, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "hb-pr-monitor-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        let actor =
            crate::issues::identity::resolve(Some("human:pr-monitor"), "test", &root).unwrap();
        store
            .db
            .execute(
                "INSERT INTO projects(id,name,next_number) VALUES('named:test','test',10)",
                [],
            )
            .unwrap();
        store
            .db
            .execute(
                "INSERT INTO agents VALUES(?1,?2,0)",
                params![actor.id, serde_json::to_string(&actor).unwrap()],
            )
            .unwrap();
        for number in 1..=5 {
            store.db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('named:test',?1,'Task','','open',?2,0,0,1,'[]')", params![number,actor.id]).unwrap();
        }
        for (number, pr, purpose) in [
            (1, 1, "fix"),
            (1, 2, "fix"),
            (1, 3, "supporting-evidence"),
            (2, 1, "prerequisite"),
            (3, 1, "unspecified"),
            (4, 1, "fix"),
            (5, 1, "fix"),
        ] {
            store.db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose) VALUES('named:test',?1,?2,?3,0,?4)",params![number,format!("https://github.com/o/r/pull/{pr}"),actor.id,purpose]).unwrap();
        }
        store
            .db
            .execute("UPDATE issues SET deleted_at=1 WHERE number=4", [])
            .unwrap();
        store
            .db
            .execute("UPDATE issues SET state='closed' WHERE number=5", [])
            .unwrap();
        store
            .db
            .execute("UPDATE global_settings SET github_user_id=42", [])
            .unwrap();
        store
            .db
            .execute("UPDATE issue_pull_requests SET author_id=42", [])
            .unwrap();
        (store, actor, root)
    }
    #[test]
    fn unchanged_watcher_open_status_does_not_wait_for_a_writer() {
        let (mut store, _, root) = fixture();
        let url = "https://github.com/o/r/pull/1";
        store.record_open_pr_if_changed(url, 200).unwrap();
        store
            .db
            .busy_timeout(std::time::Duration::from_millis(25))
            .unwrap();
        let mut writer = Connection::open(root.join("issues.db")).unwrap();
        let lock = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        store.record_open_pr_if_changed(url, 300).unwrap();
        assert_eq!(
            store
                .db
                .query_row(
                    "SELECT min(checked_at) FROM issue_pull_requests WHERE url=?1",
                    [url],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            200
        );
        drop(lock);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn older_metadata_cannot_roll_back_a_newer_pr_status_or_replace_its_error() {
        let (mut store, _, root) = fixture();
        let url = "https://github.com/o/r/pull/1";
        store
            .record_pr_status(url, Some("open"), 200, None)
            .unwrap();
        store
            .record_pr_status(url, Some("closed"), 100, None)
            .unwrap();
        store
            .record_pr_status(url, None, 150, Some("Old read failed"))
            .unwrap();
        let rows: i64 = store.db.query_row("SELECT count(*) FROM issue_pull_requests WHERE url=?1 AND status='open' AND checked_at=200 AND error IS NULL", [url], |row| row.get(0)).unwrap();
        assert_eq!(rows, 5);
        store
            .record_pr_status(url, Some("closed"), 300, None)
            .unwrap();
        assert_eq!(store.db.query_row("SELECT count(*) FROM issue_pull_requests WHERE url=?1 AND status='closed' AND checked_at=300", [url], |row| row.get::<_,i64>(0)).unwrap(), 5);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tracks_distinct_active_issue_prs_and_resumes_when_reopened() {
        let (store, _, root) = fixture();
        store.db.execute("UPDATE issue_pull_requests SET url='https://github.com/o/r/pull/99' WHERE issue_number=5", []).unwrap();
        assert!(
            !store
                .tracked_pull_requests()
                .unwrap()
                .iter()
                .any(|p| p.url.ends_with("/99"))
        );
        store
            .db
            .execute("UPDATE issues SET state='open' WHERE number=5", [])
            .unwrap();
        let prs = store.tracked_pull_requests().unwrap();
        assert_eq!(prs.len(), 4);
        assert!(
            prs.iter()
                .any(|p| p.url.ends_with("/99") && p.checked_at.is_none())
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn idle_pr_completion_does_not_wait_for_a_database_writer() {
        let (mut store, actor, root) = fixture();
        store
            .db
            .busy_timeout(std::time::Duration::from_millis(25))
            .unwrap();
        let mut writer = Connection::open(root.join("issues.db")).unwrap();
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(store.close_merged_pull_requests(&actor).unwrap(), 0);
        drop(tx);
        drop(writer);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn closes_only_after_all_fix_prs_merge_and_only_once() {
        let (mut store, actor, root) = fixture();
        assert_eq!(store.tracked_pull_requests().unwrap().len(), 3);
        store
            .record_pr_status("https://github.com/o/r/pull/1", Some("merged"), 10, None)
            .unwrap();
        assert_eq!(store.close_merged_pull_requests(&actor).unwrap(), 0);
        store
            .record_pr_status("https://github.com/o/r/pull/2", Some("merged"), 10, None)
            .unwrap();
        assert_eq!(store.close_merged_pull_requests(&actor).unwrap(), 1);
        assert_eq!(store.close_merged_pull_requests(&actor).unwrap(), 0);
        assert_eq!(
            store
                .db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .db
                .query_row("SELECT state FROM issues WHERE number=1", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "closed"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pr_status_persists_and_failed_reads_keep_previous_evidence() {
        let (mut store, actor, root) = fixture();
        let url = "https://github.com/o/r/pull/1";
        store.record_pr_status(url, Some("open"), 10, None).unwrap();
        store
            .record_pr_status(url, None, 20, Some("GitHub unavailable"))
            .unwrap();
        let prs = registry::pull_requests(&store.db, "named:test", 1).unwrap();
        assert_eq!(prs[0]["status"], "open");
        assert_eq!(prs[0]["checked_at"], 10);
        assert_eq!(prs[0]["error"], "GitHub unavailable");
        store
            .record_pr_status(url, Some("merged"), 30, None)
            .unwrap();
        let prs = registry::pull_requests(&store.db, "named:test", 1).unwrap();
        assert_eq!(prs[0]["status"], "merged");
        assert!(prs[0]["error"].is_null());
        assert!(
            store
                .tracked_pull_requests()
                .unwrap()
                .iter()
                .any(|pr| pr.url == url)
        );
        store
            .record_pr_merge_details(url, "Merged PR", Some("2026-09-29T23:30:00Z"), 30)
            .unwrap();
        assert!(
            !store
                .tracked_pull_requests()
                .unwrap()
                .iter()
                .any(|pr| pr.url == url)
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
        drop(actor);
    }
    #[test]
    fn disabled_setting_and_changed_links_are_rechecked_before_closing() {
        let (mut store, actor, root) = fixture();
        for url in [
            "https://github.com/o/r/pull/1",
            "https://github.com/o/r/pull/2",
        ] {
            store
                .record_pr_status(url, Some("merged"), 10, None)
                .unwrap();
        }
        store
            .db
            .execute("UPDATE global_settings SET auto_close_merged_prs=0", [])
            .unwrap();
        assert!(!store.tracked_pull_requests().unwrap().is_empty());
        assert_eq!(store.close_merged_pull_requests(&actor).unwrap(), 0);
        store
            .db
            .execute("UPDATE global_settings SET auto_close_merged_prs=1", [])
            .unwrap();
        store.db.execute("UPDATE issue_pull_requests SET purpose='fix' WHERE issue_number=1 AND purpose='supporting-evidence'",[]).unwrap();
        assert_eq!(store.close_merged_pull_requests(&actor).unwrap(), 0);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
