//! Admission for notices written by older, still-running scheduling clients.
//!
//! These guards contain only SQLite built-ins, so both local SQL clients and
//! fleet replay obey the current policy without restarting active workers.
use super::Result;
use crate::database::Connection;

const PREFIX: &str = "Dependency rework: upstream tasks ";
const SUFFIX: &str = " need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies.";
pub(super) const REJECTION: &str = "This dependency notice used obsolete sibling scheduling. The current declared dependencies remain in effect; your running claim is unchanged.";

// Only recognize the exact generated format. Authored prose and malformed
// lookalikes remain ordinary comments, never SQL/JSON errors.
fn numbers(text: &str) -> String {
    let list = format!(
        "substr({text},{},length({text})-{})",
        PREFIX.len() + 1,
        PREFIX.len() + SUFFIX.len()
    );
    format!(
        "CASE WHEN substr({text},1,{})='{PREFIX}' AND substr({text},-{})='{SUFFIX}' AND json_valid({list}) THEN CASE WHEN json_type({list})='array' THEN {list} ELSE '[]' END ELSE '[]' END",
        PREFIX.len(),
        SUFFIX.len()
    )
}

// A policy guard, not a second readiness evaluator: declared dependencies and
// descendant completion remain valid even after their state changes. Only
// implicit sibling relationships disappear in explicit mode. Each traversal
// starts at one issue and uses indexed parent/child keys, never the whole fleet.
fn obsolete(project: &str, number: &str, list: &str, entry: &str) -> String {
    format!("EXISTS(SELECT 1 FROM project_settings s JOIN issues i ON i.project_id=s.project_id
        WHERE s.project_id={project} AND s.subtask_scheduling='explicit' AND i.number={number}
        AND EXISTS(WITH RECURSIVE descendants(number) AS (
            SELECT r.child_number FROM issue_subtasks r JOIN issues c ON c.project_id=r.project_id AND c.number=r.child_number
            WHERE r.project_id=i.project_id AND r.parent_number=i.number AND c.deleted_at IS NULL
            UNION
            SELECT r.child_number FROM issue_subtasks r JOIN descendants d ON d.number=r.parent_number
            JOIN issues c ON c.project_id=r.project_id AND c.number=r.child_number
            WHERE r.project_id=i.project_id AND c.deleted_at IS NULL
        ) SELECT 1 FROM json_each({list}) dependency
        WHERE NOT EXISTS(SELECT 1 FROM json_each(i.blockers) link WHERE link.value={entry})
        AND NOT EXISTS(SELECT 1 FROM descendants d WHERE d.number={entry})))")
}

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row("SELECT count(*)=5 FROM sqlite_master WHERE type='trigger' AND name IN ('dependency_notice_comment','dependency_notice_event','dependency_notice_steering','dependency_notice_mode','dependency_notice_delivery')", [], |r|r.get::<_,bool>(0))? {
        return Ok(());
    }
    let tx =
        crate::database::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    let comment = obsolete(
        "NEW.project_id",
        "NEW.issue_number",
        &numbers("NEW.body"),
        "dependency.value",
    );
    let commented = obsolete(
        "NEW.project_id",
        "NEW.issue_number",
        &numbers("json_extract(NEW.data,'$.body')"),
        "dependency.value",
    );
    let event = obsolete(
        "NEW.project_id",
        "NEW.issue_number",
        "coalesce(json_extract(NEW.data,'$.dependencies'),'[]')",
        "json_extract(dependency.value,'$[0]')",
    );
    let queued = obsolete(
        "r.project_id",
        "r.issue_number",
        &numbers("q.text"),
        "dependency.value",
    );
    let incoming = obsolete(
        "r.project_id",
        "r.issue_number",
        &numbers("NEW.text"),
        "dependency.value",
    );
    // Suppression must cover the entire legacy write sequence, including its
    // stale last_insert_rowid after an ignored comment. It cannot leave a fake
    // commented event, deduplication signature, or queued instruction behind.
    tx.execute_batch(&format!("
        CREATE TRIGGER IF NOT EXISTS dependency_notice_comment BEFORE INSERT ON comments
        WHEN {comment} BEGIN SELECT RAISE(IGNORE); END;
        CREATE TRIGGER IF NOT EXISTS dependency_notice_event BEFORE INSERT ON events
        WHEN CASE WHEN json_valid(NEW.data) THEN CASE NEW.action WHEN 'commented' THEN {commented} WHEN 'dependency_rework' THEN {event} ELSE 0 END ELSE 0 END
        BEGIN SELECT RAISE(IGNORE); END;
        CREATE VIEW IF NOT EXISTS obsolete_dependency_steering AS
        SELECT q.request_id FROM agent_steering q JOIN worker_runs r ON r.id=q.run_id
        WHERE q.state='queued' AND q.scope='dependency' AND {queued};
        CREATE TRIGGER IF NOT EXISTS dependency_notice_steering AFTER INSERT ON agent_steering
        WHEN NEW.state='queued' AND NEW.scope='dependency'
        BEGIN UPDATE agent_steering SET state='rejected',error='{REJECTION}'
        WHERE request_id=NEW.request_id AND request_id IN (SELECT request_id FROM obsolete_dependency_steering); END;
        CREATE TRIGGER IF NOT EXISTS dependency_notice_mode AFTER UPDATE OF subtask_scheduling ON project_settings
        WHEN NEW.subtask_scheduling='explicit' AND OLD.subtask_scheduling<>NEW.subtask_scheduling
        BEGIN UPDATE agent_steering SET state='rejected',error='{REJECTION}'
        WHERE request_id IN (SELECT q.request_id FROM agent_steering q JOIN worker_runs r ON r.id=q.run_id
        WHERE r.project_id=NEW.project_id AND q.state='queued' AND q.scope='dependency' AND {queued}); END;
        CREATE TRIGGER IF NOT EXISTS dependency_notice_delivery BEFORE UPDATE OF state ON agent_steering
        WHEN OLD.state='queued' AND NEW.state='sending' AND NEW.scope='dependency'
        AND EXISTS(SELECT 1 FROM worker_runs r WHERE r.id=NEW.run_id AND {incoming})
        BEGIN UPDATE agent_steering SET state='rejected',error='{REJECTION}' WHERE request_id=OLD.request_id;
        SELECT RAISE(IGNORE); END;
        UPDATE agent_steering SET state='rejected',error='{REJECTION}'
        WHERE request_id IN (SELECT request_id FROM obsolete_dependency_steering);
    "))?;
    tx.commit()?;
    Ok(())
}
