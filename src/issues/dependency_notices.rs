//! Admission for notices written by older, still-running scheduling clients.
//!
//! These guards contain only SQLite built-ins, so both local SQL clients and
//! fleet replay obey the current policy without restarting active workers.
use super::Result;
use crate::database::Connection;

// A local watermark rearms a notice after its effective blockers change. History
// remains immutable and canonical fleet history is still replayed in full.
pub(super) const REWORK_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS dependency_notice_resets(
    project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,event_id INTEGER NOT NULL,
    PRIMARY KEY(project_id,issue_number));
    CREATE INDEX IF NOT EXISTS issue_dependency_rework ON events(project_id,issue_number,id DESC)
    WHERE action='dependency_rework';
    CREATE INDEX IF NOT EXISTS issue_dependency_comments ON comments(project_id,issue_number,id DESC)
    WHERE body GLOB 'Dependency rework: upstream tasks *';
    CREATE TABLE IF NOT EXISTS dependency_comment_resets(
    project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,comment_id INTEGER NOT NULL,
    PRIMARY KEY(project_id,issue_number));";

const PREFIX: &str = "Dependency rework: upstream tasks ";
const SUFFIX: &str = " need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies.";
pub(super) const REJECTION: &str = "This dependency notice used obsolete sibling scheduling. The current declared dependencies remain in effect; your running claim is unchanged.";

// Only recognize the exact generated format. Authored prose and malformed
// lookalikes remain ordinary comments, never SQL/JSON errors.
pub(super) fn numbers(text: &str) -> String {
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
// implicit sibling relationships are obsolete. Each traversal
// starts at one issue and uses indexed parent/child keys, never the whole fleet.
fn obsolete(project: &str, number: &str, list: &str, entry: &str) -> String {
    format!("EXISTS(SELECT 1 FROM issues i
        WHERE i.project_id={project} AND i.number={number}
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

// Ignore model metadata, upstream versions, list ordering and caller identity.
fn canonical(list: &str, entry: &str) -> String {
    format!(
        "(SELECT json_group_array(number) FROM (SELECT DISTINCT {entry} AS number FROM json_each({list}) dependency ORDER BY number))"
    )
}

pub(super) fn latest_comment(project: &str, number: &str) -> String {
    let list = numbers("body");
    format!("SELECT id FROM comments WHERE project_id={project} AND issue_number={number}
        AND body GLOB 'Dependency rework: upstream tasks *' AND json_array_length({list})>0 ORDER BY id DESC LIMIT 1")
}

fn duplicate_comment() -> String {
    let latest = latest_comment("NEW.project_id", "NEW.issue_number");
    let incoming = canonical(&numbers("NEW.body"), "dependency.value");
    let saved = canonical(&numbers("c.body"), "dependency.value");
    format!("EXISTS(SELECT 1 FROM comments c WHERE c.id=({latest})
        AND c.id>coalesce((SELECT comment_id FROM dependency_comment_resets WHERE project_id=NEW.project_id AND issue_number=NEW.issue_number),0)
        AND {incoming}<>'[]' AND {incoming}={saved})")
}

fn duplicate(list: &str, entry: &str) -> String {
    let incoming = canonical(list, entry);
    let saved = canonical(
        "CASE WHEN json_valid(e.data) THEN coalesce(json_extract(e.data,'$.dependencies'),'[]') ELSE '[]' END",
        "json_extract(dependency.value,'$[0]')",
    );
    format!("EXISTS(SELECT 1 FROM events e WHERE e.id=(SELECT id FROM events
        WHERE project_id=NEW.project_id AND issue_number=NEW.issue_number AND action='dependency_rework' ORDER BY id DESC LIMIT 1)
        AND e.id>coalesce((SELECT event_id FROM dependency_notice_resets WHERE project_id=NEW.project_id AND issue_number=NEW.issue_number),0)
        AND {incoming}<>'[]' AND {incoming}={saved})")
}

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row("SELECT count(*)=5 AND NOT EXISTS(SELECT 1 FROM sqlite_master WHERE name='dependency_notice_mode' OR (name='dependency_notice_comment' AND instr(sql,'comment_id FROM dependency_comment_resets')=0)) FROM sqlite_master WHERE type='trigger' AND name IN ('dependency_notice_comment','dependency_notice_event','dependency_notice_steering','dependency_notice_delivery','dependency_notice_state')", [], |r|r.get::<_,bool>(0))? {
        return Ok(());
    }
    let tx =
        crate::database::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    let duplicate_comment = duplicate_comment();
    let duplicate_commented = duplicate(
        &numbers("json_extract(NEW.data,'$.body')"),
        "dependency.value",
    );
    let duplicate_event = duplicate(
        "coalesce(json_extract(NEW.data,'$.dependencies'),'[]')",
        "json_extract(dependency.value,'$[0]')",
    );
    let commented_list = numbers("json_extract(NEW.data,'$.body')");
    let invalid_comment = format!("json_array_length({commented_list})>0 AND (
        NOT EXISTS(SELECT 1 FROM comments c WHERE c.id=json_extract(NEW.data,'$.comment_id')
            AND c.project_id=NEW.project_id AND c.issue_number=NEW.issue_number
            AND c.author=NEW.actor AND c.created_at=NEW.created_at AND c.body=json_extract(NEW.data,'$.body'))
        OR EXISTS(SELECT 1 FROM events e WHERE e.project_id=NEW.project_id AND e.issue_number=NEW.issue_number
            AND e.action='commented' AND json_extract(e.data,'$.comment_id')=json_extract(NEW.data,'$.comment_id')))");
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
    let blocked = obsolete(
        "NEW.project_id",
        "NEW.issue_number",
        "coalesce(json_extract(NEW.data,'$.blocked_by'),'[]')",
        "dependency.value",
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
        DROP TRIGGER IF EXISTS dependency_notice_comment;
        DROP TRIGGER IF EXISTS dependency_notice_state;
        DROP TRIGGER IF EXISTS dependency_notice_steering;
        DROP TRIGGER IF EXISTS dependency_notice_mode;
        DROP TRIGGER IF EXISTS dependency_notice_delivery;
        DROP VIEW IF EXISTS obsolete_dependency_steering;
        CREATE TRIGGER dependency_notice_comment BEFORE INSERT ON comments
        WHEN (SELECT syncing FROM fleet_meta WHERE id=1)<>2 AND ({comment} OR {duplicate_comment}) BEGIN SELECT RAISE(IGNORE); END;
        DROP TRIGGER IF EXISTS dependency_notice_event;
        CREATE TRIGGER dependency_notice_event BEFORE INSERT ON events
        WHEN (SELECT syncing FROM fleet_meta WHERE id=1)<>2 AND CASE WHEN json_valid(NEW.data) THEN CASE NEW.action WHEN 'commented' THEN ({commented} OR {duplicate_commented} OR ({invalid_comment})) WHEN 'dependency_rework' THEN ({event} OR {duplicate_event}) WHEN 'blocked' THEN {blocked} ELSE 0 END ELSE 0 END
        BEGIN SELECT RAISE(IGNORE); END;
        -- Old reconcilers continue after ignored notices. Reject an automatic
        -- transition with no unfinished declared prerequisite or descendant.
        -- Descendants traverse closed parents, like Graph::descendants; Ready
        -- dependencies are satisfied only while their own prerequisites are.
        CREATE TRIGGER IF NOT EXISTS dependency_notice_state BEFORE UPDATE OF state ON issues
        WHEN NEW.state='blocked' AND OLD.state<>'blocked' AND NEW.manual_blocked=0
        AND NOT EXISTS(
            WITH RECURSIVE dependencies(number,descend) AS (
                SELECT value,0 FROM json_each(NEW.blockers)
                UNION
                SELECT r.child_number,1 FROM issue_subtasks r JOIN issues c ON c.project_id=r.project_id AND c.number=r.child_number
                WHERE r.project_id=NEW.project_id AND r.parent_number=NEW.number AND c.deleted_at IS NULL
                UNION
                SELECT r.child_number,1 FROM dependencies d JOIN issues i ON i.project_id=NEW.project_id AND i.number=d.number
                JOIN issue_subtasks r ON r.project_id=i.project_id AND r.parent_number=i.number
                JOIN issues c ON c.project_id=r.project_id AND c.number=r.child_number
                WHERE i.deleted_at IS NULL AND c.deleted_at IS NULL AND (d.descend=1 OR i.state='ready')
                UNION
                SELECT link.value,0 FROM dependencies d JOIN issues i ON i.project_id=NEW.project_id AND i.number=d.number, json_each(i.blockers) link
                WHERE i.deleted_at IS NULL AND i.state='ready'
            ) SELECT 1 FROM dependencies d LEFT JOIN issues i ON i.project_id=NEW.project_id AND i.number=d.number
            WHERE i.number IS NULL OR (i.deleted_at IS NULL AND i.state<>'closed' AND
                i.state<>'ready')
        ) BEGIN SELECT RAISE(IGNORE); END;
        CREATE VIEW IF NOT EXISTS obsolete_dependency_steering AS
        SELECT q.request_id FROM agent_steering q JOIN worker_runs r ON r.id=q.run_id
        WHERE q.state='queued' AND q.scope='dependency' AND {queued};
        CREATE TRIGGER IF NOT EXISTS dependency_notice_steering AFTER INSERT ON agent_steering
        WHEN NEW.state='queued' AND NEW.scope='dependency'
        BEGIN UPDATE agent_steering SET state='rejected',error='{REJECTION}'
        WHERE request_id=NEW.request_id AND request_id IN (SELECT request_id FROM obsolete_dependency_steering); END;
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
