//! An ad-hoc Mac executable can lose Keychain access when it is rebuilt.
//! Install a private, create-only reader before writing its first identity.
use super::*;
use std::{fs, io::ErrorKind, os::unix::fs::DirBuilderExt};

const READER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/keychain-reader"));
#[cfg(test)]
const SERVICE: &str = "hey-boss.secrets";

pub struct Keychain(PathBuf);

impl Keychain {
    pub(super) fn new(home: &Path) -> Self {
        Self(home.join(".local/share/hey-boss/keychain"))
    }
    pub(super) fn prepare(&self) -> Result<()> {
        if self.reader()?.is_none() {
            self.install()?;
        }
        Ok(())
    }
    fn reader(&self) -> Result<Option<PathBuf>> {
        let dir = match fs::symlink_metadata(&self.0) {
            Ok(dir) => dir,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::StoreUnavailable),
        };
        let owner = unsafe { libc::geteuid() };
        if !dir.is_dir() || dir.uid() != owner || dir.mode() & 0o077 != 0 {
            return Err(Error::StoreUnavailable);
        }
        let path = self.0.join("reader-v1");
        match fs::symlink_metadata(&path) {
            Ok(f) if f.is_file() && f.uid() == owner && f.mode() & 0o7777 == 0o500 => {
                Ok(Some(path))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            _ => Err(Error::StoreUnavailable),
        }
    }
    fn install(&self) -> Result<PathBuf> {
        if self.reader()?.is_none() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.0)
                .map_err(|_| Error::StorageFailure)?;
            // Validate an existing directory too; never chmod a symlink or shared path.
            if self.reader()?.is_none() {
                let mut temp =
                    tempfile::NamedTempFile::new_in(&self.0).map_err(|_| Error::StorageFailure)?;
                temp.write_all(READER).map_err(|_| Error::StorageFailure)?;
                temp.as_file()
                    .set_permissions(fs::Permissions::from_mode(0o500))
                    .map_err(|_| Error::StorageFailure)?;
                temp.as_file()
                    .sync_all()
                    .map_err(|_| Error::StorageFailure)?;
                match temp.persist_noclobber(self.0.join("reader-v1")) {
                    Ok(_) => (),
                    Err(e) if e.error.kind() == ErrorKind::AlreadyExists => (),
                    Err(_) => return Err(Error::StorageFailure),
                }
            }
        }
        fs::File::open(&self.0)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::StorageFailure)?;
        self.reader()?.ok_or(Error::StoreUnavailable)
    }
    fn call(reader: &Path, args: &[&str], input: &[u8]) -> Result<(i32, Secret)> {
        let mut cmd = Command::new(reader);
        cmd.env_clear().args(args);
        let reply = command(cmd, input)?;
        let (status, value) = reply
            .expose()
            .split_first_chunk::<4>()
            .ok_or(Error::StoreUnavailable)?;
        Ok((i32::from_be_bytes(*status), Secret::new(value.to_vec())))
    }

    fn read(reader: &Path, args: &[&str]) -> Result<Option<Secret>> {
        match Self::call(reader, args, &[])? {
            (0, bytes) => Ok(Some(bytes)),
            (-25300, _) => Ok(None),
            _ => Err(Error::StoreUnavailable),
        }
    }

    /// Explicit external imports have their own retained reader and no write operation.
    pub fn read_service(service: &str, account: &str) -> Result<Option<Secret>> {
        if service.is_empty() || account.is_empty() || service == "hey-boss.secrets" {
            return Err(Error::InvalidInput);
        }
        let home = std::env::home_dir().ok_or(Error::StoreUnavailable)?;
        let path = Self::new(&home);
        path.prepare()?;
        let reader = Self(path.0.join("external")).install()?;
        Self::read(&reader, &["import", service, account])
    }
}
impl Backend for Keychain {
    fn name(&self) -> &'static str {
        "keychain"
    }
    fn get(&self, key: &str) -> Result<Option<Secret>> {
        // A missing helper does not mean an absent Keychain item.
        let reader = self.reader()?.ok_or(Error::StoreUnavailable)?;
        if key == "identity"
            && let Some(imported) = Self(self.0.join("imported")).reader()?
            && let Some(raw) = Self::read(&imported, &["read", "identity.native-v1"])?
        {
            return Ok(Some(raw));
        }
        Self::read(&reader, &["read", key])
    }
    fn create(&self, key: &str, value: &[u8]) -> Result<()> {
        let reader = self.install()?;
        // Never deliver a newly generated secret to unknown executable bytes.
        if fs::metadata(&reader)
            .map_err(|_| Error::StoreUnavailable)?
            .len()
            != READER.len() as u64
            || fs::read(&reader).map_err(|_| Error::StoreUnavailable)? != READER
        {
            return Err(Error::StoreUnavailable);
        }
        match Self::call(&reader, &["create", key], value)?.0 {
            0 => match Self::call(&reader, &["read", key], &[])? {
                (0, bytes) if bytes.expose() == value => Ok(()),
                _ => Err(Error::StoreUnavailable),
            },
            -25299 => Err(Error::Conflict),
            _ => Err(Error::StoreUnavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Item(String);
    impl Drop for Item {
        fn drop(&mut self) {
            let _ = Command::new("/usr/bin/security")
                .args(["delete-generic-password", "-s", SERVICE, "-a", &self.0])
                .output();
        }
    }

    #[test]
    #[ignore = "uses an unlocked macOS Keychain with a disposable public fixture"]
    fn external_import_distinguishes_a_denied_item_from_a_missing_item() {
        let item = format!("synthetic-import-{}", rand::random::<u128>());
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let reader = Keychain(temp.path().join("external")).install().unwrap();
        let service = "hey-boss.synthetic-public-import";
        let lookup = || Keychain::read(&reader, &["import", service, &item]);
        assert!(lookup().unwrap().is_none());
        // This disposable item contains only the four public bytes below.
        let status = Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-s",
                service,
                "-a",
                &item,
                "-X",
                "4100ff0a",
            ])
            .output()
            .unwrap();
        assert!(status.status.success());
        let read = lookup();
        assert!(
            Command::new("/usr/bin/security")
                .args(["delete-generic-password", "-s", service, "-a", &item])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(read.unwrap_err(), Error::StoreUnavailable);
    }

    #[test]
    fn installing_again_preserves_the_exact_reader_and_rejects_symlinks() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let vault = Keychain(temp.path().join("private"));
        let path = vault.install().unwrap();
        assert_eq!(fs::read(&path).unwrap(), READER);
        let inode = path.metadata().unwrap().ino();
        assert_eq!(vault.install().unwrap().metadata().unwrap().ino(), inode);
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"previous build").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).unwrap();
        vault.install().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"previous build");
        assert_eq!(
            vault.create("unused", b"fixture"),
            Err(Error::StoreUnavailable)
        );

        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(temp.path().join("missing"), &path).unwrap();
        assert_eq!(vault.install(), Err(Error::StoreUnavailable));
    }

    #[test]
    #[ignore = "uses an unlocked macOS Keychain with a disposable synthetic item"]
    fn native_reader_recovers_binary_data_and_never_replaces_an_existing_identity() {
        let item = Item(format!("synthetic-{}", rand::random::<u128>()));
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let dir = temp.path().join("private");
        let vault = Keychain(dir.clone());
        assert_eq!(vault.get(&item.0).unwrap_err(), Error::StoreUnavailable);
        vault.prepare().unwrap();
        assert!(vault.get(&item.0).unwrap().is_none());
        vault.create(&item.0, b"fixture\0key\xff").unwrap();
        assert_eq!(vault.create(&item.0, b"replacement"), Err(Error::Conflict));
        assert_eq!(
            Keychain(dir).get(&item.0).unwrap().unwrap().expose(),
            b"fixture\0key\xff"
        );
        // The provisioning API must never import the vault’s own private keys.
        assert_eq!(
            Keychain::read_service(SERVICE, &item.0).unwrap_err(),
            Error::InvalidInput
        );
    }
}
