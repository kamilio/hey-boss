//! Reuse the companion connector transport without owning its lifetime.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn control_path(directory: &Path, host: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    host.hash(&mut hash);
    directory.join(format!("ssh-{:016x}.sock", hash.finish()))
}

pub fn reuse_connection(command: &mut Command, directory: &Path, host: &str) {
    use std::os::unix::fs::FileTypeExt;
    let socket = control_path(directory, host);
    if std::fs::symlink_metadata(&socket).is_ok_and(|m| m.file_type().is_socket()) {
        // OpenSSH falls back to the normal host configuration if the master
        // exits between this check and connect. Never take over its lifetime.
        command
            .args(["-o", "ControlMaster=no", "-o"])
            .arg(format!("ControlPath={}", socket.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fleet_ssh_reuses_only_its_registered_socket_and_keeps_normal_fallback() {
        let root = crate::admin::Temporary::new().unwrap();
        let directory = root.0.join("ssh");
        std::fs::create_dir_all(&directory).unwrap();
        let socket = control_path(&directory, "devbox");
        let arguments = |host| {
            let mut command = Command::new("ssh");
            reuse_connection(&mut command, &directory, host);
            command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert!(arguments("devbox").is_empty());
        std::fs::write(&socket, "not a socket").unwrap();
        assert!(arguments("devbox").is_empty());
        std::fs::remove_file(&socket).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert_eq!(
            arguments("devbox"),
            vec![
                "-o",
                "ControlMaster=no",
                "-o",
                &format!("ControlPath={}", socket.display())
            ]
        );
        assert!(arguments("other").is_empty());
        drop(listener);
        std::fs::remove_file(socket).unwrap();
        assert!(arguments("devbox").is_empty());
    }
}
