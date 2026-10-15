use crate::{Result, digest, invalid};
use rusqlite::Connection;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

const LIBRARY: &[u8] = include_bytes!(env!("CRSQLITE_LIBRARY"));

pub fn load(db: &Connection, root: &Path) -> Result<()> {
    let directory = root.join("sync-extensions");
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = fs::symlink_metadata(&directory)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o077 != 0 {
        return Err(invalid(
            "Replication extension directory must be private and owned by this user",
        ));
    }
    let extension = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    let path = directory.join(format!("crsqlite-{}.{}", digest(LIBRARY), extension));
    if !path.try_exists()? {
        let mut nonce = [0u8; 16];
        fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
        let staging = directory.join(format!(".crsqlite-{}.tmp", digest(&nonce)));
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&staging)?;
            file.write_all(LIBRARY)?;
            file.sync_all()?;
            fs::rename(&staging, &path)?;
            fs::File::open(&directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&staging);
        }
        result?;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    if file.metadata()?.len() != LIBRARY.len() as u64 {
        return Err(invalid("Replication extension has an unexpected length"));
    }
    let mut bytes = Vec::with_capacity(LIBRARY.len());
    file.take(LIBRARY.len() as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes != LIBRARY {
        return Err(invalid(
            "Replication extension differs from the embedded build",
        ));
    }
    // Only our embedded, verified native library is loaded; SQL cannot load others.
    unsafe {
        let guard = rusqlite::LoadExtensionGuard::new(db)?;
        db.load_extension(&path, Some("sqlite3_crsqlite_init"))?;
        drop(guard);
    }
    Ok(())
}
