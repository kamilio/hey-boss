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
    let bytes = STANDARD.decode(&resource.bytes)?;
    if hey_boss_sync::digest(&bytes) != resource.digest {
        return Err(replica::invalid("Corrupt replication resource"));
    }
    let path = database_path(db)?;
    match resource.kind.as_str() {
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
