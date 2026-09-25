//! Fleet stdout/stderr logs are diagnostics; durable progress lives in the issue DB.
use super::Item;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const MAX_BYTES: u64 = 128 * 1024 * 1024;
const KEEP_BYTES: u64 = 64 * 1024 * 1024;

fn trim(path: &Path, maximum: u64, keep: u64) -> io::Result<bool> {
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let m = file.metadata()?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.nlink() != 1 {
        return Err(io::Error::other(
            "Unowned, linked, or special worker log; preserved",
        ));
    }
    if m.len() <= maximum {
        return Ok(false);
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if super::databases::protected(path)? || super::databases::header(&mut file)? {
        return Err(io::Error::other(
            "SQLite database preserved; not a diagnostic log",
        ));
    }
    let keep = keep.min(m.len());
    file.seek(SeekFrom::Start(m.len() - keep))?;
    let mut tail = Vec::new();
    Read::by_ref(&mut file).take(keep).read_to_end(&mut tail)?;
    // Keep the inode: existing fleet workers own O_APPEND descriptors. Replacing
    // the path would leave them filling an unlinked file. This is copy-truncate;
    // concurrent diagnostic output may race rotation; issue progress is unchanged.
    file.set_len(0)?;
    file.write_all(&tail)?;
    Ok(true)
}

pub(super) fn clean(home: &Path, apply: bool) -> io::Result<(Vec<Item>, usize)> {
    let directory = home.join(".local/share/hey-boss");
    if !directory.exists() {
        return Ok((vec![], 0));
    }
    let m = fs::symlink_metadata(&directory)?;
    if !m.is_dir()
        || m.uid() != unsafe { libc::geteuid() }
        || directory.canonicalize()? != directory
    {
        return Err(io::Error::other(
            "Worker-log directory is unowned or symlinked; preserved",
        ));
    }
    let mut items = Vec::new();
    let mut count = 0;
    for entry in fs::read_dir(directory)?.take(1000) {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name
            .strip_prefix("fleet-worker-")
            .and_then(|s| s.strip_suffix(".log"))
        else {
            continue;
        };
        if id.len() != 24 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let m = fs::symlink_metadata(entry.path())?;
        if m.len() <= MAX_BYTES {
            continue;
        }
        let safe = m.is_file() && m.uid() == unsafe { libc::geteuid() } && m.nlink() == 1;
        let detail = if !safe {
            "Linked or unowned diagnostic log; preserved".into()
        } else if apply {
            match trim(&entry.path(), MAX_BYTES, KEEP_BYTES) {
                Ok(true) => {
                    count += 1;
                    "Trimmed oversized worker diagnostics; retained the recent 64 MiB; active writer kept running".into()
                }
                Ok(false) => "Log already below its size limit".into(),
                Err(error) => format!("Worker log rotation failed; preserved: {error}"),
            }
        } else {
            "Worker diagnostic log exceeds 128 MiB; eligible to retain the recent 64 MiB".into()
        };
        items.push(Item {
            name: entry.path().display().to_string(),
            detail,
            eligible: safe,
            worktree: None,
        });
    }
    Ok((items, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn disguised_sqlite_log_is_never_truncated() {
        let path =
            std::env::temp_dir().join(format!("harvester-database-log-{}.log", std::process::id()));
        let bytes = b"SQLite format 3\0database content that must remain";
        fs::write(&path, bytes).unwrap();
        assert!(trim(&path, 1, 1).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn trim_retains_recent_diagnostics_and_existing_append_writer() {
        let root = std::env::temp_dir().join(format!("hb-health-log-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("log");
        fs::write(&path, b"old diagnostics recent diagnostics").unwrap();
        let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
        let inode = writer.metadata().unwrap().ino();
        assert!(trim(&path, 16, 8).unwrap());
        writer.write_all(b" appended").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"gnostics appended");
        assert_eq!(writer.metadata().unwrap().ino(), inode);
        assert!(!trim(&path, 64, 8).unwrap());
        let link = root.join("link");
        symlink(&path, &link).unwrap();
        assert!(trim(&link, 1, 1).is_err());
        fs::remove_file(&link).unwrap();
        fs::hard_link(&path, &link).unwrap();
        assert!(trim(&path, 1, 1).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"gnostics appended");
        fs::remove_dir_all(root).unwrap();
    }
}
