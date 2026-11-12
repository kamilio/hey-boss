//! An ad-hoc Mac executable can lose Keychain access when it is rebuilt.
//! Install a private, create-only reader before writing its first identity.
use super::*;
use std::{fs, io::ErrorKind, os::unix::fs::DirBuilderExt};

const READER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/keychain-reader"));
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
    fn call(reader: &Path, op: &str, key: &str, input: &[u8]) -> Result<(i32, Secret)> {
        let mut cmd = Command::new(reader);
        cmd.env_clear().args([op, key]);
        let reply = command(cmd, input)?;
        let (status, value) = reply
            .expose()
            .split_first_chunk::<4>()
            .ok_or(Error::StoreUnavailable)?;
        Ok((i32::from_be_bytes(*status), Secret::new(value.to_vec())))
    }

    fn native_read(&self, key: &str) -> Result<Option<Secret>> {
        let Some(reader) = self.reader()? else {
            return Ok(None);
        };
        match Self::call(&reader, "read", key, &[])? {
            (0, bytes) => Ok(Some(bytes)),
            (-25300, _) => Ok(None),
            _ => Err(Error::StoreUnavailable),
        }
    }

    fn imported(&self) -> Self {
        Self(self.0.join("imported"))
    }

    pub fn migrate_identity(home: &Path, card: &crate::Enrollment) -> Result<()> {
        Self::new(home).migrate("identity", "identity.native-v1", card)
    }

    fn migrate(&self, key: &str, dest: &str, card: &crate::Enrollment) -> Result<()> {
        card.validate()?;
        let verify = |raw: &Secret| -> Result<()> {
            let id: crate::crypto::Identity =
                serde_json::from_slice(raw.expose()).map_err(|_| Error::InvalidRecord)?;
            if id.enrollment(&card.machine.label)? != *card {
                return Err(Error::InvalidRecord);
            }
            Ok(())
        };
        let reader = self.reader()?.ok_or(Error::StoreUnavailable)?;
        let imported = self.imported();
        if let Some(raw) = imported.native_read(dest)? {
            return verify(&raw);
        }
        match Self::call(&reader, "read", key, &[])? {
            (0, raw) => return verify(&raw),
            (-25293 | -25308, _) => (),
            _ => return Err(Error::StoreUnavailable),
        }
        let raw = Self::read_service(SERVICE, key)?.ok_or(Error::NotInitialized)?;
        verify(&raw)?;
        match imported.create(dest, raw.expose()) {
            Err(Error::Conflict) => match imported.native_read(dest)? {
                Some(copy) if copy.expose() == raw.expose() => Ok(()),
                _ => Err(Error::Conflict),
            },
            result => result,
        }
    }

    /// Legacy identities and explicitly requested imports keep their original trusted accessor.
    pub fn read_service(service: &str, account: &str) -> Result<Option<Secret>> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let input =
            serde_json::to_vec(&serde_json::json!({"service": service, "account": account}))
                .map_err(|_| Error::InvalidInput)?;
        let mut cmd = Command::new("/usr/bin/osascript");
        cmd.args(["-l", "JavaScript", "-e", include_str!("keychain.js")]);
        let output = command(cmd, &input)?;
        #[derive(serde::Deserialize)]
        struct Reply {
            status: i32,
            data: Option<String>,
        }
        let reply: Reply =
            serde_json::from_slice(output.expose()).map_err(|_| Error::StoreUnavailable)?;
        match reply.status {
            -25300 => Ok(None),
            0 => {
                let data = zeroize::Zeroizing::new(reply.data.ok_or(Error::InvalidRecord)?);
                Ok(Some(Secret::new(
                    STANDARD
                        .decode(data.as_bytes())
                        .map_err(|_| Error::InvalidRecord)?,
                )))
            }
            _ => Err(Error::StoreUnavailable),
        }
    }
}
impl Backend for Keychain {
    fn name(&self) -> &'static str {
        "keychain"
    }
    fn get(&self, key: &str) -> Result<Option<Secret>> {
        if key == "identity"
            && let Some(raw) = self.imported().native_read("identity.native-v1")?
        {
            return Ok(Some(raw));
        }
        if let Some(reader) = self.reader()? {
            match Self::call(&reader, "read", key, &[])? {
                (0, bytes) => return Ok(Some(bytes)),
                (-25300, _) => return Ok(None),
                // An old client may have enrolled first. Only its original
                // authorized process may read that item, still without prompting.
                (-25293 | -25308, _) => (),
                _ => return Err(Error::StoreUnavailable),
            }
        }
        Self::read_service(SERVICE, key)
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
        match Self::call(&reader, "create", key, value)?.0 {
            0 => match Self::call(&reader, "read", key, &[])? {
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
    #[ignore = "uses an unlocked macOS Keychain with disposable synthetic items"]
    fn migrates_only_the_enrolled_identity_and_preserves_the_original() {
        let original = Item(format!("synthetic-old-{}", rand::random::<u128>()));
        let destination = Item(format!("synthetic-copy-{}", rand::random::<u128>()));
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let vault = Keychain(temp.path().join("private"));
        vault.prepare().unwrap();
        let id = crate::crypto::Identity::generate();
        let card = id.enrollment("fixture").unwrap();
        let wrong = crate::crypto::Identity::generate()
            .enrollment("fixture")
            .unwrap();
        let raw = Secret::new(crate::crypto::encode(&id).unwrap());
        // Build the fixture with the actual previous writer and its original Keychain ACL.
        use base64::Engine;
        let writer = Command::new("git")
            .args(["show", "b8304b4d:packages/hey-boss-secrets/src/keychain.js"])
            .output()
            .unwrap();
        assert!(writer.status.success());
        let mut legacy = Command::new("/usr/bin/osascript");
        legacy.args([
            "-l",
            "JavaScript",
            "-e",
            std::str::from_utf8(&writer.stdout).unwrap(),
        ]);
        let setup = serde_json::to_vec(&serde_json::json!({
            "op": "put", "service": SERVICE, "account": original.0,
            "data": base64::engine::general_purpose::STANDARD.encode(raw.expose()),
        }))
        .unwrap();
        let status = command(legacy, &setup).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(status.expose()).unwrap()["status"],
            0
        );
        let migrate = |card| vault.migrate(&original.0, &destination.0, card);
        assert_eq!(migrate(&wrong), Err(Error::InvalidRecord));
        assert!(
            vault
                .imported()
                .native_read(&destination.0)
                .unwrap()
                .is_none()
        );
        migrate(&card).unwrap();
        migrate(&card).unwrap();
        assert_eq!(migrate(&wrong), Err(Error::InvalidRecord));
        assert_eq!(
            vault
                .imported()
                .native_read(&destination.0)
                .unwrap()
                .unwrap()
                .expose(),
            raw.expose()
        );
        assert_eq!(
            Keychain::read_service(SERVICE, &original.0)
                .unwrap()
                .unwrap()
                .expose(),
            raw.expose()
        );
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
        assert!(vault.get(&item.0).unwrap().is_none());
        vault.create(&item.0, b"fixture\0key\xff").unwrap();
        assert_eq!(vault.create(&item.0, b"replacement"), Err(Error::Conflict));
        assert_eq!(
            Keychain(dir).get(&item.0).unwrap().unwrap().expose(),
            b"fixture\0key\xff"
        );
        // A different host process is not authorized to read the new item.
        assert_eq!(
            Keychain::read_service(SERVICE, &item.0).unwrap_err(),
            Error::StoreUnavailable
        );
    }
}
