use rusqlite::Connection;
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    binary: PathBuf,
    cwd: PathBuf,
    db: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-notif-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let cwd = root.join("project");
        fs::create_dir_all(&cwd).unwrap();
        let binary = root.join("hey-boss");
        // Avoid writable executable descriptors being inherited by concurrent
        // test launches, which Linux rejects with ETXTBSY.
        fs::hard_link(env!("CARGO_BIN_EXE_hey-boss"), &binary).unwrap();
        fs::write(root.join("hey-boss.state"), root.to_str().unwrap()).unwrap();
        let db = root.join("issues.db");
        Self {
            root,
            binary,
            cwd,
            db,
        }
    }
    fn command(&self, cwd: &Path) -> Command {
        let mut c = Command::new(&self.binary);
        c.current_dir(cwd)
            .env("HEY_BOSS_ISSUE_DB", &self.db)
            .env("GIT_CEILING_DIRECTORIES", &self.root)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .env_remove("HEY_BOSS_AGENT_ID")
            .env_remove("CODEX_THREAD_ID");
        c
    }
    fn notify(&self, command: &mut Command) -> Value {
        let socket = self.root.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let peer = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < until, "CLI did not contact mock daemon");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            // macOS may inherit the listener's nonblocking mode. Wait for the
            // client payload rather than racing its first write after accept.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let request = serde_json::from_slice(&bytes).unwrap();
            stream
                .write_all(br#"{"task_id":"test","status":"ok"}"#)
                .unwrap();
            request
        });
        let output = command.output().unwrap();
        let request = peer.join().unwrap();
        fs::remove_file(socket).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        request
    }
    fn projects(&self) -> Value {
        let output = self
            .command(&self.cwd)
            .args(["issue", "projects", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn alert(&self, cwd: &Path) -> Command {
        let mut c = self.command(cwd);
        c.args(["alert", "Ready", "--title", "Review", "--json"]);
        c
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn notifications_hide_empty_temporary_projects_and_register_custom_projects() {
    let f = Fixture::new();
    let notice = f.notify(&mut f.alert(&f.cwd));
    assert_eq!(notice["project"], "project");
    let projects = f.projects();
    assert!(projects["projects"].as_array().unwrap().is_empty());
    let notice = f.notify(f.alert(&f.cwd).args(["--project", "Atlas"]));
    assert_eq!(notice["project"], "Atlas");
    let projects = f.projects();
    assert!(
        projects["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "named:Atlas")
    );
    // A temporary checkout with saved user work must still remain discoverable.
    let created = f
        .command(&f.cwd)
        .args([
            "issue",
            "create",
            "--title",
            "Saved work",
            "--agent",
            "human:test",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(
        f.projects()["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "project" && p["open"] == 1)
    );
}

#[test]
fn notifications_share_git_project_across_subdirectories_and_worktrees() {
    let f = Fixture::new();
    git(&f.cwd, &["init", "-q"]);
    git(&f.cwd, &["commit", "--allow-empty", "-qm", "initial"]);
    git(
        &f.cwd,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:example/Atlas.git",
        ],
    );
    let nested = f.cwd.join("nested");
    fs::create_dir(&nested).unwrap();
    assert_eq!(f.notify(&mut f.alert(&nested))["project"], "Atlas");
    let linked = f.root.join("linked");
    git(
        &f.cwd,
        &["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
    );
    assert_eq!(f.notify(&mut f.alert(&linked))["project"], "Atlas");
    let projects = f.projects();
    assert_eq!(projects["projects"].as_array().unwrap().len(), 1);
    assert_eq!(projects["projects"][0]["id"], "github.com/example/Atlas");
}

#[test]
fn overrides_reuse_unique_names_warn_about_collisions_and_preserve_hidden_state() {
    let f = Fixture::new();
    f.notify(
        f.alert(&f.cwd)
            .args(["--project", "github.com/example/Atlas"]),
    );
    let hidden = f
        .command(&f.cwd)
        .args([
            "issue",
            "hide-project",
            "--project",
            "Atlas",
            "--agent",
            "human:test",
        ])
        .output()
        .unwrap();
    assert!(hidden.status.success());
    assert_eq!(
        f.notify(f.alert(&f.cwd).args(["--project", "Atlas"]))["project"],
        "Atlas"
    );
    let db = Connection::open(&f.db).unwrap();
    let (count, hidden): (i64, bool) = db
        .query_row(
            "SELECT count(*),hidden_at IS NOT NULL FROM projects WHERE name='Atlas'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((count, hidden), (1, true));
    assert_eq!(
        f.notify(
            f.alert(&f.cwd)
                .env("HEY_BOSS_ISSUE_PROJECT", "github.com/example/Atlas")
        )["project"],
        "Atlas"
    );
    assert_eq!(
        f.notify(
            f.alert(&f.cwd)
                .env("HEY_BOSS_ISSUE_PROJECT", "Atlas")
                .args(["--project", "Other"])
        )["project"],
        "Other"
    );
    f.notify(
        f.alert(&f.cwd)
            .args(["--project", "github.com/other/Atlas"]),
    );
    assert_eq!(
        f.notify(f.alert(&f.cwd).args(["--project", "Atlas"]))["project"],
        "Atlas"
    );
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM projects WHERE name='Atlas'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert!(
        f.projects()["project_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["rejected_id"] == "github.com/other/Atlas")
    );
    assert_eq!(
        f.notify(
            f.alert(&f.cwd)
                .args(["--project", "github.com/example/Atlas"])
        )["project"],
        json!("Atlas")
    );
}
