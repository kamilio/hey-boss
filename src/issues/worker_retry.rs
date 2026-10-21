//! Retry evidence is fleet-wide; executable process ownership remains local.
use super::Result;
use crate::database::Connection;
use rusqlite::OptionalExtension;

#[cfg(test)]
#[path = "worker_retry_tests.rs"]
mod tests;

fn distributed_history(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_attempts')",
        [],
        |r| r.get(0),
    )?)
}

fn history_query(distributed: bool, scope: &str) -> String {
    let release = "EXISTS(SELECT 1 FROM events e WHERE e.project_id=r.project_id AND e.issue_number=r.issue_number AND e.action='reopened' AND e.created_at>r.finished_at)";
    let mut sql = format!(
        "SELECT id,project_id,issue_number,state,started_at,finished_at,summary,
        retry_allowed OR {release} AS retry_allowed,retry_at,retry_count
        FROM worker_runs r WHERE finished_at IS NOT NULL{scope}"
    );
    if distributed {
        sql.push_str(&format!(" UNION ALL
            SELECT id,project_id,issue_number,state,started_at,finished_at,summary,
            coalesce(json_extract(payload,'$.retry_allowed'),state IN ('cancelled','interrupted')) OR {release},
            json_extract(payload,'$.retry_at'),coalesce(json_extract(payload,'$.retry_count'),0)
            FROM distributed_attempts r WHERE finished_at IS NOT NULL{scope}
            AND NOT EXISTS(SELECT 1 FROM worker_runs local WHERE local.id=r.id)"));
    }
    sql
}

fn readiness_sql(distributed: bool) -> String {
    // SQLite can materialize the entire UNION view for a correlated lookup,
    // even with issue keys outside the view. Scope both indexed branches before
    // combining them, and share the retry/reopen rules with history readers.
    include_str!("subtask-readiness.sql").replace(
        "{scoped_retry_history}",
        &history_query(
            distributed,
            " AND r.project_id=i.project_id AND r.issue_number=i.number",
        ),
    )
}

pub(super) fn install_readiness(db: &Connection) -> Result<()> {
    db.execute_batch(&readiness_sql(distributed_history(db)?))?;
    Ok(())
}

pub(crate) fn migrate(db: &Connection) -> Result<()> {
    let distributed = distributed_history(db)?;
    let sql = format!(
        "CREATE VIEW worker_retry_history AS\n        {}",
        history_query(distributed, "")
    );
    let current: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='worker_retry_history' AND EXISTS(SELECT 1 FROM sqlite_master WHERE name='worker_retry_reopens')",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let readiness = readiness_sql(distributed);
    let current_readiness: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='issue_pickup_ready'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let expected_readiness = readiness
        .strip_prefix("DROP VIEW issue_pickup_ready;\n")
        .unwrap()
        .trim_end()
        .trim_end_matches(';');
    if current.as_deref() == Some(&sql) && current_readiness.as_deref() == Some(expected_readiness)
    {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    tx.execute_batch("CREATE INDEX IF NOT EXISTS worker_retry_reopens ON events(project_id,issue_number,created_at DESC) WHERE action='reopened'")?;
    if distributed {
        tx.execute_batch("CREATE INDEX IF NOT EXISTS distributed_retry_history ON distributed_attempts(project_id,issue_number,finished_at DESC,started_at DESC,id DESC) WHERE finished_at IS NOT NULL")?;
    }
    tx.execute_batch("DROP VIEW IF EXISTS worker_retry_history")?;
    tx.execute_batch(&sql)?;
    tx.execute_batch(&readiness)?;
    tx.commit()?;
    Ok(())
}
