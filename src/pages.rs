//! Private static sites. Immutable chunks and manifests use the fleet's CRDT log.
use crate::{
    database::Connection,
    issues::{Error, Result, Store},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub(crate) const CHUNK_SIZE: usize = 1024 * 1024;
const SITE_LIMIT: u64 = 256 * 1024 * 1024;
const FILE_LIMIT: usize = 10_000;
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS published_chunks(hash TEXT PRIMARY KEY,size INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS published_deployments(id TEXT PRIMARY KEY,manifest TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS published_pages(name TEXT PRIMARY KEY,deployment TEXT NOT NULL,updated_at INTEGER NOT NULL,machine TEXT NOT NULL,deleted INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS published_live(name TEXT PRIMARY KEY,deployment TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS published_page_list ON published_pages(deleted,updated_at DESC,name);";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Asset {
    pub size: u64,
    pub chunks: Vec<String>,
}
pub(crate) type Manifest = BTreeMap<String, Asset>;
#[derive(Debug, Serialize)]
pub struct Page {
    pub name: String,
    pub deployment: String,
    pub updated_at: i64,
    pub machine: String,
    pub files: usize,
    pub size: u64,
    pub syncing: bool,
    pub path: String,
}

pub(crate) fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn valid_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 63
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || name.ends_with('-')
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(Error::invalid(
            "Page name must be 1–63 lowercase letters, digits or hyphens, starting and ending with a letter or digit",
        ));
    }
    Ok(())
}
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.contains(['\\', '\0'])
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|p| !p.is_empty() && !p.starts_with('.'))
}
pub(crate) fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if !manifest.contains_key("index.html") || manifest.len() > FILE_LIMIT {
        return Err(Error::invalid(
            "Publish a site with index.html and at most 10,000 files",
        ));
    }
    let mut total = 0u64;
    for (path, asset) in manifest {
        if !valid_path(path)
            || asset.size > SITE_LIMIT
            || asset.chunks.len() as u64 != asset.size.div_ceil(CHUNK_SIZE as u64)
            || asset.chunks.iter().any(|h| !valid_hash(h))
        {
            return Err(Error::invalid("Invalid published file manifest"));
        }
        total = total
            .checked_add(asset.size)
            .ok_or_else(|| Error::invalid("Site too large"))?;
    }
    if total > SITE_LIMIT {
        return Err(Error::invalid("Published site exceeds 256 MiB"));
    }
    Ok(())
}
pub(crate) fn root(database: &Path) -> PathBuf {
    database.with_extension("pages")
}
fn private_dir(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(Error::invalid(
            "Published content directory must not be a symlink",
        ));
    }
    Ok(())
}
pub(crate) fn read_chunk(database: &Path, hash: &str) -> Result<Vec<u8>> {
    if !valid_hash(hash) {
        return Err(Error::invalid("Invalid content hash"));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root(database).join(hash))?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > CHUNK_SIZE as u64 {
        return Err(Error::invalid("Invalid content chunk"));
    }
    let mut bytes = Vec::new();
    file.take(CHUNK_SIZE as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > CHUNK_SIZE || hey_boss_sync::digest(&bytes) != hash {
        return Err(Error::invalid(
            "Published content failed its integrity check",
        ));
    }
    Ok(bytes)
}
pub(crate) fn write_chunk(database: &Path, hash: &str, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > CHUNK_SIZE || hey_boss_sync::digest(bytes) != hash {
        return Err(Error::invalid("Invalid content chunk"));
    }
    let root = root(database);
    private_dir(&root)?;
    let destination = root.join(hash);
    if destination.try_exists()? {
        read_chunk(database, hash)?;
        return Ok(());
    }
    struct Stage(PathBuf);
    impl Drop for Stage {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let stage = Stage(root.join(format!(".{}", crate::issues::worker::random_id()?)));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&stage.0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    match fs::hard_link(&stage.0, destination) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            read_chunk(database, hash)?;
        }
        Err(e) => return Err(e.into()),
    }
    File::open(&root)?.sync_all()?;
    Ok(())
}

pub fn publish(database: &Path, source: &Path, name: &str) -> Result<Page> {
    valid_name(name)?;
    let db = Store::open(database)?.into_database();
    let metadata = fs::symlink_metadata(source)?;
    let mut files = Vec::new();
    fn collect(
        root: &Path,
        path: &Path,
        files: &mut Vec<(String, PathBuf)>,
        total: &mut u64,
    ) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                collect(root, &entry.path(), files, total)?;
            }
        } else if metadata.is_file() {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .ok_or_else(|| Error::invalid("Published filenames must be UTF-8"))?
                .to_owned();
            if !valid_path(&name) {
                return Err(Error::invalid("Invalid published filename"));
            }
            *total = total.saturating_add(metadata.len());
            if files.len() == FILE_LIMIT || *total > SITE_LIMIT {
                return Err(Error::invalid("Site exceeds 10,000 files or 256 MiB"));
            }
            files.push((name, path.to_owned()));
        } else {
            return Err(Error::invalid(format!(
                "Publish only regular files; symlinks are not followed: {}",
                path.display()
            )));
        }
        Ok(())
    }
    if metadata.is_dir() {
        collect(source, source, &mut files, &mut 0)?;
    } else if metadata.is_file()
        && source
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("html") || s.eq_ignore_ascii_case("htm"))
    {
        files.push(("index.html".into(), source.to_owned()));
    } else {
        return Err(Error::invalid(
            "Publish an HTML file or a directory containing index.html; symlinks are not followed",
        ));
    }
    if !files.iter().any(|(name, _)| name == "index.html") {
        return Err(Error::invalid("Site directory needs index.html"));
    }
    let mut manifest = Manifest::new();
    let mut chunks = BTreeMap::new();
    let mut total = 0u64;
    let mut buffer = vec![0; CHUNK_SIZE];
    for (name, path) in files {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(Error::invalid("Publish only regular files"));
        }
        let mut asset = Asset {
            size: 0,
            chunks: Vec::new(),
        };
        loop {
            let mut length = 0;
            while length < buffer.len() {
                let read = file.read(&mut buffer[length..])?;
                if read == 0 {
                    break;
                }
                length += read;
            }
            if length == 0 {
                break;
            }
            total += length as u64;
            if total > SITE_LIMIT {
                return Err(Error::invalid("Published site exceeds 256 MiB"));
            }
            let hash = hey_boss_sync::digest(&buffer[..length]);
            write_chunk(database, &hash, &buffer[..length])?;
            chunks.insert(hash.clone(), length);
            asset.chunks.push(hash);
            asset.size += length as u64;
        }
        manifest.insert(name, asset);
    }
    validate_manifest(&manifest)?;
    let manifest = serde_json::to_string(&manifest)?;
    let deployment = hey_boss_sync::digest(manifest.as_bytes());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    // All disk I/O is done before taking the application database writer.
    let tx = db.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO published_chunks SELECT key,value FROM json_each(?1)",
        [serde_json::to_string(&chunks)?],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO published_deployments VALUES(?1,?2)",
        params![deployment, manifest],
    )?;
    tx.execute("INSERT INTO published_pages VALUES(?1,?2,?3,?4,0) ON CONFLICT(name) DO UPDATE SET deployment=excluded.deployment,updated_at=excluded.updated_at,machine=excluded.machine,deleted=0 WHERE published_pages.deployment<>excluded.deployment OR published_pages.deleted=1", params![name, deployment, now, crate::issues::identity::host()])?;
    tx.commit()?;
    view_in(&db, name)
}

// Immutable manifests are shared by every asset request. Keep this bounded so
// loading a large site does not repeatedly parse its entire file tree.
static MANIFESTS: Mutex<VecDeque<(String, String, Arc<Manifest>)>> = Mutex::new(VecDeque::new());
fn manifest(db: &Connection, id: &str) -> Result<Option<Arc<Manifest>>> {
    if !valid_hash(id) {
        return Err(Error::invalid("Invalid deployment"));
    }
    let database = db.path().unwrap_or("");
    if let Some((_, _, manifest)) = MANIFESTS
        .lock()
        .unwrap()
        .iter()
        .find(|(path, hash, _)| path == database && hash == id)
    {
        return Ok(Some(Arc::clone(manifest)));
    }
    let text: Option<String> = db
        .query_row(
            "SELECT manifest FROM published_deployments WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    text.map(|text| {
        if hey_boss_sync::digest(text.as_bytes()) != id {
            return Err(Error::invalid("Invalid deployment digest"));
        }
        let manifest = serde_json::from_str(&text)?;
        validate_manifest(&manifest)?;
        let manifest = Arc::new(manifest);
        let mut cache = MANIFESTS.lock().unwrap();
        if cache.len() == 8 {
            cache.pop_front();
        }
        cache.push_back((database.into(), id.into(), Arc::clone(&manifest)));
        Ok(manifest)
    })
    .transpose()
}
fn complete(db: &Connection, manifest: &Manifest) -> Result<bool> {
    let hashes: Vec<_> = manifest.values().flat_map(|a| a.chunks.iter()).collect();
    Ok(!db.query_row("SELECT EXISTS(SELECT 1 FROM json_each(?1) j WHERE NOT EXISTS(SELECT 1 FROM published_chunks c WHERE c.hash=j.value))", [serde_json::to_string(&hashes)?], |r| r.get::<_,bool>(0))?)
}
pub(crate) fn view_in(db: &Connection, name: &str) -> Result<Page> {
    valid_name(name)?;
    let row: Option<(String, i64, String)> = db
        .query_row(
            "SELECT deployment,updated_at,machine FROM published_pages WHERE name=?1 AND deleted=0",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (wanted, updated_at, machine) =
        row.ok_or_else(|| Error::new("not_found", "Published page not found"))?;
    let previous: Option<String> = db
        .query_row(
            "SELECT deployment FROM published_live WHERE name=?1",
            [name],
            |r| r.get(0),
        )
        .optional()?;
    let current = manifest(db, &wanted)?;
    let ready = previous.as_deref() == Some(wanted.as_str())
        || current
            .as_ref()
            .map(|m| complete(db, m))
            .transpose()?
            .unwrap_or(false);
    let (deployment, files) = if ready {
        if previous.as_deref() != Some(wanted.as_str()) {
            db.execute("INSERT INTO published_live VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET deployment=excluded.deployment WHERE published_live.deployment<>excluded.deployment", params![name,wanted])?;
        }
        (wanted, current)
    } else {
        match previous {
            Some(id) => {
                let m = manifest(db, &id)?;
                (id, m)
            }
            None => (String::new(), None),
        }
    };
    Ok(Page {
        name: name.into(),
        deployment,
        updated_at,
        machine,
        files: files.as_ref().map_or(0, |m| m.len()),
        size: files
            .as_ref()
            .map_or(0, |m| m.values().map(|a| a.size).sum()),
        syncing: !ready,
        path: format!("/pages/{name}"),
    })
}
pub fn view(database: &Path, name: &str) -> Result<Page> {
    view_in(&Store::open(database)?.into_database(), name)
}
pub fn list(database: &Path, offset: usize) -> Result<Vec<Page>> {
    let db = Store::open(database)?.into_database();
    let names = db.query_collect("SELECT name FROM published_pages WHERE deleted=0 ORDER BY updated_at DESC,name LIMIT 100 OFFSET ?1", [offset], |r| r.get::<_,String>(0))?;
    names.iter().map(|name| view_in(&db, name)).collect()
}
pub fn delete(database: &Path, name: &str) -> Result<()> {
    valid_name(name)?;
    let db = Store::open(database)?.into_database();
    let tx = db.unchecked_transaction()?;
    if tx.execute("UPDATE published_pages SET deleted=1 WHERE name=?1", [name])? == 0 {
        return Err(Error::new("not_found", "Published page not found"));
    }
    tx.execute("DELETE FROM published_live WHERE name=?1", [name])?;
    tx.commit()?;
    Ok(())
}

pub struct Content {
    pub reader: ChunkReader,
    pub size: u64,
    pub etag: String,
    pub kind: &'static str,
}
pub struct ChunkReader {
    database: PathBuf,
    chunks: std::vec::IntoIter<String>,
    current: std::io::Cursor<Vec<u8>>,
}
impl Read for ChunkReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.current.read(output)?;
            if count > 0 {
                return Ok(count);
            }
            let Some(hash) = self.chunks.next() else {
                return Ok(0);
            };
            self.current = std::io::Cursor::new(
                read_chunk(&self.database, &hash).map_err(std::io::Error::other)?,
            );
        }
    }
}
pub fn content(database: &Path, id: &str, path: &str) -> Result<Content> {
    if !valid_path(path) {
        return Err(Error::invalid("Invalid published path"));
    }
    let db = Store::open(database)?.into_database();
    let manifest =
        manifest(&db, id)?.ok_or_else(|| Error::new("not_found", "Deployment not found"))?;
    let asset = manifest
        .get(path)
        .ok_or_else(|| Error::new("not_found", "Published file not found"))?;
    asset_content(database, path, asset)
}
fn asset_content(database: &Path, path: &str, asset: &Asset) -> Result<Content> {
    let kind = match Path::new(path)
        .extension()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    };
    Ok(Content {
        reader: ChunkReader {
            database: database.into(),
            chunks: asset.chunks.clone().into_iter(),
            current: std::io::Cursor::new(Vec::new()),
        },
        size: asset.size,
        etag: hey_boss_sync::digest(serde_json::to_string(asset)?.as_bytes()),
        kind,
    })
}
pub fn download(database: &Path, name: &str, output: &Path) -> Result<()> {
    let page = view(database, name)?;
    if page.syncing {
        return Err(Error::new(
            "sync_pending",
            "Page is still syncing; retry when all files have arrived",
        ));
    }
    let db = Store::open(database)?.into_database();
    let manifest = manifest(&db, &page.deployment)?.unwrap();
    // create_dir is exclusive: an existing directory is never overwritten.
    fs::DirBuilder::new().mode(0o700).create(output)?;
    let result = (|| -> Result<()> {
        for (path, asset) in manifest.iter() {
            let destination = output.join(path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(destination)?;
            let mut content = asset_content(database, path, asset)?;
            let copied = std::io::copy(&mut content.reader, &mut file)?;
            if copied != content.size {
                return Err(Error::invalid("Published file size mismatch"));
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(output);
    }
    result
}

#[cfg(test)]
mod tests;
