//! Platform storage. No plaintext disk fallback and no interactive prompts.
use crate::{Error, MAX_SECRET, Result, Secret};
use std::os::unix::{
    fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    process::CommandExt,
};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub trait Backend {
    fn name(&self) -> &'static str;
    fn get(&self, key: &str) -> Result<Option<Secret>>;
    fn put(&self, key: &str, value: &[u8]) -> Result<()>;
}
pub fn native(home: &Path) -> Box<dyn Backend> {
    #[cfg(target_os = "macos")]
    {
        let _ = home;
        Box::new(Keychain)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(Pass::new(home.join(".password-store")))
    }
}

/// Bounded subprocess with private stdin/stdout and discarded stderr.
pub fn command(mut cmd: Command, input: &[u8]) -> Result<Secret> {
    let mut child = cmd
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::StoreUnavailable)?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let bytes = zeroize::Zeroizing::new(input.to_vec());
    let write = std::thread::spawn(move || stdin.write_all(&bytes));
    let read = std::thread::spawn(move || {
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        stdout
            .take(2 * MAX_SECRET as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => break None,
        }
    };
    // Kill only this command's group, including helpers holding an inherited pipe.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
    let _ = write.join();
    let mut output = read
        .join()
        .map_err(|_| Error::StoreUnavailable)?
        .map_err(|_| Error::StoreUnavailable)?;
    if status.is_none_or(|s| !s.success()) || output.len() > 2 * MAX_SECRET {
        return Err(Error::StoreUnavailable);
    }
    Ok(Secret::new(std::mem::take(&mut *output)))
}
pub(crate) fn private_dir(path: &Path) -> Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(Error::StorageFailure);
    }
    std::fs::create_dir_all(path).map_err(|_| Error::StorageFailure)?;
    if std::fs::metadata(path)
        .map_err(|_| Error::StorageFailure)?
        .uid()
        != unsafe { libc::geteuid() }
    {
        return Err(Error::StorageFailure);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| Error::StorageFailure)
}
pub(crate) fn private_file(path: &Path) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| Error::StorageFailure)?;
    let metadata = file.metadata().map_err(|_| Error::StorageFailure)?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1
    {
        return Err(Error::StorageFailure);
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|_| Error::StorageFailure)?;
    Ok(file)
}
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or(Error::InvalidInput)?;
    if !parent.is_dir() {
        return Err(Error::StorageFailure);
    }
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(Error::StorageFailure);
    }
    let stage = parent.join(format!(".hey-boss-{}", rand::random::<u128>()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&stage)
            .map_err(|_| Error::StorageFailure)?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| Error::StorageFailure)?;
        std::fs::rename(&stage, path).map_err(|_| Error::StorageFailure)?;
        std::fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::StorageFailure)
    })();
    let _ = std::fs::remove_file(stage);
    result
}
pub struct Pass {
    root: PathBuf,
}
impl Pass {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    fn path(&self, key: &str) -> Result<PathBuf> {
        if key.is_empty()
            || key.split('/').any(|s| {
                s.is_empty()
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            })
        {
            return Err(Error::InvalidInput);
        }
        let mut path = self.root.clone();
        for component in format!("hey-boss/{key}.gpg").split('/') {
            path.push(component);
            if path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                return Err(Error::StorageFailure);
            }
        }
        Ok(path)
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new("pass");
        c.env("PASSWORD_STORE_DIR", &self.root)
            .env("PASSWORD_STORE_ENABLE_EXTENSIONS", "false")
            .env(
                "PASSWORD_STORE_GPG_OPTS",
                "--batch --pinentry-mode error --no-auto-check-trustdb",
            );
        c
    }
}
impl Backend for Pass {
    fn name(&self) -> &'static str {
        "pass"
    }
    fn get(&self, key: &str) -> Result<Option<Secret>> {
        let path = self.path(key)?;
        if !path.try_exists().map_err(|_| Error::StorageFailure)? {
            return Ok(None);
        }
        let mut cmd = self.cmd();
        cmd.args(["show", &format!("hey-boss/{key}")]);
        // pass stores base64 so arbitrary bytes and trailing newlines round-trip.
        let encoded = command(cmd, &[])?;
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD
            .decode(encoded.expose().trim_ascii())
            .map_err(|_| Error::InvalidRecord)?;
        Ok(Some(Secret::new(data)))
    }
    fn put(&self, key: &str, value: &[u8]) -> Result<()> {
        use base64::Engine;
        if value.len() > MAX_SECRET + 8192 {
            return Err(Error::TooLarge);
        }
        let dest = self.path(key)?;
        private_dir(dest.parent().unwrap())?;
        let stage = format!("pending-{}", rand::random::<u128>());
        let staging = self.path(&stage)?;
        let result = (|| {
            let encoded =
                zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(value));
            // Write pass-compatible ciphertext directly so an existing pass Git
            // repository cannot invoke hooks or commit temporary cache entries.
            let recipients = dest
                .parent()
                .unwrap()
                .ancestors()
                .take_while(|p| p.starts_with(&self.root))
                .map(|p| p.join(".gpg-id"))
                .find(|p| p.is_file())
                .ok_or(Error::StoreUnavailable)?;
            let ids = std::fs::read_to_string(&recipients).map_err(|_| Error::StoreUnavailable)?;
            let mut cmd = Command::new("gpg");
            cmd.args([
                "--batch",
                "--pinentry-mode",
                "error",
                "--trust-model",
                "always",
                "--encrypt",
                "--output",
            ])
            .arg(&staging);
            let mut count = 0;
            for id in ids.lines().map(str::trim).filter(|s| !s.is_empty()) {
                if !id.bytes().all(|b| b.is_ascii_hexdigit()) || id.len() < 16 {
                    return Err(Error::InvalidInput);
                }
                cmd.arg("--recipient").arg(id);
                count += 1;
            }
            if count == 0 {
                return Err(Error::StoreUnavailable);
            }
            command(cmd, encoded.as_bytes())?;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&staging)
                .map_err(|_| Error::StorageFailure)?;
            file.sync_all().map_err(|_| Error::StorageFailure)?;
            std::fs::rename(&staging, &dest).map_err(|_| Error::StorageFailure)?;
            std::fs::File::open(dest.parent().unwrap())
                .and_then(|f| f.sync_all())
                .map_err(|_| Error::StorageFailure)
        })();
        let _ = std::fs::remove_file(staging);
        result
    }
}

#[cfg(target_os = "macos")]
pub struct Keychain;
#[cfg(target_os = "macos")]
impl Keychain {
    fn call(service: &str, account: &str, value: Option<&[u8]>) -> Result<Option<Secret>> {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        // System-signed helper avoids tying credentials to an ad-hoc CLI hash.
        // Its Security calls explicitly forbid interaction, including on a locked Keychain.
        let mut request = serde_json::json!({"op":if value.is_some(){"put"}else{"get"},"service":service,"account":account});
        if let Some(value) = value {
            request["data"] = B64.encode(value).into();
        }
        let input = Secret::new(serde_json::to_vec(&request).map_err(|_| Error::InvalidInput)?);
        if let Some(serde_json::Value::String(data)) = request.get_mut("data") {
            zeroize::Zeroize::zeroize(data);
        }
        let mut cmd = Command::new("/usr/bin/osascript");
        cmd.args(["-l", "JavaScript", "-e", include_str!("keychain.js")]);
        let output = command(cmd, input.expose())?;
        #[derive(serde::Deserialize)]
        struct Reply {
            status: i32,
            data: Option<String>,
        }
        let reply: Reply =
            serde_json::from_slice(output.expose()).map_err(|_| Error::StoreUnavailable)?;
        match reply.status {
            -25300 => Ok(None),
            0 => match reply.data {
                Some(data) => {
                    let data = zeroize::Zeroizing::new(data);
                    Ok(Some(Secret::new(
                        B64.decode(data.as_bytes())
                            .map_err(|_| Error::InvalidRecord)?,
                    )))
                }
                None => Ok(None),
            },
            _ => Err(Error::StoreUnavailable),
        }
    }
    pub fn read_service(service: &str, account: &str) -> Result<Option<Secret>> {
        Self::call(service, account, None)
    }
}
#[cfg(target_os = "macos")]
impl Backend for Keychain {
    fn name(&self) -> &'static str {
        "keychain"
    }
    fn get(&self, key: &str) -> Result<Option<Secret>> {
        Self::read_service("hey-boss.secrets", key)
    }
    fn put(&self, key: &str, value: &[u8]) -> Result<()> {
        Self::call("hey-boss.secrets", key, Some(value)).map(|_| ())
    }
}

/// Only explicit initialization may create a Linux store/key. Existing keys stay intact.
#[cfg(not(target_os = "macos"))]
pub fn prepare_native(home: &Path) -> Result<()> {
    let root = home.join(".password-store");
    if root.join(".gpg-id").is_file() {
        return Ok(());
    }
    private_dir(&root)?;
    let label = format!("hey-boss-{}", rand::random::<u128>());
    let mut generate = Command::new("gpg");
    generate.args([
        "--batch",
        "--pinentry-mode",
        "error",
        "--passphrase",
        "",
        "--quick-generate-key",
        &label,
        "default",
        "default",
        "0",
    ]);
    command(generate, &[])?;
    let mut inspect = Command::new("gpg");
    inspect.args(["--batch", "--with-colons", "--list-secret-keys", &label]);
    let output = command(inspect, &[])?;
    let text = std::str::from_utf8(output.expose()).map_err(|_| Error::StoreUnavailable)?;
    let fingerprint = text
        .lines()
        .find_map(|l| {
            let f: Vec<_> = l.split(':').collect();
            (f.first() == Some(&"fpr"))
                .then(|| f.get(9).copied())
                .flatten()
        })
        .ok_or(Error::StoreUnavailable)?;
    let mut init = Pass::new(root).cmd();
    init.args(["init", fingerprint]);
    command(init, &[])?;
    Ok(())
}
#[cfg(target_os = "macos")]
pub fn prepare_native(_home: &Path) -> Result<()> {
    Ok(())
}
