//! Disk-backed project files, shared by issues, mindmap nodes and artifacts.
use crate::database::Connection;
use crate::issues::{Error, Project, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{OptionalExtension, params};
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
fn row(r: &crate::database::Row<'_>) -> rusqlite::Result<Value> {
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
    pub new: Vec<PathBuf>,
    pub removed: Option<PathBuf>,
    pub upload: Option<PreparedUpload>,
}

pub(crate) struct PreparedUpload {
    pub id: String,
    name: String,
    size: i64,
    sha256: String,
}

pub(crate) fn check_authority(db: &Connection) -> Result<()> {
    if db.query_row("SELECT role='agent' FROM fleet_meta WHERE id=1", [], |r| {
        r.get::<_, bool>(0)
    })? {
        return Err(Error::invalid(
            "Attachments live on the authoritative store; use --host SUPERVISOR from fleet companions",
        ));
    }
    Ok(())
}

pub(crate) fn check_upload(db: &Connection, p: &Project, target: &Target) -> Result<String> {
    check_authority(db)?;
    let id = target_id(db, p, target, true)?;
    let count: i64 = db.query_row(
        "SELECT count(*) FROM file_attachments WHERE project_id=?1 AND kind=?2 AND target=?3",
        params![p.id, target.kind.as_str(), id],
        |r| r.get(0),
    )?;
    if count >= 1000 {
        return Err(Error::invalid(
            "A resource supports at most 1000 attachments; remove a file first",
        ));
    }
    Ok(id)
}

/// Persist an unreferenced file before taking the database writer. DiskChange
/// removes it unless the caller commits its metadata and request receipt.
pub(crate) fn prepare_upload(
    root: &Path,
    name: &str,
    bytes: &[u8],
    files: &mut DiskChange,
) -> Result<PreparedUpload> {
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
    files.new.push(path);
    #[cfg(test)]
    tests::before_write()?;
    file.write_all(bytes)?;
    file.sync_all()?;
    // Metadata must never commit before the directory entry is durable.
    fs::File::open(root)?.sync_all()?;
    Ok(PreparedUpload {
        id,
        name: name.into(),
        size: bytes.len() as i64,
        sha256: digest(bytes),
    })
}

pub(crate) fn insert_upload(
    db: &Connection,
    p: &Project,
    target: &Target,
    upload: &PreparedUpload,
    author: &str,
    now: i64,
) -> Result<()> {
    // Authority, target and capacity may change while disk work is running.
    let target = check_upload(db, p, target).map(|id| (target.kind.as_str(), id))?;
    db.execute("INSERT INTO file_attachments(id,project_id,kind,target,name,size,sha256,author,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![upload.id,p.id,target.0,target.1,upload.name,upload.size,upload.sha256,author,now])?;
    Ok(())
}
impl Drop for DiskChange {
    fn drop(&mut self) {
        for path in &self.new {
            let _ = fs::remove_file(path);
        }
    }
}
pub(crate) fn delete_file(path: &Path) -> Result<()> {
    #[cfg(test)]
    tests::before_delete();
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
    check_authority(db)?;
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
        Operation::Upload { target, .. } => {
            let upload = files.upload.take().expect("upload prepared before writer");
            insert_upload(db, p, target, &upload, author, now)?;
            json!({"attachment":get(db,p,&upload.id)?,"changed":true})
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::issues::{Request, Store};
    use std::sync::mpsc;
    use std::time::Duration;

    thread_local! {
        static WRITE_PAUSE: std::cell::RefCell<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
        static DELETE_PAUSE: std::cell::RefCell<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
    }

    pub(super) fn before_delete() {
        if let Some((entered, release)) = DELETE_PAUSE.with(|pause| pause.borrow_mut().take()) {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
    }

    pub(super) fn before_write() -> Result<()> {
        if let Some((entered, release)) = WRITE_PAUSE.with(|pause| pause.borrow_mut().take()) {
            entered.send(()).unwrap();
            release
                .recv()
                .map_err(|_| Error::new("io_error", "Interrupted file preparation"))?;
        }
        Ok(())
    }

    fn request(operation: Value) -> Request {
        serde_json::from_value(json!({"version":1,"project":{"id":"named:Files","name":"Files"},
            "actor":{"id":"human:boss","kind":"human","machine":"test","host":"test","cwd":"/tmp","source":"test"},
            "operation":operation,"request_id":"upload-once"})).unwrap()
    }

    #[test]
    fn raced_deletion_receipts_release_the_writer_before_file_cleanup() {
        for artifact in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "hb-delete-replay-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            let path = root.join("issues.db");
            let mut store = Store::open(&path).unwrap();
            crate::database::Connection::open(&path).unwrap().execute("INSERT OR IGNORE INTO projects(id,name,next_number) VALUES('named:Files','Files',1)", []).unwrap();
            let mut create =
                request(json!({"action":"create","title":"Issue","body":"","labels":[]}));
            create.request_id = None;
            store.execute(&create).unwrap();
            let create = if artifact {
                json!({"action":"artifact","operation":{"command":"import","operation":{"command":"create","title":"Files","body":"[a](a.csv) [b](b.csv)"},"files":[{"destination":"a.csv","name":"a.csv","data":"YQ=="},{"destination":"b.csv","name":"b.csv","data":"Yg=="}]}})
            } else {
                json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":"a.csv","data":"YQ=="}})
            };
            let mut create = request(create);
            create.request_id = None;
            let created = store.execute(&create).unwrap();
            let deletion = if artifact {
                json!({"action":"artifact","operation":{"command":"delete","id":created["artifact"]["id"],"if_version":1}})
            } else {
                json!({"action":"attachment","operation":{"command":"remove","id":created["attachment"]["id"]}})
            };
            let mut deletion = request(deletion);
            deletion.request_id = Some("delete-once".into());
            let competing = deletion.clone();
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            let (enter_writer, writer_paused) = mpsc::channel();
            let (release_writer, resume_writer) = mpsc::channel();
            let (connection, transport) =
                crate::database::tests::pause_before_writer(&path, enter_writer, resume_writer);
            store.replace_connection_for_test(connection);
            let (enter_cleanup, cleanup_paused) = mpsc::channel();
            let (release_cleanup, resume_cleanup) = mpsc::channel();
            let replaying = std::thread::spawn(move || {
                DELETE_PAUSE
                    .with(|pause| *pause.borrow_mut() = Some((enter_cleanup, resume_cleanup)));
                store.execute(&deletion)
            });
            writer_paused.recv_timeout(Duration::from_secs(5)).unwrap();
            let mut other = Store::open(&path).unwrap();
            other.replace_connection_for_test(Connection::connect(&path).unwrap());
            let expected = other.execute(&competing).unwrap();
            release_writer.send(()).unwrap();
            cleanup_paused.recv_timeout(Duration::from_secs(5)).unwrap();
            let writer = Connection::connect(&path).unwrap();
            let (written, completed) = mpsc::channel();
            let editing = std::thread::spawn(move || {
                let result = writer.execute(
                    "UPDATE projects SET activity_at=activity_at+1 WHERE id='named:Files'",
                    [],
                );
                written.send(result.is_ok()).unwrap();
                result
            });
            let available = completed.recv_timeout(Duration::from_secs(3)).ok() == Some(true);
            release_cleanup.send(()).unwrap();
            assert_eq!(replaying.join().unwrap().unwrap(), expected);
            editing.join().unwrap().unwrap();
            transport.join().unwrap();
            let inspecting = Connection::connect(&path).unwrap();
            let receipts: i64 = inspecting
                .query_row(
                    "SELECT count(*) FROM requests WHERE request_id='delete-once'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let files: i64 = inspecting
                .query_row("SELECT count(*) FROM file_attachments", [], |r| r.get(0))
                .unwrap();
            assert_eq!((receipts, files), (1, 0));
            assert_eq!(
                fs::read_dir(root.join("issues.attachments"))
                    .unwrap()
                    .count(),
                0
            );
            drop(inspecting);
            drop(other);
            owner.stop();
            fs::remove_dir_all(root).unwrap();
            assert!(
                available,
                "Replayed deletion held the writer during cleanup (artifact={artifact})"
            );
        }
    }

    #[test]
    fn attachment_disk_writes_do_not_hold_the_database_writer() {
        for operation in [
            json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":"file.bin","data":"AQID"}}),
            json!({"action":"artifact","operation":{"command":"import","operation":{"command":"create","title":"Imported","body":"![image](a.png)"},"files":[{"destination":"a.png","name":"a.png","data":"AQID"}]}}),
        ] {
            let root = std::env::temp_dir().join(format!(
                "hb-upload-writer-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            let path = root.join("issues.db");
            let mut store = Store::open(&path).unwrap();
            crate::database::Connection::open(&path).unwrap().execute("INSERT OR IGNORE INTO projects(id,name,next_number) VALUES('named:Files','Files',1)", []).unwrap();
            let mut create =
                request(json!({"action":"create","title":"Issue","body":"","labels":[]}));
            create.request_id = None;
            store.execute(&create).unwrap();
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            store.replace_connection_for_test(Connection::connect(&path).unwrap());
            let writer = Connection::connect(&path).unwrap();
            let (entered, paused) = mpsc::channel();
            let (release, released) = mpsc::channel();
            let uploading = std::thread::spawn(move || {
                WRITE_PAUSE.with(|pause| *pause.borrow_mut() = Some((entered, released)));
                store.execute(&request(operation))
            });
            paused.recv_timeout(Duration::from_secs(5)).unwrap();
            let (written, completed) = mpsc::channel();
            let editing = std::thread::spawn(move || {
                let result = writer.execute(
                    "UPDATE projects SET activity_at=activity_at+1 WHERE id='named:Files'",
                    [],
                );
                written.send(result.is_ok()).unwrap();
                result
            });
            let available = completed.recv_timeout(Duration::from_secs(3)).ok() == Some(true);
            release.send(()).unwrap();
            let result = uploading.join().unwrap();
            let edited = editing.join().unwrap();
            owner.stop();
            fs::remove_dir_all(root).unwrap();
            result.unwrap();
            edited.unwrap();
            assert!(available, "Attachment disk writes held the database writer");
        }
    }

    #[test]
    fn prepared_uploads_recheck_guards_clean_failures_and_replay_competing_receipts() {
        for (import, change) in [
            (false, "target"),
            (false, "capacity"),
            (false, "authority"),
            (false, "replay"),
            (false, "replay_error"),
            (true, "version"),
            (true, "replay"),
            (true, "replay_error"),
        ] {
            let root = std::env::temp_dir().join(format!(
                "hb-upload-races-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            let path = root.join("issues.db");
            let mut store = Store::open(&path).unwrap();
            crate::database::Connection::open(&path).unwrap().execute("INSERT OR IGNORE INTO projects(id,name,next_number) VALUES('named:Files','Files',1)", []).unwrap();
            let mut create =
                request(json!({"action":"create","title":"Issue","body":"","labels":[]}));
            create.request_id = None;
            store.execute(&create).unwrap();
            let mut create = request(
                json!({"action":"artifact","operation":{"command":"create","title":"Original","body":"Keep this"}}),
            );
            create.request_id = None;
            let document = store.execute(&create).unwrap();
            let id = document["artifact"]["id"].as_str().unwrap().to_owned();
            let op = if import {
                json!({"action":"artifact","operation":{"command":"import","operation":{"command":"edit","id":id,"if_version":1,"body":"![image](a.png)"},"files":[{"destination":"a.png","name":"a.png","data":"AQID"}]}})
            } else {
                json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":"file.bin","data":"AQID"}})
            };
            let upload = request(op);
            let competing = upload.clone();
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            store.replace_connection_for_test(Connection::connect(&path).unwrap());
            let writer = Connection::connect(&path).unwrap();
            let inspecting = Connection::connect(&path).unwrap();
            let (entered, paused) = mpsc::channel();
            let (release, released) = mpsc::channel();
            let uploading = std::thread::spawn(move || {
                WRITE_PAUSE.with(|pause| *pause.borrow_mut() = Some((entered, released)));
                store.execute(&upload)
            });
            paused.recv_timeout(Duration::from_secs(5)).unwrap();
            let (written, completed) = mpsc::channel();
            let editing = std::thread::spawn(move || {
                let result: Result<Option<Value>> = (|| {
                    match change {
                        "target" => {
                            writer.execute("UPDATE issues SET deleted_at=1 WHERE project_id='named:Files' AND number=1", [])?;
                        }
                        "capacity" => {
                            writer.execute_batch("WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<1000) INSERT INTO file_attachments SELECT 'f-'||printf('%032x',id),'named:Files','issue','1','existing',0,'digest','human:boss',0 FROM n;")?;
                        }
                        "authority" => {
                            writer.execute("UPDATE fleet_meta SET role='agent' WHERE id=1", [])?;
                        }
                        "version" => {
                            writer.execute(
                                "UPDATE artifacts SET version=version+1 WHERE id=?1",
                                [id],
                            )?;
                        }
                        "replay" | "replay_error" => {
                            let mut other = Store::open(&path)?;
                            other.replace_connection_for_test(writer);
                            return Ok(Some(other.execute(&competing)?));
                        }
                        _ => unreachable!(),
                    }
                    Ok(None)
                })();
                written.send(result.is_ok()).unwrap();
                result
            });
            let available = completed.recv_timeout(Duration::from_secs(3)).ok() == Some(true);
            if change == "replay_error" {
                drop(release);
            } else {
                release.send(()).unwrap();
            }
            let result = uploading.join().unwrap();
            let edited = editing.join().unwrap().unwrap();
            let receipts: i64 = inspecting
                .query_row(
                    "SELECT count(*) FROM requests WHERE request_id='upload-once'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let records: i64 = inspecting
                .query_row("SELECT count(*) FROM file_attachments", [], |r| r.get(0))
                .unwrap();
            let files: Vec<_> = fs::read_dir(root.join("issues.attachments"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            if let Some(expected) = edited {
                assert_eq!(result.unwrap(), expected, "{import}/{change}");
                assert_eq!((receipts, records, files.len()), (1, 1, 1));
                assert_eq!(fs::read(&files[0]).unwrap(), [1, 2, 3]);
            } else {
                let error = result.unwrap_err();
                let expected = match change {
                    "target" => "target not found",
                    "capacity" => "1000 attachments",
                    "authority" => "authoritative store",
                    "version" => "Artifact changed",
                    _ => unreachable!(),
                };
                assert!(error.message.contains(expected), "{}", error.message);
                assert_eq!(receipts, 0);
                assert_eq!(records, if change == "capacity" { 1000 } else { 0 });
                assert!(files.is_empty(), "Rejected upload left files behind");
            }
            drop(inspecting);
            owner.stop();
            fs::remove_dir_all(root).unwrap();
            assert!(available, "File preparation blocked {import}/{change}");
        }
    }
}
