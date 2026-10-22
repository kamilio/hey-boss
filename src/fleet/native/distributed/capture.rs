use super::*;

// Deliberate allowlist: credentials, worker control, local paths/configuration,
// request receipts and the derived allocation projection are never shared state.
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
    "dictation_vocabulary",
    "issue_pull_requests",
    "artifacts",
    "artifact_comments",
    "artifact_links",
    "file_attachments",
    "scheduled_jobs",
    "scheduled_job_revisions",
    "scheduled_job_runs",
    "worker_runs",
    "worker_allocations",
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
                "project_settings" => c != "mindmaps_enabled",
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
                        | "retry_at"
                        | "retry_count"
                        | "retry_allowed"
                ),
                "scheduled_jobs" => c != "next_at",
                "scheduled_job_runs" => c != "sequence",
                _ => true,
            })
            .collect(),
    )
}

pub(super) fn upgrade_retry_metadata(db: &Connection) -> Result<()> {
    if db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_capture_worker_runs_UPDATE' AND instr(sql,'''retry_at''')>0)", [], |r| r.get::<_,bool>(0))? {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        tx.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS distributed_capture_worker_runs_{operation}"
        ))?;
    }
    install(&tx)?;
    // Publish current pending retry evidence as fresh facts. A clock-zero
    // baseline cannot replace an already replicated terminal attempt.
    let after = row_json(&columns(&tx, "worker_runs")?, "r");
    let rows = tx.prepare(&format!("SELECT {after} FROM issues i JOIN worker_runs r ON r.id=(SELECT id FROM worker_runs WHERE project_id=i.project_id AND issue_number=i.number AND finished_at IS NOT NULL ORDER BY finished_at DESC,started_at DESC,id DESC LIMIT 1) WHERE i.state='open' AND i.deleted_at IS NULL AND r.retry_allowed=0 AND r.state NOT IN ('completed','cancelled','interrupted')"))?.query_map([], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for row in rows {
        tx.execute("UPDATE distributed_meta SET clock=clock+1 WHERE id=1", [])?;
        tx.execute("INSERT INTO distributed_outbox SELECT clock,'worker_runs',NULL,?1 FROM distributed_meta WHERE id=1", [row])?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn upgrade_ordering(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('distributed_order_rotations') WHERE name='number')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    install(&tx)?;
    tx.commit()?;
    Ok(())
}

pub(super) fn upgrade_worker_execution(db: &Connection) -> Result<()> {
    let current = |db: &Connection| {
        db.query_row("SELECT count(*)=3 FROM sqlite_master WHERE type='trigger' AND name IN ('distributed_capture_project_settings_INSERT','distributed_capture_project_settings_UPDATE','distributed_capture_project_settings_DELETE') AND instr(sql,'''worker_execution''')>0", [], |r| r.get::<_, bool>(0))
    };
    if current(db)? {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    if current(&tx)? {
        tx.commit()?;
        return Ok(());
    }
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        tx.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS distributed_capture_project_settings_{operation}"
        ))?;
    }
    install(&tx)?;
    // Repair choices saved before the trigger upgrade. Publish only this field,
    // preserving concurrent prompt edits and deletion. Unconfigured peers must
    // not publish their migration default over another machine's saved choice.
    let rows = replica::rows(
        &tx,
        "SELECT project_id,worker_execution FROM project_settings s WHERE worker_execution<>'{}' AND NOT EXISTS(SELECT 1 FROM distributed_fields WHERE table_name='project_settings' AND entity=json_object('project_id',s.project_id) AND field='worker_execution')",
        &[],
    )?;
    for row in rows {
        let before = serde_json::json!({"project_id": row["project_id"]});
        tx.execute("UPDATE distributed_meta SET clock=clock+1 WHERE id=1", [])?;
        tx.execute("INSERT INTO distributed_outbox SELECT clock,'project_settings',?1,?2 FROM distributed_meta WHERE id=1", [before.to_string(), row.to_string()])?;
    }
    tx.commit()?;
    Ok(())
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
    ordering::install(db)?;
    // Canonical merges can reopen an archived task without changing its cold
    // content. Only the active projector may update that compact header.
    db.execute_batch("DROP TRIGGER IF EXISTS issue_archive_mutation_guard;
        CREATE TRIGGER issue_archive_mutation_guard BEFORE UPDATE ON issues
        WHEN OLD.archive_key IS NOT NULL AND NEW.archive_key IS OLD.archive_key
        AND (NEW.body IS NOT OLD.body OR NEW.state IS NOT OLD.state OR NEW.deleted_at IS NOT OLD.deleted_at)
        AND NOT ((SELECT active FROM distributed_meta WHERE id=1)=1 AND (SELECT syncing FROM fleet_meta WHERE id=1)=2)
        BEGIN SELECT RAISE(ABORT,'Restore archived issue before changing it'); END;")?;
    db.execute_batch("CREATE TRIGGER IF NOT EXISTS distributed_archive_cooled AFTER UPDATE OF archive_key ON issues
        WHEN NEW.archive_key IS NOT NULL AND NEW.archive_key IS NOT OLD.archive_key
        BEGIN UPDATE distributed_archive_history SET cursor_kind='',cursor_id='',complete=0 WHERE key=NEW.archive_key; END;")?;
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
            let mut before = old.map(|p| row_json(&columns, p)).unwrap_or("NULL".into());
            let mut after = new.map(|p| row_json(&columns, p)).unwrap_or("NULL".into());
            let order_guard = if *table == "issues" && operation == "UPDATE" {
                let unchanged = columns
                    .iter()
                    .filter(|c| !matches!(c.as_str(), "sort_order" | "updated_at" | "version"))
                    .map(|c| format!("OLD.\"{c}\" IS NEW.\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let compact = [
                    "project_id",
                    "number",
                    "sort_order",
                    "updated_at",
                    "version",
                ]
                .map(String::from);
                before = format!(
                    "CASE WHEN {unchanged} THEN {} ELSE {before} END",
                    row_json(&compact, "OLD")
                );
                after = format!(
                    "CASE WHEN {unchanged} THEN {} ELSE {after} END",
                    row_json(&compact, "NEW")
                );
                " AND (SELECT active FROM distributed_order_capture)=0"
            } else {
                ""
            };
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
                WHEN ((SELECT active FROM distributed_meta WHERE id=1)=0 OR (SELECT syncing FROM fleet_meta WHERE id=1)=0){order_guard}{different}{logical_delete}{no_feedback} BEGIN
                UPDATE distributed_meta SET clock=clock+1 WHERE id=1;
                INSERT INTO distributed_outbox SELECT clock,'{table}',{before},{after} FROM distributed_meta WHERE id=1; {observation} END;");
            db.execute_batch(&sql)?;
        }
    }
    Ok(())
}

// Old immutable journals can still contain these records after every app is upgraded.
pub(super) fn retired(table: &str) -> bool {
    matches!(table, "mindmaps" | "mindmap_nodes" | "mindmap_links")
}

pub(super) fn retire_mindmaps(db: &Connection) -> Result<()> {
    let triggers = replica::rows(
        db,
        "SELECT name FROM sqlite_master WHERE type='trigger' AND (name GLOB 'distributed_capture_mindmap*' OR (name GLOB 'distributed_capture_project_settings_*' AND instr(sql,'mindmaps_enabled')>0))",
        &[],
    )?;
    if triggers.is_empty() {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    for trigger in triggers {
        let name = trigger["name"].as_str().unwrap().replace('"', "\"\"");
        tx.execute_batch(&format!("DROP TRIGGER \"{name}\""))?;
    }
    for table in [
        "distributed_outbox",
        "distributed_baseline",
        "distributed_fields",
        "distributed_dirty",
        "distributed_rejections",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE table_name IN ('mindmaps','mindmap_nodes','mindmap_links')"), [])?;
    }
    tx.execute("DELETE FROM distributed_fields WHERE table_name='project_settings' AND field='mindmaps_enabled'", [])?;
    install(&tx)?;
    tx.commit()?;
    Ok(())
}
