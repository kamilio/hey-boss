//! Exercise the installed command path with an existing companion service.
use hey_boss::database::Connection;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    service: Option<Child>,
    replacement: Option<u32>,
}
impl Fixture {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .args(args)
            .current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", self.root.join("fleet"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_FLEET_SUPERVISED");
        command
    }
    fn connection(&self) -> Connection {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(connection) = Connection::connect(&self.root.join("issues.db")) {
                return connection;
            }
            assert!(Instant::now() < deadline, "database owner did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn create(&self, title: &str) {
        let output = self
            .command(&[
                "issue",
                "--project",
                "Database owner",
                "--agent",
                "test:owner",
                "--json",
                "create",
                "--title",
                title,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["issue"]["title"], title);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut service) = self.service.take() {
            unsafe {
                libc::kill(service.id() as i32, libc::SIGTERM);
            }
            let _ = service.wait();
        }
        if let Some(pid) = self.replacement {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn existing_service_owns_cli_writes_and_missing_service_recovers_automatically() {
    let root = std::env::temp_dir().join(format!("hb-owner-process-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut fixture = Fixture {
        root,
        service: None,
        replacement: None,
    };
    fixture.service = Some(
        fixture
            .command(&["fleet", "companion"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let connection = fixture.connection();
    let pid = fixture.service.as_ref().unwrap().id();
    assert_eq!(connection.owner_pid().unwrap(), pid);
    fixture.create("Before restart");
    assert_eq!(
        connection.owner_pid().unwrap(),
        pid,
        "CLI started another owner despite the existing service"
    );
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    assert!(fixture.service.take().unwrap().wait().unwrap().success());
    fixture.create("After restart");
    let next = fixture.connection();
    let replacement = next.owner_pid().unwrap();
    fixture.replacement = Some(replacement);
    assert_ne!(replacement, pid);
    let count = next
        .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
        .unwrap();
    assert_eq!(count, 2);
}
