//! Local invalidation counters. SQLite triggers also cover older worker binaries
//! during rolling upgrades; timestamps alone miss late events and retry edits.
use super::Result;
use crate::database::Connection;

pub(crate) fn install(db: &Connection) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS worker_history_versions(worker_id TEXT PRIMARY KEY,version INTEGER NOT NULL) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS worker_history_issue ON worker_runs(project_id,issue_number,worker_id);
        CREATE INDEX IF NOT EXISTS worker_history_actor ON worker_runs(actor_id,worker_id);
        CREATE INDEX IF NOT EXISTS worker_origin_session ON worker_runs(session_id,started_at DESC);")?;
    for (table, scope, changed) in [
        (
            "worker_runs",
            "SELECT worker_id FROM worker_runs WHERE project_id={row}.project_id AND issue_number={row}.issue_number UNION SELECT {row}.worker_id",
            "",
        ),
        (
            "worker_events",
            "SELECT worker_id FROM worker_runs WHERE id={row}.run_id",
            "",
        ),
        (
            "issues",
            "SELECT worker_id FROM worker_runs WHERE project_id={row}.project_id AND issue_number={row}.number",
            "OLD.state IS NOT NEW.state OR OLD.assignee IS NOT NEW.assignee OR OLD.deleted_at IS NOT NEW.deleted_at",
        ),
        (
            "projects",
            "SELECT worker_id FROM worker_runs WHERE project_id={row}.id",
            "OLD.name IS NOT NEW.name",
        ),
        (
            "agents",
            "SELECT worker_id FROM worker_runs WHERE actor_id={row}.id UNION SELECT worker_id FROM worker_runs WHERE session_id=substr({row}.id,7) AND substr({row}.id,1,6)='codex:'",
            "json_extract(OLD.metadata,'$.model') IS NOT json_extract(NEW.metadata,'$.model')",
        ),
    ] {
        for action in ["INSERT", "UPDATE", "DELETE"] {
            let rows: &[&str] = match action {
                "INSERT" => &["NEW"],
                "DELETE" => &["OLD"],
                _ => &["OLD", "NEW"],
            };
            let scope = rows
                .iter()
                .map(|row| scope.replace("{row}", row))
                .collect::<Vec<_>>()
                .join(" UNION ");
            let when = if action == "UPDATE" && !changed.is_empty() {
                format!("WHEN {changed}")
            } else {
                String::new()
            };
            tx.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS worker_history_{table}_{action} AFTER {action} ON {table} {when} BEGIN
                INSERT INTO worker_history_versions(worker_id,version) SELECT worker_id,1 FROM ({scope}) WHERE worker_id IS NOT NULL
                ON CONFLICT(worker_id) DO UPDATE SET version=version+1;
                END;"))?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests;
