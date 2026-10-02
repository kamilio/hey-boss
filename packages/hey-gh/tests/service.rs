use std::process::Command;

#[test]
fn service_help_explains_durable_start_health_and_recovery() {
    let output = Command::new(env!("CARGO_BIN_EXE_hey-gh"))
        .args(["service", "--help"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8(output.stdout).unwrap();
    for command in ["start", "status", "restart"] {
        assert!(help.contains(command), "missing {command}: {help}");
    }
}

#[test]
fn occupied_listener_does_not_authenticate_or_start_pollers() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hey-gh"))
        .args([
            "serve",
            "--listen",
            &listener.local_addr().unwrap().to_string(),
            "--cache",
        ])
        .arg(dir.path().join("cache.sqlite"))
        .arg("--log-dir")
        .arg(dir.path().join("logs"))
        .env("PATH", dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("Address already in use"),
        "must bind before gh authentication: {error}"
    );
}

#[cfg(unix)]
#[test]
fn racing_daemons_reuse_cache_and_only_one_authenticates() {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        process::{Child, Stdio},
        time::{Duration, Instant},
    };
    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\necho call >> \"$HOME/auth-calls\"\n/bin/sleep 1\nprintf synthetic-test-token\n",
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let spawn = || {
        Process(
            Command::new(env!("CARGO_BIN_EXE_hey-gh"))
                .args(["serve", "--listen", &port.to_string(), "--cache"])
                .arg(dir.path().join("cache.sqlite"))
                .arg("--log-dir")
                .arg(dir.path().join("logs"))
                .env("PATH", dir.path())
                .env("HOME", dir.path())
                .env("XDG_CACHE_HOME", dir.path().join("xdg"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let mut first = spawn();
    let mut second = spawn();
    let deadline = Instant::now() + Duration::from_secs(10);
    let read = || {
        Command::new(env!("CARGO_BIN_EXE_hey-gh"))
            .args(["--server", &format!("http://{port}"), "status"])
            .env("HOME", dir.path())
            .env("XDG_CACHE_HOME", dir.path().join("xdg"))
            .output()
            .unwrap()
    };
    loop {
        if read().status.success() {
            break;
        }
        assert!(Instant::now() < deadline, "no healthy daemon after race");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("auth-calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_ne!(
        first.0.try_wait().unwrap().is_some(),
        second.0.try_wait().unwrap().is_some()
    );
    // Reads are independent caller processes and do not need GitHub traffic.
    let status: serde_json::Value = serde_json::from_slice(&read().stdout).unwrap();
    assert_eq!(status["network_requests"], 0);
    let mut third = spawn();
    assert!(!third.0.wait().unwrap().success());
    assert!(read().status.success());
    assert_eq!(
        fs::read_to_string(dir.path().join("auth-calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}
