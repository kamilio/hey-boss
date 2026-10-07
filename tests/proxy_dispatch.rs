use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hb-proxy-dispatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_hey-boss"), path.join("hey-boss")).unwrap();
        Self(path)
    }
    fn run(&self) -> Command {
        let mut command = Command::new(self.0.join("hey-boss"));
        command
            .arg("proxy")
            .current_dir(&self.0)
            .env("PATH", &self.0);
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn proxy_dispatch_preserves_arguments_io_environment_and_exit_status() {
    use std::io::Write;
    use std::process::Stdio;
    let f = Fixture::new();
    let proxy = f.0.join("hey-proxy");
    fs::write(&proxy, "#!/bin/sh\nprintf '%s\\n' \"$@\"\nprintf '%s\\n' \"$PROXY_TEST_VALUE\"\nread -r line\nprintf '%s\\n' \"$line\"\nprintf 'proxy-error\\n' >&2\nexit 23\n").unwrap();
    fs::set_permissions(&proxy, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = f
        .run()
        .args([
            "--config",
            "path with spaces",
            "usage",
            "--help",
            "--",
            "--json",
        ])
        .env("PROXY_TEST_VALUE", "inherited")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"input survives\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(
        output.stdout,
        b"--config\npath with spaces\nusage\n--help\n--\n--json\ninherited\ninput survives\n"
    );
    assert_eq!(output.stderr, b"proxy-error\n");
    assert!(!f.0.join(".hey-boss").exists());
}

#[test]
fn proxy_dispatch_reports_missing_installation() {
    let f = Fixture::new();
    let output = f.run().arg("--help").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("hey-proxy"));
}

#[test]
fn root_help_advertises_proxy() {
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("proxy"));
}

#[test]
fn older_installs_find_standalone_proxy_on_path_and_preserve_signals() {
    use std::os::unix::process::ExitStatusExt;
    let f = Fixture::new();
    let standalone = f.0.join("standalone");
    fs::create_dir(&standalone).unwrap();
    let proxy = standalone.join("hey-proxy");
    fs::write(&proxy, "#!/bin/sh\nkill -TERM $$\n").unwrap();
    fs::set_permissions(&proxy, fs::Permissions::from_mode(0o755)).unwrap();
    let output = f.run().env("PATH", standalone).output().unwrap();
    assert_eq!(output.status.signal(), Some(15));
}
