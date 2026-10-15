use super::*;

// Deliberate allowlist: credentials, worker control, local paths/configuration,
// request receipts and the old authority's allocations are never shared state.
pub(super) const TABLES: &[&str] = &[
    "projects",
    "agents",
    "issues",
    "issue_status_updates",
    "github_fetch_status",
    "issue_github_watches",
    "issue_agent_launches",
    "issue_subtasks",
    "comments",
    "events",
    "issue_commits",
    "project_settings",
    "global_settings",
    "issue_pull_requests",
    "artifacts",
    "artifact_comments",
    "artifact_links",
    "file_attachments",
    "mindmaps",
    "mindmap_nodes",
    "mindmap_links",
    "scheduled_jobs",
    "scheduled_job_revisions",
    "scheduled_job_runs",
    "worker_runs",
    "distributed_members",
];

pub(super) fn columns(db: &Connection, table: &str) -> Result<Vec<String>> {
    if !TABLES.contains(&table) {
        return Err(replica::invalid("Unknown distributed table"));
    }
    Ok(
        replica::rows(db, &format!("PRAGMA table_info({table})"), &[])?
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_owned())
            .filter(|c| match table {
                "projects" => !matches!(c.as_str(), "next_number"),
                "issues" => !super::super::archive::local_column(c) && c != "attempt_hold",
                "worker_runs" => matches!(
                    c.as_str(),
                    "id" | "project_id"
                        | "issue_number"
                        | "actor_id"
                        | "session_id"
                        | "state"
                        | "machine"
                        | "started_at"
                        | "finished_at"
                        | "summary"
                        | "worker_id"
                ),
                "scheduled_jobs" => c != "next_at",
                "scheduled_job_runs" => c != "sequence",
                _ => true,
            })
            .collect(),
    )
}

fn row_json(columns: &[String], prefix: &str) -> String {
    format!(
        "json_object({})",
        columns
            .iter()
            .map(|c| format!("'{c}',{prefix}.\"{c}\""))
            .collect::<Vec<_>>()
            .join(",")
    )
}

pub(super) fn install(db: &Connection) -> Result<()> {
    for table in TABLES {
        let columns = columns(db, table)?;
        if columns.is_empty() {
            return Err(replica::invalid(&format!(
                "Missing distributed table {table}"
            )));
        }
        for (operation, old, new) in [
            ("INSERT", None, Some("NEW")),
            ("UPDATE", Some("OLD"), Some("NEW")),
            ("DELETE", Some("OLD"), None),
        ] {
            let before = old.map(|p| row_json(&columns, p)).unwrap_or("NULL".into());
            let after = new.map(|p| row_json(&columns, p)).unwrap_or("NULL".into());
            let different = if operation == "UPDATE" {
                format!(
                    " AND NOT ({})",
                    columns
                        .iter()
                        .map(|c| format!("OLD.\"{c}\" IS NEW.\"{c}\""))
                        .collect::<Vec<_>>()
                        .join(" AND ")
                )
            } else {
                String::new()
            };
            let logical_delete = if operation == "DELETE"
                && matches!(*table, "comments" | "events" | "issue_status_updates")
            {
                " AND NOT EXISTS(SELECT 1 FROM issues WHERE project_id=OLD.project_id AND number=OLD.issue_number AND archive_key IS NOT NULL AND archive_cleanup=1)"
            } else {
                ""
            };
            let no_feedback = if *table == "distributed_members" {
                " AND (SELECT syncing FROM fleet_meta WHERE id=1)=0"
            } else {
                ""
            };
            // Own job history is immediately visible, before any sync tick.
            // Remote observations never enter the executable run table.
            let observation = if *table == "scheduled_job_runs" && operation != "DELETE" {
                format!(
                    "INSERT INTO distributed_job_attempts(id,project_id,job_id,created_at,scheduled_at,state,payload) VALUES(NEW.id,NEW.project_id,NEW.job_id,NEW.created_at,NEW.scheduled_at,NEW.state,{after}) ON CONFLICT(id) DO UPDATE SET state=excluded.state,payload=excluded.payload;"
                )
            } else {
                String::new()
            };
            // Fleet imports are captured during shadowing. After activation,
            // only the distributed projector sets the existing syncing guard.
            let sql = format!("CREATE TRIGGER IF NOT EXISTS distributed_capture_{table}_{operation} AFTER {operation} ON {table}
                WHEN ((SELECT active FROM distributed_meta WHERE id=1)=0 OR (SELECT syncing FROM fleet_meta WHERE id=1)=0){different}{logical_delete}{no_feedback} BEGIN
                UPDATE distributed_meta SET clock=clock+1 WHERE id=1;
                INSERT INTO distributed_outbox SELECT clock,'{table}',{before},{after} FROM distributed_meta WHERE id=1; {observation} END;");
            db.execute_batch(&sql)?;
        }
    }
    Ok(())
}
