#[path = "projects.rs"]
mod projects;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub struct Fleet {
    binary: PathBuf,
    project: &'static str,
    actor: &'static str,
    pub root: PathBuf,
    pub services: Vec<Child>,
}
impl Fleet {
    pub fn with_binary(
        source: &std::path::Path,
        project: &'static str,
        actor: &'static str,
    ) -> Self {
        static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::current_dir().unwrap().join(format!(
            ".t-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for side in ["main", "peer", "bin"] {
            fs::create_dir(root.join(side)).unwrap();
        }
        for side in ["main", "peer"] {
            fs::write(
                root.join(side).join("inventory.json"),
                if side == "main" {
                    r#"{"ssh_hosts":["fixture.invalid"]}"#
                } else {
                    r#"{"ssh_hosts":[]}"#
                },
            )
            .unwrap();
            fs::write(root.join(side).join("desired.json"), r#"{"machines":{}}"#).unwrap();
        }
        projects::seed(&root.join("main/issues.db"), &[&format!("named:{project}")]);
        let binary = root.join("hey-boss");
        fs::copy(source, &binary).unwrap();
        let mut fleet = Self {
            binary,
            project,
            actor,
            root,
            services: vec![],
        };
        // Own the database services too, so no fixture daemon survives the test.
        for side in ["main", "peer"] {
            let service = fleet
                .command(side, &["fleet", "companion"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            fleet.services.push(service);
            let socket = fleet.database_socket(side);
            let deadline = Instant::now() + Duration::from_secs(30);
            while !socket.exists() {
                assert!(Instant::now() < deadline, "Database owner did not start");
                thread::sleep(Duration::from_millis(50));
            }
        }
        fleet
    }
    fn database_socket(&self, side: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        let path = self
            .root
            .canonicalize()
            .unwrap()
            .join(side)
            .join("issues.db");
        let key = format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()));
        PathBuf::from(format!(
            "/tmp/hey-boss-db-{}/{}.sock",
            unsafe { libc::getuid() },
            &key[..24]
        ))
    }
    pub fn command(&self, side: &str, args: &[&str]) -> Command {
        let path = self.root.join(side);
        let mut c = Command::new(&self.binary);
        c.args(args)
            .current_dir(&path)
            .env("HOME", &path)
            .env("HEY_BOSS_INBOX_SOCKET", path.join("absent.sock"))
            .env("GIT_CEILING_DIRECTORIES", &self.root)
            .env("HEY_BOSS_ISSUE_DB", path.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &path)
            .env("HEY_BOSS_FLEET_CONFIG", path.join("inventory.json"))
            .env("HEY_BOSS_FLEET_DESIRED", path.join("desired.json"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT");
        c
    }
    pub fn cli(&self, side: &str, args: &[&str], expected: i32) -> Value {
        let out = self.command(side, args).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    pub fn issue(&self, side: &str, args: &[&str], expected: i32) -> Value {
        let mut all = vec![
            "issue",
            "--project",
            self.project,
            "--agent",
            self.actor,
            "--json",
        ];
        all.extend_from_slice(args);
        self.cli(side, &all, expected)
    }
    pub fn sql(&self, side: &str, sql: &str) -> Value {
        self.database(side, json!({"sql":sql,"args":[]}))["rows"].clone()
    }
    pub fn database(&self, side: &str, request: Value) -> Value {
        let path = self.root.join(side).join("issues.db");
        let mut c = self
            .command(
                side,
                &["fleet", "database", "--path", path.to_str().unwrap()],
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(c.stdin.take().unwrap(), "{request}").unwrap();
        let out = c.wait_with_output().unwrap();
        assert!(out.status.success());
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["ok"], true, "{value}");
        value
    }
    pub fn start(&mut self) {
        let ssh = self.root.join("bin/ssh");
        fs::write(&ssh, "#!/bin/sh\nexport HEY_BOSS_ISSUE_DB=\"$TUNNEL_PEER/issues.db\" HEY_BOSS_FLEET_STATE=\"$TUNNEL_PEER\" HEY_BOSS_FLEET_CONFIG=\"$TUNNEL_PEER/inventory.json\" HEY_BOSS_FLEET_DESIRED=\"$TUNNEL_PEER/desired.json\"\nexec \"$TUNNEL_BINARY\" fleet companion --stdio\n").unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let service = self
            .command("main", &["fleet", "supervisor"])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("TUNNEL_PEER", self.root.join("peer"))
            .env("TUNNEL_BINARY", &self.binary)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        self.services.push(service);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let out = self
                .command("peer", &["fleet", "capabilities"])
                .output()
                .unwrap();
            let ready = serde_json::from_slice::<Value>(&out.stdout).unwrap_or(Value::Null);
            if out.status.success()
                && ready["route"] == "supervisor_tunnel"
                && ready["capabilities"]["issue_draft"] == true
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Tunnel not connected: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
}
impl Fleet {
    pub fn stop_services(&mut self) -> std::io::Result<()> {
        while let Some(mut service) = self.services.pop() {
            if service.try_wait()?.is_none() {
                unsafe {
                    libc::kill(service.id() as i32, libc::SIGTERM);
                }
            }
            let status = service.wait()?;
            if !status.success() {
                return Err(std::io::Error::other(format!(
                    "fixture service {} exited with {status}",
                    service.id()
                )));
            }
        }
        Ok(())
    }
}
impl Drop for Fleet {
    fn drop(&mut self) {
        while !self.services.is_empty() {
            let _ = self.stop_services();
        }
        for side in ["main", "peer"] {
            let socket = self.database_socket(side);
            for extension in ["sock", "lock", "startup", "log"] {
                let _ = fs::remove_file(socket.with_extension(extension));
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
