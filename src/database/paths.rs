//! Guard auxiliary files against aliasing SQLite databases and sidecars.
use crate::issues::{Error, Result};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(crate) fn protect_database_paths<P: AsRef<Path>>(
    database: &Path,
    paths: impl IntoIterator<Item = P>,
) -> Result<()> {
    // Inspect inode identity without opening a raw descriptor: even closing a
    // read-only alias would release this process's SQLite record locks.
    let mut protected = Vec::new();
    let mut reserved = Vec::new();
    let canonical = match database.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let absolute = std::path::absolute(database)?;
            match (
                absolute.parent().and_then(|p| p.canonicalize().ok()),
                absolute.file_name(),
            ) {
                (Some(parent), Some(name)) => parent.join(name),
                _ => absolute,
            }
        }
        Err(error) => return Err(error.into()),
    };
    for suffix in [
        "",
        "-wal",
        "-shm",
        ".archive.db",
        ".archive.db-wal",
        ".archive.db-shm",
    ] {
        let mut path = canonical.as_os_str().to_owned();
        path.push(suffix);
        reserved.push(PathBuf::from(&path));
        match fs::metadata(path) {
            Ok(metadata) => protected.push((metadata.dev(), metadata.ino())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for path in paths {
        let path = path.as_ref();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if let (Ok(parent), Some(name)) = (parent.canonicalize(), path.file_name())
            && reserved.contains(&parent.join(name))
        {
            return Err(Error::invalid(format!(
                "Auxiliary file {} must not alias or replace the active issue database, archive or sidecars",
                path.display()
            )));
        }
        match fs::metadata(path) {
            Ok(metadata) if protected.contains(&(metadata.dev(), metadata.ino())) => {
                return Err(Error::invalid(format!(
                    "Auxiliary file {} must not alias the active issue database or its sidecars",
                    path.display(),
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
