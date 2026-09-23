//! Atomic completion of tasks whose explicitly classified fix PRs have merged.
use super::*;

impl Store {
    pub(crate) fn tracked_pull_requests(&self) -> Result<Vec<String>> {
        let mut query = self.db.prepare("SELECT DISTINCT pr.url FROM issue_pull_requests pr JOIN issues i ON i.project_id=pr.project_id AND i.number=pr.issue_number WHERE i.deleted_at IS NULL AND pr.status<>'merged' ORDER BY pr.url")?;
        Ok(query
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn record_pr_status(
        &mut self,
        url: &str,
        status: Option<&str>,
        checked_at: i64,
        error: Option<&str>,
    ) -> Result<()> {
        self.db.execute("UPDATE issue_pull_requests SET status=coalesce(?2,status),checked_at=CASE WHEN ?2 IS NULL THEN checked_at ELSE ?3 END,error=?4 WHERE url=?1 AND status<>'merged'", params![url,status,checked_at,error])?;
        Ok(())
    }

    pub(crate) fn close_merged_pull_requests(&mut self, actor: &Actor) -> Result<usize> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if super::super::global_settings::read(&tx)?["auto_close_merged_prs"] != true {
            return Ok(0);
        }
        // Re-evaluate links and state while holding the writer lock. A new fix
        // attached during the network read must prevent premature completion.
        let tasks = tx.prepare("SELECT i.project_id,p.name,i.number FROM issues i JOIN projects p ON p.id=i.project_id WHERE i.state<>'closed' AND i.deleted_at IS NULL AND i.draft=0 AND EXISTS(SELECT 1 FROM issue_pull_requests pr WHERE pr.project_id=i.project_id AND pr.issue_number=i.number AND pr.purpose='fix') AND NOT EXISTS(SELECT 1 FROM issue_pull_requests pr WHERE pr.project_id=i.project_id AND pr.issue_number=i.number AND pr.purpose='fix' AND pr.status<>'merged')")?
            .query_map([], |r| Ok((Project { id:r.get(0)?, name:r.get(1)? },r.get::<_,i64>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
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
        (store, actor, root)
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
            !store
                .tracked_pull_requests()
                .unwrap()
                .contains(&url.to_string())
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
