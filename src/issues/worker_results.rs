//! Durable results awaiting database reconciliation. No locks or live slots here.
use super::{Error, Result, worker::Job};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub(super) struct Saved {
    pub job: Job,
    pub state: String,
    pub summary: String,
}

fn directory(database: &Path) -> PathBuf {
    database.with_added_extension("worker-results")
}

fn location(database: &Path, id: &str) -> Result<PathBuf> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::invalid("Invalid saved worker result identity"));
    }
    Ok(directory(database).join(format!("{id}.json")))
}

fn read(path: &Path) -> Result<Option<Saved>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(Error::invalid("Saved worker result is not a regular file"));
    }
    Ok(Some(serde_json::from_reader(file)?))
}

pub(super) fn save(database: &Path, job: &Job, state: &str, summary: &str) -> Result<Saved> {
    let directory = directory(database);
    match DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    if !fs::symlink_metadata(&directory)?.is_dir() {
        return Err(Error::invalid(
            "Saved worker result directory is not a directory",
        ));
    }
    let destination = location(database, &job.id)?;
    let saved = if let Some(saved) = read(&destination)? {
        saved
    } else {
        let saved = Saved {
            job: job.clone(),
            state: state.into(),
            summary: summary.chars().take(16_000).collect(),
        };
        let temporary = directory.join(format!(".pending-{}", super::worker::random_id()?));
        let result = (|| -> Result<Saved> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec(&saved)?)?;
            file.sync_all()?;
            // Publish a complete record once; concurrent recovery cannot replace
            // the original result with a generic "worker died" failure.
            match fs::hard_link(&temporary, &destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            File::open(&directory)?.sync_all()?;
            read(&destination)?.ok_or_else(|| Error::invalid("Saved worker result disappeared"))
        })();
        let _ = fs::remove_file(temporary);
        result?
    };
    if saved.job.id != job.id
        || saved.job.project.id != job.project.id
        || saved.job.number() != job.number()
    {
        return Err(Error::invalid(
            "Saved worker result belongs to a different run",
        ));
    }
    Ok(saved)
}

pub(super) fn remove(database: &Path, id: &str) -> Result<()> {
    match fs::remove_file(location(database, id)?) {
        Ok(()) => File::open(directory(database))?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(super) fn pending(database: &Path) -> Result<Vec<Saved>> {
    let directory = directory(database);
    match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_dir() => {
            return Err(Error::invalid(
                "Saved worker result directory is not a directory",
            ));
        }
        _ => {}
    }
    let mut saved = vec![];
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Some(result) = read(&path)? {
            if location(database, &result.job.id)? != path {
                return Err(Error::invalid(
                    "Saved worker result filename does not match its run",
                ));
            }
            saved.push(result);
        }
    }
    Ok(saved)
}
