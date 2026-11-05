use super::*;
use std::{ffi::CString, os::unix::ffi::OsStrExt, path::Path};

fn state(db: &Connection, origin: &str, subject: &str, sensor: &str) -> Result<Option<String>> {
    Ok(db.query_row("SELECT state FROM operational_event_observations WHERE origin=?1 AND subject=?2 AND sensor=?3", params![origin,subject,sensor], |r|r.get(0)).optional()?)
}

fn transition(
    db: &Connection,
    origin: &str,
    subject: &str,
    sensor: &str,
    current: &str,
    kind: &str,
    data: Value,
) -> Result<()> {
    if state(db, origin, subject, sensor)?.as_deref() == Some(current) {
        return Ok(());
    }
    let tx = crate::database::Transaction::new_unchecked(
        db,
        crate::database::TransactionBehavior::Immediate,
    )?;
    let previous = state(&tx, origin, subject, sensor)?;
    if previous.as_deref() == Some(current) {
        return Ok(());
    }
    tx.execute("INSERT INTO operational_event_observations VALUES(?1,?2,?3,?4) ON CONFLICT(origin,subject,sensor) DO UPDATE SET state=excluded.state",params![origin,subject,sensor,current])?;
    // A first healthy disk sample is a baseline, not a recovery.
    if !(sensor == "disk" && previous.is_none() && current == "healthy") {
        append(
            &tx,
            &Event {
                id: crate::issues::worker::random_id()?,
                kind: kind.into(),
                version: 1,
                origin: origin.into(),
                time: now(),
                project: None,
                subject: subject.into(),
                data,
            },
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn disk(
    db: &Connection,
    origin: &str,
    path: &str,
    available: u64,
    total: u64,
) -> Result<()> {
    if total == 0 {
        return Ok(());
    }
    let was_low = state(db, origin, path, "disk")?.as_deref() == Some("low");
    let low = if was_low {
        available < 3 * 1024u64.pow(3) || u128::from(available) * 100 < u128::from(total) * 7
    } else {
        available < 2 * 1024u64.pow(3) || u128::from(available) * 100 < u128::from(total) * 5
    };
    transition(
        db,
        origin,
        path,
        "disk",
        if low { "low" } else { "healthy" },
        if low {
            "machine.disk_low"
        } else {
            "machine.disk_recovered"
        },
        json!({"path":path,"available_bytes":available,"total_bytes":total}),
    )
}

pub(crate) fn filesystem(db: &Connection, origin: &str, path: &Path) -> Result<()> {
    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::invalid("Invalid filesystem path"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // statvfs writes the struct only on success. f_bavail measures space the
    // worker can use, excluding blocks reserved for root.
    if unsafe { libc::statvfs(name.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    disk(
        db,
        origin,
        &path.to_string_lossy(),
        (stat.f_bavail as u64).saturating_mul(stat.f_frsize),
        (stat.f_blocks as u64).saturating_mul(stat.f_frsize),
    )
}

pub(crate) fn peer(db: &Connection, origin: &str, host: &str, connected: bool) -> Result<()> {
    transition(
        db,
        origin,
        host,
        "peer",
        if connected {
            "connected"
        } else {
            "disconnected"
        },
        if connected {
            "peer.connected"
        } else {
            "peer.disconnected"
        },
        json!({"reporter":origin,"host":host}),
    )
}

pub(crate) fn sample(db: &Connection, origin: &str, database: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let mut paths = vec![database.parent().unwrap_or(Path::new(".")).to_path_buf()];
    let directories = db.prepare("SELECT DISTINCT json_extract(job,'$.config.cwd') FROM worker_runs WHERE finished_at IS NULL AND machine=?1 LIMIT 100")?
        .query_map([origin], |r|r.get::<_,Option<String>>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    paths.extend(
        directories
            .into_iter()
            .flatten()
            .map(std::path::PathBuf::from),
    );
    let mut volumes = std::collections::BTreeSet::new();
    for path in paths {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if volumes.insert(metadata.dev()) {
            filesystem(db, origin, &path)?;
        }
    }
    Ok(())
}
