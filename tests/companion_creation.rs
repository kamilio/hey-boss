//! Real fleet transport, private stores, no worker or production queue changes.
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new(binary: &Path) -> Self {
        let path = PathBuf::from("/tmp").join(format!(
            "hb-create-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inventory.json"), r#"{"ssh_hosts":[]}"#).unwrap();
        fs::write(path.join("desired.json"), r#"{"machines":{}}"#).unwrap();
        // Fleet services reload when their executable changes. Other checks
        // rebuild target/debug, so every process in this fixture needs a stable
        // copy of the same tested executable.
        fs::copy(binary, path.join("hey-boss")).unwrap();
        Self(path)
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(self.0.join("hey-boss"));
        c.args(args)
            .current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &self.0)
            .env("HEY_BOSS_FLEET_CONFIG", self.0.join("inventory.json"))
            .env("HEY_BOSS_FLEET_DESIRED", self.0.join("desired.json"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("HEY_BOSS_FLEET_SUPERVISED");
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        let o = self.command(args).output().unwrap();
        assert_eq!(
            o.status.code(),
            Some(0),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn issue(&self, args: &[&str]) -> Value {
        let mut all = vec![
            "issue",
            "--project",
            "Creation QA",
            "--agent",
            "human:fixture",
            "--json",
        ];
        all.extend_from_slice(args);
        self.cli(&all)
    }
    fn service(&self, args: &[&str]) -> Service {
        Service(
            self.command(args)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Service(Child);
impl Service {
    fn stop(&mut self) {
        if self.0.try_wait().unwrap().is_some() {
            return;
        }
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert_eq!(
                    status.code(),
                    Some(0),
                    "fixture service did not exit normally"
                );
                break;
            }
            assert!(Instant::now() < deadline, "fixture service did not stop");
            thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn connected_creation_refreshes_numbers_and_keeps_offline_creation_safe() {
    let main = Fixture::new(Path::new(env!("CARGO_BIN_EXE_hey-boss")));
    let peer = Fixture::new(&main.0.join("hey-boss"));
    let mut main_owner = main.service(&["fleet", "companion"]);
    let mut owner = peer.service(&["fleet", "companion"]);
    // Seed rows without append history: both subprocesses initially report the
    // same physical host UUID. Give the supervisor a distinct replica origin
    // after its capture is installed, before creating any history.
    main.issue(&["create", "--title", "Existing"]);
    Connection::open(main.0.join("issues.db")).unwrap().execute_batch(
        "DELETE FROM events; INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) VALUES('named:Creation QA',1099,'Recent','','open','human:fixture',0,0,1,'[]',2); UPDATE projects SET next_number=1100;").unwrap();
    fs::write(
        main.0.join("inventory.json"),
        r#"{"ssh_hosts":["fixture.test"]}"#,
    )
    .unwrap();
    let bin = main.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    fs::write(&ssh, "#!/bin/sh\nexport HEY_BOSS_ISSUE_DB=\"$CREATION_PEER/issues.db\" HEY_BOSS_FLEET_STATE=\"$CREATION_PEER\" HEY_BOSS_FLEET_CONFIG=\"$CREATION_PEER/inventory.json\" HEY_BOSS_FLEET_DESIRED=\"$CREATION_PEER/desired.json\"\nexec \"$CREATION_BINARY\" fleet companion --stdio\n").unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let mut supervisor = Service(
        main.command(&["fleet", "supervisor"])
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("CREATION_PEER", &peer.0)
            .env("CREATION_BINARY", peer.0.join("hey-boss"))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let ready = peer.command(&["fleet", "status"]).output().unwrap();
        let synced = peer
            .command(&[
                "issue",
                "--project",
                "Creation QA",
                "--json",
                "view",
                "1099",
            ])
            .output()
            .unwrap();
        if ready.status.code() == Some(0) && synced.status.code() == Some(0) {
            break;
        }
        if Instant::now() >= deadline {
            let controller = main.command(&["fleet", "status"]).output().unwrap();
            panic!(
                "companion never became ready: supervisor={:?} {} {}; status={:?} {} {}; sync={:?} {} {}",
                supervisor.0.try_wait().unwrap(),
                String::from_utf8_lossy(&controller.stdout),
                String::from_utf8_lossy(&controller.stderr),
                ready.status,
                String::from_utf8_lossy(&ready.stdout),
                String::from_utf8_lossy(&ready.stderr),
                synced.status,
                String::from_utf8_lossy(&synced.stdout),
                String::from_utf8_lossy(&synced.stderr),
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
    Connection::open(main.0.join("issues.db"))
        .unwrap()
        .execute(
            "UPDATE fleet_meta SET node='fixture-supervisor' WHERE id=1",
            [],
        )
        .unwrap();
    // No worker and no prior project/range: creation must reserve on demand,
    // not require a worker configuration or a pre-existing offline block.
    let unstaffed = peer.cli(&[
        "issue",
        "--project",
        "Unstaffed creation QA",
        "--agent",
        "human:fixture",
        "--json",
        "create",
        "--title",
        "First issue without a worker",
    ]);
    assert_eq!(unstaffed["issue"]["number"], 1);
    assert_eq!(unstaffed["project"]["id"], "named:Unstaffed creation QA");
    // Reproduce a low block on a companion whose project has no worker.
    Connection::open(peer.0.join("issues.db")).unwrap().execute_batch("INSERT INTO fleet_number_ranges VALUES('named:Creation QA',5,104); UPDATE projects SET next_number=5 WHERE id='named:Creation QA';").unwrap();
    let args = [
        "create",
        "--title",
        "Visible new issue",
        "--request-id",
        "create-once",
    ];
    let created = peer.issue(&args);
    assert_eq!(created["issue"]["number"], 1100);
    assert_eq!(peer.issue(&args)["issue"]["number"], 1100);
    assert_eq!(
        peer.issue(&["view", "1100"])["issue"]["title"],
        "Visible new issue"
    );
    assert_eq!(peer.issue(&["list"])["issues"][0]["number"], 1100);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let result = main
            .command(&[
                "issue",
                "--project",
                "Creation QA",
                "--json",
                "view",
                "1100",
            ])
            .output()
            .unwrap();
        if result.status.code() == Some(0) {
            break;
        }
        assert!(Instant::now() < deadline, "creation was not replicated");
        thread::sleep(Duration::from_millis(100));
    }
    supervisor.stop();
    assert_eq!(
        peer.issue(&["create", "--title", "Offline", "--at-bottom"])["issue"]["number"],
        1101
    );
    let failure = peer
        .command(&[
            "issue",
            "--project",
            "No reservation",
            "--agent",
            "human:fixture",
            "--json",
            "create",
            "--title",
            "Offline unavailable",
        ])
        .output()
        .unwrap();
    assert_eq!(failure.status.code(), Some(4));
    let response: Value = serde_json::from_slice(&failure.stdout).unwrap();
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("offline issue-number allocation")
    );
    owner.stop();
    main_owner.stop();
}
