//! Observe only on the controller. Notifications wake a reconciler; the durable
//! directory inventory and run receipts, not the event stream, determine arrivals.
use super::{Context, Result};
use crate::{
    database::Connection,
    issues::Store,
    jobs::{Arrival, Occurrence, Snapshot, command},
};
use notify::{RecursiveMode, Watcher};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

type Inventory = BTreeMap<String, String>;
const SETTLE: Duration = Duration::from_millis(500);

fn identity(m: &Metadata) -> String {
    format!(
        "{}:{}:{}",
        m.dev(),
        m.ino(),
        m.created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |t| t.as_nanos())
    )
}
fn stamp(m: &Metadata) -> (i64, i64, u64) {
    (m.mtime(), m.mtime_nsec(), m.len())
}

fn capture(root: &Path, project: &str, path: &Path, expected: &Metadata) -> Result<command::Blob> {
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = source.metadata()?;
    if meta.len() > 64 * 1024 * 1024 * 1024 {
        return Err("Directory job inputs are limited to 64 GiB".into());
    }
    if !meta.is_file() || identity(&meta) != identity(expected) || stamp(&meta) != stamp(expected) {
        return Err("File changed before capture".into());
    }
    fs::create_dir_all(root)?;
    let mut tmp = tempfile::NamedTempFile::new_in(root)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0; command::CHUNK];
    let mut size = 0;
    loop {
        let n = source.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > meta.len() {
            return Err("File is still growing".into());
        }
        hash.update(&buf[..n]);
        tmp.write_all(&buf[..n])?;
    }
    if size != meta.len() || stamp(&source.metadata()?) != stamp(&meta) {
        return Err("File changed during capture".into());
    }
    let blob = command::Blob {
        size,
        sha256: format!("{:x}", hash.finalize()),
    };
    let dest = command::blob_path(root, project, &blob)?;
    tmp.as_file().sync_all()?;
    match tmp.persist_noclobber(&dest) {
        Ok(_) => File::open(dest.parent().unwrap())?.sync_all()?,
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    Ok(blob)
}

#[derive(Default)]
struct Monitor {
    // Candidate must survive two observations with the same size and timestamp.
    waiting: HashMap<String, (String, (i64, i64, u64), Instant)>,
    rescan: bool,
    dir_stamp: Option<(i64, i64, u64)>,
}
impl Monitor {
    fn scan(&mut self, db: &Connection, store: &mut Store, s: &Snapshot, from: i64) -> Result<()> {
        let dir = s.definition.directory.as_ref().unwrap();
        let patterns = dir
            .patterns
            .iter()
            .map(|p| glob::Pattern::new(p))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let checkpoint = db
            .query_row(
                "SELECT files,generation FROM directory_job_seen WHERE job_id=?1 AND revision=?2",
                params![s.job_id, s.revision],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        let (prior, generation) = checkpoint.map_or((None, 0), |(files, g)| (Some(files), g));
        let seen: Inventory = prior
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default();
        let mut current = Inventory::new();
        let mut pending = HashSet::new();
        let mut error = None;
        for entry in fs::read_dir(&dir.path)? {
            let entry = entry?;
            let Some(name) = entry.file_name().into_string().ok() else {
                continue;
            };
            if !patterns.is_empty() && !patterns.iter().any(|p| p.matches(&name)) {
                continue;
            }
            let path = entry.path();
            let m = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            if !m.is_file() {
                continue;
            }
            let id = identity(&m);
            if seen.get(&name) == Some(&id)
                || (prior.is_none()
                    && m.created()
                        .unwrap_or(UNIX_EPOCH)
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        < from as u128)
            {
                current.insert(name, id);
                continue;
            }
            let sample = stamp(&m);
            let w = self
                .waiting
                .entry(name.clone())
                .or_insert_with(|| (id.clone(), sample, Instant::now()));
            if w.0 != id || w.1 != sample {
                *w = (id.clone(), sample, Instant::now())
            }
            if w.2.elapsed() >= SETTLE {
                let mut queue = || -> Result<()> {
                    let blob = capture(&store.job_root()?, &s.project_id, &path, &m)?;
                    let key = format!(
                        "arrival-{:x}",
                        Sha256::digest(serde_json::to_vec(&(s.revision, generation, &id, &name))?)
                    );
                    let now = crate::issues::worker::now();
                    store.enqueue_job(&Occurrence {
                        snapshot: s.clone(),
                        scheduled_at: now,
                        observed_at: now,
                        expected_next: None,
                        next_at: None,
                        request_key: Some(key),
                        arrival: Some(Arrival {
                            name: name.clone(),
                            blob,
                        }),
                    })?;
                    Ok(())
                };
                match queue() {
                    Ok(()) => {
                        current.insert(name, id);
                        continue;
                    }
                    Err(e) => {
                        error = Some(e);
                    }
                }
            }
            pending.insert(name);
        }
        self.waiting.retain(|name, _| pending.contains(name));
        if prior.is_none() || current != seen {
            db.execute("INSERT INTO directory_job_seen VALUES(?1,?2,?3,?4) ON CONFLICT(job_id) DO UPDATE SET revision=excluded.revision,files=excluded.files,generation=excluded.generation",params![s.job_id,s.revision,serde_json::to_string(&current)?,generation+1])?;
        }
        if let Some(e) = error {
            return Err(e);
        }
        Ok(())
    }
}

pub(super) fn run(ctx: Context) {
    while !ctx.stopped() {
        if let Err(e) = observe(&ctx) {
            eprintln!("Directory jobs: {e}");
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}
fn observe(ctx: &Context) -> Result<()> {
    let db = ctx.db()?;
    let mut store = Store::open(&ctx.path)?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if !event.is_ok_and(|e| e.kind.is_access()) {
            let _ = tx.try_send(());
        }
    })?;
    let mut watched = HashMap::<PathBuf, (u64, u64)>::new();
    let mut monitors = HashMap::<(String, i64), Monitor>::new();
    let mut recover = Instant::now();

    while !ctx.stopped() {
        let jobs=db.query_collect("SELECT snapshot,schedule_from FROM scheduled_jobs WHERE enabled=1 AND json_extract(snapshot,'$.definition.directory') IS NOT NULL",[],|r|Ok::<_,rusqlite::Error>((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?;
        let mut dirs = HashSet::new();
        let mut keys = HashSet::new();
        let audit = recover.elapsed() >= Duration::from_secs(60);
        if audit {
            recover = Instant::now();
        }
        let mut pending = false;
        for (raw, from) in jobs {
            let s: Snapshot = serde_json::from_str(&raw)?;
            let key = (s.job_id.clone(), s.revision);
            let path = PathBuf::from(&s.definition.directory.as_ref().unwrap().path);
            dirs.insert(path.clone());
            keys.insert(key.clone());
            let fresh = !monitors.contains_key(&key);
            let m = monitors.entry(key).or_default();
            let retry = m.rescan;
            let result = (|| -> Result<()> {
                let meta = fs::metadata(&path)?;
                if !meta.is_dir() {
                    return Err("Watch path is not a directory".into());
                }
                let inode = (meta.dev(), meta.ino());
                if watched.get(&path) != Some(&inode) {
                    if watched.remove(&path).is_some() {
                        let _ = watcher.unwatch(&path);
                    }
                    watcher.watch(&path, RecursiveMode::NonRecursive)?;
                    watched.insert(path.clone(), inode);
                }
                if audit
                    || fresh
                    || m.dir_stamp != Some(stamp(&meta))
                    || m.rescan
                    || !m.waiting.is_empty()
                {
                    m.scan(&db, &mut store, &s, from)?;
                    m.dir_stamp = Some(stamp(&meta));
                }
                Ok(())
            })();
            if result.is_err() && !path.is_dir() {
                let _ = watcher.unwatch(&path);
                watched.remove(&path);
            }
            m.rescan = result.is_err();
            if fresh || retry || m.rescan {
                let error = result.err().map(|e| e.to_string());
                db.execute("INSERT INTO directory_job_status VALUES(?1,?2,?3) ON CONFLICT(job_id) DO UPDATE SET revision=excluded.revision,error=excluded.error WHERE revision!=excluded.revision OR error IS NOT excluded.error",params![s.job_id,s.revision,error])?;
            }
            pending |= !m.waiting.is_empty();
        }
        monitors.retain(|key, _| keys.contains(key));
        watched.retain(|path, _| {
            if dirs.contains(path) {
                true
            } else {
                let _ = watcher.unwatch(path);
                false
            }
        });
        let wake = rx
            .recv_timeout(if pending {
                SETTLE
            } else {
                Duration::from_secs(2)
            })
            .is_ok();
        if wake {
            std::thread::sleep(Duration::from_millis(100));
            let _ = rx.try_recv();
        }
    }
    Ok(())
}
