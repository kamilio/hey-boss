//! Database files and their transaction sidecars are never maintenance targets.
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs,
    io::{self, Read},
    path::Path,
    time::{Duration, Instant},
};

pub(super) fn protected(path: &Path) -> io::Result<bool> {
    // These temporary-directory files coordinate live SQLite owners. Their
    // age is unrelated to use: unlinking a held flock permits a second owner.
    // Check ancestors too, for cleanup cursors saved inside the runtime tree.
    if path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .and_then(|name| name.strip_prefix("hey-boss-db-"))
            .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()))
    }) {
        return Ok(true);
    }
    if path.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("hables")
    }) {
        return Ok(true);
    }
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if name.ends_with("-wal")
        || name.ends_with("-shm")
        || name.ends_with("-journal")
        || name
            .split('.')
            // The basename is not an extension: sqlite.ts is source code.
            .skip(1)
            .any(|s| matches!(s, "db" | "sqlite" | "sqlite3" | "sqlitedb"))
    {
        return Ok(true);
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Ok(false);
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    header(&mut file)
}

pub(super) fn header(file: &mut fs::File) -> io::Result<bool> {
    let mut bytes = [0; 16];
    let n = file.read(&mut bytes)?;
    Ok((n == 16 && &bytes == b"SQLite format 3\0")
        || (n >= 4 && matches!(&bytes[..4], [0x37, 0x7f, 0x06, 0x82 | 0x83]))
        || (n >= 8 && bytes[..8] == [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]))
}

/// Delete individually: a protected database never disappears with its parent.
/// No symlink traversal; bounded runs make partial progress on large trees.
pub(super) fn remove_tree(path: &Path) -> io::Result<()> {
    remove_until(path, Instant::now() + Duration::from_secs(30))
}
fn remove_until(path: &Path, deadline: Instant) -> io::Result<()> {
    if path
        .ancestors()
        .any(|p| p.file_name().is_some_and(|n| n == "node_modules"))
    {
        return Err(super::preserved(
            "Dependencies require explicit release through remove-dependencies; preserved",
        ));
    }
    if Instant::now() >= deadline {
        return Err(io::Error::other(
            "Cleanup time slice exhausted; retry next cycle",
        ));
    }
    if protected(path)? {
        return Err(super::preserved("SQLite database or sidecar preserved"));
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        let mut error = None;
        for entry in fs::read_dir(path)? {
            if Instant::now() >= deadline {
                return Err(io::Error::other(
                    "Cleanup time slice exhausted; retry next cycle",
                ));
            }
            if let Err(e) = entry.and_then(|e| remove_until(&e.path(), deadline))
                && error.as_ref().is_none_or(super::is_preserved)
            {
                error = Some(e);
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        fs::remove_dir(path)
    } else if meta.is_file() || meta.file_type().is_symlink() {
        fs::remove_file(path)
    } else {
        Err(super::preserved("Special file preserved"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn recursive_cache_removal_cannot_bypass_dependency_release() {
        let root = fixture();
        fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        fs::write(root.join("node_modules/.bin/tsc"), "retained validator").unwrap();
        assert!(
            remove_tree(&root)
                .unwrap_err()
                .to_string()
                .contains("explicit release")
        );
        assert!(root.join("node_modules/.bin/tsc").is_file());
        fs::remove_dir_all(root).unwrap();
    }
    fn fixture() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "harvester-db-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    #[test]
    fn protects_database_names_headers_sidecars_and_hables() {
        let root = fixture();
        for name in [
            "issues.sqlite",
            "Hables.db",
            "state.sqlite3",
            "data.db-wal",
            "unknown-shm",
            "database-journal",
            "store.sqlitedb",
            "store.db.backup",
        ] {
            let p = root.join(name);
            fs::write(&p, b"fixture").unwrap();
            assert!(protected(&p).unwrap(), "{name}");
        }
        for name in ["History", "database.disguised.log"] {
            let p = root.join(name);
            fs::write(&p, b"SQLite format 3\0preserve everything").unwrap();
            assert!(protected(&p).unwrap(), "{name}");
        }
        let p = root.join("Hables/cache/blob");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"encrypted or unrecognized database").unwrap();
        assert!(protected(&p).unwrap());
        let p = root.join("cache.bin");
        fs::write(&p, b"ordinary disposable content").unwrap();
        assert!(!protected(&p).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn database_named_source_files_are_not_database_extensions() {
        let root = fixture();
        for name in [
            "sqlite.ts",
            "sqlite.test.ts",
            "sqlite.d.ts",
            "db.rs",
            "sqlite3.py",
        ] {
            let path = root.join(name);
            fs::write(&path, "ordinary source code").unwrap();
            assert!(
                !protected(&path).unwrap(),
                "source filename misclassified: {name}"
            );
            fs::write(&path, b"SQLite format 3\0actual database").unwrap();
            assert!(
                protected(&path).unwrap(),
                "headers still protect disguised databases"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_database_service_runtime_files_during_recursive_cleanup() {
        let root = fixture();
        let runtime = root.join("hey-boss-db-501");
        fs::create_dir_all(&runtime).unwrap();
        for name in ["owner.lock", "owner.startup", "owner.log"] {
            fs::write(runtime.join(name), b"").unwrap();
        }
        // Also exercise a saved cleanup cursor starting inside the directory.
        assert!(protected(&runtime.join("owner.lock")).unwrap());
        assert!(remove_tree(&root).is_err());
        for name in ["owner.lock", "owner.startup", "owner.log"] {
            assert!(
                runtime.join(name).exists(),
                "Removed database runtime {name}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
