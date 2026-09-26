use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
};

static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-chief-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.current_dir(&self.0)
            .env("HEY_BOSS_ISSUE_DB", self.0.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST");
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        let out = self
            .command()
            .args([
                "issue",
                "--project",
                "Chief QA",
                "--agent",
                "human:qa",
                "--json",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "Chief CLI {args:?} exited {}\nstdout: {}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

struct Worker(std::process::Child, Option<PathBuf>);
impl Worker {
    fn stop_chief(&self) {
        let Some(root) = &self.1 else { return };
        let owned = || -> rusqlite::Result<(u32, String)> {
            let db = rusqlite::Connection::open(root.join("issues.db"))?;
            db.busy_timeout(std::time::Duration::from_millis(100))?;
            db.query_row("SELECT pid,process_start FROM project_chiefs WHERE owner_pid=?1 AND pid IS NOT NULL", [self.0.id()], |r| Ok((r.get(0)?, r.get(1)?)))
        };
        if let Ok((pid, start)) = owned()
            && hey_harvester::agents::process_identity(pid).as_deref() == Some(&start)
        {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_some() {
            self.stop_chief();
            return;
        }
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if self.0.try_wait().ok().flatten().is_some() {
                self.stop_chief();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        self.stop_chief();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[track_caller]
fn wait_for(f: &Fixture, worker: &mut Worker, stage: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !condition() {
        if let Some(status) = worker.0.try_wait().unwrap() {
            panic!(
                "Chief worker exited {status} during {stage}\n{}",
                f.diagnostics()
            );
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for Chief: {stage}\n{}",
            f.diagnostics()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

impl Fixture {
    fn diagnostics(&self) -> String {
        let state = rusqlite::Connection::open(self.0.join("issues.db")).and_then(|db| {
            db.busy_timeout(std::time::Duration::from_millis(100))?;
            db.query_row("SELECT json_object('state',state,'session_id',session_id,'owner_pid',owner_pid,'pid',pid,'next_at',next_at,'started_at',started_at,'finished_at',finished_at,'last_event',last_event,'summary',summary) FROM project_chiefs", [], |r| r.get::<_, String>(0))
        });
        format!(
            "Chief state: {state:?}\nlaunches: {}\nworker output: {}",
            log_tail(&self.0.join("launches.txt")),
            log_tail(&self.0.join("worker.log"))
        )
    }
}

fn log_tail(path: &Path) -> String {
    let read = || -> std::io::Result<String> {
        let mut file = fs::File::open(path)?;
        let start = file.metadata()?.len().saturating_sub(8192);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(8192).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    read().unwrap_or_else(|error| format!("{error}"))
}

#[test]
fn chief_wait_reports_worker_exit_with_stage_and_output() {
    let f = Fixture::new();
    let log = fs::File::create(f.0.join("worker.log")).unwrap();
    let mut worker = Worker(
        Command::new("/bin/sh")
            .args(["-c", "echo synthetic-startup-error >&2; exit 23"])
            .stderr(log)
            .spawn()
            .unwrap(),
        None,
    );
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_for(&f, &mut worker, "startup probe", || false);
    }))
    .unwrap_err();
    let message = failure.downcast_ref::<String>().unwrap();
    assert!(
        message.contains("worker exited exit status: 23"),
        "{message}"
    );
    assert!(message.contains("startup probe"), "{message}");
    assert!(message.contains("synthetic-startup-error"), "{message}");
    assert!(message.contains("Chief state:"), "{message}");
}

#[test]
fn chief_runs_without_issues_resumes_and_cleans_up() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let fake = f.0.join("codex");
    fs::write(&fake, r#"#!/bin/sh
sleep 0.2
printf '%s\n' "$*" >> launches.txt
if [ "$1" != exec ]; then exit 9; fi
if [ "$2" = resume ] && [ "$3" = missing-thread ]; then
  printf '%s\n' '{"type":"error","message":"No saved session found"}'
  exit 1
fi
printf '%s\n' '{"type":"thread.started","thread_id":"chief-saved-thread"}'
if [ -f dead-parent ]; then sleep 120 & exit 7; fi
if [ -f malformed ]; then printf 'not-json\n'; exit 0; fi
if [ -f failed-completion ]; then printf '%s\n' '{"type":"turn.failed","error":{"message":"Failed despite later text"}}'; fi
if [ -f hold ]; then
  echo $$ > chief.pid
  sleep 120
fi
if [ -f fail ]; then
  printf '%s\n' '{"type":"turn.failed","error":{"message":"Synthetic connection failure"}}'
  exit 1
fi
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"Organized the project."}}' '{"type":"turn.completed"}'
"#).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    f.cli(&["settings", "set", "--no-chief"]);
    let start = |enable: bool| {
        let mut command = f.command();
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(f.0.join("worker.log"))
            .unwrap();
        command.args(["worker", "run"]);
        if enable {
            command.arg("--chief");
        }
        Worker(
            command
                .env("HEY_BOSS_CODEX", &fake)
                .args([
                    "--project",
                    "Chief QA",
                    "--directory",
                    f.0.to_str().unwrap(),
                    "--json",
                ])
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
            Some(f.0.clone()),
        )
    };
    let db = rusqlite::Connection::open(f.0.join("issues.db")).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    let finished = || {
        db.query_row("SELECT count(*) FROM project_chiefs WHERE state='idle' AND session_id='chief-saved-thread' AND owner_pid IS NULL",[],|r|r.get::<_,i64>(0)).unwrap() == 1
    };
    let mut worker = start(true);
    wait_for(&f, &mut worker, "initial pass", finished);
    assert_eq!(
        f.cli(&["settings", "show"])["chief_enabled"],
        true,
        "worker --chief enables its selected project"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM worker_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "Chief must not reserve an issue slot"
    );
    let next: i64 = db
        .query_row("SELECT next_at FROM project_chiefs", [], |r| r.get(0))
        .unwrap();
    assert!(
        next > std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            + 3_500_000
    );
    drop(worker);
    db.execute("UPDATE project_chiefs SET next_at=0", [])
        .unwrap();
    let mut worker = start(false);
    wait_for(&f, &mut worker, "saved thread resume", || {
        finished()
            && fs::read_to_string(f.0.join("launches.txt"))
                .unwrap_or_default()
                .contains("exec resume chief-saved-thread")
    });
    drop(worker);
    db.execute(
        "UPDATE project_chiefs SET next_at=0,session_id='missing-thread'",
        [],
    )
    .unwrap();
    let mut worker = start(false);
    wait_for(&f, &mut worker, "missing thread replacement", finished);
    assert!(
        fs::read_to_string(f.0.join("launches.txt"))
            .unwrap()
            .contains("exec resume missing-thread")
    );
    drop(worker);
    fs::write(f.0.join("fail"), "").unwrap();
    for launch in fs::read_to_string(f.0.join("launches.txt"))
        .unwrap()
        .lines()
        .filter(|line| line.starts_with("exec "))
    {
        assert!(launch.contains("-c approval_policy=\"on-request\""));
        assert!(launch.contains("-c approvals_reviewer=\"auto_review\""));
        assert!(launch.contains("-c sandbox_mode=\"workspace-write\""));
        assert!(!launch.contains("approval_policy=\"never\""));
    }
    db.execute("UPDATE project_chiefs SET next_at=0", [])
        .unwrap();
    let mut worker = start(false);
    wait_for(&f, &mut worker, "failed turn", || {
        db.query_row("SELECT state FROM project_chiefs", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap()
            == "blocked"
    });
    assert_eq!(
        db.query_row("SELECT session_id FROM project_chiefs", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "chief-saved-thread"
    );
    drop(worker);
    fs::remove_file(f.0.join("fail")).unwrap();
    for mode in ["dead-parent", "malformed", "failed-completion"] {
        let previous: i64 = db
            .query_row("SELECT finished_at FROM project_chiefs", [], |r| r.get(0))
            .unwrap();
        fs::write(f.0.join(mode), "").unwrap();
        db.execute("UPDATE project_chiefs SET next_at=0", [])
            .unwrap();
        let mut worker = start(false);
        wait_for(&f, &mut worker, mode, || {
            db.query_row(
                "SELECT state='blocked' AND finished_at>?1 FROM project_chiefs",
                [previous],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
        });
        assert!(db.query_row("SELECT next_at-finished_at<=300000 AND owner_pid IS NULL AND pid IS NULL FROM project_chiefs",[],|r|r.get::<_,bool>(0)).unwrap());
        drop(worker);
        fs::remove_file(f.0.join(mode)).unwrap();
    }
    fs::write(f.0.join("hold"), "").unwrap();
    db.execute("UPDATE project_chiefs SET next_at=0", [])
        .unwrap();
    let mut worker = start(false);
    wait_for(&f, &mut worker, "held Chief startup", || {
        f.0.join("chief.pid").exists()
    });
    let pid: u32 = fs::read_to_string(f.0.join("chief.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let status = f
        .command()
        .args(["worker", "--json", "status"])
        .output()
        .unwrap();
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["active"], 0, "Chief does not consume an issue slot");
    assert_eq!(status["chiefs"][0]["state"], "running");
    assert_eq!(status["chiefs"][0]["pid"], pid);
    assert_eq!(status["chiefs"][0]["session_id"], "chief-saved-thread");
    assert_eq!(status["chiefs"][0]["kind"], "chief");
    let mut second = start(false);
    let second_pid = second.0.id();
    wait_for(&f, &mut second, "second worker startup", || {
        db.query_row(
            "SELECT count(*) FROM issue_workers WHERE owner_pid=?1",
            [second_pid],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    });
    let second_id: String = db
        .query_row(
            "SELECT id FROM issue_workers WHERE owner_pid=?1",
            [second.0.id()],
            |r| r.get(0),
        )
        .unwrap();
    let status = f
        .command()
        .args(["worker", "--id", &second_id, "--json", "status"])
        .output()
        .unwrap();
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(
        status["chiefs"].as_array().unwrap().is_empty(),
        "Only the owning worker displays Chief"
    );
    assert_eq!(
        db.query_row("SELECT pid FROM project_chiefs", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        pid,
        "Starting another worker keeps the existing Chief process"
    );
    drop(second);
    f.cli(&["settings", "set", "--no-chief"]);
    wait_for(
        &f,
        &mut worker,
        "disabled Chief cleanup",
        || unsafe { libc::kill(pid as i32, 0) } != 0,
    );
    drop(worker);
    assert!(
        db.query_row(
            "SELECT owner_pid IS NULL AND pid IS NULL FROM project_chiefs",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn chief_defaults_settings_and_version_guards() {
    let f = Fixture::new();
    let defaults = f.cli(&["settings", "show"]);
    assert_eq!(defaults["chief_enabled"], false);
    assert!(
        defaults["chief_prompt"]
            .as_str()
            .unwrap()
            .contains("Workers handle code changes")
    );
    let configured = f.cli(&[
        "settings",
        "set",
        "--chief",
        "--chief-prompt",
        "Organize this project and stop.",
    ]);
    assert_eq!(configured["chief_enabled"], true);
    assert_eq!(
        configured["chief_prompt"],
        "Organize this project and stop."
    );
    f.cli(&[
        "settings",
        "set",
        "--prompt",
        "Implement {{issue_command}}.",
    ]);
    assert_eq!(f.cli(&["settings", "show"])["chief_enabled"], true);
    let request = json!({"version":1,"project":{"id":"named:Chief QA","name":"Chief QA"},"project_override":"Chief QA","operation":{"action":"configure_project","chief_enabled":false,"if_version":0}});
    let out = f
        .command()
        .args(["issue", "rpc"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    let mut out = out;
    out.stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    assert!(!out.wait_with_output().unwrap().status.success());
    assert_eq!(f.cli(&["settings", "show"])["chief_enabled"], true);
    assert_eq!(
        f.cli(&["settings", "set", "--no-chief"])["chief_enabled"],
        false
    );
    assert!(
        !f.command()
            .args([
                "issue",
                "--project",
                "Chief QA",
                "settings",
                "set",
                "--chief-prompt",
                " "
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
}
