use super::{Actor, BODY_LIMIT, Error, Operation, Project, Request, Result, identifier};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[path = "agent_launches.rs"]
mod agent_launches;
#[path = "chief.rs"]
pub(super) mod chief;
#[path = "claim_recovery.rs"]
mod claim_recovery;
#[path = "../mindmap/store.rs"]
mod mindmap;
#[path = "origin_reader.rs"]
mod origin_reader;
#[path = "worker_registry.rs"]
mod registry;
#[path = "steering.rs"]
mod steering;
#[path = "subtasks.rs"]
mod subtasks;
#[path = "worker_store.rs"]
mod workers;

#[path = "artifacts.rs"]
mod artifacts;
#[path = "batch.rs"]
mod batch;
#[path = "project_names.rs"]
mod project_names;
#[path = "status.rs"]
mod status;
#[path = "transfer.rs"]
mod transfer;
use super::provenance;

const APPLICATION_ID: i64 = 0x48424953;
const SCHEMA_VERSION: i64 = 13;
// These additive migrations shipped independently. Verify the actual columns,
// not just user_version, so a partial upgrade can be repaired without data loss.
const ADDITIVE_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "issue_pull_requests",
        "purpose",
        "TEXT NOT NULL DEFAULT 'unspecified' CHECK(purpose IN ('unspecified','fix','prerequisite','supporting-evidence'))",
    ),
    (
        "project_settings",
        "chief_enabled",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    ("project_settings", "chief_prompt", "TEXT"),
    ("issues", "draft", "INTEGER NOT NULL DEFAULT 0"),
    ("issues", "plan", "TEXT"),
    (
        "project_settings",
        "drafts_enabled",
        "INTEGER NOT NULL DEFAULT 1",
    ),
    (
        "project_settings",
        "plan_template",
        "TEXT NOT NULL DEFAULT 'plans/{timestamp}-{number}.md'",
    ),
    (
        "project_settings",
        "worktree_enabled",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    (
        "project_settings",
        "prompt_overrides",
        "TEXT NOT NULL DEFAULT '{}'",
    ),
    ("mindmap_nodes", "display_label", "TEXT"),
];

fn missing_additive_columns(
    db: &Connection,
) -> Result<Vec<(&'static str, &'static str, &'static str)>> {
    let mut missing = Vec::new();
    for table in [
        "issues",
        "project_settings",
        "mindmap_nodes",
        "issue_pull_requests",
    ] {
        let mut statement = db.prepare("SELECT name FROM pragma_table_info(?1)")?;
        let columns = statement
            .query_map([table], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        for &(target, column, definition) in ADDITIVE_COLUMNS {
            if target == table && !columns.contains(column) {
                missing.push((target, column, definition));
            }
        }
    }
    Ok(missing)
}

fn migration_error(error: Error, path: &Path) -> Error {
    if error.code == "invalid_input" {
        return error;
    }
    Error::new(
        "migration_error",
        format!(
            "Could not migrate issue database {}: {}. No migration changes were committed; resolve the cause and retry with this hey-boss build. Preserve the store; do not delete or recreate it.",
            path.display(),
            error.message
        ),
    )
}

// Rebuild only the constrained table, retaining every column, index and fleet
// journal trigger. Foreign keys are disabled outside this atomic transaction;
// copying rows must not emit changes or rewrite child references.
fn migrate_blocked(db: &Connection) -> Result<()> {
    let sql: String = db.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='issues'",
        [],
        |r| r.get(0),
    )?;
    let objects = {
        let mut query = db.prepare("SELECT sql FROM sqlite_master WHERE tbl_name='issues' AND type IN ('index','trigger') AND sql IS NOT NULL")?;
        query
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    db.execute_batch(
        "CREATE TEMP TABLE blocked_migration AS SELECT * FROM issues; DROP TABLE issues;",
    )?;
    db.execute_batch(&sql.replace("'open','closed'", "'open','blocked','closed'"))?;
    db.execute_batch(
        "INSERT INTO issues SELECT * FROM blocked_migration; DROP TABLE blocked_migration;",
    )?;
    for sql in objects {
        db.execute_batch(&sql)?;
    }
    let violation: Option<String> = db
        .query_row(
            "SELECT \"table\" FROM pragma_foreign_key_check LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(table) = violation {
        return Err(Error::invalid(format!(
            "Foreign key violation in {table}; migration rolled back"
        )));
    }
    Ok(())
}
const PAGE_BYTES: usize = 16 * 1024 * 1024;

fn comment_page(
    db: &Connection,
    project: &Project,
    number: i64,
    limit: u32,
    offset: u32,
    sort: super::CommentSort,
    mut bytes: usize,
) -> Result<Value> {
    let total: i64 = db.query_row(
        "SELECT count(*) FROM comments WHERE project_id=?1 AND issue_number=?2",
        params![project.id, number],
        |r| r.get(0),
    )?;
    let order = match sort {
        super::CommentSort::Newest => "DESC",
        super::CommentSort::Oldest => "ASC",
    };
    // Read bodies one at a time so a page of large comments stays bounded.
    let mut stmt = db.prepare(&format!("SELECT id,author,body,created_at FROM comments WHERE project_id=?1 AND issue_number=?2 ORDER BY id {order} LIMIT ?3 OFFSET ?4"))?;
    let mut rows = stmt.query(params![project.id, number, limit, offset])?;
    let mut comments = Vec::new();
    while let Some(row) = rows.next()? {
        let comment = json!({"id":row.get::<_,i64>(0)?,"author":row.get::<_,String>(1)?,"body":row.get::<_,String>(2)?,"created_at":row.get::<_,i64>(3)?});
        bytes += serde_json::to_vec(&comment)?.len();
        if bytes > PAGE_BYTES && !comments.is_empty() {
            break;
        }
        comments.push(comment);
    }
    // Only scan resolution history when this page actually contains comments.
    if !comments.is_empty() {
        let visible: std::collections::HashSet<i64> =
            comments.iter().map(|c| c["id"].as_i64().unwrap()).collect();
        let mut stmt = db.prepare("SELECT json_extract(data,'$.comment_id'),action FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('comment_resolved','comment_unresolved') ORDER BY created_at DESC,id DESC")?;
        let mut states = std::collections::HashMap::new();
        let mut rows = stmt.query(params![project.id, number])?;
        while let Some(row) = rows.next()? {
            let id = row.get::<_, i64>(0)?;
            if !visible.contains(&id) {
                continue;
            }
            states
                .entry(id)
                .or_insert(row.get::<_, String>(1)? == "comment_resolved");
            if states.len() == visible.len() {
                break;
            }
        }
        for comment in &mut comments {
            comment["resolved"] = json!(
                states
                    .get(&comment["id"].as_i64().unwrap())
                    .copied()
                    .unwrap_or(false)
            );
        }
    }
    let next = u64::from(offset) + comments.len() as u64;
    Ok(
        json!({"ok":true,"project":project,"number":number,"comments":comments,"comment_count":total,"sort":sort,"next_offset":if next < total as u64 { Some(next) } else { None }}),
    )
}
const COLUMNS: &str = "number,title,body,state,assignee,created_by,closed_by,created_at,updated_at,closed_at,deleted_at,version,labels,sort_order,draft,plan,(SELECT count(*) FROM issue_agent_launches launches WHERE launches.project_id=issues.project_id AND launches.issue_number=issues.number) AS agent_launch_count,(SELECT json_object('id',id,'author',author,'level',level,'comment',comment,'created_at',created_at) FROM issue_status_updates s WHERE s.project_id=issues.project_id AND s.issue_number=issues.number ORDER BY created_at DESC,id DESC LIMIT 1) AS status,origin,manual_blocked,blockers";

#[derive(Debug, Serialize, Deserialize)]
struct Issue {
    number: i64,
    title: String,
    body: String,
    state: String,
    assignee: Option<String>,
    created_by: String,
    closed_by: Option<String>,
    created_at: i64,
    updated_at: i64,
    closed_at: Option<i64>,
    deleted_at: Option<i64>,
    version: i64,
    labels: Vec<String>,
    sort_order: i64,
    draft: bool,
    plan: Option<super::planning::Plan>,
    #[serde(default)]
    agent_launch_count: i64,
    #[serde(default)]
    status: Option<Value>,
    #[serde(default, flatten)]
    creation_context: origin_reader::Metadata,
    #[serde(default)]
    manual_blocked: bool,
    #[serde(default)]
    blocker_numbers: Vec<i64>,
}
fn row_issue(row: &rusqlite::Row<'_>) -> rusqlite::Result<Issue> {
    let labels: String = row.get(12)?;
    Ok(Issue {
        manual_blocked: row.get("manual_blocked")?,
        blocker_numbers: serde_json::from_str(&row.get::<_, String>("blockers")?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(20, rusqlite::types::Type::Text, Box::new(e))
        })?,
        creation_context: origin_reader::Metadata::read(row)?,
        agent_launch_count: row.get(16)?,
        status: row
            .get::<_, Option<String>>(17)?
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    17,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        sort_order: row.get(13)?,
        draft: row.get(14)?,
        plan: row
            .get::<_, Option<String>>(15)?
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    15,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        number: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        state: row.get(3)?,
        assignee: row.get(4)?,
        created_by: row.get(5)?,
        closed_by: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        closed_at: row.get(9)?,
        deleted_at: row.get(10)?,
        version: row.get(11)?,
        labels: serde_json::from_str(&labels).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(12, rusqlite::types::Type::Text, Box::new(e))
        })?,
    })
}
fn get_issue(db: &Connection, project: &str, number: i64, deleted: bool) -> Result<Issue> {
    let issue = db
        .query_row(
            &format!("SELECT {COLUMNS} FROM issues WHERE project_id=?1 AND number=?2"),
            params![project, number],
            row_issue,
        )
        .optional()?;
    issue
        .filter(|i| deleted || i.deleted_at.is_none())
        .ok_or_else(|| {
            Error::new(
                "not_found",
                format!("Issue #{number} was not found in {project}"),
            )
        })
}
fn body(value: &str, nonempty: bool) -> Result<()> {
    if value.len() > BODY_LIMIT || (nonempty && value.trim().is_empty()) {
        return Err(Error::invalid(
            "Markdown must be UTF-8 text up to 1 MiB; comments must not be blank",
        ));
    }
    Ok(())
}
fn labels(values: &[String]) -> Result<()> {
    if values.len() > 50 {
        return Err(Error::invalid("Use at most 50 labels"));
    }
    for value in values {
        identifier(value, "label", 64)?;
    }
    Ok(())
}
fn page(limit: u32) -> Result<()> {
    if !(1..=100).contains(&limit) {
        return Err(Error::invalid("--limit must be between 1 and 100"));
    }
    Ok(())
}
fn validate(r: &Request) -> Result<()> {
    if r.version != 1 {
        return Err(Error::invalid(
            "Unsupported issue protocol version; upgrade hey-boss on both machines",
        ));
    }
    identifier(&r.project.id, "project ID", 8192)?;
    identifier(&r.project.name, "project name", 1024)?;
    if let Some(value) = &r.project_override {
        identifier(value, "project", 8192)?;
    }
    if let Some(actor) = &r.actor {
        if serde_json::to_vec(actor)?.len() > 32 * 1024 {
            return Err(Error::invalid("Agent metadata exceeds 32 KiB"));
        }
        identifier(&actor.id, "agent ID", 512)?;
        identifier(&actor.machine, "machine ID", 256)?;
        identifier(&actor.host, "host", 256)?;
        identifier(&actor.kind, "agent kind", 64)?;
        if actor.pid.is_some_and(|p| p < 2 || p > i32::MAX as u32) {
            return Err(Error::invalid("Invalid agent PID"));
        }
        if let Some(id) = &actor.session_id {
            identifier(id, "session ID", 256)?;
        }
        if let Some(id) = actor.invocation.as_ref().and_then(|i| i.call_id.as_ref()) {
            identifier(id, "invocation ID", 256)?;
        }
        if let Some(run) = &actor.creation_run {
            identifier(&run.id, "origin run ID", 256)?;
            identifier(&run.project_id, "origin project ID", 8192)?;
            if run.number <= 0 || actor.session_id.is_none() {
                return Err(Error::invalid("Invalid creation run"));
            }
        }
    }
    if r.operation.needs_actor() && r.actor.is_none() {
        return Err(Error::new(
            "identity_unavailable",
            "This operation requires an agent identity",
        ));
    }
    if let Some(key) = &r.request_id {
        identifier(key, "request ID", 256)?;
        if !r.operation.writes() {
            return Err(Error::invalid("--request-id applies only to mutations"));
        }
    }
    if r.operation.number().is_some_and(|n| n <= 0) {
        return Err(Error::invalid("Issue number must be positive"));
    }
    match &r.operation {
        Operation::Batch { edits, dry_run } => {
            batch::validate(edits)?;
            if !*dry_run && r.request_id.is_none() {
                return Err(Error::invalid(
                    "issue batch requires --request-id unless --dry-run",
                ));
            }
        }
        Operation::ResolveComment { comment_id, .. } if *comment_id <= 0 => {
            return Err(Error::invalid("Comment ID must be positive"));
        }
        Operation::Attachment { operation } => operation.validate()?,
        Operation::Artifact { operation } => operation.validate()?,
        Operation::Mindmap { operation } => {
            operation.validate()?;
            if matches!(
                operation,
                crate::mindmap::Operation::Batch { dry_run: true, .. }
            ) && r.request_id.is_some()
            {
                return Err(Error::invalid("--request-id cannot be used with --dry-run"));
            }
        }
        Operation::Create {
            title,
            body: text,
            labels: values,
            ..
        }
        | Operation::CreateSubtask {
            title,
            body: text,
            labels: values,
            ..
        } => {
            identifier(title, "title", 512)?;
            body(text, false)?;
            labels(values)?;
        }
        Operation::Edit {
            title,
            body: text,
            add_labels,
            remove_labels,
            if_version,
            ..
        } => {
            if matches!(&r.operation, Operation::Edit { draft: None, .. })
                && title.is_none()
                && text.is_none()
                && add_labels.is_empty()
                && remove_labels.is_empty()
            {
                return Err(Error::invalid(
                    "edit requires --title, --body, --file, --label, or --remove-label",
                ));
            }
            if let Some(title) = title {
                identifier(title, "title", 512)?;
            }
            if let Some(text) = text {
                body(text, false)?;
            }
            labels(add_labels)?;
            labels(remove_labels)?;
            if if_version.is_some_and(|v| v < 1) {
                return Err(Error::invalid("--if-version must be positive"));
            }
            if add_labels.iter().any(|l| remove_labels.contains(l)) {
                return Err(Error::invalid("Cannot add and remove the same label"));
            }
        }
        Operation::Move {
            number,
            before,
            after,
            if_order_version,
        } => {
            if before.is_some() && after.is_some()
                || before.is_some_and(|n| n < 1 || n == *number)
                || after.is_some_and(|n| n < 1 || n == *number)
                || if_order_version.is_some_and(|v| v < 0)
            {
                return Err(Error::invalid(
                    "Use one different positive issue as --before or --after; order version must be nonnegative",
                ));
            }
        }
        Operation::Comment { body: text, .. } => body(text, true)?,
        Operation::Block {
            comment: Some(text),
            ..
        }
        | Operation::Close {
            comment: Some(text),
            ..
        } => body(text, true)?,
        Operation::List {
            state,
            mine,
            unassigned,
            assignee,
            labels: values,
            limit,
            search,
            ..
        } => {
            if !["open", "blocked", "closed", "all", "deleted"].contains(&state.as_str()) {
                return Err(Error::invalid(
                    "State must be open, blocked, closed, all, or deleted",
                ));
            }
            if assignee.is_some() && (*mine || *unassigned) {
                return Err(Error::invalid(
                    "--assignee conflicts with --mine and --unassigned",
                ));
            }
            if let Some(id) = assignee {
                identifier(id, "assignee", 512)?;
            }
            if *mine && *unassigned {
                return Err(Error::invalid("--mine conflicts with --unassigned"));
            }
            labels(values)?;
            page(*limit)?;
            if let Some(text) = search {
                identifier(text, "search", 1024)?;
            }
        }
        Operation::Status { comment, .. } => status::validate(comment)?,
        Operation::History { limit, .. }
        | Operation::StatusHistory { limit, .. }
        | Operation::Comments { limit, .. } => page(*limit)?,
        _ => {}
    }
    match &r.operation {
        Operation::CreateSubtask { if_version, .. } => {
            if if_version.is_some_and(|v| v < 1) {
                return Err(Error::invalid("--if-version must be positive"));
            }
        }
        Operation::AddSubtask {
            number,
            child,
            if_version,
            if_child_version,
        }
        | Operation::RemoveSubtask {
            number,
            child,
            if_version,
            if_child_version,
        } => {
            if *child < 1 || child == number {
                return Err(Error::invalid(
                    "Use a different positive issue number as the subtask",
                ));
            }
            if if_version.is_some_and(|v| v < 1) || if_child_version.is_some_and(|v| v < 1) {
                return Err(Error::invalid("Issue versions must be positive"));
            }
        }
        _ => {}
    }
    Ok(())
}

fn resolve_project(
    db: &Connection,
    detected: &Project,
    override_id: Option<&str>,
) -> Result<Project> {
    let Some(value) = override_id else {
        if let Some(existing) = db
            .query_row(
                "SELECT id,name FROM projects WHERE id=?1",
                [&detected.id],
                |r| {
                    Ok(Project {
                        id: r.get(0)?,
                        name: r.get(1)?,
                    })
                },
            )
            .optional()?
        {
            return project_names::canonical(db, existing);
        }
        return project_names::canonical(db, detected.clone());
    };
    if let Some(project) = project_names::by_name(db, value)? {
        return Ok(project);
    }
    let exact = db
        .query_row("SELECT id,name FROM projects WHERE id=?1", [value], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        })
        .optional()?;
    if let Some(project) = exact {
        return Ok(project);
    }
    if value == detected.id || value == detected.name {
        return project_names::canonical(db, detected.clone());
    }
    // Keep repository IDs and custom selectors compatible, then resolve their
    // name to the existing destination before registration.
    project_names::canonical(
        db,
        Project {
            id: if value.contains('/') || value.starts_with("local:") || value.starts_with("named:")
            {
                value.into()
            } else {
                format!("named:{value}")
            },
            name: value
                .rsplit('/')
                .next()
                .unwrap_or(value)
                .strip_prefix("named:")
                .unwrap_or_else(|| value.rsplit('/').next().unwrap_or(value))
                .into(),
        },
    )
}

pub struct Store {
    db: Connection,
    attachment_root: std::path::PathBuf,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        // Existing file permissions are preserved; fresh databases and their
        // SQLite sidecars are private to the user.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(Error::invalid("Issue database must be a regular file"));
        }
        let mut db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(10))?;
        db.pragma_update(None, "foreign_keys", true)?;
        let app: i64 = db.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if app != 0 && app != APPLICATION_ID
            || version > SCHEMA_VERSION
            || version > 0 && app != APPLICATION_ID
        {
            return Err(Error::invalid(
                "Incompatible issue database; use the matching hey-boss version",
            ));
        }
        // Healthy opens never take a writer lock. Recheck under the lock before
        // repairing, since another startup may have completed the migration.
        let needs_repair = version >= 10
            && !missing_additive_columns(&db)
                .map_err(|e| migration_error(e, path))?
                .is_empty();
        if version < SCHEMA_VERSION || needs_repair {
            db.pragma_update(None, "foreign_keys", false)?;
            let mut migrate = || -> Result<()> {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let app: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
                let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
                if app != 0 && app != APPLICATION_ID || version > SCHEMA_VERSION {
                    return Err(Error::invalid(
                        "Incompatible issue database; use the matching hey-boss version",
                    ));
                }
                if version == 0 {
                    let tables: i64 = tx.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |r| r.get(0))?;
                    if tables != 0 {
                        return Err(Error::invalid("Refusing to use a non-issue database"));
                    }
                    tx.execute_batch(SCHEMA)?;
                    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
                    tx.pragma_update(None, "user_version", 1)?;
                } else if app != APPLICATION_ID {
                    return Err(Error::invalid("Refusing to use a non-issue database"));
                }
                if version < 2 {
                    tx.execute_batch("ALTER TABLE projects ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;
                    ALTER TABLE projects ADD COLUMN activity_at INTEGER NOT NULL DEFAULT 0;
                    ALTER TABLE projects ADD COLUMN hidden_at INTEGER;
                    UPDATE projects SET created_at=coalesce((SELECT min(created_at) FROM issues WHERE project_id=projects.id),0);
                    UPDATE projects SET activity_at=coalesce((SELECT max(updated_at) FROM issues WHERE project_id=projects.id),created_at);
                    CREATE INDEX project_activity ON projects(hidden_at,activity_at DESC);")?;
                    tx.pragma_update(None, "user_version", 2)?;
                }
                if version < 3 {
                    tx.execute_batch(workers::SCHEMA)?;
                    tx.pragma_update(None, "user_version", 3)?;
                }
                if version < 4 {
                    tx.execute_batch(registry::SCHEMA)?;
                    tx.pragma_update(None, "user_version", 4)?;
                }
                if version < 5 {
                    tx.execute_batch("ALTER TABLE issues ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
                    UPDATE issues SET sort_order=number;
                    ALTER TABLE projects ADD COLUMN issue_order_version INTEGER NOT NULL DEFAULT 0;
                    CREATE INDEX issue_sort_order ON issues(project_id,sort_order,number);
                    CREATE INDEX worker_sort_order ON issues(sort_order,created_at,project_id,number) WHERE deleted_at IS NULL AND state='open' AND assignee IS NULL;")?;
                    tx.pragma_update(None, "user_version", 5)?;
                }
                if version < 6 {
                    tx.execute_batch("ALTER TABLE project_settings ADD COLUMN boss_name TEXT NOT NULL DEFAULT 'Boss';")?;
                    tx.pragma_update(None, "user_version", 6)?;
                }
                if version < 7 {
                    tx.execute_batch(super::global_settings::SCHEMA)?;
                    tx.pragma_update(None, "user_version", 7)?;
                }
                if version < 8 {
                    tx.execute_batch(subtasks::SCHEMA)?;
                    tx.pragma_update(None, "user_version", 8)?;
                }
                if version < 9 {
                    tx.execute_batch(super::fleet::SCHEMA)?;
                    subtasks::migrate_sync(&tx)?;
                    tx.pragma_update(None, "user_version", 9)?;
                }
                if version < 10 {
                    tx.execute_batch(mindmap::SCHEMA)?;
                    tx.pragma_update(None, "user_version", 10)?;
                }
                for (table, column, definition) in missing_additive_columns(&tx)? {
                    tx.execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN {column} {definition};"
                    ))?;
                }
                if version > 0 && version < 13 {
                    migrate_blocked(&tx)?;
                }
                tx.execute_batch(include_str!("subtask-readiness.sql"))?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                tx.commit()?;
                Ok(())
            };
            let result = migrate().map_err(|e| migration_error(e, path));
            db.pragma_update(None, "foreign_keys", true)?;
            result?;
        }
        let journal: String = db.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        if !journal.eq_ignore_ascii_case("wal") {
            db.pragma_update(None, "journal_mode", "WAL")?;
        }
        db.pragma_update(None, "synchronous", "FULL")?;
        // The additive fleet schema is installed as one batch. Its final table
        // marks completion; repeated opens must not acquire the writer lock.
        if !db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='fleet_deferred_subtasks' AND type='table')", [], |r| r.get::<_, bool>(0))? {
            db.execute_batch(super::fleet::SCHEMA)?;
        }
        if db.query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('mindmap_reference_lookup','issue_pr_canonical_url','worker_issue_history','worker_finished_history','issue_redirect')", [], |r| r.get::<_, i64>(0))? < 5 {
            db.execute_batch(mindmap::INDEXES)?;
            db.execute_batch(workers::HISTORY_INDEX)?;
            db.execute_batch(registry::FINISHED_HISTORY_INDEX)?;
            db.execute_batch(transfer::INDEX)?;
        }
        // An early updater persisted runtime state inside strict Settings JSON.
        // Normalize it without terminating workers that are still draining.
        if db.query_row("SELECT EXISTS(SELECT 1 FROM issue_workers WHERE json_type(config,'$.upgrading') IS NOT NULL)", [], |r| r.get::<_, bool>(0))? {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            registry::migrate_runtime(&tx)?;
            tx.commit()?;
        }
        if !db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='artifact_link_target' AND type='index')", [], |r|r.get::<_,bool>(0))? { db.execute_batch(artifacts::SCHEMA)?; }
        if !db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='fleet_row_local' AND type='index')", [], |r|r.get::<_,bool>(0))? { db.execute_batch(super::fleet::INDEXES)?; }
        if !db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='project_chiefs' AND type='table')", [], |r|r.get::<_,bool>(0))? {
            db.execute_batch(super::chief::SCHEMA)?;
        }
        agent_launches::migrate(&db)?;
        status::migrate(&db)?;
        provenance::migrate(&db)?;
        steering::migrate(&db)?;
        if !db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='file_attachment_target' AND type='index')", [], |r|r.get::<_,bool>(0))? { db.execute_batch(crate::attachments::SCHEMA)?; }
        project_names::migrate(&db)?;
        super::blockers::migrate(&mut db)?;
        Ok(Self {
            db,
            attachment_root: path.with_extension("attachments"),
        })
    }

    /// Resolve notification headings through the same project registry as issues.
    /// Register first use without changing issue ownership or hidden-project state.
    pub fn notification_project(
        &mut self,
        detected: &Project,
        override_id: Option<&str>,
    ) -> Result<Project> {
        if let Some(value) = override_id {
            identifier(value, "project override", 8192)?;
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let project = resolve_project(&tx, detected, override_id)?;
        if override_id.is_none() && super::identity::is_home_project(detected) {
            return Err(Error::invalid(
                "The home directory is not a project. Use --project or run from a project directory.",
            ));
        }
        identifier(&project.id, "project ID", 8192)?;
        identifier(&project.name, "project name", 1024)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        tx.execute("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES(?1,?2,1,?3,?3)
            ON CONFLICT(id) DO UPDATE SET activity_at=max(projects.activity_at,excluded.activity_at)", params![project.id,project.name,now])?;
        tx.commit()?;
        Ok(project)
    }

    pub fn execute(&mut self, r: &Request) -> Result<Value> {
        validate(r)?;
        if let Operation::ReadPlan { plan } = &r.operation {
            return super::planning::read_plan(plan);
        }
        let payload = if let Operation::Attachment { operation } = &r.operation {
            serde_json::to_string(
                &json!({"action":"attachment","operation":operation.fingerprint()?}),
            )?
        } else {
            serde_json::to_string(&r.operation)?
        };
        let write = r.operation.writes();
        // Existing-project reads use a WAL snapshot and do not compete with
        // worker reservations, event writes, or replica synchronization.
        let detected = resolve_project(&self.db, &r.project, r.project_override.as_deref())?;
        let home = r.project_override.is_none() && super::identity::is_home_project(&r.project);
        if write
            && home
            && !matches!(
                r.operation,
                Operation::ConfigureGlobal { .. } | Operation::ControlWorker { .. }
            )
        {
            return Err(Error::invalid(
                "The home directory is not a project. Use --project or run from a project directory.",
            ));
        }
        let register = !matches!(
            r.operation,
            Operation::GlobalSettings | Operation::ConfigureGlobal { .. }
        ) && !home
            && !self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                [&detected.id],
                |row| row.get::<_, bool>(0),
            )?;
        let legacy_runtime = self.db.query_row("SELECT EXISTS(SELECT 1 FROM issue_workers WHERE json_type(config,'$.upgrading') IS NOT NULL)", [], |row| row.get::<_, bool>(0))?;
        let behavior = if write || register || legacy_runtime {
            TransactionBehavior::Immediate
        } else {
            TransactionBehavior::Deferred
        };
        let tx = self.db.transaction_with_behavior(behavior)?;
        if matches!(
            r.operation,
            Operation::GlobalSettings | Operation::ConfigureGlobal { .. }
        ) {
            let result = super::global_settings::execute(&tx, r)?;
            tx.commit()?;
            return Ok(result);
        }
        let project = resolve_project(&tx, &r.project, r.project_override.as_deref())?;
        identifier(&project.id, "project ID", 8192)?;
        identifier(&project.name, "project name", 1024)?;
        let actor = r.actor.as_ref();
        if let (Some(key), Some(actor)) = (&r.request_id, actor) {
            let previous: Option<(String, String)> = tx.query_row(
                "SELECT payload,response FROM requests WHERE project_id=?1 AND actor=?2 AND request_id=?3",
                params![project.id, actor.id, key], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
            if let Some((old, response)) = previous {
                if old != payload {
                    return Err(Error::conflict(
                        "Request ID was already used for a different operation",
                    ));
                }
                if let Operation::Attachment {
                    operation: crate::attachments::Operation::Remove { id },
                } = &r.operation
                {
                    crate::attachments::delete_file(&self.attachment_root.join(id))?;
                }
                return Ok(serde_json::from_str(&response)?);
            }
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        if (write || register) && !home {
            tx.execute("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES(?1,?2,1,?3,?3) ON CONFLICT(id) DO NOTHING", params![project.id,project.name,now])?;
        }
        if write {
            let actor = actor.unwrap();
            tx.execute("INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET metadata=excluded.metadata,last_seen=excluded.last_seen",
                params![actor.id, serde_json::to_string(actor)?, now])?;
        }
        let mut attachment_files = crate::attachments::DiskChange::default();
        let mut result = match &r.operation {
            Operation::Attachment { operation } => crate::attachments::execute(
                &tx,
                &self.attachment_root,
                &project,
                operation,
                actor.map(|a| a.id.as_str()).unwrap_or(""),
                now,
                &mut attachment_files,
            )?,
            Operation::Batch { edits, dry_run } => {
                batch::execute(&tx, &project, actor, edits, *dry_run, now)?
            }
            Operation::Artifact { operation } => {
                artifacts::execute(&tx, &project, operation, actor, now)?
            }
            Operation::Mindmap { operation } => mindmap::execute(&tx, &project, operation, now)?,
            Operation::Workers { .. }
            | Operation::ConfigureWorker { .. }
            | Operation::ControlWorker { .. }
            | Operation::PreviewWorker { .. }
            | Operation::ProjectSettings
            | Operation::ConfigureProject { .. }
            | Operation::PullRequests { .. }
            | Operation::AddPullRequest { .. }
            | Operation::ClassifyPullRequest { .. }
            | Operation::RemovePullRequest { .. } => {
                registry::execute(&tx, &project, &r.operation, actor)?
            }
            Operation::WorkerConfigure { .. }
            | Operation::WorkerPool { .. }
            | Operation::WorkerControl { .. } => {
                return Err(Error::invalid(
                    "Use independent worker operations or hey-boss worker; project and global worker limits are no longer supported",
                ));
            }
            Operation::WorkerStatus => registry::execute(
                &tx,
                &project,
                &Operation::Workers { worker_id: None },
                actor,
            )?,
            Operation::WorkerPreview { .. } | Operation::WorkerRun { .. } => {
                workers::execute(&tx, &project, &r.operation, now)?
            }
            Operation::Projects { include_hidden } => {
                let mut query = tx.prepare("SELECT p.id,p.name,
                    coalesce(sum(i.state='open' AND i.deleted_at IS NULL),0),
                    coalesce(sum(i.state='closed' AND i.deleted_at IS NULL),0),
                    coalesce(sum(i.deleted_at IS NOT NULL),0),
                    coalesce(sum(i.state='open' AND i.assignee IS NULL AND i.deleted_at IS NULL),0),
                    p.activity_at,p.hidden_at,p.created_at,
                    coalesce(sum(i.state='blocked' AND i.deleted_at IS NULL),0)
                    FROM projects p LEFT JOIN issues i ON i.project_id=p.id AND NOT (i.deleted_at IS NOT NULL AND EXISTS(SELECT 1 FROM events e WHERE e.project_id=i.project_id AND e.issue_number=i.number AND e.action='moved_to'))
                    WHERE EXISTS(SELECT 1 FROM project_name_keys k WHERE k.project_id=p.id) AND (?1 OR p.hidden_at IS NULL) GROUP BY p.id ORDER BY p.activity_at DESC,lower(p.name),p.id")?;
                let projects = query.query_map([include_hidden], |row| Ok(json!({
                    "id":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,
                    "open":row.get::<_,i64>(2)?,"closed":row.get::<_,i64>(3)?,"deleted":row.get::<_,i64>(4)?,"unassigned":row.get::<_,i64>(5)?,
                    "activity_at":row.get::<_,i64>(6)?,"hidden_at":row.get::<_,Option<i64>>(7)?,"created_at":row.get::<_,i64>(8)?,"blocked":row.get::<_,i64>(9)?
                })))?.collect::<rusqlite::Result<Vec<_>>>()?;
                // Older discovery registered temporary test/agent directories.
                // Omit only empty entries, without deleting data or changing the
                // user's visibility choices. Indexed lookups run only for local
                // temporary identities; repository listings need no filesystem IO.
                let mut saved_work = tx.prepare(
                    "SELECT
                    EXISTS(SELECT 1 FROM issues WHERE project_id=?1)
                    OR EXISTS(SELECT 1 FROM artifacts WHERE project_id=?1)
                    OR EXISTS(SELECT 1 FROM mindmap_nodes WHERE project_id=?1)
                    OR EXISTS(SELECT 1 FROM project_settings WHERE project_id=?1)
                    OR EXISTS(SELECT 1 FROM project_workers WHERE project_id=?1)",
                )?;
                let mut listed_projects = Vec::with_capacity(projects.len());
                let warnings = project_names::warnings(&tx)?;
                for mut p in projects {
                    let id = p["id"].as_str().unwrap();
                    if !super::identity::is_temporary_project(id)
                        || saved_work.query_row([id], |r| r.get::<_, bool>(0))?
                    {
                        p["name_collisions"] = json!(
                            warnings
                                .as_array()
                                .unwrap()
                                .iter()
                                .filter(|w| w["project_id"] == p["id"])
                                .collect::<Vec<_>>()
                        );
                        listed_projects.push(p);
                    }
                }
                let mut query = tx.prepare("SELECT DISTINCT j.value FROM issues i,json_each(i.labels) j WHERE i.project_id=?1 AND i.deleted_at IS NULL ORDER BY j.value")?;
                let labels = query
                    .query_map([&project.id], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut query = tx.prepare("SELECT DISTINCT assignee FROM issues WHERE project_id=?1 AND assignee IS NOT NULL AND deleted_at IS NULL ORDER BY assignee")?;
                let assignees = query
                    .query_map([&project.id], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                json!({"ok":true,"project":project,"projects":listed_projects,"project_warnings":project_names::warnings(&tx)?,"labels":labels,"assignees":assignees})
            }
            Operation::HideProject | Operation::RestoreProject => {
                let changed = if matches!(r.operation, Operation::HideProject) {
                    tx.execute(
                        "UPDATE projects SET hidden_at=?2 WHERE id=?1 AND hidden_at IS NULL",
                        params![project.id, now],
                    )?
                } else {
                    tx.execute(
                        "UPDATE projects SET hidden_at=NULL WHERE id=?1 AND hidden_at IS NOT NULL",
                        [&project.id],
                    )?
                };
                let hidden_at: Option<i64> = tx.query_row(
                    "SELECT hidden_at FROM projects WHERE id=?1",
                    [&project.id],
                    |r| r.get(0),
                )?;
                json!({"ok":true,"project":project,"changed":changed>0,"hidden_at":hidden_at})
            }
            Operation::Whoami => json!({"ok":true,"project":project,"agent":actor}),
            Operation::List {
                state,
                mine,
                unassigned,
                assignee,
                labels,
                search,
                limit,
                offset,
                all,
            } => {
                let owner = if *mine {
                    actor.map(|a| a.id.as_str())
                } else {
                    assignee.as_deref()
                };
                let summary_columns = COLUMNS.replacen("body,", "'' AS body,", 1);
                let mut stmt = tx.prepare(&format!("SELECT {summary_columns},(SELECT count(*) FROM comments c WHERE c.project_id=issues.project_id AND c.issue_number=issues.number) AS comment_count FROM issues WHERE project_id=?1
                    AND ((?2='deleted' AND deleted_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM events e WHERE e.project_id=issues.project_id AND e.issue_number=issues.number AND e.action='moved_to')) OR (?2!='deleted' AND deleted_at IS NULL AND (?2='all' OR state=?2)))
                    AND (?3 IS NULL OR assignee=?3) AND (?4=0 OR assignee IS NULL)
                    AND (?5 IS NULL OR instr(lower(title),lower(?5))>0 OR instr(lower(body),lower(?5))>0)
                    AND NOT EXISTS (SELECT 1 FROM json_each(?6) wanted WHERE NOT EXISTS (SELECT 1 FROM json_each(issues.labels) existing WHERE existing.value=wanted.value))
                    ORDER BY sort_order,number LIMIT ?7 OFFSET ?8"))?;
                let rows = stmt.query_map(
                    params![
                        project.id,
                        state,
                        owner,
                        unassigned,
                        search,
                        serde_json::to_string(labels)?,
                        if *all { -1_i64 } else { i64::from(*limit) + 1 },
                        if *all { 0 } else { *offset }
                    ],
                    |row| Ok((row_issue(row)?, row.get::<_, i64>("comment_count")?)),
                )?;
                let mut found = rows.collect::<rusqlite::Result<Vec<_>>>()?;
                let more = !*all && found.len() > *limit as usize;
                if !*all {
                    found.truncate(*limit as usize);
                }
                let mut items = Vec::new();
                for (issue, comment_count) in found {
                    let mut value = serde_json::to_value(&issue)?;
                    value.as_object_mut().unwrap().remove("body");
                    value["comment_count"] = json!(comment_count);
                    items.push(value);
                }
                let order_version: i64 = tx.query_row(
                    "SELECT issue_order_version FROM projects WHERE id=?1",
                    [&project.id],
                    |r| r.get(0),
                )?;
                json!({"ok":true,"project":project,"order_version":order_version,"issues":items,"next_offset":if more { Some(u64::from(*offset) + u64::from(*limit)) } else { None }})
            }
            Operation::Move {
                number,
                before,
                after,
                if_order_version,
            } => {
                let order_version: i64 = tx.query_row(
                    "SELECT issue_order_version FROM projects WHERE id=?1",
                    [&project.id],
                    |r| r.get(0),
                )?;
                if if_order_version.is_some_and(|v| v != order_version) {
                    return Err(Error::conflict(
                        "Issue order changed. Refresh the list and try again.",
                    ));
                }
                let issue = get_issue(&tx, &project.id, *number, true)?;
                let old = issue.sort_order;
                let target = if let Some(anchor) = before {
                    let anchor = get_issue(&tx, &project.id, *anchor, true)?.sort_order;
                    anchor - i64::from(anchor > old)
                } else if let Some(anchor) = after {
                    let anchor = get_issue(&tx, &project.id, *anchor, true)?.sort_order;
                    anchor + i64::from(anchor < old)
                } else {
                    tx.query_row(
                        "SELECT max(sort_order) FROM issues WHERE project_id=?1",
                        [&project.id],
                        |r| r.get(0),
                    )?
                };
                let changed = old != target;
                if changed {
                    tx.execute("UPDATE issues SET sort_order=CASE WHEN number=?2 THEN ?3 WHEN ?3<?4 THEN sort_order+1 ELSE sort_order-1 END WHERE project_id=?1 AND sort_order BETWEEN min(?3,?4) AND max(?3,?4)", params![project.id,number,target,old])?;
                    tx.execute("UPDATE issues SET updated_at=?3,version=version+1 WHERE project_id=?1 AND number=?2",params![project.id,number,now])?;
                    tx.execute(
                        "UPDATE projects SET issue_order_version=issue_order_version+1 WHERE id=?1",
                        [&project.id],
                    )?;
                    event(
                        &tx,
                        &project.id,
                        *number,
                        &actor.unwrap().id,
                        "reordered",
                        now,
                        &json!({"before":before,"after":after,"from":old,"to":target}),
                    )?;
                }
                json!({"ok":true,"project":project,"issue":get_issue(&tx,&project.id,*number,true)?,"changed":changed,"order_version":order_version+i64::from(changed)})
            }
            Operation::Status {
                number,
                level,
                comment,
            } => status::update(&tx, &project, actor.unwrap(), *number, *level, comment, now)?,
            Operation::StatusHistory {
                number,
                limit,
                offset,
                before,
            } => status::history(&tx, &project, *number, *limit, *offset, *before)?,
            Operation::StatusView { number } => status::current(&tx, &project, *number)?,
            Operation::View { number } => {
                let issue = get_issue(&tx, &project.id, *number, true)?;
                if issue.deleted_at.is_some()
                    && let Some(destination) = transfer::destination(&tx, &project.id, *number)?
                {
                    tx.commit()?;
                    return Ok(
                        json!({"ok":true,"project":project,"issue":issue,"moved_to":destination}),
                    );
                }
                let mut page = comment_page(
                    &tx,
                    &project,
                    *number,
                    20,
                    0,
                    super::CommentSort::Newest,
                    serde_json::to_vec(&issue)?.len(),
                )?;
                page["comments"].as_array_mut().unwrap().reverse();
                let assignee: Option<Actor> = if let Some(id) = &issue.assignee {
                    let raw: String =
                        tx.query_row("SELECT metadata FROM agents WHERE id=?1", [id], |r| {
                            r.get(0)
                        })?;
                    Some(serde_json::from_str(&raw)?)
                } else {
                    None
                };
                json!({"ok":true,"project":project,"issue":issue,"comments":page["comments"],"comment_count":page["comment_count"],"more_comments":!page["next_offset"].is_null(),"next_comment_offset":page["next_offset"],"assignee_agent":assignee,"artifacts":artifacts::links(&tx,&project,Some(*number),None)?["artifacts"]})
            }
            Operation::Comments {
                number,
                limit,
                offset,
                sort,
            } => {
                get_issue(&tx, &project.id, *number, true)?;
                comment_page(&tx, &project, *number, *limit, *offset, *sort, 0)?
            }
            Operation::History {
                number,
                limit,
                offset,
            } => {
                get_issue(&tx, &project.id, *number, true)?;
                let mut stmt = tx.prepare("SELECT id,actor,action,created_at,data FROM events WHERE project_id=?1 AND issue_number=?2 ORDER BY id LIMIT ?3 OFFSET ?4")?;
                let rows = stmt
                    .query_map(params![project.id, number, limit + 1, offset], |row| {
                        let data: String = row.get(4)?;
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            data,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut more = rows.len() > *limit as usize;
                let mut events = Vec::new();
                let mut bytes = 0;
                for (id, actor, action, at, data) in rows.into_iter().take(*limit as usize) {
                    let event = json!({"id":id,"actor":actor,"action":action,"created_at":at,"data":serde_json::from_str::<Value>(&data)?});
                    bytes += serde_json::to_vec(&event)?.len();
                    if bytes > PAGE_BYTES && !events.is_empty() {
                        more = true;
                        break;
                    }
                    events.push(event);
                }
                json!({"ok":true,"project":project,"events":events,"next_offset":if more { Some(u64::from(*offset) + events.len() as u64) } else { None }})
            }
            Operation::Subtasks { .. }
            | Operation::CreateSubtask { .. }
            | Operation::AddSubtask { .. }
            | Operation::RemoveSubtask { .. } => {
                subtasks::execute(&tx, &project, &r.operation, actor, now)?
            }
            Operation::Transfer {
                number,
                destination,
                if_version,
            } => transfer::execute(
                &tx,
                &project,
                actor.unwrap(),
                *number,
                destination,
                *if_version,
                now,
            )?,
            Operation::Create { .. } => {
                let issue = create_issue(&tx, &project, actor.unwrap(), &r.operation, now)?;
                json!({"ok":true,"project":project,"issue":issue,"changed":true})
            }
            operation => mutate(&tx, &project, actor.unwrap(), operation, now)?,
        };
        let response_project = if matches!(r.operation, Operation::Transfer { .. }) {
            serde_json::from_value::<Project>(result["project"].clone())?
        } else {
            project.clone()
        };
        if matches!(
            r.operation,
            Operation::Create { .. }
                | Operation::CreateSubtask { .. }
                | Operation::AddSubtask { .. }
                | Operation::RemoveSubtask { .. }
                | Operation::Block { .. }
                | Operation::SetBlockers { .. }
                | Operation::Close { .. }
                | Operation::Reopen { .. }
                | Operation::Delete { .. }
                | Operation::Restore { .. }
                | Operation::Transfer { .. }
        ) {
            super::blockers::reconcile(
                &tx,
                &response_project.id,
                actor.map(|a| a.id.as_str()),
                now,
            )?;
            for key in ["issue", "parent_issue", "child_issue"] {
                if let Some(number) = result[key]["number"].as_i64() {
                    result[key] =
                        serde_json::to_value(get_issue(&tx, &response_project.id, number, true)?)?;
                }
            }
        }
        result["drafts_enabled"] =
            registry::project_settings(&tx, &response_project)?["drafts_enabled"].clone();
        let settings = super::global_settings::read(&tx)?;
        result["boss"] =
            json!({"id":"human:boss","name":settings["boss_name"],"version":settings["version"]});
        if let Some(issue) = result.get_mut("issue")
            && let Some(number) = issue["number"].as_i64()
        {
            issue["pull_requests"] =
                json!(registry::pull_requests(&tx, &response_project.id, number)?);
            if issue["assignee"] == "human:boss" {
                issue["assignee_name"] = settings["boss_name"].clone();
            }
        }
        if let Some(issues) = result["issues"].as_array_mut() {
            for issue in issues {
                if let Some(number) = issue["number"].as_i64() {
                    issue["pull_requests"] =
                        json!(registry::pull_requests(&tx, &project.id, number)?);
                    if issue["assignee"] == "human:boss" {
                        issue["assignee_name"] = settings["boss_name"].clone();
                    }
                }
            }
        }
        if !matches!(
            r.operation,
            Operation::Attachment { .. }
                | Operation::Artifact { .. }
                | Operation::Batch { .. }
                | Operation::Comments { .. }
        ) {
            subtasks::enrich(&tx, &response_project.id, &mut result)?;
        }
        super::blockers::enrich(&tx, &response_project.id, &mut result)?;
        if matches!(r.operation, Operation::Claim { .. }) {
            result["instructions"] = json!(registry::claim_instructions(
                &tx,
                &project,
                &result["issue"],
                actor.unwrap()
            )?);
        }
        if (r.operation.number().is_some()
            || matches!(
                r.operation,
                Operation::Create { .. } | Operation::Batch { .. } | Operation::Attachment { .. }
            ))
            && result["changed"] == true
        {
            tx.execute(
                "UPDATE projects SET activity_at=max(activity_at,?2) WHERE id=?1",
                params![project.id, now],
            )?;
        }
        if (write || register) && result.get("project_warnings").is_none() {
            let warnings = project_names::warnings(&tx)?;
            let warnings = warnings
                .as_array()
                .unwrap()
                .iter()
                .filter(|w| w["project_id"] == project.id)
                .collect::<Vec<_>>();
            if !warnings.is_empty() {
                result["project_warnings"] = json!(warnings);
            }
        }
        if matches!(
            r.operation,
            Operation::Status { .. } | Operation::Comment { .. }
        ) {
            let actor = actor.unwrap();
            let owner = result["issue"]["assignee"].as_str().map(str::to_owned);
            if owner.as_deref() != Some(&actor.id) {
                result["ownership_warning"] = json!(match owner.as_deref() {
                    Some(owner) => format!(
                        "You are not the owner; this issue is assigned to {owner}. Consider claiming it before starting work."
                    ),
                    None => "This issue is unassigned. Consider claiming it before starting work."
                        .into(),
                });
                if let Some(owner) = owner {
                    let raw: String =
                        tx.query_row("SELECT metadata FROM agents WHERE id=?1", [&owner], |r| {
                            r.get(0)
                        })?;
                    result["assignee_agent"] = serde_json::from_str::<Value>(&raw)?;
                }
            }
        }
        if let (Some(key), Some(actor)) = (&r.request_id, actor) {
            tx.execute("INSERT INTO requests(project_id,actor,request_id,payload,response) VALUES(?1,?2,?3,?4,?5)",
                params![project.id,actor.id,key,payload,serde_json::to_string(&result)?])?;
        }
        if matches!(
            &r.operation,
            Operation::Mindmap {
                operation: crate::mindmap::Operation::Batch { dry_run: true, .. }
            } | Operation::Batch { dry_run: true, .. }
        ) {
            tx.rollback()?;
        } else {
            tx.commit()?;
            attachment_files.new = None;
            if let Some(path) = attachment_files.removed.take() {
                crate::attachments::delete_file(&path)?;
            }
        }
        if matches!(&r.operation, Operation::ControlWorker { command, .. } if command == "stop_worker" || command == "stop")
        {
            // No process inspection or termination while holding the writer lock.
            // A dead worker cannot observe its durable stop request itself.
            super::worker::recover(self, &super::identity::machine()?)?;
        }
        Ok(result)
    }

    /// Register observed projects without undoing a user's hidden-project choice.
    /// Repeated observations use the source's event time, never polling time.
    pub fn discover_projects(&mut self, projects: &[(Project, i64)]) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (project, activity) in projects {
            if super::identity::is_home_project(project)
                || super::identity::is_temporary_project(&project.id)
            {
                continue;
            }
            identifier(&project.id, "project ID", 8192)?;
            identifier(&project.name, "project name", 1024)?;
            let project = project_names::canonical(&tx, project.clone())?;
            let at = (*activity).clamp(0, now);
            tx.execute("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES(?1,?2,1,?3,?4)
                ON CONFLICT(id) DO UPDATE SET activity_at=max(projects.activity_at,excluded.activity_at)
                WHERE excluded.activity_at>projects.activity_at",params![project.id,project.name,now,at])?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn create_issue(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    operation: &Operation,
    now: i64,
) -> Result<Issue> {
    let (title, body, labels, at_top) = match operation {
        Operation::Create {
            title,
            body,
            labels,
            at_top,
            ..
        }
        | Operation::CreateSubtask {
            title,
            body,
            labels,
            at_top,
            ..
        } => (title, body, labels, at_top),
        _ => unreachable!(),
    };
    let draft = matches!(operation, Operation::Create { draft: true, .. });
    if draft {
        super::planning::drafts_allowed(db, project)?;
    }
    let number: i64 = db.query_row(
        "SELECT next_number FROM projects WHERE id=?1",
        [&project.id],
        |r| r.get(0),
    )?;
    super::fleet::check_create(db, &project.id, number)?;
    db.execute(
        "UPDATE projects SET next_number=next_number+1 WHERE id=?1",
        [&project.id],
    )?;
    let labels: BTreeSet<_> = labels.iter().collect();
    let sort_order: i64 = db.query_row(
        "SELECT CASE WHEN ?2 THEN coalesce(min(sort_order),1) ELSE coalesce(max(sort_order),0)+1 END FROM issues WHERE project_id=?1",
        params![project.id,at_top],
        |r| r.get(0),
    )?;
    if *at_top {
        db.execute(
            "UPDATE issues SET sort_order=sort_order+1 WHERE project_id=?1",
            [&project.id],
        )?;
    }
    db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order,draft,origin) VALUES(?1,?2,?3,?4,'open',?5,?6,?6,1,?7,?8,?9,?10)",
        params![project.id,number,title,body,actor.id,now,serde_json::to_string(&labels)?,sort_order,draft,provenance::capture(db,actor,now)?])?;
    db.execute("INSERT OR IGNORE INTO fleet_allocations(project_id,issue_number,node) SELECT ?1,?2,node FROM fleet_meta WHERE id=1 AND role='agent'", params![project.id,number])?;
    db.execute(
        "UPDATE projects SET issue_order_version=issue_order_version+1 WHERE id=?1",
        [&project.id],
    )?;
    let issue = get_issue(db, &project.id, number, false)?;
    event(
        db,
        &project.id,
        number,
        &actor.id,
        "created",
        now,
        &json!({"issue":issue}),
    )?;
    Ok(issue)
}

pub(super) fn event(
    db: &Connection,
    project: &str,
    number: i64,
    actor: &str,
    action: &str,
    now: i64,
    data: &Value,
) -> Result<()> {
    db.execute("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES(?1,?2,?3,?4,?5,?6)",
        params![project,number,actor,action,now,serde_json::to_string(data)?])?;
    Ok(())
}
fn ownership(issue: &Issue, actor: &Actor, force: bool) -> Result<()> {
    if !force && issue.assignee.as_ref().is_some_and(|id| id != &actor.id) {
        return Err(Error::conflict(format!(
            "Issue #{} is claimed by {}; use --force for an intentional takeover or removal",
            issue.number,
            issue.assignee.as_deref().unwrap()
        )));
    }
    Ok(())
}
fn comment(
    db: &Connection,
    project: &str,
    number: i64,
    actor: &Actor,
    body: &str,
    now: i64,
) -> Result<i64> {
    db.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?1,?2,?3,?4,?5)", params![project,number,actor.id,body,now])?;
    let id = db.last_insert_rowid();
    event(
        db,
        project,
        number,
        &actor.id,
        "commented",
        now,
        &json!({"comment_id":id,"body":body}),
    )?;
    Ok(id)
}
fn mutate(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    operation: &Operation,
    now: i64,
) -> Result<Value> {
    let number = operation.number().unwrap();
    let mut issue = get_issue(
        db,
        &project.id,
        number,
        matches!(
            operation,
            Operation::Delete { .. } | Operation::Restore { .. }
        ),
    )?;
    if issue.deleted_at.is_some() && transfer::destination(db, &project.id, number)?.is_some() {
        return Err(Error::conflict(
            "This issue moved to another project; open its destination to make changes",
        ));
    }
    let before = serde_json::to_value(&issue)?;
    let mut action = "";
    let mut data = json!({});
    let mut comment_id = None;
    match operation {
        Operation::Edit {
            draft,
            title,
            body,
            add_labels,
            remove_labels,
            if_version,
            ..
        } => {
            if if_version.is_some_and(|v| v != issue.version) {
                return Err(Error::conflict(format!(
                    "Issue changed; current version is {}",
                    issue.version
                )));
            }
            if let Some(draft) = draft {
                if *draft && !issue.draft {
                    super::planning::can_draft(
                        db,
                        project,
                        number,
                        &issue.state,
                        issue.assignee.as_deref(),
                    )?;
                }
                issue.draft = *draft;
            }
            if let Some(title) = title {
                issue.title = title.clone();
            }
            if let Some(body) = body {
                issue.body = body.clone();
            }
            if *draft == Some(false) && before["draft"] == true {
                super::planning::final_sync(
                    db,
                    project,
                    &mut issue.title,
                    &mut issue.body,
                    issue.plan.as_ref(),
                )?;
            }
            let mut values: BTreeSet<_> = issue.labels.iter().cloned().collect();
            values.extend(add_labels.iter().cloned());
            for value in remove_labels {
                values.remove(value);
            }
            issue.labels = values.into_iter().collect();
            labels(&issue.labels)?;
            if serde_json::to_value(&issue)? != before {
                action = "edited";
                data = json!({"before":{"title":before["title"],"body":before["body"],"labels":before["labels"]},"after":{"title":issue.title,"body":issue.body,"labels":issue.labels,"draft":issue.draft}});
            }
        }
        Operation::Undraft { .. } => {
            if issue.draft || issue.plan.is_some() {
                super::planning::final_sync(
                    db,
                    project,
                    &mut issue.title,
                    &mut issue.body,
                    issue.plan.as_ref(),
                )?;
                issue.draft = false;
                if serde_json::to_value(&issue)? != before {
                    action = "undrafted";
                }
            }
        }
        Operation::BindPlan {
            plan, if_version, ..
        } => {
            super::planning::drafts_allowed(db, project)?;
            if (!issue.draft && issue.plan.is_none()) || *if_version != issue.version {
                return Err(Error::conflict("Planning requires an unchanged draft"));
            }
            plan.validate()?;
            if plan.machine != actor.machine
                || plan.checkout != actor.cwd
                || plan.host != actor.host
            {
                return Err(Error::invalid(
                    "Bind the plan from its owning machine and checkout",
                ));
            }
            issue.plan = Some(plan.clone());
            action = "plan_bound";
            data = json!({"plan":plan});
        }
        Operation::Claim { force, .. } | Operation::AssignBoss { force, .. } => {
            if issue.draft {
                return Err(Error::conflict("Undraft the issue before claiming it"));
            }
            registry::claim_lock(db, project, number, actor, *force)?;
            if issue.state != "open" {
                return Err(Error::conflict("Reopen the issue before claiming it"));
            }
            let target = if matches!(operation, Operation::AssignBoss { .. }) {
                "human:boss"
            } else {
                &actor.id
            };
            if issue.assignee.as_deref() != Some(target) {
                ownership(&issue, actor, *force)?;
            }
            if target == "human:boss" {
                let mut boss = actor.clone();
                boss.id = target.into();
                boss.kind = "human".into();
                boss.session_id = None;
                boss.pid = None;
                boss.process_start = None;
                boss.source = "Boss assignment".into();
                db.execute("INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING", params![target,serde_json::to_string(&boss)?,now])?;
            }
            if issue.assignee.as_deref() != Some(target) {
                action = "claimed";
                data = json!({"previous_assignee":issue.assignee,"assignee":target,"forced":force});
                issue.assignee = Some(target.into());
            }
        }
        Operation::Unassign { force, .. } => {
            ownership(&issue, actor, *force)?;
            if issue.assignee.is_some() {
                action = "unassigned";
                data = json!({"previous_assignee":issue.assignee,"forced":force});
                issue.assignee = None;
            }
        }
        Operation::Comment { body, .. } => {
            comment_id = Some(comment(db, &project.id, number, actor, body, now)?);
        }
        Operation::ResolveComment {
            comment_id: id,
            resolved,
            ..
        } => {
            let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM comments WHERE project_id=?1 AND issue_number=?2 AND id=?3)", params![project.id,number,id], |row| row.get(0))?;
            if !exists {
                return Err(Error::new("not_found", "Comment not found on this issue"));
            }
            let previous: Option<String> = db.query_row("SELECT action FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('comment_resolved','comment_unresolved') AND json_extract(data,'$.comment_id')=?3 ORDER BY created_at DESC,id DESC LIMIT 1", params![project.id,number,id], |row| row.get(0)).optional()?;
            if (previous.as_deref() == Some("comment_resolved")) != *resolved {
                action = if *resolved {
                    "comment_resolved"
                } else {
                    "comment_unresolved"
                };
                // A comment can originate on a different fleet machine than this
                // event. Preserve that identity when the offline journal replays.
                let reference: Option<(String,i64)> = db.query_row("SELECT origin,origin_id FROM fleet_row_ids WHERE table_name='comments' AND local_id=?1 ORDER BY rowid LIMIT 1", [id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
                let (origin, origin_id) = match reference {
                    Some(reference) => reference,
                    None => (
                        db.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |row| {
                            row.get::<_, String>(0)
                        })?,
                        *id,
                    ),
                };
                data =
                    json!({"comment_id":id,"comment_origin":origin,"comment_origin_id":origin_id});
            }
        }
        Operation::Close {
            comment: text,
            force,
            ..
        } => {
            ownership(&issue, actor, *force)?;
            if issue.state == "closed" && text.is_some() {
                return Err(Error::conflict(
                    "Issue is already closed; use comment to add further findings",
                ));
            }
            if issue.state != "closed" {
                if let Some(body) = text {
                    comment_id = Some(comment(db, &project.id, number, actor, body, now)?);
                }
                action = "closed";
                data = json!({"previous_assignee":issue.assignee,"forced":force});
                issue.state = "closed".into();
                issue.assignee = None;
                issue.closed_by = Some(actor.id.clone());
                issue.closed_at = Some(now);
            }
        }
        Operation::SetBlockers {
            blockers,
            if_version,
            force,
            ..
        } => {
            ownership(&issue, actor, *force)?;
            if if_version.is_some_and(|v| v != issue.version) {
                return Err(Error::conflict(
                    "Issue changed. Refresh before changing its blockers.",
                ));
            }
            super::blockers::validate_links(db, &project.id, number, blockers)?;
            if issue.blocker_numbers != *blockers {
                action = "blockers_changed";
                data = json!({"previous_blockers":issue.blocker_numbers,"blockers":blockers});
                issue.blocker_numbers = blockers.clone();
                // Linking a manual block gives it a concrete resolution condition.
                if !blockers.is_empty() {
                    issue.manual_blocked = false;
                }
            }
        }
        Operation::Block {
            comment: text,
            force,
            blockers,
            ..
        } => {
            ownership(&issue, actor, *force)?;
            if issue.state == "closed" {
                return Err(Error::conflict(
                    "Reopen the closed issue before blocking it",
                ));
            }
            if let Some(links) = blockers {
                if links.is_empty() {
                    return Err(Error::invalid("Use blocked-by to remove blocker links"));
                }
                super::blockers::validate_links(db, &project.id, number, links)?;
            }
            if issue.state == "blocked" && text.is_some() && blockers.is_none() {
                return Err(Error::conflict(
                    "Issue is already blocked; use comment to add further findings",
                ));
            }
            let manual = blockers.is_none();
            let changed_links = blockers
                .as_ref()
                .is_some_and(|links| issue.blocker_numbers != *links);
            if issue.state != "blocked" || issue.manual_blocked != manual || changed_links {
                if let Some(body) = text {
                    comment_id = Some(comment(db, &project.id, number, actor, body, now)?);
                }
                action = "blocked";
                data =
                    json!({"previous_assignee":issue.assignee,"forced":force,"blockers":blockers});
                issue.state = "blocked".into();
                issue.manual_blocked = manual;
                if let Some(links) = blockers {
                    issue.blocker_numbers = links.clone();
                }
                issue.assignee = None;
                issue.closed_at = None;
                issue.closed_by = None;
            }
        }
        Operation::Reopen { if_version, .. } => {
            if super::blockers::has_dependencies(db, &project.id, number)? {
                return Err(Error::conflict(
                    "This issue is blocked by unfinished issues. Resolve or unlink its blockers before reopening.",
                ));
            }
            if if_version.is_some_and(|v| v != issue.version) {
                return Err(Error::conflict(format!(
                    "Issue changed; current version is {}",
                    issue.version
                )));
            }
            if issue.state != "open" {
                if issue.state == "blocked" {
                    // Explicitly reopening a blocker also releases old approval
                    // holds and cooldowns; it must actually resume eligibility.
                    db.execute("UPDATE worker_runs SET retry_allowed=1 WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NOT NULL", params![project.id,number])?;
                }
                action = "reopened";
                data = json!({"previous_state":issue.state,"previous_closed_by":issue.closed_by,"previous_closed_at":issue.closed_at});
                issue.manual_blocked = false;
                issue.state = "open".into();
                issue.assignee = None;
                issue.closed_by = None;
                issue.closed_at = None;
            }
        }
        Operation::Delete { force, .. } => {
            ownership(&issue, actor, *force)?;
            if issue.deleted_at.is_none() {
                action = "deleted";
                data = json!({"previous_assignee":issue.assignee,"forced":force});
                issue.deleted_at = Some(now);
                issue.assignee = None;
            }
        }
        Operation::Restore { .. } => {
            if issue.deleted_at.is_some() {
                action = "restored";
                issue.deleted_at = None;
            }
        }
        _ => unreachable!(),
    }
    let changed = !action.is_empty() || comment_id.is_some();
    if changed {
        issue.version += 1;
        issue.updated_at = now;
        db.execute("UPDATE issues SET title=?3,body=?4,state=?5,assignee=?6,closed_by=?7,updated_at=?8,closed_at=?9,deleted_at=?10,version=?11,labels=?12,draft=?13,plan=?14,manual_blocked=?15,blockers=?16 WHERE project_id=?1 AND number=?2",
            params![project.id,number,issue.title,issue.body,issue.state,issue.assignee,issue.closed_by,now,issue.closed_at,issue.deleted_at,issue.version,serde_json::to_string(&issue.labels)?,issue.draft,issue.plan.as_ref().map(serde_json::to_string).transpose()?,issue.manual_blocked,serde_json::to_string(&issue.blocker_numbers)?])?;
        if !action.is_empty() {
            event(db, &project.id, number, &actor.id, action, now, &data)?;
        }
    }
    Ok(json!({"ok":true,"project":project,"issue":issue,"changed":changed,"comment_id":comment_id}))
}

const SCHEMA: &str = "
CREATE TABLE projects(id TEXT PRIMARY KEY, name TEXT NOT NULL, next_number INTEGER NOT NULL CHECK(next_number>0));
CREATE INDEX project_names ON projects(name);
CREATE TABLE agents(id TEXT PRIMARY KEY, metadata TEXT NOT NULL CHECK(json_valid(metadata)), last_seen INTEGER NOT NULL);
CREATE TABLE issues(
 project_id TEXT NOT NULL REFERENCES projects(id), number INTEGER NOT NULL CHECK(number>0),
 title TEXT NOT NULL, body TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('open','blocked','closed')),
 assignee TEXT REFERENCES agents(id), created_by TEXT NOT NULL REFERENCES agents(id), closed_by TEXT REFERENCES agents(id),
 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, closed_at INTEGER, deleted_at INTEGER,
 version INTEGER NOT NULL CHECK(version>0), labels TEXT NOT NULL CHECK(json_valid(labels)),
 PRIMARY KEY(project_id,number), CHECK(state='open' OR assignee IS NULL), CHECK(deleted_at IS NULL OR assignee IS NULL)
);
CREATE INDEX issue_queue ON issues(project_id,state,assignee,number) WHERE deleted_at IS NULL;
CREATE TABLE comments(id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, issue_number INTEGER NOT NULL,
 author TEXT NOT NULL REFERENCES agents(id), body TEXT NOT NULL, created_at INTEGER NOT NULL,
 FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number));
CREATE INDEX issue_comments ON comments(project_id,issue_number,id);
CREATE TABLE events(id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, issue_number INTEGER NOT NULL,
 actor TEXT NOT NULL REFERENCES agents(id), action TEXT NOT NULL, created_at INTEGER NOT NULL, data TEXT NOT NULL CHECK(json_valid(data)),
 FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number));
CREATE INDEX issue_events ON events(project_id,issue_number,id);
CREATE TABLE requests(project_id TEXT NOT NULL REFERENCES projects(id), actor TEXT NOT NULL REFERENCES agents(id), request_id TEXT NOT NULL,
 payload TEXT NOT NULL, response TEXT NOT NULL, PRIMARY KEY(project_id,actor,request_id));
";
