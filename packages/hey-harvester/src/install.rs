//! Install only the standalone worker, leaving all Hey Boss services running.
use std::{fs, io, os::unix::fs::PermissionsExt, path::Path};

pub(crate) fn publish(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let folder = destination
        .parent()
        .ok_or_else(|| io::Error::other("Missing installation directory"))?;
    fs::create_dir_all(folder)?;
    let temporary = folder.join(format!(".hey-harvester-install-{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(&temporary)?;
    let result = (|| {
        io::copy(&mut fs::File::open(source)?, &mut file)?;
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
        file.sync_all()?;
        fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_replaces_binary_atomically_and_keeps_old_open_handle_valid() {
        use std::io::Read;
        let root =
            std::env::temp_dir().join(format!("hey-harvester-install-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        let target = root.join("bin/hey-harvester");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&source, "new executable").unwrap();
        fs::write(&target, "old executable").unwrap();
        let mut old = fs::File::open(&target).unwrap();
        publish(&source, &target).unwrap();
        let mut value = String::new();
        old.read_to_string(&mut value).unwrap();
        assert_eq!(value, "old executable");
        assert_eq!(fs::read_to_string(&target).unwrap(), "new executable");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_dir_all(root).unwrap();
    }
}
