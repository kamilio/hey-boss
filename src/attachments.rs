//! Disk-backed project files, shared by issues, mindmap nodes and artifacts.
use crate::issues::{Error, Project, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub const FILE_LIMIT: usize = 10 * 1024 * 1024;
const ENCODED_LIMIT: usize = FILE_LIMIT.div_ceil(3) * 4;
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS file_attachments(id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES projects(id),kind TEXT NOT NULL,target TEXT NOT NULL,name TEXT NOT NULL,size INTEGER NOT NULL,sha256 TEXT NOT NULL,author TEXT NOT NULL,created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS file_attachment_target ON file_attachments(project_id,kind,target,created_at,id);";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Issue,
    Node,
    Artifact,
}
impl Kind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Node => "node",
            Self::Artifact => "artifact",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub kind: Kind,
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    List {
        target: Target,
    },
    Upload {
        target: Target,
        name: String,
        data: String,
    },
    Download {
        id: String,
    },
    Remove {
        id: String,
    },
}
impl Operation {
    pub fn writes(&self) -> bool {
        matches!(self, Self::Upload { .. } | Self::Remove { .. })
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::List { target } | Self::Upload { target, .. } => {
                crate::issues::identifier(&target.id, "attachment target", 16384)?;
                if matches!(target.kind, Kind::Issue)
                    && !target.id.parse::<i64>().is_ok_and(|n| n > 0)
                {
                    return Err(Error::invalid(
                        "Issue target must be a positive issue number",
                    ));
                }
            }
            Self::Download { id } | Self::Remove { id } => validate_id(id)?,
        }
        if let Self::Upload { name, data, .. } = self {
            validate_name(name)?;
            if data.len() > ENCODED_LIMIT {
                return Err(Error::invalid("Attachments must be at most 10 MiB"));
            }
            let bytes = decode(data)?;
            if bytes.len() > FILE_LIMIT {
                return Err(Error::invalid("Attachments must be at most 10 MiB"));
            }
        }
        Ok(())
    }
    /// Idempotency journals keep a digest, never the file's base64 contents.
    pub fn fingerprint(&self) -> Result<Value> {
        match self {
            Self::Upload { target, name, data } => Ok(
                json!({"command":"upload","target":target,"name":name,"sha256":digest(&decode(data)?)}),
            ),
            _ => Ok(serde_json::to_value(self)?),
        }
    }
}
fn validate_id(id: &str) -> Result<()> {
    if id.len() != 34
        || !id.starts_with("f-")
        || !id[2..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(Error::invalid("Invalid attachment ID"));
    }
    Ok(())
}
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 255
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(Error::invalid(
            "Attachment name must be a filename of at most 255 bytes, without paths or control characters",
        ));
    }
    Ok(())
}
pub fn decode(data: &str) -> Result<Vec<u8>> {
    STANDARD
        .decode(data)
        .map_err(|_| Error::invalid("Invalid attachment base64"))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::invalid("Attachment must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > FILE_LIMIT {
        return Err(Error::invalid("Attachments must be at most 10 MiB"));
    }
    Ok(bytes)
}
fn target_id(db: &Connection, p: &Project, t: &Target, upload: bool) -> Result<String> {
    let id=match t.kind {
        Kind::Issue=>db.query_row("SELECT cast(number AS TEXT) FROM issues WHERE project_id=?1 AND number=?2 AND (?3=0 OR deleted_at IS NULL)",params![p.id,t.id,upload],|r|r.get(0)).optional()?,
        Kind::Artifact=>db.query_row("SELECT id FROM artifacts WHERE project_id=?1 AND id=?2",params![p.id,t.id],|r|r.get(0)).optional()?,
        Kind::Node=>db.query_row("SELECT id FROM mindmap_nodes WHERE project_id=?1 AND (id=?2 OR alias=?2 OR (kind='issue' AND 'issue:'||reference=?2))",params![p.id,t.id],|r|r.get(0)).optional()?,
    };
    id.ok_or_else(|| Error::new("not_found", "Attachment target not found in this project"))
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(
        json!({"id":r.get::<_,String>(0)?,"target":{"kind":r.get::<_,String>(1)?,"id":r.get::<_,String>(2)?},"name":r.get::<_,String>(3)?,"size":r.get::<_,i64>(4)?,"sha256":r.get::<_,String>(5)?,"author":r.get::<_,String>(6)?,"created_at":r.get::<_,i64>(7)?}),
    )
}
const COLUMNS: &str = "id,kind,target,name,size,sha256,author,created_at";
fn get(db: &Connection, p: &Project, id: &str) -> Result<Value> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM file_attachments WHERE project_id=?1 AND id=?2"),
        params![p.id, id],
        row,
    )
    .optional()?
    .ok_or_else(|| Error::new("not_found", "Attachment not found in this project"))
}

/// Files use opaque server-generated IDs; supplied filenames never become disk paths.
/// The caller owns the transaction and cleans a new file if committing fails.
#[derive(Default)]
pub(crate) struct DiskChange {
    pub new: Option<PathBuf>,
    pub removed: Option<PathBuf>,
}
impl Drop for DiskChange {
    fn drop(&mut self) {
        if let Some(path) = &self.new {
            let _ = fs::remove_file(path);
        }
    }
}
pub(crate) fn delete_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn execute(
    db: &Connection,
    root: &Path,
    p: &Project,
    op: &Operation,
    author: &str,
    now: i64,
    files: &mut DiskChange,
) -> Result<Value> {
    if db.query_row("SELECT role='agent' FROM fleet_meta WHERE id=1", [], |r| {
        r.get::<_, bool>(0)
    })? {
        return Err(Error::invalid(
            "Attachments live on the authoritative store; use --host SUPERVISOR from fleet companions",
        ));
    }
    let result = match op {
        Operation::List { target } => {
            let id = target_id(db, p, target, false)?;
            let mut stmt=db.prepare(&format!("SELECT {COLUMNS} FROM file_attachments WHERE project_id=?1 AND kind=?2 AND target=?3 ORDER BY created_at,id LIMIT 1001"))?;
            let mut entries = stmt
                .query_map(params![p.id, target.kind.as_str(), id], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let more = entries.len() > 1000;
            entries.truncate(1000);
            json!({"attachments":entries,"more":more})
        }
        Operation::Upload { target, name, data } => {
            let target_id = target_id(db, p, target, true)?;
            let count:i64=db.query_row("SELECT count(*) FROM file_attachments WHERE project_id=?1 AND kind=?2 AND target=?3",params![p.id,target.kind.as_str(),target_id],|r|r.get(0))?;
            if count >= 1000 {
                return Err(Error::invalid(
                    "A resource supports at most 1000 attachments; remove a file first",
                ));
            }
            let bytes = decode(data)?;
            let mut random = [0u8; 16];
            fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
            let id = format!(
                "f-{}",
                random
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(root)?;
            if fs::symlink_metadata(root)?.file_type().is_symlink() {
                return Err(Error::invalid("Attachment directory must not be a symlink"));
            }
            let path = root.join(&id);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            files.new = Some(path);
            file.write_all(&bytes)?;
            file.sync_all()?;
            // Sync the directory before committing metadata so power loss cannot
            // leave a successful upload referring to an unpersisted directory entry.
            fs::File::open(root)?.sync_all()?;
            db.execute("INSERT INTO file_attachments(id,project_id,kind,target,name,size,sha256,author,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![id,p.id,target.kind.as_str(),target_id,name,bytes.len() as i64,digest(&bytes),author,now])?;
            json!({"attachment":get(db,p,&id)?,"changed":true})
        }
        Operation::Download { id } => {
            let attachment = get(db, p, id)?;
            let path = root.join(id);
            if let Some(database) = db.path().filter(|path| !path.is_empty()) {
                crate::issues::planning::protect_database_paths(Path::new(database), [&path])?;
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)?;
            if !file.metadata()?.is_file() {
                return Err(Error::invalid("Stored attachment must be a regular file"));
            }
            let mut bytes = Vec::new();
            file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != attachment["size"].as_u64().unwrap()
                || digest(&bytes) != attachment["sha256"].as_str().unwrap()
            {
                return Err(Error::new(
                    "io_error",
                    "Stored attachment failed its integrity check",
                ));
            }
            json!({"attachment":attachment,"data":STANDARD.encode(bytes)})
        }
        Operation::Remove { id } => {
            let attachment = get(db, p, id)?;
            db.execute(
                "DELETE FROM file_attachments WHERE project_id=?1 AND id=?2",
                params![p.id, id],
            )?;
            files.removed = Some(root.join(id));
            json!({"attachment":attachment,"changed":true})
        }
    };
    let mut result = result;
    result["ok"] = json!(true);
    result["project"] = json!(p);
    Ok(result)
}

/// A downloaded file always belongs to this caller's filesystem, including SSH reads.
/// Explicit destinations are never overwritten. The default is a private temp folder.
pub fn materialize(value: &Value, destination: Option<&Path>) -> Result<PathBuf> {
    let name = value["attachment"]["name"]
        .as_str()
        .ok_or_else(|| Error::invalid("Missing attachment filename"))?;
    validate_name(name)?;
    let bytes = decode(
        value["data"]
            .as_str()
            .ok_or_else(|| Error::invalid("Missing attachment contents"))?,
    )?;
    if bytes.len() > FILE_LIMIT
        || value["attachment"]["size"].as_u64() != Some(bytes.len() as u64)
        || value["attachment"]["sha256"].as_str() != Some(digest(&bytes).as_str())
    {
        return Err(Error::invalid(
            "Downloaded attachment failed its integrity check",
        ));
    }
    let temp = if destination.is_none() {
        let mut random = [0u8; 16];
        fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        let dir = std::env::temp_dir().join(format!(
            "hey-boss-download-{:x}",
            u128::from_ne_bytes(random)
        ));
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Some(dir)
    } else {
        None
    };
    let path = match destination {
        Some(path) if path.is_dir() => path.join(name),
        Some(path) => path.to_owned(),
        None => temp.as_ref().unwrap().join(name),
    };
    let write = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        if let Err(e) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&path);
            return Err(e.into());
        }
        Ok(())
    })();
    if let Err(error) = write {
        if let Some(dir) = temp {
            let _ = fs::remove_dir(dir);
        }
        return Err(error);
    }
    Ok(path.canonicalize()?)
}
