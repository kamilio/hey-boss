use crate::{Error, Response, Result, Source, digest, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

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
    retention: std::time::Duration,
    max_events: usize,
    max_snapshot_bytes: usize,
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
        conn.execute(
            "INSERT OR IGNORE INTO metadata(key,value) VALUES('database_id',?1)",
            [digest(&format!("{}-{}", now_ms(), fastrand::u128(..)))],
        )
        .map_err(storage)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(conn)),
            retention,
            max_events,
            max_snapshot_bytes,
        })
    }

    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let conn = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = conn.lock().map_err(storage)?;
            f(&mut conn)
        })
        .await
        .map_err(storage)?
    }

    pub async fn get(&self, scope: &str, key: &str) -> Result<Option<Response>> {
        let (scope, key) = (scope.to_owned(), key.to_owned());
        self.run(move |conn| {
            let value: Option<String> = conn
                .query_row(
                    "SELECT response FROM cache WHERE scope=?1 AND key=?2",
                    params![scope, key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            value
                .map(|v| serde_json::from_str(&v).map_err(storage))
                .transpose()
        })
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
        self.run(move |conn| {
            let resource = resolve_pr_resource(conn, &scope, &resource)?;
            let data: Option<(String, String)> = conn
                .query_row(
                    "SELECT data,hash FROM snapshots WHERE scope=?1 AND resource=?2",
                    params![scope, resource],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            data.map(|(data, hash)| {
                serde_json::from_str(&data)
                    .map(|data| (data, hash))
                    .map_err(storage)
            })
            .transpose()
        })
        .await
    }

    pub async fn pr_resource_key(&self, scope: &str, resource: &str) -> Result<String> {
        let (scope, resource) = (scope.to_owned(), resource.to_owned());
        self.run(move |conn| resolve_pr_resource(conn, &scope, &resource))
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
        let upper = prefix
            .strip_suffix('/')
            .map_or_else(|| format!("{prefix}\u{10ffff}"), |base| format!("{base}0"));
        let prefix_length = prefix.chars().count();
        let (scope, prefix, suffix) = (scope.to_owned(), prefix.to_owned(), suffix.to_owned());
        self.run(move |conn| {
            // Only the host/repository prefix is case-insensitive. Branches,
            // refs, pagination/query values, and generation suffixes stay exact.
            let data: Option<String> = conn.query_row(
                "SELECT response FROM cache WHERE scope=?1 AND key>=?3 COLLATE NOCASE AND key<?6 COLLATE NOCASE AND substr(key,1,?2)=?3 COLLATE NOCASE AND substr(key,?4)=?5 LIMIT 1",
                params![scope,prefix_length,prefix,prefix_length+1,suffix,upper], |r|r.get(0),
            ).optional().map_err(storage)?;
            data.map(|data|serde_json::from_str(&data).map_err(storage)).transpose()
        }).await
    }

    pub async fn repository_generation(&self, scope: &str, repository: &str) -> Result<u64> {
        let (scope, repository) = (scope.to_owned(), repository.to_ascii_lowercase());
        self.run(move |conn| repository_generation(conn, &scope, &repository))
            .await
    }

    pub async fn pr_identity(
        &self,
        scope: &str,
        repository: &str,
        number: u64,
    ) -> Result<Option<String>> {
        let (scope, repository) = (scope.to_owned(), repository.to_ascii_lowercase());
        self.run(move |conn|conn.query_row("SELECT node_id FROM pr_identity WHERE scope=?1 AND repository=?2 AND pull_number=?3",params![scope,repository,number],|r|r.get(0)).optional().map_err(storage)).await
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
        self.run(move |conn| owner_is_current(conn, &scope, &owner))
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
        self.run(move |conn| {
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
            data.map(|data| serde_json::from_str(&data).map_err(storage))
                .transpose()
        })
        .await
    }

    pub async fn validation_clock(&self, scope: &str, resource: &str) -> Result<u64> {
        let (scope, resource) = (scope.to_owned(), resource.to_owned());
        self.run(move |conn| {
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
        self.run(move |conn| conn.query_row(
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
        self.observe_many_with_owner(scope, observations, clocks, None)
            .await
    }

    pub async fn observe_owned(
        &self,
        scope: &str,
        observations: &[(String, Value)],
        owner: &PrOwner,
    ) -> Result<String> {
        self.observe_many_with_owner(scope, observations, &[], Some(owner.clone()))
            .await
    }

    pub async fn observe_validated_owned(
        &self,
        scope: &str,
        observations: &[(String, Value)],
        clocks: &[(String, u64)],
        owner: &PrOwner,
    ) -> Result<String> {
        self.observe_many_with_owner(scope, observations, clocks, Some(owner.clone()))
            .await
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
    ) -> Result<String> {
        let clocks = clocks.to_vec();
        let (scope, observations) = (scope.to_owned(), observations.to_vec());
        let cutoff = now_ms().saturating_sub(self.retention.as_millis() as u64);
        let max_events = self.max_events;
        self.run(move |conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(storage)?;
            if let Some(owner)=&owner && !owner_is_current(&tx,&scope,owner)? {
                return Err(Error::Invalid("PR entity changed while collecting evidence".into()));
            }
            for (resource,value) in observations {
            let resource=resolve_pr_resource(&tx,&scope,&resource)?;
            let data=serde_json::to_string(&value).map_err(storage)?;
            let hash=digest(&data);
            let old:Option<(String,String)>=tx.query_row("SELECT hash,data FROM snapshots WHERE scope=?1 AND resource=?2",params![scope,resource],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(storage)?;
            if old.as_ref().map(|(h,_)|h.as_str())!=Some(&hash) {
                let previous:Option<Value>=old.map(|(_,data)|serde_json::from_str(&data).map_err(storage)).transpose()?;
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
            Ok(cursor)
        }).await
    }

    /// Validate feed identity and retention without scanning observation bodies.
    pub async fn validate_cursor(&self, scope: &str, cursor: &str) -> Result<()> {
        let (scope, cursor) = (scope.to_owned(), cursor.to_owned());
        self.run(move |conn| {
            let tx = conn.transaction().map_err(storage)?;
            cursor_position(&tx, &scope, Some(&cursor)).map(|_| ())
        })
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
        self.run(move |conn| {
            let tx = conn.transaction().map_err(storage)?;
            let (prefix, sequence, head) = cursor_position(&tx, &scope, cursor.as_deref())?;
            let mut stmt = tx.prepare("SELECT c.cursor,c.resource,c.observed_at_ms,length(CAST(s.data AS BLOB)),length(CAST(c.fields AS BLOB)) FROM changes c LEFT JOIN snapshots s ON s.scope=c.scope AND s.resource=c.resource AND s.cursor=c.cursor WHERE c.scope=?1 AND c.cursor>?2 ORDER BY c.cursor LIMIT ?3").map_err(storage)?;
            let rows = stmt.query_map(params![scope, sequence, limit + 1], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?, r.get::<_, u64>(2)?, r.get::<_, Option<usize>>(3)?, r.get::<_, usize>(4)?))).map_err(storage)?;
            let mut body = tx.prepare("SELECT s.data,c.fields FROM changes c JOIN snapshots s ON s.scope=c.scope AND s.resource=c.resource AND s.cursor=c.cursor WHERE c.scope=?1 AND c.cursor=?2").map_err(storage)?;
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
        self.run(move |conn| {
            let tx=conn.transaction().map_err(storage)?;
            let head:u64=tx.query_row("SELECT head FROM feeds WHERE scope=?1",[&scope],|r|r.get(0)).optional().map_err(storage)?.unwrap_or(0);
            let cursor=format!("{}.{}",feed_prefix(&tx,&scope)?,head);
            let mut stmt=tx.prepare("SELECT resource,data,observed_at_ms FROM snapshots WHERE scope=?1 AND resource>=?2 AND resource<?3 ORDER BY resource").map_err(storage)?;
            let rows=stmt.query_map(params![scope,prefix,upper],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?))).map_err(storage)?;
            let mut snapshots=Vec::new();
            let mut bytes=0usize;
            for row in rows { let (resource,data,observed_at_ms)=row.map_err(storage)?; bytes=bytes.saturating_add(data.len()); if bytes>max_bytes {return Err(Error::Invalid("bootstrap exceeds configured snapshot byte limit".into()));} snapshots.push(Snapshot {resource,data:crate::pr_fields::decode_stored(&data, fields.as_deref()).map_err(storage)?,observed_at_ms}); }
            Ok(SnapshotPage {snapshots,cursor})
        }).await
    }

    /// Bootstrap only the current open roster. Terminal rows remain in the
    /// snapshot/change feed, but cannot consume an open list's byte budget.
    pub(crate) async fn bootstrap_open_prs(
        &self,
        scope: &str,
        prefix: &str,
        repository: Option<&str>,
        fields: Option<Vec<String>>,
    ) -> Result<SnapshotPage> {
        let scope = scope.to_owned();
        let repository = repository.map(str::to_owned);
        let prefix = repository
            .as_ref()
            .map_or_else(|| prefix.to_owned(), |repo| format!("{prefix}{repo}/"));
        let upper = format!("{prefix}\u{10ffff}");
        let max_bytes = self.max_snapshot_bytes;
        self.run(move |conn| {
            let tx = conn.transaction().map_err(storage)?;
            let head: u64 = tx.query_row("SELECT head FROM feeds WHERE scope=?1", [&scope], |r| r.get(0))
                .optional().map_err(storage)?.unwrap_or(0);
            let cursor = format!("{}.{}", feed_prefix(&tx, &scope)?, head);
            // Lifecycle and original byte counts come from the partial index;
            // closed/removed bodies are not scanned to select the open roster.
            // Require that index: without statistics SQLite can prefer the
            // general resource index and read every terminal JSON body again.
            let mut stmt = tx.prepare(&format!("SELECT resource,length(CAST(data AS BLOB)),observed_at_ms FROM snapshots INDEXED BY snapshot_open_prs
                WHERE scope=?1 AND resource>=?2 COLLATE NOCASE AND resource<?3 COLLATE NOCASE
                AND ({OPEN_PR_SELECTION})
                AND (?4 IS NULL OR json_extract(data,'$.pullRequest.repository.nameWithOwner')=?4 COLLATE NOCASE)
                ORDER BY resource")).map_err(storage)?;
            let rows = stmt.query_map(params![scope, prefix, upper, repository], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, usize>(1)?, r.get::<_, u64>(2)?))
            }).map_err(storage)?;
            // Sort only small metadata, and reject oversized selections before
            // allocating their bodies. Full JSON must not enter SQLite's sorter.
            let mut body = tx.prepare("SELECT data FROM snapshots WHERE scope=?1 AND resource=?2").map_err(storage)?;
            let mut snapshots = Vec::new();
            let mut bytes = 0usize;
            for row in rows {
                let (resource, data_bytes, observed_at_ms) = row.map_err(storage)?;
                bytes = bytes.saturating_add(data_bytes);
                if bytes > max_bytes {
                    return Err(Error::Invalid("bootstrap exceeds configured snapshot byte limit".into()));
                }
                let data: String = body.query_row(params![scope, resource], |r| r.get(0)).map_err(storage)?;
                snapshots.push(Snapshot {
                    resource,
                    data: crate::pr_fields::decode_stored(&data, fields.as_deref()).map_err(storage)?,
                    observed_at_ms,
                });
            }
            Ok(SnapshotPage { snapshots, cursor })
        }).await
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
        self.run(move |conn| {
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
        self.run(move |conn| {
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
                .run(|conn| {
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
                .run(|conn| Ok(cache_misses(conn, false)))
                .await
                .unwrap();
            eprintln!("bootstrap {repository:?}: {pages} page misses");
            assert!(
                pages < 128,
                "bootstrap read {pages} SQLite pages for one tiny open row; terminal bodies must not be scanned"
            );
        }
    }

    fn cache_misses(conn: &Connection, reset: bool) -> i32 {
        let (mut misses, mut unused) = (0, 0);
        // The test holds the store connection lock; SQLite only writes these
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
