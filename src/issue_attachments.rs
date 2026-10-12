//! CLI attachment paths always belong to the caller, even for SSH/fleet reads.
use hey_boss::{
    attachments::{self, Kind, Operation, Target},
    issues::{self, Error, Request, Result},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

fn private_directory(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(path)?.file_type().is_dir() {
                Ok(())
            } else {
                Err(Error::invalid(
                    "Attachment cache must be a directory, not a symlink",
                ))
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn cached_path(root: &Path, file: &Value) -> Result<PathBuf> {
    let id = file["id"]
        .as_str()
        .ok_or_else(|| Error::invalid("Missing attachment ID"))?;
    Operation::Download { id: id.into() }.validate()?;
    let name = file["name"]
        .as_str()
        .ok_or_else(|| Error::invalid("Missing attachment filename"))?;
    attachments::validate_name(name)?;
    let sha = file["sha256"]
        .as_str()
        .filter(|sha| sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| Error::invalid("Invalid attachment digest"))?;
    private_directory(root)?;
    let directory = root.join(format!("{id}-{sha}"));
    private_directory(&directory)?;
    Ok(directory.join(name))
}

fn verified(path: &Path, file: &Value) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && attachments::read_file(path).is_ok_and(|bytes| {
            file["size"].as_u64() == Some(bytes.len() as u64)
                && file["sha256"].as_str() == Some(format!("{:x}", Sha256::digest(&bytes)).as_str())
        })
}

pub(crate) fn enrich(value: &mut Value, request: &Request, host: Option<&str>) {
    let result = load(value, request, host);
    if let Err(error) = result {
        // An unavailable list is not an empty list. Keep the issue readable and
        // make the missing evidence visible in both text and structured output.
        value["attachments"] = Value::Null;
        value["attachments_error"] = json!(error.message);
        value["attachments_complete"] = json!(false);
    }
}

fn load(value: &mut Value, request: &Request, host: Option<&str>) -> Result<()> {
    let Some(number) = value["issue"]["number"].as_i64() else {
        return Ok(());
    };
    let mut read = request.clone();
    read.project = serde_json::from_value(value["project"].clone())?;
    read.project_override = Some(read.project.id.clone());
    read.request_id = None;
    let mut call = |operation| {
        read.operation = issues::Operation::Attachment { operation };
        // Attachment operations already use the resource relay on companions.
        // They must not go through the narrower --supervisor metadata route.
        crate::cli_request::execute(&read, host, false)
    };
    let mut listed = call(Operation::List {
        target: Target {
            kind: Kind::Issue,
            id: number.to_string(),
        },
    })?;
    let more = listed["more"] == true;
    let files = listed["attachments"]
        .as_array_mut()
        .ok_or_else(|| Error::invalid("Missing attachment list"))?;
    let database = issues::database_path()?;
    // Store::open creates the local database for fleet/local reads. SSH-only
    // readers may not have one yet, but its state directory is still local.
    let parent = database
        .parent()
        .ok_or_else(|| Error::invalid("Missing database directory"))?;
    fs::create_dir_all(parent)?;
    let root = parent
        .canonicalize()?
        .join(database.file_name().unwrap())
        .with_extension("attachment-cache");
    let mut complete = !more;
    for file in files.iter_mut() {
        let local = (|| -> Result<PathBuf> {
            let path = cached_path(&root, file)?;
            if verified(&path, file) {
                return Ok(path);
            }
            let downloaded = call(Operation::Download {
                id: file["id"].as_str().unwrap().into(),
            })?;
            if downloaded["attachment"] != *file {
                return Err(Error::conflict(
                    "Attachment changed while reading; run the command again",
                ));
            }
            // Never overwrite an existing cache entry. A concurrent reader may
            // finish first; accept it only after checking the complete contents.
            match attachments::materialize(&downloaded, Some(&path)) {
                Ok(path) => Ok(path),
                Err(_) if verified(&path, file) => Ok(path),
                Err(error) => Err(error),
            }
        })();
        match local {
            Ok(path) => file["path"] = json!(path),
            Err(error) => {
                complete = false;
                file["path"] = Value::Null;
                file["error"] = json!(error.message);
            }
        }
    }
    value["attachments"] = json!(files);
    value["attachments_complete"] = json!(complete);
    Ok(())
}
