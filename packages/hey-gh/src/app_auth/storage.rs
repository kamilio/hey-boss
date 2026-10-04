use super::*;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Credentials {
    client_id: String,
    installation_id: u64,
    repositories: Vec<String>,
    private_key: String,
}

impl Credentials {
    fn installation(&self) -> Result<AppInstallation> {
        AppInstallation::new(
            self.client_id.clone(),
            self.installation_id,
            self.repositories.clone(),
            &self.private_key,
        )
    }
}

fn path(hostname: &str) -> Result<PathBuf> {
    if hostname.is_empty()
        || !hostname
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err(Error::Invalid("invalid GitHub App hostname".into()));
    }
    Ok(dirs::data_local_dir()
        .ok_or_else(|| Error::Invalid("user data directory unavailable".into()))?
        .join("hey-gh/apps")
        .join(format!("{hostname}.json")))
}

/// Load private daemon credentials. Missing configuration preserves gh-only
/// behavior; malformed or unsafe configuration fails without exposing its text.
pub fn load(hostname: &str) -> Result<Option<AppInstallation>> {
    load_path(&path(hostname)?)
}

fn load_path(path: &Path) -> Result<Option<AppInstallation>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(storage()),
    };
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return Err(storage());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(storage());
        }
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| storage())?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| storage())?;
    if bytes.len() > 64 * 1024 {
        return Err(storage());
    }
    let credentials: Credentials = serde_json::from_slice(&bytes).map_err(|_| storage())?;
    credentials.installation().map(Some)
}

/// Supply the key only through HEY_GH_APP_PRIVATE_KEY, for example using the
/// hey-boss secret-entry command. Never pass credentials in command arguments.
pub fn configure(
    hostname: &str,
    client_id: String,
    installation_id: u64,
    repositories: Vec<String>,
) -> Result<()> {
    let private_key = std::env::var("HEY_GH_APP_PRIVATE_KEY").map_err(|_| {
        Error::Invalid("HEY_GH_APP_PRIVATE_KEY is required; use secure secret entry".into())
    })?;
    save(
        &path(hostname)?,
        &Credentials {
            client_id,
            installation_id,
            repositories,
            private_key,
        },
    )
}

fn save(path: &Path, credentials: &Credentials) -> Result<()> {
    credentials.installation()?;
    let directory = path.parent().ok_or_else(storage)?;
    std::fs::create_dir_all(directory).map_err(|_| storage())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| storage())?;
    }
    let temporary = directory.join(format!("app-{:032x}.tmp", fastrand::u128(..)));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|_| storage())?;
    let result = serde_json::to_writer(&mut file, credentials)
        .map_err(|_| storage())
        .and_then(|_| file.flush().map_err(|_| storage()))
        .and_then(|_| file.sync_all().map_err(|_| storage()))
        .and_then(|_| std::fs::rename(&temporary, path).map_err(|_| storage()));
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn storage() -> Error {
    Error::Storage(
        "GitHub App credentials must be a private, valid file; configure the installation again"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_round_trip_privately_and_reject_unsafe_files_without_disclosure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apps/github.com.json");
        assert!(load_path(&path).unwrap().is_none());
        let credentials = Credentials {
            client_id: "test".into(),
            installation_id: 42,
            repositories: vec!["acme/demo".into()],
            private_key: include_str!("../../tests/fixtures/github-app-test-key.pem").into(),
        };
        save(&path, &credentials).unwrap();
        assert!(load_path(&path).unwrap().unwrap().covers("Acme/Demo"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load_path(&path).is_err());
            let link = dir.path().join("link.json");
            symlink(&path, &link).unwrap();
            assert!(load_path(&link).is_err());
        }
        std::fs::write(&path, "synthetic-private-material").unwrap();
        let error = load_path(&path).err().unwrap().to_string();
        assert!(!error.contains("synthetic-private-material"));
    }
}
