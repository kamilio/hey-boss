#![cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        let until = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < until {
            if self.0.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn status_path(state: &std::path::Path, host: &str) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    host.hash(&mut hash);
    state.join(format!("connection-status-{:016x}.json", hash.finish()))
}
#[test]
fn absent_vpn_and_saved_cooldown_send_no_ssh_requests() {
    let root = std::env::temp_dir().join(format!("hb-auto-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut children = Vec::new();
    for (name, domain, retry) in [
        ("offline", "hey-boss-test.invalid", 0),
        ("corrupt-cooldown", "local", u64::MAX),
        (
            "cooldown",
            "local",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 3600,
        ),
    ] {
        let home = root.join(name);
        let state = home.join(".local/share/hey-boss");
        let bin = home.join("bin");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let ssh = bin.join("ssh");
        std::fs::write(
            &ssh,
            format!(
                "#!/bin/sh\necho attempted >> '{}'\nexit 1\n",
                home.join("attempts").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let host = if domain == "local" {
            "test.local"
        } else {
            "devbox"
        };
        std::fs::create_dir_all(home.join(".hey-boss")).unwrap();
        std::fs::write(
            home.join(".hey-boss/config.json"),
            serde_json::json!({"ssh_hosts":[{"host":host,"vpn_domain":domain,"enabled":true}]})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            status_path(&state, host),
            serde_json::json!({"state":"backoff","updated_at":0,"retry_at":retry,"failures":3})
                .to_string(),
        )
        .unwrap();
        let executable = bin.join("hey-boss");
        std::fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &executable).unwrap();
        std::fs::write(
            bin.join("hey-boss.state"),
            state.to_string_lossy().as_bytes(),
        )
        .unwrap();
        let child = Command::new(executable)
            .args(["companion", "auto"])
            .env("HOME", &home)
            .env("PATH", &bin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        children.push((home, host.to_owned(), Process(child)));
    }
    // Cross the second DNS sample; a spinning or cooldown-bypassing connector
    // would have invoked our fake SSH rather than accessing a real server.
    std::thread::sleep(Duration::from_secs(17));
    for (home, host, mut child) in children {
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(!home.join("attempts").exists());
        let status: serde_json::Value = serde_json::from_slice(
            &std::fs::read(status_path(&home.join(".local/share/hey-boss"), &host)).unwrap(),
        )
        .unwrap();
        assert_eq!(status["failures"], 3);
        if home.ends_with("offline") {
            assert_eq!(status["state"], "waiting-for-vpn");
        } else {
            // Local DNS can temporarily reset the stability samples. Both states
            // must preserve cooldown and avoid SSH entirely.
            assert!(matches!(
                status["state"].as_str(),
                Some("backoff" | "waiting-for-stable-vpn" | "waiting-for-vpn")
            ));
            assert_eq!(status["failures"], 3);
            if home.ends_with("corrupt-cooldown") {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                assert!(status["retry_at"].as_u64().unwrap() <= now + 915);
                assert!(status["retry_at"].as_u64().unwrap() > now);
            }
        }
        drop(child);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unavailable_status_storage_does_not_restart_connector() {
    let home = std::env::temp_dir().join(format!("hb-auto-storage-{}", std::process::id()));
    let state = home.join(".local/share/hey-boss");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        state.join("companion.json"),
        serde_json::json!({"host":"devbox","vpn_domain":"hey-boss-test.invalid","enabled":true})
            .to_string(),
    )
    .unwrap();
    // A directory at the atomic-write temporary path deterministically makes
    // status writes fail without filling or changing the user's disk.
    std::fs::create_dir(status_path(&state, "devbox").with_extension("new")).unwrap();
    let mut child = Process(
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args([
                "companion",
                "auto",
                "--host",
                "devbox",
                "--vpn-domain",
                "hey-boss-test.invalid",
            ])
            .env("HOME", &home)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(Duration::from_secs(3));
    assert!(child.0.try_wait().unwrap().is_none());
    unsafe {
        libc::kill(child.0.id() as i32, libc::SIGTERM);
    }
    assert!(child.0.wait().unwrap().success());
    let mut log = String::new();
    use std::io::Read;
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut log)
        .unwrap();
    assert_eq!(
        log.matches("cannot save status; retaining connection")
            .count(),
        1
    );
    std::fs::remove_dir_all(home).unwrap();
}
