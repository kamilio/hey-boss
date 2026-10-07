//! On-disk credentials use authenticated encryption; plaintext exists only in memory.
use aes_gcm_siv::{
    Aes256GcmSiv, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::TryRngCore;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const AAD: &[u8] = b"hey-proxy config credentials v1";

fn visit(value: &mut Value, mut change: impl FnMut(&mut Value) -> Result<()>) -> Result<()> {
    for pointer in ["/api_keys", "/providers/openai/api_keys"] {
        if let Some(keys) = value.pointer_mut(pointer).and_then(Value::as_object_mut) {
            for key in keys.values_mut() {
                change(key)?;
            }
        }
    }
    for pointer in [
        "/gemini/api_key",
        "/providers/gemini/api_key",
        "/connection/api_key",
    ] {
        if let Some(key) = value.pointer_mut(pointer).filter(|v| !v.is_null()) {
            change(key)?;
        }
    }
    Ok(())
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Cannot generate credential encryption randomness"))?;
    Ok(bytes)
}

pub(super) fn key(config: &Path, create: bool) -> Result<[u8; 32]> {
    let path = config.with_extension("credentials.key");
    if create && !path.exists() {
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
        file.write_all(&random::<32>()?)?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(&path) {
            Ok(_) => {
                sync_parent(&path)?;
            }
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => bail!("Cannot save credential encryption key"),
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&path).context(
        "Cannot read credential encryption key; restore the matching .credentials.key file",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            bail!("Credential encryption key must be private (chmod 600)");
        }
    }
    if !file.metadata()?.is_file() || file.metadata()?.len() != 32 {
        bail!("Invalid credential encryption key");
    }
    let mut bytes = [0; 32];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
    Ok(())
}

pub(super) fn decrypt(value: &mut Value, path: &Path) -> Result<()> {
    let mut cipher = None;
    visit(value, |value| {
        if !value.is_object() {
            return Ok(());
        }
        let encrypted = value
            .as_object()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.get("encrypted"))
            .and_then(Value::as_str)
            .and_then(|v| v.strip_prefix("v1:"))
            .context("Invalid encrypted credential field")?;
        let bytes = STANDARD
            .decode(encrypted)
            .ok()
            .filter(|b| b.len() >= 28)
            .context("Invalid encrypted credential encoding")?;
        if cipher.is_none() {
            // Only encrypted credentials need a file-backed key. Resolve links
            // here so a config symlink uses the actual config's sibling key.
            let path = fs::canonicalize(path).context("Cannot locate encrypted config")?;
            cipher = Some(Aes256GcmSiv::new((&key(&path, false)?).into()));
        }
        let plaintext = cipher
            .as_ref()
            .unwrap()
            .decrypt(
                Nonce::from_slice(&bytes[..12]),
                Payload {
                    msg: &bytes[12..],
                    aad: AAD,
                },
            )
            .map_err(|_| {
                anyhow::anyhow!("Cannot decrypt credential; key mismatch or damaged ciphertext")
            })?;
        *value =
            Value::String(String::from_utf8(plaintext).context("Invalid decrypted credential")?);
        Ok(())
    })
}

/// Validate before changing anything. Atomic replacement never creates a plaintext backup.
pub(super) fn protect(path: &Path) -> Result<super::Config> {
    let path = fs::canonicalize(path).context("Cannot locate config")?;
    let config = super::load(&path)?;
    let mut value: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let mut needs_encryption = false;
    visit(&mut value, |v| {
        needs_encryption |= v.as_str().is_some_and(|s| {
            !s.starts_with("sh://") && !s.starts_with("op://") && !s.starts_with("file://")
        });
        Ok(())
    })?;
    if !needs_encryption {
        return Ok(config);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options.open(path.with_extension("credentials.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("Credential migration is busy; retry shortly")?;
    let original = fs::read(&path)?;
    let config = super::parse(&original, &path)?;
    let mut value: Value = serde_json::from_slice(&original)?;
    let cipher = Aes256GcmSiv::new((&key(&path, true)?).into());
    visit(&mut value, |value| {
        let Some(secret) = value.as_str().filter(|s| {
            !s.starts_with("sh://") && !s.starts_with("op://") && !s.starts_with("file://")
        }) else {
            return Ok(());
        };
        let nonce = random::<12>()?;
        let encrypted = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: secret.as_bytes(),
                    aad: AAD,
                },
            )
            .map_err(|_| anyhow::anyhow!("Cannot encrypt credential"))?;
        let mut bytes = nonce.to_vec();
        bytes.extend(encrypted);
        *value = json!({"encrypted":format!("v1:{}", STANDARD.encode(bytes))});
        Ok(())
    })?;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.write_all(&serde_json::to_vec_pretty(&value)?)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    // Do not overwrite an editor's changes made while encryption was running.
    if fs::read(&path)? != original {
        bail!("Config changed during credential migration; retry");
    }
    file.persist(&path)
        .context("Cannot save encrypted config")?;
    sync_parent(&path)?;
    Ok(config)
}
