//! Local encrypted store integration; deliberately independent of the database service.
use hey_boss_secrets::{Error, Result, Store, backend};
use std::path::PathBuf;
pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or(Error::StoreUnavailable)
}
pub fn open() -> Result<Store> {
    let db = crate::issues::database_path().map_err(|_| Error::StorageFailure)?;
    let root = db.parent().ok_or(Error::StorageFailure)?.join("secrets");
    Store::open(
        &root,
        &db.with_extension("sync.db"),
        backend::native(&home()?),
    )
}

pub fn initialize(store: &mut Store, label: &str) -> Result<hey_boss_secrets::Enrollment> {
    let home = home()?;
    backend::prepare_native(&home)?;
    let card = store.initialize(label)?;
    #[cfg(target_os = "macos")]
    backend::Keychain::migrate_identity(&home, &card)?;
    Ok(card)
}

/// Installer entry point: creates a machine identity, never grants record access.
pub fn install() -> Result<()> {
    let mut label = [0u8; 256];
    let label = if unsafe { libc::gethostname(label.as_mut_ptr().cast(), label.len()) } == 0 {
        String::from_utf8_lossy(&label[..label.iter().position(|b| *b == 0).unwrap_or(label.len())])
            .into_owned()
    } else {
        "local".into()
    };
    let initialized = initialize(&mut open()?, &label);
    #[cfg(target_os = "macos")]
    let initialized = match initialized {
        Err(Error::StoreUnavailable) => {
            install_session::initialize(
                &std::env::current_exe().map_err(|_| Error::StorageFailure)?,
                &label,
            )?;
            open()?.enrollment()
        }
        result => result,
    };
    initialized.map(|_| ())
}

#[cfg(target_os = "macos")]
#[path = "secrets_install_session.rs"]
mod install_session;
