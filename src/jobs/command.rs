//! Portable command inputs and content-addressed, chunked file transfer.
use crate::issues::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
};

pub const CHUNK: usize = 256 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Blob {
    pub sha256: String,
    pub size: u64,
}
impl Blob {
    pub fn validate(&self) -> Result<()> {
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.size > 64 * 1024 * 1024 * 1024
        {
            return Err(Error::invalid("Invalid execution file digest or size"));
        }
        Ok(())
    }
    pub fn of(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(Error::invalid("Execution input must be a regular file"));
        }
        let size = file.metadata()?.len();
        let mut hash = Sha256::new();
        let mut bytes = [0u8; CHUNK];
        loop {
            let n = file.read(&mut bytes)?;
            if n == 0 {
                break;
            }
            hash.update(&bytes[..n]);
        }
        let blob = Self {
            sha256: format!("{:x}", hash.finalize()),
            size,
        };
        blob.validate()?;
        Ok(blob)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Blob>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}
impl Spec {
    pub fn validate(&self) -> Result<()> {
        crate::issues::identifier(&self.program, "executable", 4096)?;
        if self.args.len() > 256
            || self.args.iter().any(|s| s.contains('\0'))
            || self.args.iter().map(String::len).sum::<usize>() > 65536
        {
            return Err(Error::invalid(
                "Command arguments exceed 64 KiB or contain NUL",
            ));
        }
        if self.harness.is_some() != self.model.is_some() {
            return Err(Error::invalid("Specify both harness and model"));
        }
        if let Some(h) = &self.harness {
            if !["codex", "claude", "gemini"].contains(&h.as_str()) {
                return Err(Error::invalid("Harness must be codex, claude, or gemini"));
            }
        }
        if let Some(m) = &self.model {
            crate::issues::identifier(m, "model", 256)?;
        }
        if let Some(e) = &self.effort {
            crate::issues::identifier(e, "effort", 64)?;
            if self.harness.is_none() {
                return Err(Error::invalid("Effort requires harness and model"));
            }
        }
        if let Some(b) = &self.input {
            b.validate()?;
        }
        Ok(())
    }
}
pub fn blob_path(root: &Path, project: &str, blob: &Blob) -> Result<PathBuf> {
    blob.validate()?;
    let dir = root
        .join("exec-files")
        .join(format!("{:x}", Sha256::digest(project.as_bytes())));
    fs::create_dir_all(&dir)?;
    Ok(dir.join(&blob.sha256))
}
pub fn put(root: &Path, project: &str, blob: &Blob, offset: u64, data: &str) -> Result<Value> {
    let path = blob_path(root, project, blob)?;
    if data.len() > CHUNK.div_ceil(3) * 4 {
        return Err(Error::invalid("Execution chunk too large"));
    }
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| Error::invalid("Invalid execution file base64"))?;
    if offset > blob.size
        || bytes.len() as u64 > blob.size - offset
        || (bytes.is_empty() && offset < blob.size)
    {
        return Err(Error::invalid("Invalid execution chunk bounds"));
    }
    if path.is_file() {
        if fs::metadata(&path)?.len() != blob.size {
            return Err(Error::invalid("Execution file size mismatch"));
        }
        return Ok(json!({"offset":blob.size,"complete":true}));
    }
    let stage = path.with_extension("part");
    let mut f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&stage)?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // The uploader ahead of us may have renamed this inode while we waited.
    if path.is_file() {
        if fs::metadata(&path)?.len() != blob.size {
            return Err(Error::invalid("Execution file size mismatch"));
        }
        let _ = fs::remove_file(&stage);
        return Ok(json!({"offset":blob.size,"complete":true}));
    }
    let size = f.metadata()?.len();
    if offset > size {
        return Err(Error::conflict(format!(
            "Missing input bytes; next offset is {size}"
        )));
    }
    if offset < size {
        let mut existing = vec![0; bytes.len()];
        f.seek(SeekFrom::Start(offset))?;
        if f.read_exact(&mut existing).is_err() || existing != bytes {
            return Err(Error::conflict("Execution chunk differs from saved bytes"));
        }
    } else {
        f.seek(SeekFrom::End(0))?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    let next = f.metadata()?.len();
    if next == blob.size {
        if Blob::of(&stage)? != *blob {
            fs::remove_file(&stage)?;
            return Err(Error::invalid("Execution file digest mismatch"));
        }
        // A concurrent retry may already have published the same verified bytes.
        if !path.exists() {
            fs::rename(&stage, &path)?;
            File::open(path.parent().unwrap())?.sync_all()?;
        }
    }
    Ok(json!({"offset":next,"complete":next==blob.size}))
}
pub fn get(root: &Path, project: &str, blob: &Blob, offset: u64) -> Result<Value> {
    if offset > blob.size {
        return Err(Error::invalid("Execution file offset exceeds size"));
    }
    let mut f = File::open(blob_path(root, project, blob)?)?;
    if f.metadata()?.len() != blob.size {
        return Err(Error::invalid("Execution file size mismatch"));
    }
    f.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; CHUNK.min((blob.size - offset) as usize)];
    f.read_exact(&mut bytes)?;
    Ok(json!({"data":STANDARD.encode(&bytes),"offset":offset+bytes.len() as u64}))
}
pub fn upload(
    path: &Path,
    mut call: impl FnMut(super::Operation) -> Result<Value>,
) -> Result<Blob> {
    let blob = Blob::of(path)?;
    let mut f = File::open(path)?;
    let mut offset = 0;
    loop {
        f.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; CHUNK.min((blob.size - offset) as usize)];
        f.read_exact(&mut bytes)?;
        let reply = call(super::Operation::FilePut {
            blob: blob.clone(),
            offset,
            data: STANDARD.encode(bytes),
        })?;
        let next = reply["offset"]
            .as_u64()
            .ok_or_else(|| Error::invalid("Missing upload acknowledgment"))?;
        if next > blob.size || (next <= offset && blob.size != 0) {
            return Err(Error::invalid("Invalid upload acknowledgment"));
        }
        offset = next;
        if offset == blob.size {
            break;
        }
    }
    Ok(blob)
}
pub fn download(
    blob: &Blob,
    path: &Path,
    mut call: impl FnMut(super::Operation) -> Result<Value>,
) -> Result<()> {
    blob.validate()?;
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    let result = (|| {
        let mut offset = 0;
        while offset < blob.size {
            let reply = call(super::Operation::FileGet {
                blob: blob.clone(),
                offset,
            })?;
            let data = reply["data"]
                .as_str()
                .ok_or_else(|| Error::invalid("Missing download bytes"))?;
            let bytes = STANDARD
                .decode(data)
                .map_err(|_| Error::invalid("Invalid download base64"))?;
            if bytes.is_empty() || bytes.len() > CHUNK || bytes.len() as u64 > blob.size - offset {
                return Err(Error::invalid("Invalid download chunk"));
            }
            offset += bytes.len() as u64;
            f.write_all(&bytes)?;
        }
        f.sync_all()?;
        if Blob::of(path)? != *blob {
            return Err(Error::invalid("Downloaded execution file digest mismatch"));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

pub fn pack(root: &Path, target: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(target)?;
    let gzip = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut archive = tar::Builder::new(gzip);
    archive.follow_symlinks(false);
    archive.append_dir_all(".", root)?;
    archive.into_inner()?.finish()?.sync_all()?;
    Ok(())
}
pub fn unpack(archive: &Path, target: &Path) -> Result<()> {
    fs::create_dir(target)?;
    let result = (|| {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(archive)?));
        for entry in archive.entries()? {
            let mut entry = entry?;
            let name = entry.path()?.into_owned();
            if name.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            }) {
                return Err(Error::invalid("Unsafe result path"));
            }
            let path = target.join(&name);
            let kind = entry.header().entry_type();
            if kind.is_dir() {
                fs::create_dir_all(path)?;
            } else if kind.is_file() {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)?;
                std::io::copy(&mut entry, &mut f)?;
            } else {
                return Err(Error::invalid(
                    "Result archives may contain only regular files and directories",
                ));
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(target);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exec_concurrent_uploads_of_the_same_input_all_succeed() {
        let root = std::env::current_dir().unwrap().join(format!(
            ".exec-file-test-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::write(&source, vec![42u8; CHUNK]).unwrap();
        let blob = Blob::of(&source).unwrap();
        let bytes = STANDARD.encode(fs::read(&source).unwrap());
        let start = std::sync::Barrier::new(10);
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..10)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        put(&root, "p", &blob, 0, &bytes)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        let saved = Blob::of(&blob_path(&root, "p", &blob).unwrap()).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(saved, blob);
        assert!(results.iter().all(Result::is_ok), "{results:?}");
    }
    #[test]
    fn exec_files_retry_chunks_and_verify_before_publication() {
        let root = std::env::current_dir().unwrap().join(format!(
            ".exec-file-test-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let bytes = vec![42u8; CHUNK + 17];
        let source = root.join("source");
        fs::write(&source, &bytes).unwrap();
        let blob = Blob::of(&source).unwrap();
        let first = STANDARD.encode(&bytes[..CHUNK]);
        put(&root, "p", &blob, 0, &first).unwrap();
        put(&root, "p", &blob, 0, &first).unwrap();
        assert!(get(&root, "p", &blob, 0).is_err());
        assert!(put(&root, "p", &blob, 0, &STANDARD.encode([1, 2, 3])).is_err());
        put(
            &root,
            "p",
            &blob,
            CHUNK as u64,
            &STANDARD.encode(&bytes[CHUNK..]),
        )
        .unwrap();
        let output = root.join("out");
        download(&blob, &output, |op| match op {
            super::super::Operation::FileGet { blob, offset } => get(&root, "p", &blob, offset),
            _ => unreachable!(),
        })
        .unwrap();
        assert_eq!(fs::read(output).unwrap(), bytes);
        assert!(get(&root, "other-project", &blob, 0).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn exec_requires_harness_and_model_together() {
        let mut spec = Spec {
            program: "agent-eval".into(),
            args: vec![],
            input: None,
            harness: Some("claude".into()),
            model: None,
            effort: None,
        };
        assert!(spec.validate().is_err());
        spec.model = Some("exact-model".into());
        assert!(spec.validate().is_ok());
    }
}
