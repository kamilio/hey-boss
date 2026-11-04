use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resource {
    kind: String,
    id: String,
    digest: String,
    bytes: String,
}

fn database_path(db: &Connection) -> Result<&Path> {
    db.path()
        .map(Path::new)
        .ok_or_else(|| replica::invalid("Sync requires a persistent database"))
}
fn read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > limit as u64 {
        return Err(replica::invalid("Invalid replicated resource size"));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(replica::invalid("Replicated resource grew past its limit"));
    }
    Ok(bytes)
}
fn private_directory(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(replica::invalid("Invalid replication resource directory"));
    }
    Ok(())
}
pub(super) fn initialize(db: &Connection) -> Result<()> {
    let root = database_path(db)?.with_extension("attachments");
    private_directory(&root)?;
    private_directory(&root.join(".replication"))?;
    for row in replica::rows(db, "SELECT id,sha256 FROM file_attachments", &[])? {
        let id = row["id"].as_str().unwrap();
        let sha = row["sha256"].as_str().unwrap();
        if !valid_file(id) || !valid_digest(sha) {
            return Err(replica::invalid("Invalid attachment identity"));
        }
        let cache = root.join(".replication").join(sha);
        if !cache.try_exists()? {
            match fs::hard_link(root.join(id), &cache) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    fs::File::open(root.join(".replication"))?.sync_all()?;
    Ok(())
}
fn valid_digest(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn valid_file(id: &str) -> bool {
    id.len() == 34 && id.starts_with("f-") && id[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn capture(db: &Connection, fact: &Revision) -> Result<Option<Resource>> {
    let Some(row) = &fact.after else {
        return Ok(None);
    };
    let path = database_path(db)?;
    let (kind, id, sha, bytes) = match fact.table.as_str() {
        "published_chunks" => {
            let hash = row["hash"]
                .as_str()
                .ok_or_else(|| replica::invalid("Missing published chunk hash"))?;
            let bytes = crate::pages::read_chunk(path, hash)?;
            if row["size"].as_u64() != Some(bytes.len() as u64) {
                return Err(replica::invalid("Invalid published chunk size"));
            }
            ("published_chunk", hash.to_owned(), hash.to_owned(), bytes)
        }
        "file_attachments" => {
            let id = row["id"]
                .as_str()
                .ok_or_else(|| replica::invalid("Missing attachment ID"))?;
            let sha = row["sha256"]
                .as_str()
                .ok_or_else(|| replica::invalid("Missing attachment digest"))?;
            if !valid_file(id) || !valid_digest(sha) {
                return Err(replica::invalid("Invalid attachment identity"));
            }
            let root = path.with_extension("attachments");
            let cache = root.join(".replication").join(sha);
            let bytes = read(
                &if cache.try_exists()? {
                    cache
                } else {
                    root.join(id)
                },
                10 * 1024 * 1024,
            )?;
            ("attachment", id.to_owned(), sha.to_owned(), bytes)
        }
        "scheduled_job_revisions" => {
            let snapshot: crate::jobs::Snapshot = serde_json::from_str(
                row["snapshot"]
                    .as_str()
                    .ok_or_else(|| replica::invalid("Missing job snapshot"))?,
            )?;
            if snapshot.definition.executable.is_some() {
                return Ok(None);
            }
            let sha = snapshot.instruction_digest;
            let bytes = crate::jobs::files::load(&path.with_extension("jobs"), &sha)?.into_bytes();
            ("job", sha.clone(), sha, bytes)
        }
        _ => return Ok(None),
    };
    if hey_boss_sync::digest(&bytes) != sha {
        return Err(replica::invalid(
            "Resource content does not match its digest",
        ));
    }
    let (kind, bytes) = if kind == "published_chunk" {
        (
            "published_chunk_gzip",
            hey_boss_sync::compression::encode(&bytes)?,
        )
    } else {
        (kind, bytes)
    };
    Ok(Some(Resource {
        kind: kind.into(),
        id,
        digest: sha,
        bytes: STANDARD.encode(bytes),
    }))
}

pub(super) fn receive(db: &Connection, resource: &Resource) -> Result<()> {
    if !valid_digest(&resource.digest) || resource.bytes.len() > 14 * 1024 * 1024 {
        return Err(replica::invalid("Invalid replication resource"));
    }
    let bytes = resource_bytes(resource)?;
    if hey_boss_sync::digest(&bytes) != resource.digest {
        return Err(replica::invalid("Corrupt replication resource"));
    }
    let path = database_path(db)?;
    match resource.kind.as_str() {
        "published_chunk" | "published_chunk_gzip" => {
            if resource.id != resource.digest {
                return Err(replica::invalid("Invalid published chunk identity"));
            }
            crate::pages::write_chunk(path, &resource.digest, &bytes)?;
        }
        "job" => {
            if resource.id != resource.digest {
                return Err(replica::invalid("Invalid instruction identity"));
            }
            let sha = crate::jobs::files::persist(
                &path.with_extension("jobs"),
                std::str::from_utf8(&bytes)?,
            )?;
            if sha != resource.digest {
                return Err(replica::invalid("Invalid instruction digest"));
            }
        }
        "attachment" => {
            if !valid_file(&resource.id) || bytes.len() > 10 * 1024 * 1024 {
                return Err(replica::invalid("Invalid attachment identity or size"));
            }
            let root = path.with_extension("attachments");
            private_directory(&root)?;
            publish(&root.join(&resource.id), &bytes)?;
            let cache = root.join(".replication");
            private_directory(&cache)?;
            publish(&cache.join(&resource.digest), &bytes)?;
        }
        _ => return Err(replica::invalid("Unsupported replicated resource")),
    }
    Ok(())
}

fn resource_bytes(resource: &Resource) -> Result<Vec<u8>> {
    let bytes = STANDARD.decode(&resource.bytes)?;
    if resource.kind == "published_chunk_gzip" {
        hey_boss_sync::compression::decode(&bytes, crate::pages::CHUNK_SIZE)
    } else {
        Ok(bytes)
    }
}

fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.try_exists()? {
        if read(path, 10 * 1024 * 1024)? != bytes {
            return Err(replica::invalid("Existing resource has different content"));
        }
        return Ok(());
    }
    struct Stage(PathBuf);
    impl Drop for Stage {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let stage = Stage(path.with_extension(format!("{}.tmp", crate::issues::worker::random_id()?)));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&stage.0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    match fs::hard_link(&stage.0, path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if read(path, 10 * 1024 * 1024)? != bytes {
                return Err(replica::invalid("Resource identity collision"));
            }
        }
        Err(e) => return Err(e.into()),
    }
    fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

/// Validate immutable page records before advancing the replication checkpoint.
pub(super) fn validate_page(fact: &Revision) -> Result<()> {
    if !fact.table.starts_with("published_") {
        return Ok(());
    }
    let row = fact
        .after
        .as_ref()
        .ok_or_else(|| replica::invalid("Published history uses tombstones, not row deletion"))?;
    match fact.table.as_str() {
        "published_chunks" => {
            let hash = row["hash"]
                .as_str()
                .ok_or_else(|| replica::invalid("Missing chunk hash"))?;
            let resource = fact
                .resource
                .as_ref()
                .ok_or_else(|| replica::invalid("Missing published chunk"))?;
            if !crate::pages::valid_hash(hash)
                || !matches!(
                    resource.kind.as_str(),
                    "published_chunk" | "published_chunk_gzip"
                )
                || resource.id != hash
                || resource.digest != hash
                || row["size"].as_u64().is_none_or(|size| {
                    size == 0
                        || size > crate::pages::CHUNK_SIZE as u64
                        || resource_bytes(resource).map_or(true, |b| b.len() as u64 != size)
                })
            {
                return Err(replica::invalid("Invalid published chunk record"));
            }
        }
        "published_deployments" => {
            let manifest = row["manifest"]
                .as_str()
                .ok_or_else(|| replica::invalid("Missing deployment manifest"))?;
            if row["id"] != hey_boss_sync::digest(manifest.as_bytes()) {
                return Err(replica::invalid("Invalid deployment identity"));
            }
            crate::pages::validate_manifest(&serde_json::from_str(manifest)?)?;
        }
        "published_pages" => {
            crate::pages::valid_name(row["name"].as_str().unwrap_or(""))?;
            if !row["deployment"]
                .as_str()
                .is_some_and(crate::pages::valid_hash)
                || !matches!(row["deleted"].as_i64(), Some(0 | 1))
                || row["updated_at"].as_i64().is_none_or(|n| n < 0)
                || row["machine"].as_str().is_none_or(|s| s.len() > 1024)
            {
                return Err(replica::invalid("Invalid published page record"));
            }
        }
        _ => return Err(replica::invalid("Unknown published table")),
    }
    Ok(())
}
