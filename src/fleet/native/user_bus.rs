//! Prepare user-systemd commands without requiring an interactive login.
use std::{ffi::OsStr, os::unix::fs::FileTypeExt, path::Path, process::Command};

pub(crate) fn command(runtime: &Path, address: Option<&OsStr>) -> Command {
    let mut command = Command::new("systemctl");
    command.arg("--user");
    let address = address.filter(|address| !address.is_empty());
    if let Some(address) = address {
        command.env("DBUS_SESSION_BUS_ADDRESS", address);
    }
    let bus = runtime.join("bus");
    if std::fs::metadata(&bus).is_ok_and(|metadata| metadata.file_type().is_socket()) {
        command.env("XDG_RUNTIME_DIR", runtime);
        if address.is_none() {
            let mut address = std::ffi::OsString::from("unix:path=");
            address.push(bus);
            command.env("DBUS_SESSION_BUS_ADDRESS", address);
        }
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn test_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hb-bus-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn linux_systemctl_reuses_existing_user_bus_without_login_environment() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(root.join("bus")).unwrap();
        let command = command(&root, None);
        let environment = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(|value| value.to_owned())))
            .collect::<std::collections::HashMap<_, _>>();
        drop(listener);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            environment.get(std::ffi::OsStr::new("XDG_RUNTIME_DIR")),
            Some(&Some(root.as_os_str().to_owned()))
        );
        assert_eq!(
            environment.get(std::ffi::OsStr::new("DBUS_SESSION_BUS_ADDRESS")),
            Some(&Some(format!("unix:path={}/bus", root.display()).into()))
        );
    }
    #[test]
    fn linux_systemctl_preserves_explicit_bus_and_falls_back_for_empty_address() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(root.join("bus")).unwrap();
        let addresses = ["unix:path=/explicit/session/bus", ""];
        let actual = addresses.map(|address| {
            command(&root, Some(std::ffi::OsStr::new(address)))
                .get_envs()
                .find(|(key, _)| *key == "DBUS_SESSION_BUS_ADDRESS")
                .and_then(|(_, value)| value.map(|value| value.to_owned()))
        });
        drop(listener);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(actual[0], Some(addresses[0].into()));
        assert_eq!(
            actual[1],
            Some(format!("unix:path={}/bus", root.display()).into())
        );
    }
    #[test]
    fn linux_systemctl_does_not_invent_a_user_bus() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let missing = command(&root, None).get_envs().count();
        fs::write(root.join("bus"), "not a socket").unwrap();
        let regular_file = command(&root, None).get_envs().count();
        fs::remove_dir_all(root).unwrap();
        assert_eq!(missing, 0);
        assert_eq!(regular_file, 0);
    }
}
