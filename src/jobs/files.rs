//! Content-addressed Markdown, published and fsynced before committing references.
use crate::issues::{Error, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
pub(crate) fn digest(markdown: &str) -> String {
    format!("{:x}", Sha256::digest(markdown.as_bytes()))
}
pub(crate) fn validate(markdown: &str) -> Result<()> {
    if markdown.len() > crate::issues::BODY_LIMIT || markdown.trim().is_empty() {
        return Err(Error::invalid(
            "Instructions must be nonempty UTF-8 Markdown, at most 1 MiB",
        ));
    }
    Ok(())
}
fn path(root: &Path, digest: &str) -> Result<PathBuf> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::invalid("Invalid instruction digest"));
    }
    Ok(root.join(format!("{digest}.md")))
}
fn directory(root: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => {
            if let Some(parent) = root.parent() {
                File::open(parent)?.sync_all()?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(Error::invalid(
            "Instruction directory must not be a symlink",
        ));
    }
    Ok(())
}
pub(crate) fn load(root: &Path, sha: &str) -> Result<String> {
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(Error::invalid("Invalid instruction directory"));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path(root, sha)?)?;
    if !file.metadata()?.is_file() {
        return Err(Error::invalid(
            "Instructions must be a regular Markdown file",
        ));
    }
    let mut text = String::new();
    file.take(crate::issues::BODY_LIMIT as u64 + 1)
        .read_to_string(&mut text)?;
    validate(&text)?;
    if digest(&text) != sha {
        return Err(Error::new(
            "instruction_corrupt",
            "Instruction digest does not match the immutable revision",
        ));
    }
    Ok(text)
}
pub(crate) fn persist(root: &Path, markdown: &str) -> Result<String> {
    validate(markdown)?;
    directory(root)?;
    let sha = digest(markdown);
    let target = path(root, &sha)?;
    if target.try_exists()? {
        load(root, &sha)?;
        return Ok(sha);
    }
    struct Stage(PathBuf);
    impl Drop for Stage {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let stage = Stage(root.join(format!(".{}.tmp", crate::issues::worker::random_id()?)));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(&stage.0)?;
    file.write_all(markdown.as_bytes())?;
    file.sync_all()?;
    match fs::hard_link(&stage.0, &target) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            load(root, &sha)?;
        }
        Err(e) => return Err(e.into()),
    }
    drop(stage);
    File::open(root)?.sync_all()?;
    Ok(sha)
}
/// Receipt/revision responses carry exact bytes; a companion acknowledges only after fsync.
pub(crate) fn cache_response(root: &Path, response: &serde_json::Value) -> Result<()> {
    if let Some(bundle) = response.get("instructions") {
        let text = bundle["markdown"]
            .as_str()
            .ok_or_else(|| Error::invalid("Missing instruction bytes"))?;
        let expected = bundle["digest"]
            .as_str()
            .ok_or_else(|| Error::invalid("Missing instruction digest"))?;
        if digest(text) != expected {
            return Err(Error::new(
                "instruction_corrupt",
                "Transferred instruction digest mismatch",
            ));
        }
        persist(root, text)?;
    }
    Ok(())
}
