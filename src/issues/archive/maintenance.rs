//! Bounded catch-up with per-item backoff. Failures retain their hot data and
//! cannot block unrelated receipts, runs, issues, or interrupted restorations.
use super::*;
use crate::database::Connection as HotConnection;
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Maintenance {
    failed: BTreeMap<String, i64>,
    after_failure: [Option<String>; 6],
}

const CANDIDATES: &[&str] = &[
    "SELECT json_array('restore',project_id,number) work FROM issues
        WHERE archive_key IS NOT NULL AND archive_restoring=1 AND (?3 IS NULL OR (project_id,number)>(json_extract(?3,'$[1]'),json_extract(?3,'$[2]'))) ORDER BY project_id,number",
    "SELECT json_array('issue-cleanup',project_id,number) work FROM issues
        WHERE archive_key IS NOT NULL AND archive_cleanup=1 AND archive_restoring=0 AND (?3 IS NULL OR (project_id,number)>(json_extract(?3,'$[1]'),json_extract(?3,'$[2]'))) ORDER BY project_id,number",
    "SELECT json_array('run-cleanup',id) work FROM worker_runs
        WHERE archive_cleanup=1 AND finished_at IS NOT NULL AND (?3 IS NULL OR id>json_extract(?3,'$[1]')) ORDER BY id",
    "SELECT json_array('receipt',project_id,actor,request_id,created_at) work FROM requests
        WHERE archive_key IS NULL AND created_at<=?1
        AND (?3 IS NULL OR (created_at,project_id,actor,request_id)>(json_extract(?3,'$[4]'),json_extract(?3,'$[1]'),json_extract(?3,'$[2]'),json_extract(?3,'$[3]')))
        ORDER BY created_at,project_id,actor,request_id",
    "SELECT json_array('run',id,finished_at) work FROM worker_runs
        WHERE archive_pending=1 AND finished_at IS NOT NULL AND finished_at<=?1 AND updated_at<=?1
        AND NOT EXISTS(SELECT 1 FROM worker_events e WHERE e.run_id=worker_runs.id AND e.created_at>?1)
        AND (?3 IS NULL OR (finished_at,id)>(json_extract(?3,'$[2]'),json_extract(?3,'$[1]')))
        ORDER BY finished_at,id",
    "SELECT json_array('issue',project_id,number,max(updated_at,coalesce(closed_at,0),coalesce(deleted_at,0),archive_touched_at)) work FROM issues i
        WHERE archive_key IS NULL AND (state='closed' OR deleted_at IS NOT NULL)
        AND max(updated_at,coalesce(closed_at,0),coalesce(deleted_at,0),archive_touched_at)<=?1
        AND attempt_hold IS NULL AND (SELECT role!='agent' FROM fleet_meta WHERE id=1)
        AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL)
        AND NOT EXISTS(SELECT 1 FROM fleet_allocations a WHERE a.project_id=i.project_id AND a.issue_number=i.number)
        AND NOT EXISTS(SELECT 1 FROM comments h WHERE h.project_id=i.project_id AND h.issue_number=i.number AND h.created_at>?1)
        AND NOT EXISTS(SELECT 1 FROM events h WHERE h.project_id=i.project_id AND h.issue_number=i.number AND h.created_at>?1)
        AND NOT EXISTS(SELECT 1 FROM issue_status_updates h WHERE h.project_id=i.project_id AND h.issue_number=i.number AND h.created_at>?1)
        AND (?3 IS NULL OR (max(updated_at,coalesce(closed_at,0),coalesce(deleted_at,0),archive_touched_at),project_id,number)>(json_extract(?3,'$[3]'),json_extract(?3,'$[1]'),json_extract(?3,'$[2]')))
        ORDER BY max(updated_at,coalesce(closed_at,0),coalesce(deleted_at,0),archive_touched_at),project_id,number",
];

impl Maintenance {
    /// Returns attempted work, including a newly deferred failure. The owner
    /// keeps catching up while candidates exist, then backs off when idle.
    pub(crate) fn run(&mut self, db: &HotConnection, now: i64) -> Result<usize> {
        if !db.is_autocommit() {
            return Err(unavailable(
                "Archive maintenance cannot run inside a hot transaction",
            ));
        }
        let (_, version) = db.check_schema()?;
        if version != Store::schema_version() {
            return Ok(0);
        }
        self.failed.retain(|_, retry| *retry > now);
        let excluded = serde_json::to_string(&self.failed.keys().collect::<Vec<_>>())?;
        let mut attempted = 0;
        for (category, candidates) in CANDIDATES.iter().enumerate() {
            let sql = format!(
                "SELECT work FROM ({candidates}) WHERE work NOT IN (SELECT value FROM json_each(?2)) LIMIT 1"
            );
            let work: Option<String> = db
                .query_row(
                    &sql,
                    params![
                        now.saturating_sub(GRACE_MS),
                        excluded,
                        self.after_failure[category]
                    ],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(work) = work else {
                // Wrap after visiting later records. Keep catching up so a
                // valid item before the cursor is not mistaken for an idle queue.
                attempted += usize::from(self.after_failure[category].take().is_some());
                continue;
            };
            let item: Value = serde_json::from_str(&work)?;
            attempted += 1;
            if let Err(error) = perform(db, &item, now) {
                eprintln!("Archive maintenance deferred {work}: {error}");
                // A cursor also advances past evicted failures, so a damaged
                // prefix larger than the cache cannot starve later healthy work.
                self.after_failure[category] = Some(work.clone());
                // Bound failure bookkeeping even if the underlying disk fails.
                if self.failed.len() >= 128
                    && let Some(oldest) = self
                        .failed
                        .iter()
                        .min_by_key(|(_, at)| *at)
                        .map(|(key, _)| key.clone())
                {
                    self.failed.remove(&oldest);
                }
                self.failed.insert(work, now.saturating_add(60_000));
            }
        }
        match transfer::cleanup_downloads(db, now) {
            Ok(removed) => attempted += usize::from(removed > 0),
            Err(error) => eprintln!("Archive transfer cleanup: {error}"),
        }
        Ok(attempted)
    }
}

fn perform(db: &HotConnection, item: &Value, now: i64) -> Result<()> {
    let text = |n: usize| {
        item[n]
            .as_str()
            .ok_or_else(|| unavailable("Invalid maintenance identity"))
    };
    let number = || {
        item[2]
            .as_i64()
            .ok_or_else(|| unavailable("Invalid maintenance issue"))
    };
    match text(0)? {
        "restore" => restore_issue(db, text(1)?, number()?, now)?,
        "issue-cleanup" => {
            history::cleanup_issue(db, text(1)?, number()?)?;
        }
        "run-cleanup" => {
            runs::cleanup_run(db, text(1)?)?;
        }
        "receipt" => {
            archive_receipt(
                db,
                text(1)?,
                text(2)?,
                text(3)?,
                item[4]
                    .as_i64()
                    .ok_or_else(|| unavailable("Invalid receipt time"))?,
            )?;
        }
        "run" => {
            runs::archive_run(db, now, text(1)?)?;
        }
        "issue" => {
            archive_issue(db, text(1)?, number()?, now)?;
        }
        _ => return Err(unavailable("Invalid maintenance operation")),
    }
    Ok(())
}
