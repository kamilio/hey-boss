//! Local checkout overrides; ordinary repository identities need no file.
use super::{Error, Project, Result, identifier};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const FILE: &str = ".hey-boss";
const LIMIT: u64 = 16 * 1024;

fn contents(root: &Path) -> Result<Option<Vec<u8>>> {
    let path = root.join(FILE);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Error::invalid(format!(
                "Cannot read {}: {error}",
                path.display()
            )));
        }
    };
    let metadata = file.metadata()?;
    // Older installations keep their state in ~/.hey-boss. Directories are
    // not checkout markers, including when walking up from a nested folder.
    if metadata.is_dir() {
        return Ok(None);
    }
    if !metadata.is_file() {
        return Err(Error::invalid(format!(
            "{} must be a regular file",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(Error::invalid(format!("{} is too large", path.display())));
    }
    Ok(Some(bytes))
}

pub(super) fn read(root: &Path) -> Result<Option<Project>> {
    let Some(bytes) = contents(root)? else {
        return Ok(None);
    };
    let project: Project = serde_json::from_slice(&bytes).map_err(|_| {
        Error::invalid(format!(
            "Invalid {}: expected a project id and name",
            root.join(FILE).display()
        ))
    })?;
    identifier(&project.id, ".hey-boss project id", 8192)?;
    identifier(&project.name, ".hey-boss project name", 512)?;
    Ok(Some(project))
}

/// Capture the local file before reviewing settings; cancellation never writes it.
pub struct CheckoutBinding {
    root: PathBuf,
    previous: Option<Vec<u8>>,
    ordinary: Project,
}

impl CheckoutBinding {
    pub fn is_custom(&self, project: &Project) -> bool {
        project != &self.ordinary
    }

    pub fn prepare(cwd: &Path, machine: &str) -> Result<Self> {
        let cwd = cwd.canonicalize()?;
        let (root, ordinary) =
            if let Some(git) = crate::agents::git_repository_info(&cwd.to_string_lossy()) {
                (
                    PathBuf::from(&git.worktree),
                    super::inherited_git_project(&git, machine)?,
                )
            } else {
                let root = cwd.clone();
                let mut inherited = None;
                for parent in cwd.ancestors().skip(1) {
                    if let Some(project) = read(parent)? {
                        inherited = Some(project);
                        break;
                    }
                }
                let ordinary = inherited.unwrap_or_else(|| Project {
                    id: format!("local:{machine}:{}", root.display()),
                    name: root
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("directory")
                        .into(),
                });
                (root, ordinary)
            };
        // Validate an existing binding before changing any project settings.
        read(&root)?;
        let previous = contents(&root)?;
        Ok(Self {
            root,
            previous,
            ordinary,
        })
    }

    pub fn save(&self, project: &Project) -> Result<()> {
        if contents(&self.root)? != self.previous {
            return Err(Error::conflict(
                ".hey-boss changed during setup; run project init again",
            ));
        }
        let path = self.root.join(FILE);
        if !self.is_custom(project) {
            if self.previous.is_some() {
                fs::remove_file(path)?;
            }
            return Ok(());
        }
        let mut bytes = serde_json::to_vec_pretty(project)?;
        bytes.push(b'\n');
        if self.previous.as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        let temporary = self.root.join(format!(
            ".hey-boss-{}.tmp",
            crate::issues::worker::random_id()?
        ));
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        let _ = fs::remove_file(temporary);
        result
    }
}
