//! Private, automatic credentials for local daemon callers, separate from gh.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{io::Write, net::SocketAddr, path::PathBuf};

#[derive(Serialize, Deserialize)]
struct Registration {
    token: String,
}
pub struct RegistrationGuard {
    path: PathBuf,
    token: String,
}
impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        let owned = std::fs::read(&self.path)
            .ok()
            .and_then(|data| serde_json::from_slice::<Registration>(&data).ok())
            .is_some_and(|registration| registration.token == self.token);
        if owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn path(port: u16) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hey-gh/instances")
        .join(format!("{port}.json"))
}

/// Register only after successfully binding the listener, so each live port has
/// a single owner. The token never needs to be printed or entered by the user.
pub fn register(address: SocketAddr) -> Result<(String, RegistrationGuard)> {
    if !address.ip().is_loopback() {
        return Err(Error::Invalid(
            "local API registration requires loopback".into(),
        ));
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Storage(format!("local API entropy unavailable: {e}")))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let destination = path(address.port());
    let directory = destination.parent().expect("registry parent");
    std::fs::create_dir_all(directory).map_err(storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(storage)?;
    }
    let temporary = directory.join(format!("{}.{}.tmp", address.port(), fastrand::u64(..)));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(storage)?;
    let write = serde_json::to_writer(
        &mut file,
        &Registration {
            token: token.clone(),
        },
    )
    .map_err(storage)
    .and_then(|_| file.flush().map_err(storage))
    .and_then(|_| std::fs::rename(&temporary, &destination).map_err(storage));
    if let Err(error) = write {
        let _ = std::fs::remove_file(temporary);
        return Err(error);
    }
    Ok((
        token.clone(),
        RegistrationGuard {
            path: destination,
            token,
        },
    ))
}

pub(crate) fn token(port: u16) -> Result<Option<String>> {
    let data = match std::fs::read(path(port)) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(storage(e)),
    };
    let registration: Registration = serde_json::from_slice(&data).map_err(|_| {
        Error::Invalid("invalid local API registration; restart hey-gh serve".into())
    })?;
    Ok(Some(registration.token))
}
fn storage(error: impl std::fmt::Display) -> Error {
    Error::Storage(error.to_string())
}
