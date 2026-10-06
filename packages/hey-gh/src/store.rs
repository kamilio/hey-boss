use crate::{Error, Response, Result, Source, digest, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

mod checkpoint;
#[cfg(test)]
mod checkpoint_tests;
#[cfg(test)]
mod decode_tests;
#[cfg(test)]
mod writer_tests;

// Keep the partial index and bootstrap selection identical. Malformed PR JSON
// remains a candidate so reads report corruption instead of silently hiding it.
const OPEN_PR_SELECTION: &str = "resource GLOB 'pr-status://*' AND
    CASE WHEN json_valid(data) THEN
        json_extract(data,'$.pullRequest.state')='OPEN'
        AND json_type(data,'$.pullRequest.removed') IS NOT 'true'
    ELSE 1 END";

#[derive(Clone, Debug)]
pub(crate) struct PrOwner {
    pub repository: String,
    pub number: u64,
    pub node_id: Option<String>,
    pub generation: u64,
}

// PR source selectors contain no case-sensitive branch or ref component.
// Preserve the first stored spelling so aliases cannot duplicate feed rows or
// change the resource keys held by existing incremental consumers.
fn resolve_pr_resource(conn: &Connection, scope: &str, resource: &str) -> Result<String> {
    let (base, suffix) = resource
        .strip_suffix("#discovery")
        .map_or((resource, ""), |base| (base, "#discovery"));
    let Some((scheme, path)) = base.split_once("://") else {
        return Ok(resource.to_owned());
    };
    if !matches!(
        scheme,
        "pr-status"
            | "metadata"
            | "ci"
            | "comments"
            | "review_comments"
            | "reviews"
            | "timeline"
            | "review_events"
            | "review_threads"
            | "review_status"
            | "required_checks"
            | "pr"
    ) {
        return Ok(resource.to_owned());
    }
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 4 || !parts[3].parse::<u64>().is_ok_and(|n| n > 0) {
        return Ok(resource.to_owned());
    }
    let existing: Option<String> = conn.query_row(
        "SELECT resource FROM snapshots WHERE scope=?1 AND resource=?2 COLLATE NOCASE ORDER BY cursor DESC LIMIT 1",
        params![scope,base], |r|r.get(0),
    ).optional().map_err(storage)?;
    Ok(existing.map_or_else(|| resource.to_owned(), |key| format!("{key}{suffix}")))
}

fn repository_generation(conn: &Connection, scope: &str, repository: &str) -> Result<u64> {
    conn.query_row(
        "SELECT generation FROM repository_generation WHERE scope=?1 AND repository=?2",
        params![scope, repository],
        |r| r.get(0),
    )
    .optional()
    .map_err(storage)
    .map(|v| v.unwrap_or(0))
}

// Identity clocks order observations; retired identities prevent a fresh but
// lagging REST response from undoing a validated account identity change.
fn accept_identity(
    conn: &Connection,
    scope: &str,
    repository: &str,
    number: u64,
    node_id: &str,
    clock: u64,
    authoritative: bool,
) -> Result<bool> {
    let generation = repository_generation(conn, scope, repository)?;
    let mut old: Option<(String,u64,u64)> = conn.query_row("SELECT node_id,validated_at_ms,generation FROM pr_identity WHERE scope=?1 AND repository=?2 AND pull_number=?3", params![scope,repository,number], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(storage)?;
    if old.is_none() {
        // Older releases persisted detailed snapshots before this identity ledger
        // existed. The first account scan must compare against those too, even if
        // no completed account collection was ever saved.
        let suffix = format!("/{repository}/{number}");
        let legacy: Option<String> = conn.query_row(
        "SELECT data FROM snapshots WHERE scope=?1 AND substr(resource,-length(?2))=?2 COLLATE NOCASE AND (resource LIKE 'pr-status://%' OR resource LIKE 'metadata://%') ORDER BY CASE WHEN resource LIKE 'pr-status://%' THEN 0 ELSE 1 END LIMIT 1",
        params![scope,suffix], |r|r.get(0),
    ).optional().map_err(storage)?;
        if let Some(legacy) = legacy {
            let value: Value = serde_json::from_str(&legacy).map_err(storage)?;
            if let Some(id) = value["pullRequest"]["id"]
                .as_str()
                .or_else(|| value["pull_request"]["node_id"].as_str())
            {
                old = Some((id.to_owned(), 0, 0));
            }
        }
    }
    let mut next = generation;
    if let Some((old_id, old_clock, old_generation)) = &old {
        if old_id != node_id {
            let retired: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM retired_pr_identity WHERE scope=?1 AND repository=?2 AND pull_number=?3 AND node_id=?4)",params![scope,repository,number,node_id],|r|r.get(0)).map_err(storage)?;
            if clock < *old_clock || (!authoritative && retired) {
                return Ok(false);
            }
            if *old_generation == generation {
                next = generation
                    .checked_add(1)
                    .ok_or_else(|| Error::Storage("repository generation exhausted".into()))?;
                conn.execute("INSERT INTO repository_generation(scope,repository,generation) VALUES(?1,?2,?3) ON CONFLICT(scope,repository) DO UPDATE SET generation=excluded.generation",params![scope,repository,next]).map_err(storage)?;
            }
            conn.execute("INSERT OR IGNORE INTO retired_pr_identity(scope,repository,pull_number,node_id) VALUES(?1,?2,?3,?4)",params![scope,repository,number,old_id]).map_err(storage)?;
        } else if *old_generation != generation && (!authoritative || clock < *old_clock) {
            return Ok(false);
        }
    }
    conn.execute("INSERT INTO pr_identity(scope,repository,pull_number,node_id,validated_at_ms,generation) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scope,repository,pull_number) DO UPDATE SET node_id=excluded.node_id,validated_at_ms=MAX(pr_identity.validated_at_ms,excluded.validated_at_ms),generation=excluded.generation",params![scope,repository,number,node_id,clock,next]).map_err(storage)?;
    Ok(true)
}

fn owner_is_current(conn: &Connection, scope: &str, owner: &PrOwner) -> Result<bool> {
    if repository_generation(conn, scope, &owner.repository)? != owner.generation {
        return Ok(false);
    }
    let node: Option<(String,u64)> = conn.query_row("SELECT node_id,generation FROM pr_identity WHERE scope=?1 AND repository=?2 AND pull_number=?3",params![scope,owner.repository,owner.number],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(storage)?;
    Ok(node.is_none_or(|(id, generation)| {
        owner.node_id.as_deref() == Some(id.as_str()) && generation == owner.generation
    }))
}

fn prune_changes(conn: &Connection, scope: &str, cutoff: u64, max_events: usize) -> Result<()> {
    let age_floor: u64 = conn
        .query_row(
            "SELECT COALESCE(MAX(cursor),0) FROM changes WHERE scope=?1 AND observed_at_ms<?2",
            params![scope, cutoff],
            |r| r.get(0),
        )
        .map_err(storage)?;
    let count_floor: Option<u64> = conn
        .query_row(
            "SELECT cursor FROM changes WHERE scope=?1 ORDER BY cursor DESC LIMIT 1 OFFSET ?2",
            params![scope, max_events],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    let floor = age_floor.max(count_floor.unwrap_or(0));
    if floor > 0 {
        conn.execute(
            "DELETE FROM changes WHERE scope=?1 AND cursor<=?2",
            params![scope, floor],
        )
        .map_err(storage)?;
        conn.execute(
            "UPDATE feeds SET floor=MAX(floor,?2) WHERE scope=?1",
            params![scope, floor],
        )
        .map_err(storage)?;
    }
    Ok(())
}

fn discovery_identity(node: &Value, collection: &Value) -> Option<(String, u64, String, u64)> {
    let repository = node["repository"]["nameWithOwner"].as_str()?;
    let number = node["number"].as_u64()?;
    let id = node["id"].as_str()?;
    let clock = collection["validatedAtByPr"][format!("{repository}/{number}")]
        .as_u64()
        .or_else(|| {
            collection["validatedAtByPr"][format!("{}/{number}", repository.to_ascii_lowercase())]
                .as_u64()
        })
        .or_else(|| collection["validatedAtMs"].as_u64())?;
    Some((
        repository.to_ascii_lowercase(),
        number,
        id.to_owned(),
        clock,
    ))
}

#[derive(Clone)]
pub(crate) struct Store {
    connection: Arc<Mutex<Connection>>,
    writer_admission: Arc<tokio::sync::Semaphore>,
    checkpoint: Option<Arc<checkpoint::Checkpointer>>,
    readers: Option<Readers>,
    payload_decoders: Arc<tokio::sync::Semaphore>,
    retention: std::time::Duration,
    max_events: usize,
    max_snapshot_bytes: usize,
}

#[derive(Clone)]
struct Readers {
    lookups: Arc<tokio::sync::Mutex<Connection>>,
    bulk: Arc<tokio::sync::Mutex<Connection>>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DiscoveryHealth {
    pub last_poll_at_ms: Option<u64>,
    pub last_success_at_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Change {
    pub cursor: String,
    pub resource: String,
    pub changed_fields: Vec<String>,
    pub observed_at_ms: u64,
    /// The current resource value. Superseded observations are coalesced.
    pub data: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangePage {
    pub changes: Vec<Change>,
    pub next_cursor: String,
    pub head_cursor: String,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub resource: String,
    pub data: Value,
    pub observed_at_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotPage {
    pub snapshots: Vec<Snapshot>,
    pub cursor: String,
}

#[derive(Debug)]
pub(crate) struct PrBootstrapPage {
    pub snapshots: Vec<Snapshot>,
    pub cursor: String,
    pub has_more: bool,
}

/// Resume a roster scan, then replay changes from its initial watermark.
#[derive(Serialize, Deserialize)]
pub(crate) struct PrBootstrapCursor {
    pub boundary: String,
    pub after: String,
}

impl PrBootstrapCursor {
    pub fn encode(&self) -> String {
        format!(
            "pb1:{}",
            serde_json::to_string(self).expect("string cursor")
        )
    }

    pub fn decode(cursor: &str, prefix: &str) -> Result<Option<Self>> {
        let Some(value) = cursor.strip_prefix("pb1:") else {
            return Ok(None);
        };
        let invalid = || Error::Invalid("invalid PR bootstrap cursor".into());
        if value.len() > 4096 {
            return Err(invalid());
        }
        let cursor: Self = serde_json::from_str(value).map_err(|_| invalid())?;
        if cursor.after.len() <= prefix.len()
            || !cursor
                .after
                .as_bytes()
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
        {
            return Err(invalid());
        }
        Ok(Some(cursor))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Watch {
    pub id: String,
    pub repository: String,
    pub pull_number: u64,
    pub interval_seconds: u64,
    #[serde(default)]
    pub kind: WatchKind,
    #[serde(default)]
    pub branches: Vec<String>,
    #[serde(default)]
    pub all_branches: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchKind {
    #[default]
    PullRequests,
    Branches,
    Account,
}

impl Store {
    pub fn open(
        path: &Path,
        retention: std::time::Duration,
        max_events: usize,
        max_snapshot_bytes: usize,
    ) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(storage)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path).map_err(storage)?;
        let mut conn = Connection::open(path).map_err(storage)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage)?;
        // New databases return freed pages incrementally instead of retaining
        // their historical high-water size. Existing databases are converted
        // after the payload migration below.
        conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS cache (
                scope TEXT NOT NULL, key TEXT NOT NULL, response TEXT NOT NULL,
                PRIMARY KEY(scope, key));
            CREATE TABLE IF NOT EXISTS snapshots (
                scope TEXT NOT NULL, resource TEXT NOT NULL, hash TEXT NOT NULL, data TEXT NOT NULL, cursor INTEGER NOT NULL, observed_at_ms INTEGER NOT NULL,
                PRIMARY KEY(scope, resource));
            CREATE TABLE IF NOT EXISTS snapshot_validation (
                scope TEXT NOT NULL, resource TEXT NOT NULL, validated_at_ms INTEGER NOT NULL,
                PRIMARY KEY(scope, resource));
            CREATE TABLE IF NOT EXISTS repository_generation (
                scope TEXT NOT NULL, repository TEXT NOT NULL, generation INTEGER NOT NULL,
                PRIMARY KEY(scope,repository));
            CREATE TABLE IF NOT EXISTS pr_identity (
                scope TEXT NOT NULL, repository TEXT NOT NULL, pull_number INTEGER NOT NULL,
                node_id TEXT NOT NULL, validated_at_ms INTEGER NOT NULL, generation INTEGER NOT NULL,
                PRIMARY KEY(scope,repository,pull_number));
            CREATE TABLE IF NOT EXISTS retired_pr_identity (
                scope TEXT NOT NULL, repository TEXT NOT NULL, pull_number INTEGER NOT NULL, node_id TEXT NOT NULL,
                PRIMARY KEY(scope,repository,pull_number,node_id));
            CREATE TABLE IF NOT EXISTS source_owner (
                scope TEXT NOT NULL, resource TEXT NOT NULL, repository TEXT NOT NULL,
                pull_number INTEGER NOT NULL, node_id TEXT, generation INTEGER NOT NULL,
                PRIMARY KEY(scope,resource));
            CREATE INDEX IF NOT EXISTS cache_repository_case ON cache(scope,key COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS snapshot_pr_case ON snapshots(scope,resource COLLATE NOCASE,cursor DESC);
            CREATE TABLE IF NOT EXISTS discovery_health (
                scope TEXT NOT NULL, resource TEXT NOT NULL,
                generation INTEGER NOT NULL DEFAULT 0,
                completed_generation INTEGER NOT NULL DEFAULT 0,
                success_generation INTEGER NOT NULL DEFAULT 0,
                last_poll_at_ms INTEGER, last_success_at_ms INTEGER,
                last_error TEXT, PRIMARY KEY(scope,resource));
            CREATE TABLE IF NOT EXISTS changes (
                cursor INTEGER PRIMARY KEY AUTOINCREMENT, scope TEXT NOT NULL,
                resource TEXT NOT NULL, observed_at_ms INTEGER NOT NULL, fields TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS changes_scope_cursor ON changes(scope, cursor);
            CREATE INDEX IF NOT EXISTS changes_scope_time ON changes(scope, observed_at_ms);
            CREATE TABLE IF NOT EXISTS feeds (scope TEXT PRIMARY KEY, head INTEGER NOT NULL DEFAULT 0, floor INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS watch_tracking (scope TEXT NOT NULL, watch_id TEXT NOT NULL, kind TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(scope,watch_id,kind));
            CREATE TABLE IF NOT EXISTS watches (
                scope TEXT NOT NULL, id TEXT NOT NULL, value TEXT NOT NULL,
                PRIMARY KEY(scope, id));").map_err(storage)?;
        migrate_current_data(&mut conn)?;
        conn.execute_batch(&format!(
            "CREATE INDEX IF NOT EXISTS snapshot_open_prs ON snapshots(
                scope,resource COLLATE NOCASE,length(CAST(data AS BLOB)),observed_at_ms)
             WHERE {OPEN_PR_SELECTION}"
        ))
        .map_err(storage)?;
        conn.execute_batch(&format!(
            "CREATE INDEX IF NOT EXISTS snapshot_open_prs_compact ON snapshots(
                scope,resource COLLATE NOCASE,length(CAST(data AS BLOB)),observed_at_ms,{})
             WHERE {OPEN_PR_SELECTION}",
            crate::pr_fields::compact_sql()
        ))
        .map_err(storage)?;
        conn.execute(
            "INSERT OR IGNORE INTO metadata(key,value) VALUES('database_id',?1)",
            [digest(&format!("{}-{}", now_ms(), fastrand::u128(..)))],
        )
        .map_err(storage)?;
        // WAL readers can use committed evidence while background publication
        // is still writing. Full feeds can decode hundreds of MB: give them
        // their own bounded reader so compact feeds and lookups can progress.
        // Async locks queue callers without occupying the blocking pool.
        // Anonymous databases cannot open a second connection to the same data.
        let readers = conn
            .path()
            .filter(|path| !path.is_empty())
            .map(|path| {
                let open = || -> Result<_> {
                    let reader = Connection::open_with_flags(
                        path,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                    )
                    .map_err(storage)?;
                    reader
                        .busy_timeout(std::time::Duration::from_secs(5))
                        .map_err(storage)?;
                    Ok(Arc::new(tokio::sync::Mutex::new(reader)))
                };
                Ok(Readers {
                    lookups: open()?,
                    bulk: open()?,
                })
            })
            .transpose()?;
        let checkpoint = conn
            .path()
            .filter(|path| !path.is_empty())
            .map(checkpoint::Checkpointer::open)
            .transpose()?;
        if checkpoint.is_some() {
            // Only checkpoint placement changes. Commits retain their existing
            // FULL WAL sync, and a dedicated connection is ready before the
            // writer's inline checkpoint hook is disabled.
            conn.pragma_update(None, "wal_autocheckpoint", 0)
                .map_err(storage)?;
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(conn)),
            writer_admission: Arc::new(tokio::sync::Semaphore::new(1)),
            checkpoint,
            readers,
            payload_decoders: Arc::new(tokio::sync::Semaphore::new(4)),
            retention,
            max_events,
            max_snapshot_bytes,
        })
    }

    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let started = std::time::Instant::now();
        // This static closure type identifies the method, never captured data.
        let operation = std::any::type_name_of_val(&f);
        let conn = self.connection.clone();
        let admission = self.writer_admission.clone();
        let checkpoint = self.checkpoint.clone();
        // Wait in FIFO order without occupying blocking threads needed by
        // independent WAL reads and decoders. Drive admission independently:
        // a paused/cancelled caller must not reserve and stall the writer turn.
        tokio::spawn(async move {
            let permit = admission.acquire_owned().await.map_err(storage)?;
            let admitted = std::time::Instant::now();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                // Retain poisoning semantics if a writer panics.
                let mut conn = conn.lock().map_err(storage)?;
                let working = std::time::Instant::now();
                let result = f(&mut conn);
                let pending_checkpoint =
                    checkpoint.is_some() && conn.is_autocommit() && checkpoint::pending(&conn);
                let finished = std::time::Instant::now();
                // Diagnostics must not extend ownership of the writer turn.
                drop(conn);
                drop(_permit);
                if pending_checkpoint && let Some(checkpoint) = checkpoint {
                    checkpoint.request();
                }
                if finished.duration_since(started) >= std::time::Duration::from_millis(250) {
                    // Emit from the independently driven write, even when its
                    // original caller stopped waiting. No keys or raw errors.
                    tracing::info!(
                        operation,
                        succeeded = result.is_ok(),
                        error_code = result.as_ref().err().map_or("none", Error::diagnostic_code),
                        elapsed_ms = finished.duration_since(started).as_millis() as u64,
                        queue_ms = admitted.duration_since(started).as_millis() as u64,
                        dispatch_ms = working.duration_since(admitted).as_millis() as u64,
                        work_ms = finished.duration_since(working).as_millis() as u64,
                        "Cache writer operation finished"
                    );
                }
                result
            })
            .await
            .map_err(storage)?
        })
        .await
        .map_err(storage)?
    }

    async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.read_on(self.readers.as_ref().map(|r| &r.lookups), f)
            .await
    }

    async fn read_bulk<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.read_on(self.readers.as_ref().map(|r| &r.bulk), f)
            .await
    }

    async fn read_decode<T: Send + 'static, U: Send + 'static>(
        &self,
        read: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
        decode: impl FnOnce(T) -> Result<U> + Send + 'static,
    ) -> Result<U> {
        let store = self.clone();
        // Bound both raw buffers and CPU work before reading. Admission must
        // progress independently of a hydration caller that pauses its peers.
        tokio::spawn(async move {
            let permit = store
                .payload_decoders
                .clone()
                .acquire_owned()
                .await
                .map_err(storage)?;
            let captured = store.read(read).await?;
            // The transaction, ownership checks and raw payload belong to one
            // snapshot. Decoding that captured value needs no database lock.
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                decode(captured)
            })
            .await
            .map_err(storage)?
        })
        .await
        .map_err(storage)?
    }

    async fn read_on<T: Send + 'static>(
        &self,
        connection: Option<&Arc<tokio::sync::Mutex<Connection>>>,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        // All lookups in one operation share a snapshot, including identity
        // fences, aliases, validation clocks and cursor/payload pairs. Dropping
        // the transaction releases it before the next caller takes its turn.
        let read = move |conn: &mut Connection| {
            let tx = conn.transaction().map_err(storage)?;
            f(&tx)
        };
        if let Some(conn) = connection {
            let conn = conn.clone();
            // Hydration pauses peer futures while processing a completed PR.
            // A paused FIFO waiter must not reserve the reader and prevent
            // that processing from reading. Drive admitted work independently,
            // just as the writer's spawn_blocking work already does.
            tokio::spawn(async move {
                let mut conn = conn.lock_owned().await;
                tokio::task::spawn_blocking(move || read(&mut conn))
                    .await
                    .map_err(storage)?
            })
            .await
            .map_err(storage)?
        } else {
            self.run(read).await
        }
    }

    pub async fn get(&self, scope: &str, key: &str) -> Result<Option<Response>> {
        let (scope, key) = (scope.to_owned(), key.to_owned());
        self.read_decode(
            move |conn| {
                let value: Option<String> = conn
                    .query_row(
                        "SELECT response FROM cache WHERE scope=?1 AND key=?2",
                        params![scope, key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage)?;
                Ok(value)
            },
            |value| {
                value
                    .map(|v| serde_json::from_str(&v).map_err(storage))
                    .transpose()
            },
        )
        .await
    }

    pub async fn snapshot(&self, scope: &str, resource: &str) -> Result<Option<Value>> {
        Ok(self
            .snapshot_with_hash(scope, resource)
            .await?
            .map(|(data, _)| data))
    }

    pub async fn snapshot_with_hash(
        &self,
        scope: &str,
        resource: &str,
    ) -> Result<Option<(Value, String)>> {
        let (scope, resource) = (scope.to_owned(), resource.to_owned());
        self.read_decode(
            move |conn| {
                let resource = resolve_pr_resource(conn, &scope, &resource)?;
                let data: Option<(String, String)> = conn
                    .query_row(
                        "SELECT data,hash FROM snapshots WHERE scope=?1 AND resource=?2",
                        params![scope, resource],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(storage)?;
                Ok(data)
            },
            |data| {
                data.map(|(data, hash)| {
                    serde_json::from_str(&data)
                        .map(|data| (data, hash))
                        .map_err(storage)
                })
                .transpose()
            },
        )
        .await
    }

    pub async fn pr_resource_key(&self, scope: &str, resource: &str) -> Result<String> {
        let (scope, resource) = (scope.to_owned(), resource.to_owned());
        self.read(move |conn| resolve_pr_resource(conn, &scope, &resource))
            .await
    }

    pub async fn get_repository_alias(
        &self,
        scope: &str,
        key: &str,
        prefix: Option<&str>,
    ) -> Result<Option<Response>> {
        if let Some(response) = self.get(scope, key).await? {
            return Ok(Some(response));
        }
        let Some(prefix) = prefix else {
            return Ok(None);
        };
        let Some(suffix) = key.strip_prefix(prefix) else {
            return Ok(None);
        };
        let prefix_length = prefix.chars().count();
        let (scope, key, suffix) = (scope.to_owned(), key.to_owned(), suffix.to_owned());
        self.read_decode(move |conn| {
            // Only the host/repository prefix is case-insensitive. Branches,
            // refs, pagination/query values, and generation suffixes stay exact.
            // Seek the complete key first; a cold miss must not scan every
            // cached URL in the repository while holding the lookup reader.
            let data: Option<String> = conn.query_row(
                "SELECT response FROM cache WHERE scope=?1 AND key=?2 COLLATE NOCASE AND substr(key,?3)=?4 COLLATE BINARY LIMIT 1",
                params![scope,key,prefix_length+1,suffix], |r|r.get(0),
            ).optional().map_err(storage)?;
            Ok(data)
        }, |data| data.map(|data|serde_json::from_str(&data).map_err(storage)).transpose()).await
    }

    pub async fn repository_generation(&self, scope: &str, repository: &str) -> Result<u64> {
        let (scope, repository) = (scope.to_owned(), repository.to_ascii_lowercase());
        self.read(move |conn| repository_generation(conn, &scope, &repository))
            .await
    }

    pub async fn pr_identity(
        &self,
        scope: &str,
        repository: &str,
        number: u64,
    ) -> Result<Option<String>> {
        let (scope, repository) = (scope.to_owned(), repository.to_ascii_lowercase());
        self.read(move |conn|conn.query_row("SELECT node_id FROM pr_identity WHERE scope=?1 AND repository=?2 AND pull_number=?3",params![scope,repository,number],|r|r.get(0)).optional().map_err(storage)).await
    }

    pub async fn accept_rest_identity(
        &self,
        scope: &str,
        repository: &str,
        number: u64,
        node_id: &str,
        clock: u64,
        legacy_resources: &[String],
    ) -> Result<bool> {
        let (scope, repository, node_id, legacy_resources) = (
            scope.to_owned(),
            repository.to_ascii_lowercase(),
            node_id.to_owned(),
            legacy_resources.to_vec(),
        );
        self.run(move |conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(storage)?;
            let exists: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM pr_identity WHERE scope=?1 AND repository=?2 AND pull_number=?3)",params![scope,repository,number],|r|r.get(0)).map_err(storage)?;
            if !exists {
                for resource in legacy_resources {
                    let data:Option<String>=tx.query_row("SELECT data FROM snapshots WHERE scope=?1 AND resource=?2 COLLATE NOCASE",params![scope,resource],|r|r.get(0)).optional().map_err(storage)?;
                    if let Some(data)=data {
                        let value:Value=serde_json::from_str(&data).map_err(storage)?;
                        if let Some(id)=value["pullRequest"]["id"].as_str().or_else(||value["pull_request"]["node_id"].as_str()) {
                            tx.execute("INSERT OR IGNORE INTO pr_identity(scope,repository,pull_number,node_id,validated_at_ms,generation) VALUES(?1,?2,?3,?4,0,0)",params![scope,repository,number,id]).map_err(storage)?;
                            break;
                        }
                    }
                }
            }
            let accepted=accept_identity(&tx,&scope,&repository,number,&node_id,clock,false)?;
            tx.commit().map_err(storage)?;
            Ok(accepted)
        }).await
    }

    pub async fn owner_is_current(&self, scope: &str, owner: &PrOwner) -> Result<bool> {
        let (scope, owner) = (scope.to_owned(), owner.clone());
        self.read(move |conn| owner_is_current(conn, &scope, &owner))
            .await
    }

    pub async fn snapshot_for_pr(
        &self,
        scope: &str,
        resource: &str,
        repository: &str,
        node_id: Option<&str>,
    ) -> Result<Option<Value>> {
        let (scope, resource, repository, node_id) = (
            scope.to_owned(),
            resource.to_owned(),
            repository.to_ascii_lowercase(),
            node_id.map(str::to_owned),
        );
        self.read_decode(
            move |conn| {
                let resource = resolve_pr_resource(conn, &scope, &resource)?;
                let generation = repository_generation(conn, &scope, &repository)?;
                let owner: Option<(Option<String>, u64)> = conn
                .query_row(
                    "SELECT node_id,generation FROM source_owner WHERE scope=?1 AND resource=?2",
                    params![scope, resource],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
                let usable = match owner {
                    Some((id, epoch)) => {
                        epoch == generation
                            && node_id
                                .as_ref()
                                .is_none_or(|expected| id.as_ref() == Some(expected))
                            && (generation == 0 || id.is_some())
                    }
                    None => generation == 0,
                };
                if !usable {
                    return Ok(None);
                }
                let data: Option<String> = conn
                    .query_row(
                        "SELECT data FROM snapshots WHERE scope=?1 AND resource=?2",
                        params![scope, resource],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(storage)?;
                Ok(data)
            },
            |data| {
                data.map(|data| serde_json::from_str(&data).map_err(storage))
                    .transpose()
            },
        )
        .await
    }

    pub async fn validation_clock(&self, scope: &str, resource: &str) -> Result<u64> {
        let (scope, resource) = (scope.to_owned(), resource.to_owned());
        self.read(move |conn| {
            let resource = resolve_pr_resource(conn, &scope, &resource)?;
            conn.query_row(
            // Legacy rows have no validation clock. Their last semantic
            // observation is a conservative barrier until a fresh validation.
            "SELECT COALESCE((SELECT validated_at_ms FROM snapshot_validation WHERE scope=?1 AND resource=?2),(SELECT observed_at_ms FROM snapshots WHERE scope=?1 AND resource=?2),0)",
            params![scope, resource], |row| row.get(0)).map_err(storage)
        }).await
    }

    pub async fn put(&self, scope: &str, resource: &str, response: &Response) -> Result<()> {
        let (scope, resource, response) = (scope.to_owned(), resource.to_owned(), response.clone());
        self.run(move |conn| {
            conn.execute("INSERT INTO cache(scope,key,response) VALUES(?1,?2,?3) ON CONFLICT(scope,key) DO UPDATE SET response=excluded.response",
                params![scope, resource, serde_json::to_string(&response).map_err(storage)?]).map_err(storage)?;
            Ok(())
        }).await
    }

    pub async fn discovery_health(
        &self,
        scope: &str,
        resource: &str,
    ) -> Result<Option<DiscoveryHealth>> {
        let (scope, resource) = (scope.to_owned(), resource.to_owned());
        self.read(move |conn| conn.query_row(
            "SELECT last_poll_at_ms,last_success_at_ms,last_error FROM discovery_health WHERE scope=?1 AND resource=?2",
            params![scope,resource], |row| Ok(DiscoveryHealth {
                last_poll_at_ms: row.get(0)?, last_success_at_ms: row.get(1)?, last_error: row.get(2)?,
            })).optional().map_err(storage)).await
    }

    pub async fn begin_discovery(
        &self,
        scope: &str,
        resource: &str,
        legacy_success_at_ms: Option<u64>,
    ) -> Result<u64> {
        let (scope, resource) = (scope.to_owned(), resource.to_owned());
        self.run(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(storage)?;
            tx.execute("INSERT INTO discovery_health(scope,resource,generation,last_poll_at_ms,last_success_at_ms) VALUES(?1,?2,1,?3,?4) ON CONFLICT(scope,resource) DO UPDATE SET generation=discovery_health.generation+1,last_poll_at_ms=excluded.last_poll_at_ms", params![scope,resource,now_ms(),legacy_success_at_ms]).map_err(storage)?;
            let generation = tx.query_row("SELECT generation FROM discovery_health WHERE scope=?1 AND resource=?2", params![scope,resource], |row| row.get(0)).map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(generation)
        }).await
    }

    /// Commit a last-good collection with recovery health atomically. Attempt
    /// generations prevent late results from independent SDK clients from
    /// undoing a newer failure/recovery or replacing a newer successful scan.
    pub async fn finish_discovery(
        &self,
        scope: &str,
        resource: &str,
        generation: u64,
        collection: Option<Response>,
        error: Option<String>,
    ) -> Result<()> {
        let (scope, resource) = (scope.to_owned(), resource.to_owned());
        self.run(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(storage)?;
            let success_generation: u64 = tx.query_row("SELECT success_generation FROM discovery_health WHERE scope=?1 AND resource=?2", params![scope,resource], |row| row.get(0)).map_err(storage)?;
            if let Some(collection) = collection.filter(|_| generation > success_generation) {
                let previous:Option<String>=tx.query_row("SELECT response FROM cache WHERE scope=?1 AND key=?2",params![scope,resource],|r|r.get(0)).optional().map_err(storage)?;
                if let Some(previous)=previous {
                    let previous:Response=serde_json::from_str(&previous).map_err(storage)?;
                    for node in previous.data["pulls"].as_array().into_iter().flatten() {
                        if let Some((repo,number,id,clock))=discovery_identity(node,&previous.data) {
                            tx.execute("INSERT OR IGNORE INTO pr_identity(scope,repository,pull_number,node_id,validated_at_ms,generation) VALUES(?1,?2,?3,?4,?5,0)",params![scope,repo,number,id,clock]).map_err(storage)?;
                        }
                    }
                }
                for node in collection.data["pulls"].as_array().into_iter().flatten() {
                    if let Some((repo,number,id,clock))=discovery_identity(node,&collection.data) {
                        accept_identity(&tx,&scope,&repo,number,&id,clock,true)?;
                    }
                }
                tx.execute("INSERT INTO cache(scope,key,response) VALUES(?1,?2,?3) ON CONFLICT(scope,key) DO UPDATE SET response=excluded.response", params![scope,resource,serde_json::to_string(&collection).map_err(storage)?]).map_err(storage)?;
                tx.execute("UPDATE discovery_health SET success_generation=?3,last_success_at_ms=?4 WHERE scope=?1 AND resource=?2", params![scope,resource,generation,collection.fetched_at_ms]).map_err(storage)?;
            }
            tx.execute("UPDATE discovery_health SET completed_generation=?3,last_error=?4 WHERE scope=?1 AND resource=?2 AND completed_generation<?3", params![scope,resource,generation,error]).map_err(storage)?;
            tx.commit().map_err(storage)
        }).await
    }

    /// Replace current data and append compact cursor metadata atomically.
    pub async fn observe(&self, scope: &str, resource: &str, value: &Value) -> Result<String> {
        self.observe_many(scope, &[(resource.to_owned(), value.clone())])
            .await
    }

    pub async fn observe_many(
        &self,
        scope: &str,
        observations: &[(String, Value)],
    ) -> Result<String> {
        self.observe_many_validated(scope, observations, &[]).await
    }

    /// Internal validation clocks and semantic replacements commit together.
    /// Clock-only validation never appends an event or advances the cursor.
    pub async fn observe_many_validated(
        &self,
        scope: &str,
        observations: &[(String, Value)],
        clocks: &[(String, u64)],
    ) -> Result<String> {
        self.observe_many_with_owner(scope, observations, clocks, None, None)
            .await
            .map(|(cursor, _)| cursor)
    }

    pub async fn observe_owned(
        &self,
        scope: &str,
        observations: &[(String, Value)],
        owner: &PrOwner,
    ) -> Result<String> {
        self.observe_validated_owned(scope, observations, &[], owner)
            .await
    }

    pub async fn observe_validated_owned(
        &self,
        scope: &str,
        observations: &[(String, Value)],
        clocks: &[(String, u64)],
        owner: &PrOwner,
    ) -> Result<String> {
        self.observe_many_with_owner(scope, observations, clocks, Some(owner.clone()), None)
            .await
            .map(|(cursor, _)| cursor)
    }

    pub async fn replace_validated_status(
        &self,
        scope: &str,
        resource: &str,
        data: &Value,
        clocks: &[(String, u64)],
        expected_hash: &str,
        owner: &PrOwner,
    ) -> Result<bool> {
        self.observe_many_with_owner(
            scope,
            &[(resource.to_owned(), data.clone())],
            clocks,
            Some(owner.clone()),
            Some((resource.to_owned(), expected_hash.to_owned())),
        )
        .await
        .map(|(_, applied)| applied)
    }

    /// Validate the exact previously read body without cloning or decoding it.
    /// A concurrent replacement is left untouched, including its clocks.
    pub async fn revalidate_owned(
        &self,
        scope: &str,
        resource: &str,
        expected_hash: &str,
        clocks: &[(String, u64)],
        owner: &PrOwner,
    ) -> Result<bool> {
        let (scope, resource, expected_hash, clocks, owner) = (
            scope.to_owned(),
            resource.to_owned(),
            expected_hash.to_owned(),
            clocks.to_vec(),
            owner.clone(),
        );
        let cutoff = now_ms().saturating_sub(self.retention.as_millis() as u64);
        let max_events = self.max_events;
        self.run(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(storage)?;
            if !owner_is_current(&tx, &scope, &owner)? {
                return Err(Error::Invalid("PR entity changed while collecting evidence".into()));
            }
            let resource = resolve_pr_resource(&tx, &scope, &resource)?;
            // A separate SDK process can publish after the caller reads its
            // snapshot. Never validate its replacement using our older facts.
            let unchanged: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM snapshots WHERE scope=?1 AND resource=?2 AND hash=?3)",
                params![scope, resource, expected_hash], |r| r.get(0),
            ).map_err(storage)?;
            if !unchanged {
                return Ok(false);
            }
            tx.execute("INSERT INTO source_owner(scope,resource,repository,pull_number,node_id,generation) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scope,resource) DO UPDATE SET repository=excluded.repository,pull_number=excluded.pull_number,node_id=excluded.node_id,generation=excluded.generation",params![scope,resource,owner.repository,owner.number,owner.node_id,owner.generation]).map_err(storage)?;
            for (resource, clock) in clocks {
                let resource = resolve_pr_resource(&tx, &scope, &resource)?;
                tx.execute("INSERT INTO snapshot_validation(scope,resource,validated_at_ms) VALUES(?1,?2,?3) ON CONFLICT(scope,resource) DO UPDATE SET validated_at_ms=MAX(snapshot_validation.validated_at_ms,excluded.validated_at_ms)", params![scope,resource,clock]).map_err(storage)?;
            }
            prune_changes(&tx, &scope, cutoff, max_events)?;
            tx.commit().map_err(storage)?;
            let _ = conn.execute_batch("PRAGMA incremental_vacuum(64);");
            Ok(true)
        }).await
    }

    async fn observe_many_with_owner(
        &self,
        scope: &str,
        observations: &[(String, Value)],
        clocks: &[(String, u64)],
        owner: Option<PrOwner>,
        expected: Option<(String, String)>,
    ) -> Result<(String, bool)> {
        let clocks = clocks.to_vec();
        let (scope, observations) = (scope.to_owned(), observations.to_vec());
        let cutoff = now_ms().saturating_sub(self.retention.as_millis() as u64);
        let max_events = self.max_events;
        self.run(move |conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(storage)?;
            if let Some(owner)=&owner && !owner_is_current(&tx,&scope,owner)? {
                return Err(Error::Invalid("PR entity changed while collecting evidence".into()));
            }
            if let Some((resource, expected_hash)) = expected {
                let resource = resolve_pr_resource(&tx, &scope, &resource)?;
                let hash:Option<String> = tx.query_row("SELECT hash FROM snapshots WHERE scope=?1 AND resource=?2", params![scope,resource], |r|r.get(0)).optional().map_err(storage)?;
                if hash.as_deref().unwrap_or_default() != expected_hash {
                    // A separate client replaced the row used for projection.
                    // Leave its payload, clocks and cursor untouched.
                    let sequence:u64=tx.query_row("SELECT head FROM feeds WHERE scope=?1",[&scope],|r|r.get(0)).optional().map_err(storage)?.unwrap_or(0);
                    return Ok((format!("{}.{}", feed_prefix(&tx, &scope)?, sequence), false));
                }
            }
            for (resource,value) in observations {
            let resource=resolve_pr_resource(&tx,&scope,&resource)?;
            let data=serde_json::to_string(&value).map_err(storage)?;
            let hash=digest(&data);
            // Matching hashes need neither the old body nor its overflow pages.
            // Keep the comparison and changed-body read in the same transaction.
            let old:Option<(String,Option<String>)>=tx.query_row("SELECT hash,CASE WHEN hash=?3 THEN NULL ELSE data END FROM snapshots WHERE scope=?1 AND resource=?2",params![scope,resource,hash],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(storage)?;
            if old.as_ref().map(|(h,_)|h.as_str())!=Some(&hash) {
                let previous:Option<Value>=old.and_then(|(_,data)|data).map(|data|serde_json::from_str(&data).map_err(storage)).transpose()?;
                if resource.starts_with("pr-status://") && previous.as_ref().and_then(|v|v["pullRequest"]["id"].as_str()).zip(value["pullRequest"]["id"].as_str()).is_some_and(|(old,new)|old!=new) {
                    tx.execute("DELETE FROM snapshot_validation WHERE scope=?1 AND resource IN (?2,?3)",params![scope,resource,format!("{resource}#discovery")]).map_err(storage)?;
                }
                let fields:Vec<String>=value.as_object().map(|object|object.iter().filter(|(k,v)|previous.as_ref().and_then(|p|p.get(*k))!=Some(*v)).map(|(k,_)|k.clone()).collect()).unwrap_or_else(||vec!["data".into()]);
                let stamp=now_ms();
                tx.execute("INSERT INTO changes(scope,resource,observed_at_ms,fields) VALUES(?1,?2,?3,?4)",params![scope,resource,stamp,serde_json::to_string(&fields).map_err(storage)?]).map_err(storage)?;
                let sequence=tx.last_insert_rowid();
                tx.execute("INSERT INTO snapshots(scope,resource,hash,data,cursor,observed_at_ms) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scope,resource) DO UPDATE SET hash=excluded.hash,data=excluded.data,cursor=excluded.cursor,observed_at_ms=excluded.observed_at_ms",params![scope,resource,hash,data,sequence,stamp]).map_err(storage)?;
                tx.execute("INSERT INTO feeds(scope,head) VALUES(?1,?2) ON CONFLICT(scope) DO UPDATE SET head=excluded.head",params![scope,sequence]).map_err(storage)?;
            }
            if let Some(owner)=&owner {
                tx.execute("INSERT INTO source_owner(scope,resource,repository,pull_number,node_id,generation) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scope,resource) DO UPDATE SET repository=excluded.repository,pull_number=excluded.pull_number,node_id=excluded.node_id,generation=excluded.generation",params![scope,resource,owner.repository,owner.number,owner.node_id,owner.generation]).map_err(storage)?;
            }
            }
            for (resource, clock) in clocks {
                let resource=resolve_pr_resource(&tx,&scope,&resource)?;
                tx.execute("INSERT INTO snapshot_validation(scope,resource,validated_at_ms) VALUES(?1,?2,?3) ON CONFLICT(scope,resource) DO UPDATE SET validated_at_ms=MAX(snapshot_validation.validated_at_ms,excluded.validated_at_ms)", params![scope,resource,clock]).map_err(storage)?;
            }
            prune_changes(&tx, &scope, cutoff, max_events)?;
            let sequence:u64=tx.query_row("SELECT head FROM feeds WHERE scope=?1",[&scope],|r|r.get(0)).optional().map_err(storage)?.unwrap_or(0);
            let cursor=format!("{}.{}",feed_prefix(&tx,&scope)?,sequence);
            tx.commit().map_err(storage)?;
            // Bound maintenance work per observation; the WAL checkpoint will
            // return these pages to the filesystem without a full vacuum.
            let _ = conn.execute_batch("PRAGMA incremental_vacuum(64);");
            Ok((cursor, true))
        }).await
    }

    /// Validate feed identity and retention without scanning observation bodies.
    pub async fn validate_cursor(&self, scope: &str, cursor: &str) -> Result<()> {
        let (scope, cursor) = (scope.to_owned(), cursor.to_owned());
        self.read(move |conn| cursor_position(conn, &scope, Some(&cursor)).map(|_| ()))
            .await
    }

    pub async fn changes(
        &self,
        scope: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ChangePage> {
        self.changes_prefix(scope, cursor, limit, "").await
    }

    /// Limit counts scanned observations, even when the prefix selects none.
    /// Read selectors and byte lengths before loading bodies so unrelated and
    /// lookahead rows never consume memory or the selected page's byte budget.
    pub async fn changes_prefix(
        &self,
        scope: &str,
        cursor: Option<&str>,
        limit: usize,
        resource_prefix: &str,
    ) -> Result<ChangePage> {
        self.changes_prefix_projected(scope, cursor, limit, resource_prefix, None)
            .await
    }

    pub(crate) async fn changes_prefix_projected(
        &self,
        scope: &str,
        cursor: Option<&str>,
        limit: usize,
        resource_prefix: &str,
        fields: Option<Vec<String>>,
    ) -> Result<ChangePage> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::Invalid("limit must be 1..1000".into()));
        }
        let (scope, cursor, resource_prefix) = (
            scope.to_owned(),
            cursor.map(str::to_owned),
            resource_prefix.to_owned(),
        );
        // A normal page shares the configured collection budget. A single
        // indivisible larger observation can use the bootstrap budget, ensuring
        // forward progress without permitting arbitrarily large allocations.
        let max_bytes = (self.max_snapshot_bytes / 4).max(1);
        let max_event_bytes = self.max_snapshot_bytes;
        self.read_bulk(move |conn| {
            let (prefix, sequence, head) = cursor_position(conn, &scope, cursor.as_deref())?;
            let mut stmt = conn.prepare("SELECT c.cursor,c.resource,c.observed_at_ms,length(CAST(s.data AS BLOB)),length(CAST(c.fields AS BLOB)) FROM changes c LEFT JOIN snapshots s ON s.scope=c.scope AND s.resource=c.resource AND s.cursor=c.cursor WHERE c.scope=?1 AND c.cursor>?2 ORDER BY c.cursor LIMIT ?3").map_err(storage)?;
            let rows = stmt.query_map(params![scope, sequence, limit + 1], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?, r.get::<_, u64>(2)?, r.get::<_, Option<usize>>(3)?, r.get::<_, usize>(4)?))).map_err(storage)?;
            let mut body = conn.prepare("SELECT s.data,c.fields FROM changes c JOIN snapshots s ON s.scope=c.scope AND s.resource=c.resource AND s.cursor=c.cursor WHERE c.scope=?1 AND c.cursor=?2").map_err(storage)?;
            let mut changes = Vec::new();
            let mut position = sequence;
            let mut bytes = 0usize;
            let mut has_more = false;
            for (index, row) in rows.enumerate() {
                let (sequence, resource, observed_at_ms, data_bytes, field_bytes) = row.map_err(storage)?;
                if index == limit {
                    has_more = true;
                    break;
                }
                // Historical payloads do not exist. Advance over their small
                // cursor markers, returning a resource only at its latest
                // observation with that observation's own timestamp/cursor.
                let Some(data_bytes) = data_bytes else {
                    position = sequence;
                    continue;
                };
                // Only PR selectors have GitHub's case-insensitive repository
                // naming. Other prefixes can contain case-sensitive branch refs.
                let matches = if resource_prefix.starts_with("pr-status://") {
                    resource.as_bytes().get(..resource_prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(resource_prefix.as_bytes()))
                } else {
                    resource.starts_with(&resource_prefix)
                };
                if matches {
                    let event_bytes = data_bytes.saturating_add(field_bytes).saturating_add(resource.len());
                    if !changes.is_empty() && bytes.saturating_add(event_bytes) > max_bytes {
                        has_more = true;
                        break;
                    }
                    if event_bytes > max_event_bytes {
                        return Err(Error::Invalid("change observation exceeds configured snapshot byte limit".into()));
                    }
                    let (data, stored_fields): (String, String) = body.query_row(params![scope, sequence], |r| Ok((r.get(0)?, r.get(1)?))).map_err(storage)?;
                    changes.push(Change {
                        cursor: format!("{prefix}.{sequence}"),
                        resource,
                        observed_at_ms,
                        changed_fields: serde_json::from_str(&stored_fields).map_err(storage)?,
                        data: crate::pr_fields::decode_stored(&data, fields.as_deref()).map_err(storage)?,
                    });
                    bytes = bytes.saturating_add(event_bytes);
                }
                // Advance only after decoding an included event or explicitly
                // scanning an excluded one. The byte-boundary row remains next.
                position = sequence;
            }
            Ok(ChangePage {
                changes,
                next_cursor: format!("{prefix}.{position}"),
                head_cursor: format!("{prefix}.{head}"),
                has_more,
            })
        }).await
    }

    /// The state and cursor come from the same SQLite read transaction.
    pub async fn bootstrap(&self, scope: &str) -> Result<SnapshotPage> {
        self.bootstrap_prefix(scope, "").await
    }

    /// Select before decoding: unrelated source bodies must not consume a
    /// scoped feed's byte budget or hold the connection while being parsed.
    pub async fn bootstrap_prefix(&self, scope: &str, prefix: &str) -> Result<SnapshotPage> {
        self.bootstrap_prefix_projected(scope, prefix, None).await
    }

    pub(crate) async fn bootstrap_prefix_projected(
        &self,
        scope: &str,
        prefix: &str,
        fields: Option<Vec<String>>,
    ) -> Result<SnapshotPage> {
        let (scope, prefix) = (scope.to_owned(), prefix.to_owned());
        let upper = format!("{prefix}\u{10ffff}");
        let max_bytes = self.max_snapshot_bytes;
        self.read_bulk(move |conn| {
            let head:u64=conn.query_row("SELECT head FROM feeds WHERE scope=?1",[&scope],|r|r.get(0)).optional().map_err(storage)?.unwrap_or(0);
            let cursor=format!("{}.{}",feed_prefix(conn,&scope)?,head);
            let mut stmt=conn.prepare("SELECT resource,data,observed_at_ms FROM snapshots WHERE scope=?1 AND resource>=?2 AND resource<?3 ORDER BY resource").map_err(storage)?;
            let rows=stmt.query_map(params![scope,prefix,upper],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?))).map_err(storage)?;
            let mut snapshots=Vec::new();
            let mut bytes=0usize;
            for row in rows { let (resource,data,observed_at_ms)=row.map_err(storage)?; bytes=bytes.saturating_add(data.len()); if bytes>max_bytes {return Err(Error::Invalid("bootstrap exceeds configured snapshot byte limit".into()));} snapshots.push(Snapshot {resource,data:crate::pr_fields::decode_stored(&data, fields.as_deref()).map_err(storage)?,observed_at_ms}); }
            Ok(SnapshotPage {snapshots,cursor})
        }).await
    }

    /// Bootstrap only the current open roster. Terminal rows remain in the
    /// snapshot/change feed, but cannot consume an open list's byte budget.
    #[cfg(test)]
    async fn bootstrap_open_prs(
        &self,
        scope: &str,
        prefix: &str,
        repository: Option<&str>,
        fields: Option<Vec<String>>,
    ) -> Result<PrBootstrapPage> {
        self.pr_bootstrap_page(scope, prefix, repository, fields, None)
            .await
    }

    pub(crate) async fn pr_bootstrap_page(
        &self,
        scope: &str,
        prefix: &str,
        repository: Option<&str>,
        fields: Option<Vec<String>>,
        cursor: Option<&str>,
    ) -> Result<PrBootstrapPage> {
        let scope = scope.to_owned();
        let repository = repository.map(str::to_owned);
        let prefix = repository
            .as_ref()
            .map_or_else(|| prefix.to_owned(), |repo| format!("{prefix}{repo}/"));
        let upper = format!("{prefix}\u{10ffff}");
        let position = cursor
            .map(|raw| {
                PrBootstrapCursor::decode(raw, &prefix).and_then(|position| {
                    position.ok_or_else(|| Error::Invalid("expected PR bootstrap cursor".into()))
                })
            })
            .transpose()?;
        let max_bytes = self.max_snapshot_bytes;
        let compact = crate::pr_fields::can_read_compact(fields.as_deref());
        let read = move |conn: &Connection| {
            let head: u64 = conn
                .query_row("SELECT head FROM feeds WHERE scope=?1", [&scope], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(storage)?
                .unwrap_or(0);
            let cursor = format!("{}.{}", feed_prefix(conn, &scope)?, head);
            let (boundary, after) = if let Some(position) = &position {
                cursor_position(conn, &scope, Some(&position.boundary))?;
                (position.boundary.clone(), position.after.as_str())
            } else {
                (cursor.clone(), "")
            };
            // Lifecycle and original byte counts come from the partial index;
            // closed/removed bodies are not scanned to select the open roster.
            // Require that index: without statistics SQLite can prefer the
            // general resource index and read every terminal JSON body again.
            let compact_sql = crate::pr_fields::compact_sql();
            let (index, payload) = if compact {
                ("snapshot_open_prs_compact", compact_sql.as_str())
            } else {
                ("snapshot_open_prs", "data")
            };
            let mut stmt = conn.prepare(&format!("SELECT resource,length(CAST(data AS BLOB)),observed_at_ms FROM snapshots INDEXED BY {index}
                WHERE scope=?1 AND resource>=?2 COLLATE NOCASE AND resource<?3 COLLATE NOCASE
                AND ({OPEN_PR_SELECTION})
                AND (?4 IS NULL OR json_extract(({payload}),'$.pullRequest.repository.nameWithOwner')=?4 COLLATE NOCASE)
                AND resource>?5
                ORDER BY resource LIMIT 1001")).map_err(storage)?;
            let rows = stmt
                .query_map(params![scope, prefix, upper, repository, after], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, usize>(1)?,
                        r.get::<_, u64>(2)?,
                    ))
                })
                .map_err(storage)?;
            // Sort only small metadata, and reject oversized selections before
            // allocating their bodies. Full JSON must not enter SQLite's sorter.
            let body_sql = if compact {
                // Match both the index collation and exact stored spelling.
                // Metadata and payload share one read transaction; the original
                // byte budget is checked before fetching even compact bodies.
                format!(
                    "SELECT {payload} FROM snapshots INDEXED BY {index} WHERE scope=?1 AND resource=?2 COLLATE NOCASE AND resource=?2 AND ({OPEN_PR_SELECTION})"
                )
            } else {
                "SELECT data FROM snapshots WHERE scope=?1 AND resource=?2".to_owned()
            };
            let mut body = conn.prepare(&body_sql).map_err(storage)?;
            let mut snapshots = Vec::new();
            let mut bytes = 0usize;
            let mut more_snapshots = false;
            let mut last = after.to_owned();
            for row in rows {
                let (resource, data_bytes, observed_at_ms) = row.map_err(storage)?;
                if !snapshots.is_empty()
                    && (snapshots.len() == 1000 || bytes.saturating_add(data_bytes) > max_bytes)
                {
                    more_snapshots = true;
                    break;
                }
                if data_bytes > max_bytes {
                    return Err(Error::Invalid(
                        "bootstrap exceeds configured snapshot byte limit".into(),
                    ));
                }
                let data: String = body
                    .query_row(params![scope, resource], |r| r.get(0))
                    .map_err(storage)?;
                bytes = bytes.saturating_add(data_bytes);
                last = resource.clone();
                snapshots.push(Snapshot {
                    resource,
                    data: crate::pr_fields::decode_stored(&data, fields.as_deref())
                        .map_err(storage)?,
                    observed_at_ms,
                });
            }
            let has_more = more_snapshots || cursor != boundary;
            let cursor = if more_snapshots {
                PrBootstrapCursor {
                    boundary,
                    after: last,
                }
                .encode()
            } else {
                boundary
            };
            Ok(PrBootstrapPage {
                snapshots,
                cursor,
                has_more,
            })
        };
        if compact {
            self.read(read).await
        } else {
            self.read_bulk(read).await
        }
    }

    pub async fn save_watch(&self, scope: &str, watch: &Watch) -> Result<()> {
        let (scope, watch) = (scope.to_owned(), watch.clone());
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO watches(scope,id,value) VALUES(?1,?2,?3)
                ON CONFLICT(scope,id) DO UPDATE SET value=excluded.value",
                params![
                    scope,
                    watch.id,
                    serde_json::to_string(&watch).map_err(storage)?
                ],
            )
            .map_err(storage)?;
            Ok(())
        })
        .await
    }

    pub async fn tracking(&self, scope: &str, id: &str, kind: &str) -> Result<Vec<u64>> {
        let (scope, id, kind) = (scope.to_owned(), id.to_owned(), kind.to_owned());
        self.read(move |conn| {
            let data: Option<String> = conn
                .query_row(
                    "SELECT value FROM watch_tracking WHERE scope=?1 AND watch_id=?2 AND kind=?3",
                    params![scope, id, kind],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?;
            data.map(|data| serde_json::from_str(&data).map_err(storage))
                .transpose()
                .map(|v| v.unwrap_or_default())
        })
        .await
    }
    pub async fn save_tracking(
        &self,
        scope: &str,
        id: &str,
        kind: &str,
        numbers: &[u64],
    ) -> Result<()> {
        let (scope, id, kind, numbers) = (
            scope.to_owned(),
            id.to_owned(),
            kind.to_owned(),
            numbers.to_vec(),
        );
        self.run(move |conn| {
            // A removed watcher cannot recreate tracking after deletion.
            conn.execute("INSERT INTO watch_tracking(scope,watch_id,kind,value) SELECT ?1,?2,?3,?4 WHERE EXISTS(SELECT 1 FROM watches WHERE scope=?1 AND id=?2) ON CONFLICT(scope,watch_id,kind) DO UPDATE SET value=excluded.value",params![scope,id,kind,serde_json::to_string(&numbers).map_err(storage)?]).map_err(storage)?;
            Ok(())
        }).await
    }

    pub async fn watches(&self, scope: &str) -> Result<Vec<Watch>> {
        let scope = scope.to_owned();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT value FROM watches WHERE scope=?1 ORDER BY id")
                .map_err(storage)?;
            let rows = stmt
                .query_map([scope], |r| r.get::<_, String>(0))
                .map_err(storage)?;
            rows.map(|r| serde_json::from_str(&r.map_err(storage)?).map_err(storage))
                .collect()
        })
        .await
    }

    pub async fn delete_watch(&self, scope: &str, id: &str) -> Result<()> {
        let (scope, id) = (scope.to_owned(), id.to_owned());
        self.run(move |conn| {
            let tx = conn.transaction().map_err(storage)?;
            tx.execute(
                "DELETE FROM watches WHERE scope=?1 AND id=?2",
                params![scope, id],
            )
            .map_err(storage)?;
            tx.execute(
                "DELETE FROM watch_tracking WHERE scope=?1 AND watch_id=?2",
                params![scope, id],
            )
            .map_err(storage)?;
            tx.commit().map_err(storage)
        })
        .await
    }

    pub async fn revalidated(
        &self,
        scope: &str,
        resource: &str,
        mut response: Response,
    ) -> Result<Response> {
        response.validated_at_ms = now_ms();
        response.source = Source::Revalidated;
        self.put(scope, resource, &response).await?;
        Ok(response)
    }
}

// `snapshots` is the legacy name for the single current value per resource;
// it has never been a version archive. Only `changes` formerly duplicated bodies.
fn migrate_current_data(conn: &mut Connection) -> Result<()> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(storage)?;
    let legacy: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('changes') WHERE name='data')",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if legacy {
        tx.execute_batch(
            "ALTER TABLE changes RENAME TO historical_changes;
            CREATE TABLE changes (
                cursor INTEGER PRIMARY KEY AUTOINCREMENT, scope TEXT NOT NULL,
                resource TEXT NOT NULL, observed_at_ms INTEGER NOT NULL, fields TEXT NOT NULL);
            INSERT INTO changes(cursor,scope,resource,observed_at_ms,fields)
                SELECT cursor,scope,resource,observed_at_ms,fields FROM historical_changes;
            DROP TABLE historical_changes;
            CREATE INDEX changes_scope_cursor ON changes(scope,cursor);
            CREATE INDEX changes_scope_time ON changes(scope,observed_at_ms);
            INSERT OR REPLACE INTO metadata(key,value) VALUES('compact_current_data','pending');",
        )
        .map_err(storage)?;
        // Preserve the high-water sequence even when all markers were pruned.
        tx.execute("UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT MAX(head) FROM feeds),0)) WHERE name='changes'", []).map_err(storage)?;
    }
    let pending: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='compact_current_data')",
            [],
            |r| r.get(0),
        )
        .map_err(storage)?;
    tx.commit().map_err(storage)?;
    if pending {
        conn.execute_batch(
            "PRAGMA auto_vacuum=INCREMENTAL; VACUUM;
            PRAGMA wal_checkpoint(TRUNCATE);
            DELETE FROM metadata WHERE key='compact_current_data';",
        )
        .map_err(storage)?;
    }
    Ok(())
}

fn storage(e: impl std::fmt::Display) -> Error {
    Error::Storage(e.to_string())
}

fn cursor_position(
    conn: &Connection,
    scope: &str,
    cursor: Option<&str>,
) -> Result<(String, u64, u64)> {
    let prefix = feed_prefix(conn, scope)?;
    let sequence = match cursor {
        None => 0,
        Some(cursor) => {
            let (feed, sequence) = cursor
                .rsplit_once('.')
                .ok_or_else(|| Error::Invalid("invalid change cursor".into()))?;
            if feed != prefix {
                return Err(Error::Invalid(
                    "cursor belongs to a different database, API version, or gh credential scope"
                        .into(),
                ));
            }
            sequence
                .parse::<u64>()
                .ok()
                .filter(|c| *c <= i64::MAX as u64)
                .ok_or_else(|| Error::Invalid("invalid change cursor sequence".into()))?
        }
    };
    let (head, floor): (u64, u64) = conn
        .query_row(
            "SELECT head,floor FROM feeds WHERE scope=?1",
            [scope],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(storage)?
        .unwrap_or((0, 0));
    if sequence > head {
        return Err(Error::Invalid("cursor is ahead of this feed".into()));
    }
    if sequence < floor {
        return Err(Error::CursorExpired);
    }
    Ok((prefix, sequence, head))
}

fn feed_prefix(conn: &Connection, scope: &str) -> Result<String> {
    let database: String = conn
        .query_row(
            "SELECT value FROM metadata WHERE key='database_id'",
            [],
            |r| r.get(0),
        )
        .map_err(storage)?;
    Ok(format!("v1.{}", digest(&format!("{database}:{scope}"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unchanged_observation_skips_old_payload_allocation_but_keeps_validation_and_ownership()
    {
        // SQLite's largest-allocation counter is global. Isolate it from
        // unrelated tests instead of using wall-clock or pager-cache counts
        // (direct overflow reads can bypass SQLite's page cache).
        const CHILD: &str = "HEY_GH_UNCHANGED_PAYLOAD_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "store::tests::unchanged_observation_skips_old_payload_allocation_but_keeps_validation_and_ownership", "--test-threads=1", "--nocapture"])
                .env(CHILD, "1").output().await.unwrap();
            let logs = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.status.success(), "{logs}");
            eprintln!("{logs}");
            return;
        }
        fn largest_sqlite_allocation(reset: bool) -> i64 {
            let (mut current, mut peak) = (0, 0);
            // SQLite writes these counters and retains neither pointer.
            let result = unsafe {
                rusqlite::ffi::sqlite3_status64(
                    rusqlite::ffi::SQLITE_STATUS_MALLOC_SIZE,
                    &mut current,
                    &mut peak,
                    i32::from(reset),
                )
            };
            assert_eq!(result, rusqlite::ffi::SQLITE_OK);
            peak
        }
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            16 * 1024 * 1024,
        )
        .unwrap();
        let resource = "ci://github.com/acme/demo/7";
        let alias = "ci://github.com/ACME/DEMO/7";
        let owner = PrOwner {
            repository: "acme/demo".into(),
            number: 7,
            node_id: Some("PR_7".into()),
            generation: 0,
        };
        let value =
            serde_json::json!({"head_sha":"head", "jobs":[{"output":"x".repeat(4 * 1024 * 1024)}]});
        let cursor = store
            .observe_owned("scope", &[(resource.into(), value.clone())], &owner)
            .await
            .unwrap();
        store.run(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA mmap_size=0; PRAGMA cache_size=32; PRAGMA shrink_memory;") .map_err(storage)?;
            Ok(largest_sqlite_allocation(true))
        }).await.unwrap();
        let repeated = store
            .observe_validated_owned(
                "scope",
                &[(alias.into(), value.clone())],
                &[(alias.into(), 42)],
                &owner,
            )
            .await
            .unwrap();
        let allocation = store
            .run(|_| Ok(largest_sqlite_allocation(false)))
            .await
            .unwrap();
        eprintln!("unchanged observation: {allocation} bytes in largest SQLite allocation");
        assert_eq!(repeated, cursor);
        assert_eq!(store.validation_clock("scope", resource).await.unwrap(), 42);
        assert!(
            store
                .changes("scope", Some(&cursor), 100)
                .await
                .unwrap()
                .changes
                .is_empty()
        );
        let stored_owner = store.read(|conn| conn.query_row("SELECT node_id,generation FROM source_owner WHERE scope='scope' AND resource='ci://github.com/acme/demo/7'", [], |r| Ok((r.get::<_,String>(0)?, r.get::<_,u64>(1)?))).map_err(storage)).await.unwrap();
        assert_eq!(stored_owner, ("PR_7".into(), 0));
        assert!(
            allocation > 0 && allocation < 1024 * 1024,
            "unchanged publication allocated {allocation} bytes in SQLite; the old body must stay off the writer connection"
        );

        let mut changed = value;
        changed["head_sha"] = serde_json::json!("new-head");
        let next = store
            .observe_validated_owned(
                "scope",
                &[(alias.into(), changed.clone())],
                &[(resource.into(), 41)],
                &owner,
            )
            .await
            .unwrap();
        assert_ne!(next, cursor);
        assert_eq!(store.validation_clock("scope", resource).await.unwrap(), 42);
        let events = store.changes("scope", Some(&cursor), 100).await.unwrap();
        assert_eq!(events.changes.len(), 1);
        assert_eq!(events.changes[0].changed_fields, ["head_sha"]);
        assert_eq!(
            store.snapshot("scope", resource).await.unwrap(),
            Some(changed)
        );
    }

    #[tokio::test]
    async fn repository_alias_misses_do_not_scan_other_cached_urls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 100, 4096).unwrap();
        store.run(|conn| {
            let tx = conn.transaction().map_err(storage)?;
            let response = serde_json::to_string(&Response {
                data: serde_json::json!({"value":"legacy"}), fetched_at_ms: 1, validated_at_ms: 2,
                source: Source::Cache, etag: None, last_modified: None, link: None,
            }).map_err(storage)?;
            {
                let mut insert = tx.prepare("INSERT INTO cache(scope,key,response) VALUES('scope',?1,?2)").map_err(storage)?;
                for number in 0..4096 {
                    insert.execute(params![format!("https://api.github.com/repos/Acme/Demo/actions/runs/{number}/{}", "x".repeat(128)), response]).map_err(storage)?;
                }
                insert.execute(params!["https://api.github.com/repos/Acme/Demo/branches/Feature", response]).map_err(storage)?;
                insert.execute(params!["completed-jobs://github.com/Acme/Demo/123/1#Version", response]).map_err(storage)?;
            }
            tx.commit().map_err(storage)
        }).await.unwrap();
        drop(store);
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 100, 4096).unwrap();
        for (prefix, suffix, present) in [
            (
                "https://api.github.com/repos/acme/demo/",
                "branches/missing",
                false,
            ),
            (
                "https://api.github.com/repos/acme/demo/",
                "branches/feature",
                false,
            ),
            (
                "https://api.github.com/repos/acme/demo/",
                "branches/Feature",
                true,
            ),
            (
                "completed-jobs://github.com/acme/demo/",
                "123/1#Version",
                true,
            ),
            (
                "completed-jobs://github.com/acme/demo/",
                "123/1#version",
                false,
            ),
        ] {
            store
                .read(|conn| {
                    conn.execute_batch(
                        "PRAGMA mmap_size=0; PRAGMA cache_size=16; PRAGMA shrink_memory",
                    )
                    .map_err(storage)?;
                    Ok(cache_misses(conn, true))
                })
                .await
                .unwrap();
            let found = store
                .get_repository_alias("scope", &format!("{prefix}{suffix}"), Some(prefix))
                .await
                .unwrap();
            let pages = store
                .read(|conn| Ok(cache_misses(conn, false)))
                .await
                .unwrap();
            assert_eq!(
                found.is_some(),
                present,
                "repository casing can change, suffix casing cannot: {suffix}"
            );
            if let Some(response) = found {
                assert_eq!(response.data["value"], "legacy");
                assert_eq!(response.validated_at_ms, 2);
            }
            assert!(pages > 0, "must measure the actual cold reader connection");
            assert!(
                pages < 32,
                "alias lookup loaded {pages} SQLite pages; unrelated repository URLs must not be scanned"
            );
        }
    }

    #[tokio::test]
    async fn bulk_feed_reads_do_not_block_compact_feeds_or_background_lookups() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            4096,
        )
        .unwrap();
        let resource = "pr-status://github.com/acme/repo/7";
        let before = serde_json::json!({"pullRequest":{"number":7,"state":"OPEN","title":"before","complete":false,"sourceErrors":{},"repository":{"nameWithOwner":"acme/repo"}}});
        let old_cursor = store.observe("scope", resource, &before).await.unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let bulk = tokio::spawn({
            let store = store.clone();
            async move {
                store
                    .read_bulk(move |conn| {
                        let (_, _, head) = cursor_position(conn, "scope", None)?;
                        entered.send(()).unwrap();
                        held.recv_timeout(std::time::Duration::from_secs(5))
                            .map_err(storage)?;
                        let data: String = conn
                            .query_row(
                                "SELECT data FROM snapshots WHERE scope='scope' AND resource=?1",
                                [resource],
                                |r| r.get(0),
                            )
                            .map_err(storage)?;
                        Ok((
                            format!("{}.{}", feed_prefix(conn, "scope")?, head),
                            serde_json::from_str::<Value>(&data).map_err(storage)?,
                        ))
                    })
                    .await
            }
        });
        ready.await.unwrap();
        let mut after = before.clone();
        after["pullRequest"]["title"] = serde_json::json!("after");
        let current_cursor = store.observe("scope", resource, &after).await.unwrap();
        let reads = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(
                store.bootstrap_open_prs(
                    "scope",
                    "pr-status://github.com/",
                    None,
                    Some(vec!["number".into(), "title".into()])
                ),
                store.snapshot("scope", resource),
                store.get("scope", "missing"),
                store.watches("scope"),
                store.discovery_health("scope", "discovery")
            )
        })
        .await;
        release.send(()).unwrap();
        let old = bulk.await.unwrap().unwrap();
        let (page, snapshot, cache, watches, health) =
            reads.expect("bulk feed held up compact reads and background lookups");
        let page = page.unwrap();
        assert_eq!(page.cursor, current_cursor);
        assert_eq!(page.snapshots[0].data["pullRequest"]["title"], "after");
        assert_eq!(snapshot.unwrap().unwrap(), after);
        assert!(cache.unwrap().is_none());
        assert!(watches.unwrap().is_empty());
        assert!(health.unwrap().is_none());
        assert_eq!(
            old,
            (old_cursor, before),
            "bulk cursor and payload must retain their original read snapshot"
        );
    }

    #[tokio::test]
    async fn queued_read_progresses_while_its_caller_processes_another_result() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            4096,
        )
        .unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let reader = tokio::spawn({
            let store = store.clone();
            async move {
                store
                    .read(move |_| {
                        entered.send(()).unwrap();
                        held.recv_timeout(std::time::Duration::from_secs(5))
                            .map_err(storage)
                    })
                    .await
            }
        });
        ready.await.unwrap();
        // Account hydration polls several borrowed futures, then processes one
        // completed PR (including cache reads) before polling its peers again.
        let mut queued = Box::pin(store.get("scope", "queued"));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(queued.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        tokio::task::yield_now().await;
        release.send(()).unwrap();
        reader.await.unwrap().unwrap();
        let next = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.get("scope", "next"),
        )
        .await;
        assert!(
            next.is_ok(),
            "queued reader depended on its caller being polled again"
        );
        assert!(next.unwrap().unwrap().is_none());
        assert!(queued.await.unwrap().is_none());
    }

    #[tokio::test]
    async fn cached_reads_remain_available_while_a_background_write_is_uncommitted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            4096,
        )
        .unwrap();
        let start = store.bootstrap("scope").await.unwrap().cursor;
        let resource = "pr-status://github.com/acme/repo/7";
        let row = serde_json::json!({"pullRequest":{"number":7,"state":"OPEN","repository":{"nameWithOwner":"acme/repo"},"complete":false,"sourceErrors":{}}});
        store.observe("scope", resource, &row).await.unwrap();
        let response = Response {
            data: serde_json::json!({"value":"before"}),
            fetched_at_ms: 10,
            validated_at_ms: 10,
            source: Source::Cache,
            etag: None,
            last_modified: None,
            link: None,
        };
        store.put("scope", "cached", &response).await.unwrap();
        let generation = store
            .begin_discovery("scope", "discovery", None)
            .await
            .unwrap();
        store
            .finish_discovery(
                "scope",
                "discovery",
                generation,
                None,
                Some("pending".into()),
            )
            .await
            .unwrap();
        let clock = store.validation_clock("scope", resource).await.unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let writer = tokio::spawn({
            let store = store.clone();
            async move {
                store.run(move |conn| {
                let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(storage)?;
                tx.execute("UPDATE cache SET response=json_set(response,'$.data.value','after') WHERE scope='scope' AND key='cached'", []).map_err(storage)?;
                entered.send(()).unwrap();
                held.recv_timeout(std::time::Duration::from_secs(5)).map_err(storage)?;
                tx.commit().map_err(storage)
            }).await
            }
        });
        ready.await.unwrap();
        let read = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(
                store.get("scope", "cached"),
                store.snapshot("scope", resource),
                store.bootstrap_open_prs(
                    "scope",
                    "pr-status://github.com/",
                    None,
                    Some(vec!["number".into()])
                ),
                store.changes("scope", Some(&start), 100),
                store.discovery_health("scope", "discovery"),
                store.validation_clock("scope", resource)
            )
        })
        .await;
        // Always unblock the writer before asserting, including the failing baseline.
        release.send(()).unwrap();
        writer.await.unwrap().unwrap();
        let (cached, snapshot, page, changes, health, observed_clock) =
            read.expect("cached reads waited behind the background writer");
        let cached = cached.unwrap().unwrap();
        assert_eq!(
            cached.data["value"], "before",
            "uncommitted evidence must not leak"
        );
        assert_eq!(
            cached.validated_at_ms, 10,
            "reading must not refresh evidence"
        );
        assert_eq!(snapshot.unwrap().unwrap(), row);
        assert_eq!(page.unwrap().snapshots[0].data["pullRequest"]["number"], 7);
        assert_eq!(changes.unwrap().changes.len(), 1);
        assert_eq!(
            health.unwrap().unwrap().last_error.as_deref(),
            Some("pending")
        );
        assert_eq!(observed_clock.unwrap(), clock);
        assert_eq!(
            store.get("scope", "cached").await.unwrap().unwrap().data["value"],
            "after"
        );
    }

    #[tokio::test]
    async fn read_transaction_keeps_cursor_and_payload_consistent_during_publication() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            4096,
        )
        .unwrap();
        let before = store
            .observe("scope", "source", &serde_json::json!({"value":"before"}))
            .await
            .unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let reader = tokio::spawn({
            let store = store.clone();
            async move {
                store.read(move |conn| {
                let head: u64 = conn.query_row("SELECT head FROM feeds WHERE scope='scope'", [], |row|row.get(0)).map_err(storage)?;
                entered.send(()).unwrap();
                held.recv_timeout(std::time::Duration::from_secs(5)).map_err(storage)?;
                let data: String = conn.query_row("SELECT data FROM snapshots WHERE scope='scope' AND resource='source'", [], |row|row.get(0)).map_err(storage)?;
                let prefix = feed_prefix(conn, "scope")?;
                Ok((format!("{prefix}.{head}"), serde_json::from_str::<Value>(&data).map_err(storage)?))
            }).await
            }
        });
        ready.await.unwrap();
        let published = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.observe("scope", "source", &serde_json::json!({"value":"after"})),
        )
        .await;
        release.send(()).unwrap();
        let (cursor, data) = reader.await.unwrap().unwrap();
        let after = published
            .expect("read transaction blocked publication")
            .unwrap();
        assert_eq!(cursor, before);
        assert_eq!(
            data["value"], "before",
            "old cursor must not describe a newer payload"
        );
        let current = store.bootstrap("scope").await.unwrap();
        assert_eq!(current.cursor, after);
        assert_ne!(before, after);
        assert_eq!(
            current.snapshots[0].data["value"], "after",
            "the next read must release the old SQLite snapshot"
        );
    }

    #[tokio::test]
    async fn repeated_large_updates_keep_only_current_payload_and_page_over_old_markers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(
            &path,
            std::time::Duration::from_secs(3600),
            100,
            1024 * 1024,
        )
        .unwrap();
        let start = store.bootstrap("account").await.unwrap().cursor;
        for index in 0..40 {
            store
                .observe(
                    "account",
                    "source",
                    &serde_json::json!({"index":index,"body":"x".repeat(64*1024)}),
                )
                .await
                .unwrap();
        }
        let latest = store.bootstrap("account").await.unwrap();
        let mut cursor = start;
        let mut values = Vec::new();
        let mut empty_pages = 0;
        loop {
            let page = store.changes("account", Some(&cursor), 7).await.unwrap();
            if page.changes.is_empty() {
                empty_pages += 1;
            }
            assert_ne!(page.next_cursor, cursor);
            cursor = page.next_cursor;
            values.extend(page.changes);
            if !page.has_more {
                break;
            }
        }
        assert!(empty_pages > 0);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].data["index"], 39);
        assert_eq!(values[0].cursor, latest.cursor);
        assert_eq!(cursor, latest.cursor);
        assert_eq!(latest.snapshots.len(), 1);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        assert!(
            std::fs::metadata(&path).unwrap().len() < 512 * 1024,
            "forty 64-KiB updates must not store forty bodies"
        );
        drop(store);
        let reopened = Store::open(
            &path,
            std::time::Duration::from_secs(3600),
            100,
            1024 * 1024,
        )
        .unwrap();
        assert_eq!(reopened.bootstrap("account").await.unwrap().cursor, cursor);
        assert!(
            reopened
                .changes("account", Some(&cursor), 100)
                .await
                .unwrap()
                .changes
                .is_empty()
        );
    }

    #[tokio::test]
    async fn legacy_migration_removes_history_compacts_and_preserves_current_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let open = || {
            Store::open(
                &path,
                std::time::Duration::from_secs(3600),
                100,
                1024 * 1024,
            )
            .unwrap()
        };
        let store = open();
        let start = store.bootstrap("account").await.unwrap().cursor;
        for index in 0..30 {
            store
                .observe(
                    "account",
                    "source",
                    &serde_json::json!({"index":index,"body":"x".repeat(32*1024)}),
                )
                .await
                .unwrap();
        }
        store
            .observe("other", "source", &serde_json::json!({"private":true}))
            .await
            .unwrap();
        let current = store.bootstrap("account").await.unwrap();
        let watch = Watch {
            id: "watch".into(),
            repository: "acme/repo".into(),
            pull_number: 7,
            interval_seconds: 60,
            kind: WatchKind::PullRequests,
            branches: vec![],
            all_branches: false,
        };
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO watches(scope,id,value) VALUES('account','watch',?1)",
            [serde_json::to_string(&watch).unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cache(scope,key,response) VALUES('account','cache-key','preserve-me')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO snapshot_validation VALUES('account','source',1234)",
            [],
        )
        .unwrap();
        drop(store);
        // Construct the previous on-disk format, including a high-water file
        // size left behind by pruning and duplicate historical payloads.
        conn.execute_batch("PRAGMA auto_vacuum=NONE; VACUUM;
            ALTER TABLE changes ADD COLUMN data TEXT NOT NULL DEFAULT '';
            UPDATE changes SET data=(SELECT data FROM snapshots s WHERE s.scope=changes.scope AND s.resource=changes.resource);
            CREATE TABLE old_allocation(data BLOB);
            INSERT INTO old_allocation VALUES(zeroblob(4194304));
            DROP TABLE old_allocation;
            PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        let before = std::fs::metadata(&path).unwrap().len();
        drop(conn);
        let migrated = open();
        let after = std::fs::metadata(&path).unwrap().len();
        assert!(
            after < before / 4,
            "migration must physically return historical/free pages: {before} -> {after}"
        );
        assert_eq!(
            serde_json::to_value(migrated.bootstrap("account").await.unwrap()).unwrap(),
            serde_json::to_value(&current).unwrap()
        );
        assert_eq!(migrated.watches("account").await.unwrap()[0].id, "watch");
        assert_eq!(
            migrated
                .validation_clock("account", "source")
                .await
                .unwrap(),
            1234
        );
        let page = migrated
            .changes("account", Some(&start), 100)
            .await
            .unwrap();
        assert_eq!(page.changes.len(), 1);
        assert_eq!(page.changes[0].data["index"], 29);
        assert_eq!(page.next_cursor, current.cursor);
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT response FROM cache WHERE scope='account'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "preserve-me"
        );
        assert_eq!(
            conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            conn.query_row("PRAGMA auto_vacuum", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            2
        );
        assert!(
            !conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('changes') WHERE name='data')",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
        let next = migrated
            .observe("account", "source", &serde_json::json!({"index":30}))
            .await
            .unwrap();
        assert_ne!(next, current.cursor);
        assert_eq!(
            migrated
                .changes("account", Some(&current.cursor), 100)
                .await
                .unwrap()
                .changes
                .len(),
            1
        );
        drop(migrated);
        assert_eq!(open().bootstrap("account").await.unwrap().cursor, next);
    }

    #[tokio::test]
    async fn legacy_migration_preserves_sequence_when_the_journal_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let open = || Store::open(&path, std::time::Duration::from_secs(3600), 100, 4096).unwrap();
        let store = open();
        let head = store
            .observe("account", "source", &serde_json::json!({"v":1}))
            .await
            .unwrap();
        drop(store);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "ALTER TABLE changes ADD COLUMN data TEXT NOT NULL DEFAULT ''; DELETE FROM changes;",
        )
        .unwrap();
        drop(conn);
        let store = open();
        store
            .observe("account", "source", &serde_json::json!({"v":2}))
            .await
            .unwrap();
        assert_eq!(
            store
                .changes("account", Some(&head), 100)
                .await
                .unwrap()
                .changes
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn cursor_validation_checks_identity_bounds_and_retention_without_decoding_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 100, 256).unwrap();
        let start = store.bootstrap("account").await.unwrap().cursor;
        store.validate_cursor("account", &start).await.unwrap();
        let head = store
            .observe(
                "account",
                "source",
                &serde_json::json!({"body":"x".repeat(2048)}),
            )
            .await
            .unwrap();
        store.validate_cursor("account", &start).await.unwrap();
        assert!(matches!(
            store.changes("account", Some(&start), 100).await,
            Err(Error::Invalid(_))
        ));
        let conn = Connection::open(&path).unwrap();
        conn.execute("UPDATE snapshots SET data='broken'", [])
            .unwrap();
        store.validate_cursor("account", &start).await.unwrap();
        assert!(matches!(
            store.changes("account", Some(&start), 100).await,
            Err(Error::Storage(_))
        ));
        assert!(matches!(
            store.validate_cursor("other-account", &start).await,
            Err(Error::Invalid(_))
        ));
        let prefix = head.rsplit_once('.').unwrap().0;
        for cursor in [
            "invalid".to_owned(),
            format!("{prefix}.2"),
            format!("{prefix}.-1"),
            format!("{prefix}.9223372036854775808"),
        ] {
            assert!(matches!(
                store.validate_cursor("account", &cursor).await,
                Err(Error::Invalid(_))
            ));
        }
        let other = Store::open(
            &dir.path().join("other.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            256,
        )
        .unwrap();
        assert!(matches!(
            other.validate_cursor("account", &start).await,
            Err(Error::Invalid(_))
        ));
        conn.execute("UPDATE feeds SET floor=head WHERE scope='account'", [])
            .unwrap();
        assert!(matches!(
            store.validate_cursor("account", &start).await,
            Err(Error::CursorExpired)
        ));
        assert!(matches!(
            store.changes("account", Some(&start), 100).await,
            Err(Error::CursorExpired)
        ));
        store.validate_cursor("account", &head).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "manual benchmark for unchanged large PR publication"]
    async fn unchanged_publication_benchmark() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            8 * 1024 * 1024,
        )
        .unwrap();
        let resource = "pr-status://github.com/acme/repo/7";
        let owner = PrOwner {
            repository: "acme/repo".into(),
            number: 7,
            node_id: Some("PR_7".into()),
            generation: 0,
        };
        let data = serde_json::json!({"pullRequest":{"id":"PR_7","ci":{"jobs":(0..1024).map(|id|serde_json::json!({"id":id,"name":"x".repeat(1024),"state":"completed"})).collect::<Vec<_>>()}}});
        let observations = [(resource.to_owned(), data.clone())];
        let cursor = store
            .observe_owned("scope", &observations, &owner)
            .await
            .unwrap();
        let (_, hash) = store
            .snapshot_with_hash("scope", resource)
            .await
            .unwrap()
            .unwrap();
        let start = std::time::Instant::now();
        for clock in 1..=64 {
            store
                .observe_validated_owned(
                    "scope",
                    &observations,
                    &[(resource.into(), clock)],
                    &owner,
                )
                .await
                .unwrap();
        }
        let full = start.elapsed();
        let start = std::time::Instant::now();
        for clock in 65..=128 {
            assert!(
                store
                    .revalidate_owned(
                        "scope",
                        resource,
                        &hash,
                        &[(resource.into(), clock)],
                        &owner
                    )
                    .await
                    .unwrap()
            );
        }
        let metadata = start.elapsed();
        assert_eq!(store.bootstrap("scope").await.unwrap().cursor, cursor);
        assert_eq!(
            store.validation_clock("scope", resource).await.unwrap(),
            128
        );
        eprintln!(
            "64 unchanged publications, {} bytes: full={full:?}, metadata={metadata:?}",
            serde_json::to_vec(&data).unwrap().len()
        );
    }

    #[tokio::test]
    async fn unchanged_validation_is_bound_to_the_observed_version_and_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let open = || {
            Store::open(
                &path,
                std::time::Duration::from_secs(3600),
                100,
                8 * 1024 * 1024,
            )
            .unwrap()
        };
        let store = open();
        let other = open();
        let resource = "pr-status://github.com/ACME/Repo/7";
        let alias = "pr-status://github.com/acme/repo/7";
        let owner = PrOwner {
            repository: "acme/repo".into(),
            number: 7,
            node_id: Some("PR_7".into()),
            generation: 0,
        };
        store
            .accept_rest_identity("scope", "acme/repo", 7, "PR_7", 100, &[])
            .await
            .unwrap();
        let old =
            serde_json::json!({"pullRequest":{"id":"PR_7","ci":{"jobs":"x".repeat(1024 * 1024)}}});
        let cursor = store
            .observe_validated_owned(
                "scope",
                &[(resource.into(), old.clone())],
                &[(resource.into(), 100)],
                &owner,
            )
            .await
            .unwrap();
        let (_, hash) = store
            .snapshot_with_hash("scope", alias)
            .await
            .unwrap()
            .unwrap();
        let clocks = [(alias.into(), 200), (format!("{alias}#discovery"), 150)];
        assert!(
            store
                .revalidate_owned("scope", alias, &hash, &clocks, &owner)
                .await
                .unwrap()
        );
        assert_eq!(store.snapshot("scope", resource).await.unwrap(), Some(old));
        assert_eq!(
            store.validation_clock("scope", resource).await.unwrap(),
            200
        );
        assert_eq!(
            store
                .validation_clock("scope", &format!("{resource}#discovery"))
                .await
                .unwrap(),
            150
        );
        assert_eq!(store.bootstrap("scope").await.unwrap().cursor, cursor);
        assert!(
            store
                .changes("scope", Some(&cursor), 100)
                .await
                .unwrap()
                .changes
                .is_empty()
        );
        assert!(
            store
                .revalidate_owned("scope", alias, &hash, &[(alias.into(), 150)], &owner)
                .await
                .unwrap()
        );
        assert_eq!(
            store.validation_clock("scope", resource).await.unwrap(),
            200
        );

        // Independent clients do not share the daemon's report mutex. A late
        // unchanged observation must not validate or replace their new value.
        let newer = serde_json::json!({"pullRequest":{"id":"PR_7","ci":{"state":"failure"}}});
        let next = other
            .observe_validated_owned(
                "scope",
                &[(resource.into(), newer.clone())],
                &[(resource.into(), 300)],
                &owner,
            )
            .await
            .unwrap();
        assert!(
            !store
                .revalidate_owned("scope", alias, &hash, &[(alias.into(), 400)], &owner)
                .await
                .unwrap()
        );
        assert_eq!(
            store.snapshot("scope", resource).await.unwrap(),
            Some(newer)
        );
        assert_eq!(
            store.validation_clock("scope", resource).await.unwrap(),
            300
        );
        assert_eq!(store.bootstrap("scope").await.unwrap().cursor, next);
        assert!(
            !store
                .revalidate_owned("other-scope", alias, &hash, &clocks, &owner)
                .await
                .unwrap()
        );
        let (_, current_hash) = store
            .snapshot_with_hash("scope", alias)
            .await
            .unwrap()
            .unwrap();
        let stale_owner = PrOwner {
            generation: 1,
            ..owner
        };
        assert!(
            store
                .revalidate_owned("scope", alias, &current_hash, &clocks, &stale_owner)
                .await
                .is_err()
        );
        assert_eq!(
            store.validation_clock("scope", resource).await.unwrap(),
            300
        );
    }

    #[tokio::test]
    async fn legacy_repository_case_aliases_preserve_resource_identity_and_entity_fences() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            4096,
        )
        .unwrap();
        let resource = "comments://github.com/ACME/Repo/7";
        let owner = PrOwner {
            repository: "acme/repo".into(),
            number: 7,
            node_id: Some("old".into()),
            generation: 0,
        };
        store
            .accept_rest_identity("scope", "acme/repo", 7, "old", 100, &[])
            .await
            .unwrap();
        let payload = serde_json::json!({"comments":[{"id":1}]});
        let cursor = store
            .observe_owned("scope", &[(resource.into(), payload.clone())], &owner)
            .await
            .unwrap();
        assert!(
            store
                .snapshot_for_pr(
                    "scope",
                    "comments://github.com/acme/repo/7",
                    "acme/repo",
                    Some("old")
                )
                .await
                .unwrap()
                .is_some()
        );
        store
            .observe_owned(
                "scope",
                &[("comments://github.com/acme/repo/7".into(), payload)],
                &owner,
            )
            .await
            .unwrap();
        let page = store.bootstrap("scope").await.unwrap();
        assert_eq!(page.cursor, cursor);
        assert_eq!(page.snapshots.len(), 1);
        assert_eq!(page.snapshots[0].resource, resource);

        let prefix = "https://api.github.com/repos/acme/repo/";
        let key = "https://api.github.com/repos/ACME/Repo/branches/Feature?ref=Feature";
        let response = Response {
            data: serde_json::json!({"name":"Feature"}),
            fetched_at_ms: 100,
            validated_at_ms: 100,
            source: Source::Cache,
            etag: None,
            last_modified: None,
            link: None,
        };
        store.put("scope", key, &response).await.unwrap();
        assert!(
            store
                .get_repository_alias(
                    "scope",
                    &format!("{prefix}branches/Feature?ref=Feature"),
                    Some(prefix)
                )
                .await
                .unwrap()
                .is_some()
        );
        for (scope, suffix) in [
            ("scope", "branches/feature?ref=Feature"),
            ("scope", "branches/Feature?ref=feature"),
            (
                "scope",
                "branches/Feature?ref=Feature#repository-generation=1",
            ),
            ("other-scope", "branches/Feature?ref=Feature"),
        ] {
            assert!(
                store
                    .get_repository_alias(scope, &format!("{prefix}{suffix}"), Some(prefix))
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        store
            .accept_rest_identity("scope", "acme/repo", 7, "new", 200, &[])
            .await
            .unwrap();
        assert!(
            store
                .snapshot_for_pr(
                    "scope",
                    "comments://github.com/acme/repo/7",
                    "acme/repo",
                    Some("new")
                )
                .await
                .unwrap()
                .is_none()
        );
        for branch in ["Feature", "feature"] {
            store
                .observe(
                    "scope",
                    &format!("branch://github.com/acme/repo/{branch}"),
                    &serde_json::json!({"name":branch}),
                )
                .await
                .unwrap();
        }
        assert_eq!(store.bootstrap("scope").await.unwrap().snapshots.len(), 3);
    }

    #[tokio::test]
    async fn status_replacement_requires_the_exact_snapshot_even_with_current_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 100, 4096).unwrap();
        let peer = Store::open(&path, std::time::Duration::from_secs(3600), 100, 4096).unwrap();
        let resource = "pr-status://github.com/Acme/Repo/7";
        let alias = "pr-status://github.com/acme/repo/7";
        let old = serde_json::json!({"pullRequest":{"id":"old","state":"OPEN"}});
        store.observe("scope", resource, &old).await.unwrap();
        store
            .accept_rest_identity("scope", "acme/repo", 7, "new", 200, &[])
            .await
            .unwrap();
        let owner = PrOwner {
            repository: "acme/repo".into(),
            number: 7,
            node_id: Some("new".into()),
            generation: 1,
        };
        // Before discovery publishes the replacement, an unresolved old row
        // may still be retired by a collector holding the new selector owner.
        let retired =
            serde_json::json!({"pullRequest":{"id":"old","state":"UNKNOWN","removed":true}});
        store
            .observe_owned("scope", &[(resource.into(), retired.clone())], &owner)
            .await
            .unwrap();
        let (_, retired_hash) = store
            .snapshot_with_hash("scope", resource)
            .await
            .unwrap()
            .unwrap();
        let current = serde_json::json!({"pullRequest":{"id":"new","state":"OPEN"}});
        let clocks = [(alias.into(), 200)];
        let before = peer
            .observe_validated_owned("scope", &[(alias.into(), current.clone())], &clocks, &owner)
            .await
            .unwrap();
        // A second client can finish its old projection after the replacement.
        // Valid ownership does not make that retired payload current.
        assert!(
            !store
                .replace_validated_status(
                    "scope",
                    resource,
                    &retired,
                    &[(resource.into(), 300)],
                    &retired_hash,
                    &owner
                )
                .await
                .unwrap()
        );
        assert_eq!(
            store.snapshot("scope", alias).await.unwrap(),
            Some(current.clone())
        );
        assert_eq!(store.validation_clock("scope", alias).await.unwrap(), 200);
        assert!(
            store
                .changes("scope", Some(&before), 100)
                .await
                .unwrap()
                .changes
                .is_empty()
        );
        // The same identity can also race: an early lifecycle projection must
        // not erase CI already attached by another client.
        let (_, hash) = store
            .snapshot_with_hash("scope", resource)
            .await
            .unwrap()
            .unwrap();
        let completed = serde_json::json!({"pullRequest":{"id":"new","state":"OPEN","ci":{"summary":"success"}}});
        assert!(
            peer.replace_validated_status(
                "scope",
                alias,
                &completed,
                &[(alias.into(), 300)],
                &hash,
                &owner
            )
            .await
            .unwrap()
        );
        let before = peer.bootstrap("scope").await.unwrap().cursor;
        assert!(
            !store
                .replace_validated_status(
                    "scope",
                    resource,
                    &current,
                    &[(resource.into(), 400)],
                    &hash,
                    &owner
                )
                .await
                .unwrap()
        );
        assert_eq!(
            store.snapshot("scope", alias).await.unwrap(),
            Some(completed)
        );
        assert_eq!(store.validation_clock("scope", alias).await.unwrap(), 300);
        assert_eq!(store.bootstrap("scope").await.unwrap().cursor, before);
    }

    #[tokio::test]
    async fn entity_fences_survive_restart_and_reject_retired_work_without_cursor_churn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let open = || Store::open(&path, std::time::Duration::from_secs(3600), 100, 4096).unwrap();
        let a = open();
        let b = open();
        for scope in ["account", "other-credential"] {
            assert!(
                a.accept_rest_identity(scope, "ACME/Repo", 7, "old", 100, &[])
                    .await
                    .unwrap()
            );
        }
        let old = PrOwner {
            repository: "acme/repo".into(),
            number: 7,
            node_id: Some("old".into()),
            generation: 0,
        };
        let source = [(
            "comments://github.com/acme/repo/7".into(),
            serde_json::json!({"comments":[{"id":1}]}),
        )];
        a.observe_owned("account", &source, &old).await.unwrap();
        a.observe_owned("other-credential", &source, &old)
            .await
            .unwrap();
        let before = a.bootstrap("account").await.unwrap().cursor;
        assert!(
            b.accept_rest_identity("account", "acme/repo", 7, "new", 200, &[])
                .await
                .unwrap()
        );
        assert_eq!(
            a.repository_generation("account", "ACME/Repo")
                .await
                .unwrap(),
            1
        );
        assert!(
            !a.accept_rest_identity("account", "acme/repo", 7, "old", 300, &[])
                .await
                .unwrap()
        );
        assert!(a.observe_owned("account", &source, &old).await.is_err());
        assert!(
            open()
                .snapshot_for_pr("account", &source[0].0, "ACME/Repo", Some("new"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            b.snapshot_for_pr("other-credential", &source[0].0, "acme/repo", Some("old"))
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(b.bootstrap("account").await.unwrap().cursor, before);
        let current = PrOwner {
            node_id: Some("new".into()),
            generation: 1,
            ..old
        };
        // Equal content freshly collected for a new node must bind ownership,
        // while internal generation changes alone do not create feed events.
        b.observe_owned("account", &source, &current).await.unwrap();
        assert!(
            open()
                .snapshot_for_pr("account", &source[0].0, "acme/repo", Some("new"))
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(a.bootstrap("account").await.unwrap().cursor, before);
    }

    #[tokio::test]
    async fn legacy_identity_migration_and_older_graph_pages_cannot_lend_retired_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 100, 4096).unwrap();
        let source = "comments://github.com/ACME/Repo/7";
        let metadata = "metadata://github.com/ACME/Repo/7";
        store
            .observe("scope", source, &serde_json::json!({"comments":[{"id":1}]}))
            .await
            .unwrap();
        store
            .observe(
                "scope",
                metadata,
                &serde_json::json!({"pull_request":{"node_id":"old"}}),
            )
            .await
            .unwrap();
        assert!(
            store
                .accept_rest_identity(
                    "scope",
                    "acme/repo",
                    7,
                    "new",
                    200,
                    &[metadata.to_lowercase()]
                )
                .await
                .unwrap()
        );
        assert_eq!(
            store
                .repository_generation("scope", "acme/repo")
                .await
                .unwrap(),
            1
        );
        assert!(
            store
                .snapshot_for_pr("scope", source, "acme/repo", Some("new"))
                .await
                .unwrap()
                .is_none()
        );
        let db = Connection::open(&path).unwrap();
        assert!(!accept_identity(&db, "scope", "acme/repo", 7, "old", 100, true).unwrap());
        assert_eq!(
            store
                .pr_identity("scope", "acme/repo", 7)
                .await
                .unwrap()
                .as_deref(),
            Some("new")
        );
        // One changed selector fences the repository. Older pages for another
        // selector must not endorse its old generation merely by matching ID.
        accept_identity(&db, "scope", "acme/repo", 8, "peer", 250, true).unwrap();
        accept_identity(&db, "scope", "acme/repo", 7, "newer", 300, true).unwrap();
        assert!(!accept_identity(&db, "scope", "acme/repo", 8, "peer", 240, true).unwrap());
        assert!(
            !store
                .accept_rest_identity("scope", "acme/repo", 8, "peer", 400, &[])
                .await
                .unwrap()
        );
        assert!(accept_identity(&db, "scope", "acme/repo", 8, "peer-new", 400, true).unwrap());
        assert_eq!(
            store
                .repository_generation("scope", "acme/repo")
                .await
                .unwrap(),
            2
        );
        // Migration through an account scan also fences a legacy detailed
        // snapshot when no previous account collection exists. Use the PR's
        // own page clock, not the minimum clock from another page.
        store
            .observe(
                "scope",
                "metadata://github.com/ACME/Graph/7",
                &serde_json::json!({"pull_request":{"node_id":"graph-old"}}),
            )
            .await
            .unwrap();
        let scan = store
            .begin_discovery("scope", "discovery", None)
            .await
            .unwrap();
        store.finish_discovery("scope", "discovery", scan, Some(Response {
            data: serde_json::json!({"pulls":[{"id":"graph-new","number":7,"repository":{"nameWithOwner":"ACME/Graph"}}],"validatedAtMs":100,"validatedAtByPr":{"acme/graph/7":300}}),
            fetched_at_ms: 100, validated_at_ms: 100, source: Source::Cache, etag: None, last_modified: None, link: None,
        }), None).await.unwrap();
        assert_eq!(
            store
                .repository_generation("scope", "acme/graph")
                .await
                .unwrap(),
            1
        );
        assert!(
            !store
                .accept_rest_identity("scope", "acme/graph", 7, "too-old", 200, &[])
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn discovery_recovery_is_atomic_and_late_clients_cannot_undo_newer_health_or_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let a = Store::open(&path, std::time::Duration::from_secs(3600), 100, 4096).unwrap();
        let b = Store::open(&path, std::time::Duration::from_secs(3600), 100, 4096).unwrap();
        let key = "discovery";
        let cursor = a.bootstrap("account").await.unwrap().cursor;
        let older = a.begin_discovery("account", key, Some(50)).await.unwrap();
        let newer = b.begin_discovery("account", key, None).await.unwrap();
        assert!(newer > older);
        b.finish_discovery(
            "account",
            key,
            newer,
            None,
            Some("permission denied".into()),
        )
        .await
        .unwrap();
        let response = |data: &str, stamp| Response {
            data: serde_json::json!({"version": data}),
            fetched_at_ms: stamp,
            validated_at_ms: stamp,
            source: Source::Cache,
            etag: None,
            last_modified: None,
            link: None,
        };
        a.finish_discovery(
            "account",
            key,
            older,
            Some(response("last good", 100)),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            a.discovery_health("account", key)
                .await
                .unwrap()
                .unwrap()
                .last_error
                .as_deref(),
            Some("permission denied")
        );
        assert_eq!(
            b.get("account", key).await.unwrap().unwrap().data["version"],
            "last good"
        );
        let recovered = a.begin_discovery("account", key, None).await.unwrap();
        // Fail after the cache replacement but before health commit. Neither
        // cache nor recovery may escape the rolled-back transaction.
        let db = Connection::open(&path).unwrap();
        db.execute_batch(&format!("CREATE TRIGGER reject_recovery BEFORE UPDATE ON discovery_health WHEN NEW.completed_generation={recovered} BEGIN SELECT RAISE(ABORT,'injected failure'); END;")).unwrap();
        assert!(
            a.finish_discovery(
                "account",
                key,
                recovered,
                Some(response("recovered", 200)),
                None
            )
            .await
            .is_err()
        );
        assert_eq!(
            b.get("account", key).await.unwrap().unwrap().data["version"],
            "last good"
        );
        assert_eq!(
            b.discovery_health("account", key)
                .await
                .unwrap()
                .unwrap()
                .last_error
                .as_deref(),
            Some("permission denied")
        );
        db.execute_batch("DROP TRIGGER reject_recovery").unwrap();
        a.finish_discovery(
            "account",
            key,
            recovered,
            Some(response("recovered", 200)),
            None,
        )
        .await
        .unwrap();
        b.finish_discovery("account", key, newer, None, Some("late failure".into()))
            .await
            .unwrap();
        b.finish_discovery(
            "account",
            key,
            older,
            Some(response("late old cache", 300)),
            None,
        )
        .await
        .unwrap();
        let health = b.discovery_health("account", key).await.unwrap().unwrap();
        assert!(health.last_error.is_none());
        assert_eq!(health.last_success_at_ms, Some(200));
        assert_eq!(
            b.get("account", key).await.unwrap().unwrap().data["version"],
            "recovered"
        );
        assert_eq!(b.bootstrap("account").await.unwrap().cursor, cursor);
        assert!(
            b.discovery_health("other account", key)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn change_pages_bound_bytes_without_skipping_current_resources() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            1024,
        )
        .unwrap();
        let mut cursor = store.bootstrap("account").await.unwrap().cursor;
        let mut expected = Vec::new();
        for (index, size) in [50, 50, 600, 50, 50].into_iter().enumerate() {
            expected.push(
                store
                    .observe(
                        "account",
                        &format!("source{index}"),
                        &serde_json::json!({"index":index,"body":"x".repeat(size)}),
                    )
                    .await
                    .unwrap(),
            );
        }
        let mut actual = Vec::new();
        let mut sizes = Vec::new();
        for _ in 0..10 {
            let page = store.changes("account", Some(&cursor), 1000).await.unwrap();
            let bytes: usize = page
                .changes
                .iter()
                .map(|change| change.data.to_string().len())
                .sum();
            assert!(
                bytes <= 256 || page.changes.len() == 1,
                "a page must honor the byte budget, with one indivisible larger event allowed"
            );
            assert!(!page.changes.is_empty());
            sizes.push(page.changes.len());
            assert_ne!(page.next_cursor, cursor);
            actual.extend(page.changes.iter().map(|change| change.cursor.clone()));
            cursor = page.next_cursor;
            if !page.has_more {
                break;
            }
        }
        assert_eq!(sizes, [2, 1, 2]);
        assert_eq!(
            actual, expected,
            "byte boundaries must not skip or duplicate observations"
        );
        let empty = store.changes("account", Some(&cursor), 1000).await.unwrap();
        assert!(empty.changes.is_empty());
        assert!(!empty.has_more);
    }

    #[tokio::test]
    async fn projected_pr_reads_keep_stored_byte_boundaries_health_and_activity() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            8192,
        )
        .unwrap();
        let mut cursor = store.bootstrap("account").await.unwrap().cursor;
        let fields = Some(vec!["number".to_owned()]);
        for (number, state) in [(1, "OPEN"), (2, "CLOSED"), (3, "MERGED")] {
            store.observe("account", &format!("pr-status://github.com/acme/demo/{number}"),
                &serde_json::json!({"pullRequest":{"number":number,"repository":{"nameWithOwner":"acme/demo"},"state":state,"removed":false,"complete":false,"sourceErrors":{"details":"denied"},"body":"x".repeat(1200),"ci":{"extra":"y".repeat(900)}},"kind":state,"changedFields":["state"],"activity":[{"kind":"comment_added","body":"keep activity"}]})).await.unwrap();
        }
        for _ in 0..3 {
            let full = store
                .changes_prefix("account", Some(&cursor), 1000, "pr-status://")
                .await
                .unwrap();
            let projected = store
                .changes_prefix_projected(
                    "account",
                    Some(&cursor),
                    1000,
                    "pr-status://",
                    fields.clone(),
                )
                .await
                .unwrap();
            assert_eq!(projected.next_cursor, full.next_cursor);
            assert_eq!(projected.head_cursor, full.head_cursor);
            assert_eq!(projected.has_more, full.has_more);
            assert_eq!(
                projected.changes.len(),
                1,
                "original stored bytes must still split tiny projected output"
            );
            let a = &projected.changes[0];
            let b = &full.changes[0];
            assert_eq!(a.cursor, b.cursor);
            assert_eq!(a.changed_fields, b.changed_fields);
            assert_eq!(a.observed_at_ms, b.observed_at_ms);
            for field in ["kind", "activity", "changedFields"] {
                assert_eq!(a.data[field], b.data[field]);
            }
            for field in [
                "number",
                "repository",
                "state",
                "removed",
                "complete",
                "sourceErrors",
            ] {
                assert_eq!(a.data["pullRequest"][field], b.data["pullRequest"][field]);
            }
            assert!(a.data["pullRequest"].get("body").is_none());
            assert!(a.data["pullRequest"].get("ci").is_none());
            assert!(a.data.to_string().len() < 512);
            cursor = projected.next_cursor;
        }
        let full = store
            .bootstrap_prefix("account", "pr-status://")
            .await
            .unwrap();
        let projected = store
            .bootstrap_prefix_projected("account", "pr-status://", fields)
            .await
            .unwrap();
        assert_eq!(projected.cursor, full.cursor);
        assert_eq!(projected.snapshots.len(), full.snapshots.len());
        for (a, b) in projected.snapshots.iter().zip(full.snapshots.iter()) {
            assert_eq!(a.resource, b.resource);
            assert_eq!(a.observed_at_ms, b.observed_at_ms);
            assert_eq!(
                a.data["pullRequest"]["state"],
                b.data["pullRequest"]["state"]
            );
            assert_eq!(
                a.data["pullRequest"]["sourceErrors"],
                b.data["pullRequest"]["sourceErrors"]
            );
        }
    }

    #[tokio::test]
    async fn open_pr_bootstrap_skips_terminal_payload_pages_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 1000, 4096).unwrap();
        let mut rows = Vec::new();
        for number in 1..=512 {
            let (state, removed) = match number % 3 {
                0 => ("OPEN", true),
                1 => ("CLOSED", false),
                _ => ("MERGED", false),
            };
            rows.push((
                format!("pr-status://github.com/acme/demo/{number}"),
                serde_json::json!({
                    "pullRequest":{"number":number,"state":state,"removed":removed,
                    "repository":{"nameWithOwner":"acme/demo"},"body":"x".repeat(2048)}
                }),
            ));
        }
        store.observe_many("account", &rows).await.unwrap();
        drop(rows);
        let resource = "pr-status://github.com/acme/demo/1000";
        let open = serde_json::json!({"pullRequest":{"number":1000,"state":"OPEN","repository":{"nameWithOwner":"acme/demo"},"complete":false,"sourceErrors":{}}});
        let cursor = store.observe("account", resource, &open).await.unwrap();
        // Exercise migration from an existing store, including its feed cursor.
        store
            .run(|conn| {
                conn.execute_batch("DROP INDEX IF EXISTS snapshot_open_prs")
                    .map_err(storage)
            })
            .await
            .unwrap();
        drop(store);
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 1000, 4096).unwrap();
        let stored_bytes: i64 = store
            .run(|conn| {
                conn.query_row(
                    "SELECT sum(length(CAST(data AS BLOB))) FROM snapshots",
                    [],
                    |r| r.get(0),
                )
                .map_err(storage)
            })
            .await
            .unwrap();
        assert!(stored_bytes >= 1024 * 1024);
        for repository in [None, Some("ACME/DEMO")] {
            store
                .read(|conn| {
                    conn.execute_batch(
                        "PRAGMA mmap_size=0; PRAGMA cache_size=32; PRAGMA shrink_memory",
                    )
                    .map_err(storage)?;
                    Ok(cache_misses(conn, true))
                })
                .await
                .unwrap();
            let page = store
                .bootstrap_open_prs(
                    "account",
                    "pr-status://github.com/",
                    repository,
                    Some(vec!["number".into()]),
                )
                .await
                .unwrap();
            assert_eq!(page.cursor, cursor);
            assert_eq!(page.snapshots.len(), 1);
            assert_eq!(page.snapshots[0].data["pullRequest"]["number"], 1000);
            let pages = store
                .read(|conn| Ok(cache_misses(conn, false)))
                .await
                .unwrap();
            eprintln!("bootstrap {repository:?}: {pages} page misses");
            assert!(
                pages > 0 && pages < 128,
                "bootstrap read {pages} SQLite pages for one tiny open row; terminal bodies must not be scanned"
            );
        }
    }

    fn cache_misses(conn: &Connection, reset: bool) -> i32 {
        let (mut misses, mut unused) = (0, 0);
        // The test holds the measured connection guard; SQLite only writes these
        // counters and does not retain either pointer.
        let result = unsafe {
            rusqlite::ffi::sqlite3_db_status(
                conn.handle(),
                rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                &mut misses,
                &mut unused,
                i32::from(reset),
            )
        };
        assert_eq!(result, rusqlite::ffi::SQLITE_OK);
        misses
    }

    #[tokio::test]
    async fn compact_open_pr_reads_skip_omitted_payload_pages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(
            &path,
            std::time::Duration::from_secs(3600),
            1000,
            16 * 1024 * 1024,
        )
        .unwrap();
        let mut rows = Vec::new();
        // Keep each payload within a SQLite page: larger overflow payloads can
        // use direct reads that bypass SQLite's page-cache miss counter.
        for number in 1..=512 {
            rows.push((format!("pr-status://github.com/acme/demo/{number}"), serde_json::json!({
                "pullRequest": {"number": number, "state": "OPEN", "repository": {"nameWithOwner": "acme/demo"},
                    "complete": false, "sourceErrors": {"details": "pending"}, "body": "x".repeat(1024),
                    "ci": {"jobs": [{"output": "y".repeat(1024)}]}},
                "kind": "updated", "activity": [{"kind": "comment_added", "body": "preserve"}], "changedFields": ["ci"]
            })));
        }
        let cursor = store.observe_many("account", &rows).await.unwrap();
        drop(store);
        let store = Store::open(
            &path,
            std::time::Duration::from_secs(3600),
            1000,
            16 * 1024 * 1024,
        )
        .unwrap();
        let fields = vec!["number".to_owned(), "title".to_owned()];
        for repository in [None, Some("ACME/DEMO")] {
            store
                .read(|conn| {
                    conn.execute_batch(
                        "PRAGMA mmap_size=0; PRAGMA cache_size=32; PRAGMA shrink_memory",
                    )
                    .map_err(storage)?;
                    Ok(cache_misses(conn, true))
                })
                .await
                .unwrap();
            let page = store
                .bootstrap_open_prs(
                    "account",
                    "pr-status://github.com/",
                    repository,
                    Some(fields.clone()),
                )
                .await
                .unwrap();
            let pages = store
                .read(|conn| Ok(cache_misses(conn, false)))
                .await
                .unwrap();
            assert_eq!(page.cursor, cursor);
            assert_eq!(page.snapshots.len(), rows.len());
            for snapshot in &page.snapshots {
                let original = &rows
                    .iter()
                    .find(|(resource, _)| resource == &snapshot.resource)
                    .unwrap()
                    .1;
                assert_eq!(
                    snapshot.data,
                    crate::pr_fields::decode_stored(&original.to_string(), Some(&fields)).unwrap()
                );
                assert!(
                    snapshot.data["pullRequest"].get("title").is_none(),
                    "missing fields stay missing internally"
                );
            }
            eprintln!("compact {repository:?}: {pages} page misses");
            assert!(
                pages > 0 && pages < 128,
                "compact {repository:?} read loaded {pages} SQLite pages for tiny selected fields"
            );
        }
    }

    #[tokio::test]
    async fn compact_open_pr_index_tracks_updates_scopes_and_original_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 100, 8192).unwrap();
        let resource = "pr-status://github.com/acme/demo/1";
        let fields = Some(vec!["number".to_owned()]);
        let mut value = serde_json::json!({"pullRequest": {"id": "old", "number": 1, "state": "OPEN",
            "repository": {"nameWithOwner": "acme/demo"}, "complete": false, "sourceErrors": {"ci": "pending"},
            "ci": {"checks": [1]}, "body": "x".repeat(1024)}});
        store
            .observe("other-account", resource, &value)
            .await
            .unwrap();
        store.observe("account", resource, &value).await.unwrap();
        // Simulate upgrading a database that only has the original roster index.
        store
            .run(|conn| {
                conn.execute_batch("DROP INDEX snapshot_open_prs_compact")
                    .map_err(storage)
            })
            .await
            .unwrap();
        drop(store);
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 100, 8192).unwrap();
        for state in ["OPEN", "CLOSED", "OPEN"] {
            value["pullRequest"]["id"] = serde_json::json!("new");
            value["pullRequest"]["state"] = serde_json::json!(state);
            value["pullRequest"]["complete"] = serde_json::json!(true);
            value["pullRequest"]["sourceErrors"] = serde_json::json!({});
            let cursor = store.observe("account", resource, &value).await.unwrap();
            let compact = store
                .bootstrap_open_prs(
                    "account",
                    "pr-status://github.com/",
                    Some("ACME/DEMO"),
                    fields.clone(),
                )
                .await
                .unwrap();
            let full = store
                .bootstrap_open_prs(
                    "account",
                    "pr-status://github.com/",
                    Some("ACME/DEMO"),
                    None,
                )
                .await
                .unwrap();
            assert_eq!(compact.cursor, cursor);
            assert_eq!(compact.cursor, full.cursor);
            assert_eq!(compact.snapshots.len(), usize::from(state == "OPEN"));
            if state == "OPEN" {
                assert_eq!(
                    compact.snapshots[0].observed_at_ms,
                    full.snapshots[0].observed_at_ms
                );
                assert_eq!(
                    compact.snapshots[0].data,
                    crate::pr_fields::decode_stored(&value.to_string(), fields.as_deref()).unwrap()
                );
            }
        }
        for field in crate::pr_fields::PR_STATUS_FIELDS {
            let selection = vec![(*field).to_owned()];
            let page = store
                .bootstrap_open_prs("account", "pr-status://", None, Some(selection.clone()))
                .await
                .unwrap();
            assert_eq!(
                page.snapshots[0].data,
                crate::pr_fields::decode_stored(&value.to_string(), Some(&selection)).unwrap(),
                "field {field}"
            );
        }
        let other = store
            .bootstrap_open_prs("other-account", "pr-status://", None, fields.clone())
            .await
            .unwrap();
        assert_eq!(other.snapshots[0].data["pullRequest"]["complete"], false);
        value["pullRequest"]["removed"] = serde_json::json!(true);
        store.observe("account", resource, &value).await.unwrap();
        assert!(
            store
                .bootstrap_open_prs("account", "pr-status://", None, fields.clone())
                .await
                .unwrap()
                .snapshots
                .is_empty()
        );
        // Tiny projected output must still reject oversized original evidence.
        let bounded = Store::open(&path, std::time::Duration::from_secs(3600), 100, 512).unwrap();
        assert!(matches!(
            bounded
                .bootstrap_open_prs("other-account", "pr-status://", None, fields)
                .await,
            Err(Error::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn projected_reads_validate_omitted_json_and_do_not_bypass_original_size_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 100, 8192).unwrap();
        let cursor = store.bootstrap("account").await.unwrap().cursor;
        let resource = "pr-status://github.com/acme/demo/1";
        let data = serde_json::json!({"pullRequest":{"number":1,"comments":[],"complete":true,"sourceErrors":{}}});
        let head = store.observe("account", resource, &data).await.unwrap();
        let db = Connection::open(&path).unwrap();
        let malformed = "{\"pullRequest\":{\"number\":1,\"comments\":[not-json]}}";
        db.execute(
            "UPDATE snapshots SET data=?1 WHERE resource=?2",
            params![malformed, resource],
        )
        .unwrap();
        let fields = Some(vec!["number".to_owned()]);
        assert!(matches!(
            store
                .bootstrap_open_prs("account", "pr-status://", None, fields.clone())
                .await,
            Err(Error::Storage(_))
        ));
        assert!(matches!(
            store
                .changes_prefix_projected(
                    "account",
                    Some(&cursor),
                    1000,
                    "pr-status://",
                    fields.clone()
                )
                .await,
            Err(Error::Storage(_))
        ));
        assert!(matches!(
            store
                .bootstrap_prefix_projected("account", "pr-status://", fields.clone())
                .await,
            Err(Error::Storage(_))
        ));
        db.execute(
            "UPDATE snapshots SET data=?1 WHERE resource=?2",
            params![data.to_string(), resource],
        )
        .unwrap();
        let repaired = store
            .changes_prefix_projected(
                "account",
                Some(&cursor),
                1000,
                "pr-status://",
                fields.clone(),
            )
            .await
            .unwrap();
        assert_eq!(repaired.next_cursor, head);
        assert_eq!(repaired.changes.len(), 1);
        store
            .observe(
                "account",
                resource,
                &serde_json::json!({"pullRequest":{"number":1,"body":"x".repeat(10000)}}),
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .changes_prefix_projected(
                    "account",
                    Some(&head),
                    1000,
                    "pr-status://",
                    fields.clone()
                )
                .await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            store
                .bootstrap_prefix_projected("account", "pr-status://", fields)
                .await,
            Err(Error::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn scoped_change_pages_skip_unrelated_bodies_and_keep_scan_positions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(&path, std::time::Duration::from_secs(3600), 100, 256).unwrap();
        let start = store.bootstrap("account").await.unwrap().cursor;
        let unrelated = store
            .observe(
                "account",
                "comments://github.com/acme/demo/1",
                &serde_json::json!({"body":"x".repeat(2048)}),
            )
            .await
            .unwrap();
        let first = store
            .observe(
                "account",
                "pr-status://github.com/acme/demo/1",
                &serde_json::json!({"state":"OPEN"}),
            )
            .await
            .unwrap();
        let second = store
            .observe(
                "account",
                "pr-status://github.com/acme/demo/2",
                &serde_json::json!({"state":"OPEN"}),
            )
            .await
            .unwrap();
        let head = store
            .observe(
                "account",
                "comments://github.com/acme/demo/2",
                &serde_json::json!({"body":"y".repeat(2048)}),
            )
            .await
            .unwrap();
        // A corrupt unrelated body must never be decoded by this scoped feed.
        Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE snapshots SET data='not-json' WHERE resource LIKE 'comments://%'",
                [],
            )
            .unwrap();
        let empty = store
            .changes_prefix("account", Some(&start), 1, "pr-status://github.com/")
            .await
            .unwrap();
        assert!(empty.changes.is_empty());
        assert!(empty.has_more);
        assert_eq!(empty.next_cursor, unrelated);
        let page = store
            .changes_prefix(
                "account",
                Some(&empty.next_cursor),
                1000,
                "pr-status://github.com/",
            )
            .await
            .unwrap();
        assert_eq!(page.changes.len(), 1);
        assert_eq!(page.next_cursor, first);
        assert!(page.has_more);
        let page = store
            .changes_prefix(
                "account",
                Some(&page.next_cursor),
                1000,
                "pr-status://github.com/",
            )
            .await
            .unwrap();
        assert_eq!(page.changes.len(), 1);
        assert_eq!(page.changes[0].cursor, second);
        assert_eq!(page.next_cursor, head);
        assert!(!page.has_more);
        assert_eq!(page.head_cursor, head);
        // Selected oversized bodies fail explicitly before decoding/advancing.
        let huge = store
            .observe(
                "account",
                "pr-status://github.com/acme/other/3",
                &serde_json::json!({"body":"z".repeat(1024)}),
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .changes_prefix("account", Some(&head), 1000, "pr-status://github.com/")
                .await,
            Err(Error::Invalid(_))
        ));
        let selected = store
            .changes_prefix(
                "account",
                Some(&start),
                1000,
                "pr-status://github.com/ACME/DEMO/",
            )
            .await
            .unwrap();
        assert_eq!(selected.changes.len(), 1);
        assert_eq!(selected.next_cursor, first);
        assert!(selected.has_more);
        let selected = store
            .changes_prefix(
                "account",
                Some(&selected.next_cursor),
                1000,
                "pr-status://github.com/acme/demo/",
            )
            .await
            .unwrap();
        assert_eq!(selected.changes.len(), 1);
        assert_eq!(selected.changes[0].cursor, second);
        assert_eq!(
            selected.next_cursor, huge,
            "repository selection skips foreign oversized observations without decoding them"
        );
        assert!(!selected.has_more);
        assert_eq!(
            store
                .bootstrap_prefix("account", "comments://missing/")
                .await
                .unwrap()
                .cursor,
            huge
        );
    }

    #[tokio::test]
    async fn scoped_bootstrap_excludes_other_sources_and_preserves_global_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            std::time::Duration::from_secs(3600),
            100,
            256,
        )
        .unwrap();
        store
            .observe(
                "account",
                "pr-status://github.com/acme/demo/1",
                &serde_json::json!({"state":"OPEN"}),
            )
            .await
            .unwrap();
        let head = store
            .observe(
                "account",
                "comments://github.com/acme/demo/1",
                &serde_json::json!({"body":"x".repeat(1024)}),
            )
            .await
            .unwrap();
        assert!(store.bootstrap("account").await.is_err());
        let scoped = store
            .bootstrap_prefix("account", "pr-status://github.com/")
            .await
            .unwrap();
        assert_eq!(scoped.snapshots.len(), 1);
        assert_eq!(scoped.cursor, head);
        assert!(
            store
                .changes("account", Some(&scoped.cursor), 100)
                .await
                .unwrap()
                .changes
                .is_empty()
        );
        let empty = store
            .bootstrap_prefix("account", "pr-status://other-host/")
            .await
            .unwrap();
        assert!(empty.snapshots.is_empty());
        assert_eq!(empty.cursor, head);
        assert!(
            store
                .bootstrap_prefix("account", "comments://")
                .await
                .is_err(),
            "selected sources still obey the byte limit"
        );
    }
}
