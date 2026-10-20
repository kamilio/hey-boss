//! Provision once from 1Password; normal reads never call a provisioning source.
use crate::{Error, Result, Secret, backend};
use std::{path::Path, process::Command};
pub fn read(home: &Path, reference: &str, fallback: Option<(&str, &str)>) -> Result<Secret> {
    if !reference.starts_with("op://Agents/") || reference.chars().any(char::is_control) {
        return Err(Error::InvalidInput);
    }
    let mut cmd = Command::new(home.join(".hey-proxy/op-agent"));
    cmd.args(["read", "--no-newline", reference]);
    match backend::command(cmd, &[]) {
        Ok(bytes) => Ok(bytes),
        Err(_) => {
            #[cfg(target_os = "macos")]
            if let Some((service, account)) = fallback {
                return backend::Keychain::read_service(service, account)?.ok_or(Error::NotFound);
            }
            let _ = fallback;
            Err(Error::StoreUnavailable)
        }
    }
}
