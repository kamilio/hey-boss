use super::{Service, managed_worker, worker_fixtures};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixStream,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn wait_for(description: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for {description}"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn worker_owns_job_across_supervisor_restart() {
    const PROJECT: &str = "Job worker QA";
    let dir = tempfile::Builder::new()
        .prefix(".job-")
        .tempdir_in(".")
        .unwrap();
    let root = dir.path().canonicalize().unwrap();
    let checkout = root.join("checkout");
    let home = root.join("home");
    fs::create_dir(&checkout).unwrap();
    fs::create_dir(&home).unwrap();
    fs::write(root.join("inventory.json"), r#"{"ssh_hosts":[]}"#).unwrap();
    fs::write(root.join("desired.json"), r#"{"machines":{}}"#).unwrap();
    let db_path = root.join("issues.db");
    let _owner = hey_boss::database::Owner::start(&db_path).unwrap().unwrap();
    let audit = root.join("launches.jsonl");
    let binary = std::env::var_os("HEY_BOSS_TEST_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_hey-boss").into())
        .canonicalize()
        .unwrap();
    let runtime = worker_fixtures::binary("agent-runtime");
    let command = |program: &Path| {
        let mut c = Command::new(program);
        c.current_dir(&checkout)
            .env("HOME", &home)
            .env("CODEX_HOME", root.join("codex"))
            .env("GIT_CEILING_DIRECTORIES", &root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("HEY_BOSS_ISSUE_DB", &db_path)
            .env("HEY_BOSS_FLEET_STATE", &root)
            .env("HEY_BOSS_FLEET_CONFIG", root.join("inventory.json"))
            .env("HEY_BOSS_FLEET_DESIRED", root.join("desired.json"))
            .env("HEY_BOSS_INBOX_SOCKET", root.join("inbox.sock"))
            .env("HEY_BOSS_AGENT_ID", "human:fixture")
            .env("HEY_BOSS_TEST_CLI", &binary)
            .env("HEY_BOSS_CODEX", &runtime)
            .env("HEY_BOSS_FIXTURE_PROVIDER", "codex")
            .env("HEY_BOSS_FIXTURE_EXPECT_MODEL", "exact-model")
            .env("HEY_BOSS_FIXTURE_JOB_HOLD", "1")
            .env("HEY_BOSS_FIXTURE_JOB_AUDIT", &audit)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .env_remove("HEY_BOSS_FLEET_SUPERVISED")
            .env_remove("CODEX_THREAD_ID");
        c
    };
    let cli = |args: &[&str]| -> Value {
        let out = command(&binary)
            .args(args)
            .args(["--project", PROJECT, "--json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let supervisor = || {
        let service = Service(
            command(&binary)
                .args(["fleet", "supervisor"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        wait_for("fleet socket", || {
            UnixStream::connect(root.join("fleet.sock")).is_ok()
        });
        service
    };
    let mut fleet = supervisor();
    cli(&[
        "project",
        "init",
        "--yes",
        "--prs",
        "false",
        "--worktree",
        "true",
    ]);
    let git = |args: &[&str]| {
        let out = command(Path::new("git")).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let id = managed_worker::register(&db_path);
    let mut worker = Service(
        command(&binary)
            .args([
                "worker",
                "--id",
                &id,
                "--project",
                PROJECT,
                "--cwd",
                checkout.to_str().unwrap(),
                "--concurrency",
                "1",
                "--worktree",
                "--json",
            ])
            .env("HEY_BOSS_FLEET_MANAGED", "1")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let db =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    wait_for("registered job worker", || {
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_job_workers WHERE worker_id=?1)",
            [&id],
            |r| r.get(0),
        )
        .unwrap()
    });
    git(&["init", "-q", "--initial-branch=legacy"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "fixture",
    ]);
    let wait_state = |run: &str, expected: &str| {
        wait_for(expected, || {
            let (state, reason): (String, Option<String>) = db
                .query_row(
                    "SELECT state, reason FROM scheduled_job_runs WHERE id=?1",
                    [run],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            if state == expected {
                return true;
            }
            assert!(
                !matches!(
                    state.as_str(),
                    "failed" | "cancelled" | "succeeded" | "skipped"
                ),
                "{run}: {state} {reason:?}"
            );
            false
        });
    };
    let executable = root.join("maintenance script");
    fs::write(&executable, "#!/bin/sh\npwd\necho diagnostic >&2\nexit 7\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    cli(&[
        "job",
        "create",
        "--id",
        "script",
        "--name",
        "Maintenance",
        "--cron",
        "0 3 * * *",
        "--timezone",
        "UTC",
        "--executable",
        executable.to_str().unwrap(),
        "--paused",
    ]);
    let script = cli(&["job", "run-now", "script"]);
    let run = script["run"]["id"].as_str().unwrap();
    wait_state(run, "failed");
    let logs = cli(&["job", "logs", "script", run]);
    let log = logs["log"].as_str().unwrap();
    assert!(
        log.lines().any(|l| l == checkout.to_str().unwrap()) && log.contains("diagnostic"),
        "{log}"
    );
    assert!(
        cli(&["job", "run", "script", run])["run"]["reason"]
            .as_str()
            .unwrap()
            .contains('7')
    );

    cli(&[
        "job",
        "create",
        "--id",
        "agent",
        "--name",
        "Agent",
        "--cron",
        "0 * * * *",
        "--timezone",
        "UTC",
        "--harness",
        "codex",
        "--model",
        "exact-model",
        "--instructions",
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/jobs/fixtures/original.md"),
        "--paused",
    ]);
    let agent = cli(&["job", "run-now", "agent"]);
    let run = agent["run"]["id"].as_str().unwrap();
    wait_state(run, "running");
    wait_for("agent prompt", || {
        audit.metadata().is_ok_and(|m| m.len() > 0)
    });
    let execution = || {
        let (pid, report): (u32, String) = db
            .query_row(
                "SELECT owner_pid,report FROM local_job_executions WHERE run_id=?1",
                [run],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(pid, worker.0.id());
        serde_json::from_str::<Value>(&report).unwrap()
    };
    let report = execution();
    let cwd = Path::new(report["cwd"].as_str().unwrap());
    assert!(cwd.is_dir() && cwd != checkout);
    assert!(
        git(&["worktree", "list", "--porcelain"])
            .lines()
            .any(|line| line.strip_prefix("worktree ") == cwd.to_str())
    );
    let no_tasks = || {
        let counts: (u32, u32) = db
            .query_row(
                "SELECT (SELECT count(*) FROM issues),(SELECT count(*) FROM worker_runs)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0));
    };
    no_tasks();
    let status = cli(&["worker", "status", "--id", &id]);
    assert_eq!(status["active"], 0);
    assert_eq!(status["free"], 1);
    assert_eq!(
        status["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id)
            .unwrap()["job_active"],
        1
    );
    fleet.terminate();
    assert_eq!(execution()["state"], "running");
    assert!(worker.0.try_wait().unwrap().is_none());
    let mut fleet = supervisor();
    cli(&["job", "stop", "agent", run]);
    wait_state(run, "cancelled");
    let launches = fs::read_to_string(audit).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(launches.trim()).unwrap()["run"],
        run
    );
    no_tasks();
    worker.terminate();
    fleet.terminate();
}
