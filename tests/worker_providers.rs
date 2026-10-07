#[path = "support/projects.rs"]
mod projects;
use hey_boss::{agent_runtime::Provider, issues::worker::Settings};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn worker_provider_is_explicit_and_old_workers_keep_codex() {
    let old: Settings = serde_json::from_value(json!({"name":"Existing"})).unwrap();
    assert_eq!(serde_json::to_value(old).unwrap()["provider"], "codex");
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let settings: Settings = serde_json::from_value(json!({"provider":provider})).unwrap();
        assert_eq!(
            serde_json::to_value(settings).unwrap()["provider"],
            json!(provider)
        );
    }
    assert!(serde_json::from_value::<Settings>(json!({"provider":"unknown"})).is_err());
}

struct Fixture {
    root: PathBuf,
    child: Option<Child>,
}
impl Fixture {
    fn new(provider: &str, goal: bool) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "hey-boss-provider-{}-{provider}-{goal}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        projects::seed(&root.join("issues.db"), &["named:Provider test"]);
        let mut f = Self { root, child: None };
        f.cli(&["issue", "create", "--title", "Provider worker"]);
        let mut c = f.command();
        c.args([
            "worker",
            "run",
            "--project",
            "Provider test",
            "--directory",
            f.root.to_str().unwrap(),
            "--provider",
            provider,
        ]);
        if goal {
            c.args(["--prompt", "/goal goal fixture"]);
        }
        f.child = Some(
            c.env("HEY_BOSS_FIXTURE_PROVIDER", provider)
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        f
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_ISSUE_PROJECT", "named:Provider test")
            .env("HEY_BOSS_AGENT_ID", "human:provider-test")
            .env("HEY_BOSS_TEST_CLI", env!("CARGO_BIN_EXE_hey-boss"))
            .env(
                "HEY_BOSS_CLAUDE",
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/provider-worker.mjs"),
            )
            .env(
                "HEY_BOSS_PI",
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/provider-worker.mjs"),
            )
            .env_remove("HEY_BOSS_WORKER_RUN")
            .env_remove("HEY_BOSS_ISSUE_HOST");
        c
    }
    fn cli(&self, args: &[&str]) -> serde_json::Value {
        let out = self.command().args(args).arg("--json").output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn wait(&self, query: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let db = rusqlite::Connection::open(self.root.join("issues.db")).unwrap();
            if let Ok(value) = db.query_row(query, [], |r| r.get::<_, String>(0)) {
                return value;
            }
            assert!(Instant::now() < deadline, "Timed out: {query}");
            thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            };
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn claude_and_pi_workers_claim_steer_save_history_and_complete() {
    for provider in ["claude", "pi"] {
        let f = Fixture::new(provider, false);
        let run = f.wait("SELECT id FROM worker_runs WHERE claimed_at IS NOT NULL");
        let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
        db.execute("INSERT INTO agent_steering(request_id,run_id,scope,text,state,created_at) VALUES('steer',?1,'session','STEER','queued',0)",[&run]).unwrap();
        if provider == "claude" {
            thread::sleep(Duration::from_millis(1500));
            fs::write(f.root.join("finish"), "").unwrap();
        }
        assert_eq!(
            f.wait("SELECT state FROM worker_runs WHERE finished_at IS NOT NULL"),
            "completed",
            "{}",
            f.wait("SELECT summary FROM worker_runs WHERE finished_at IS NOT NULL")
        );
        assert_eq!(
            f.wait("SELECT state FROM agent_steering WHERE request_id='steer'"),
            "delivered"
        );
        let job: serde_json::Value =
            serde_json::from_str(&f.wait("SELECT job FROM worker_runs LIMIT 1")).unwrap();
        assert_eq!(job["session_ref"]["provider"], provider);
        assert_eq!(job["actor"]["kind"], provider);
        assert_eq!(
            job["actor"]["session_id"],
            "12345678-1234-1234-1234-123456789abc"
        );
        let transcript = fs::read_to_string(f.root.join(format!(
            "issues.db.agent-sessions/{provider}-12345678-1234-1234-1234-123456789abc.jsonl"
        )))
        .unwrap();
        assert!(transcript.contains("STEER") && transcript.contains("Provider worker verified"));
        let mut child = f
            .command()
            .args(["fleet", "conversation"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(json!({"run":run,"latest":true}).to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let page: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(page["availability"], "available", "{page}");
        assert!(
            page["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "assistant"
                    && m["label"] == if provider == "claude" { "Claude" } else { "Pi" }),
            "{page}"
        );
    }
}
#[test]
fn claude_and_pi_goals_continue_in_the_same_session() {
    for provider in ["claude", "pi"] {
        let f = Fixture::new(provider, true);
        assert_eq!(
            f.wait("SELECT state FROM worker_runs WHERE finished_at IS NOT NULL"),
            "completed",
            "{}",
            f.wait("SELECT summary FROM worker_runs WHERE finished_at IS NOT NULL")
        );
        let inputs = fs::read_to_string(f.root.join("inputs.jsonl")).unwrap();
        assert_eq!(inputs.lines().count(), 2);
        assert_eq!(
            f.wait("SELECT CAST(count(*) AS TEXT) FROM issue_agent_launches"),
            "1"
        );
        assert!(inputs.contains("Continue the saved goal"));
        assert_eq!(
            f.wait("SELECT json_extract(goal,'$.status') FROM worker_runs LIMIT 1"),
            "complete"
        );
    }
}

#[test]
fn provider_workers_stop_and_resume_only_matching_saved_sessions() {
    for (provider, next) in [("claude", "claude"), ("pi", "pi"), ("claude", "pi")] {
        let mut f = Fixture::new(provider, false);
        f.wait("SELECT id FROM worker_runs WHERE claimed_at IS NOT NULL");
        let mut child = f.child.take().unwrap();
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        };
        child.wait().unwrap();
        assert_eq!(
            f.wait("SELECT state FROM worker_runs WHERE finished_at IS NOT NULL"),
            "cancelled"
        );
        let mut c = f.command();
        c.args([
            "worker",
            "run",
            "--project",
            "Provider test",
            "--directory",
            f.root.to_str().unwrap(),
            "--provider",
            next,
        ])
        .env("HEY_BOSS_FIXTURE_PROVIDER", next)
        .stdout(Stdio::null());
        f.child = Some(c.spawn().unwrap());
        let job: serde_json::Value = serde_json::from_str(&f.wait(
            "SELECT job FROM worker_runs WHERE claimed_at IS NOT NULL AND finished_at IS NULL",
        ))
        .unwrap();
        assert_eq!(job["resume_session"].is_string(), provider == next);
        if provider == next {
            assert_eq!(job["session_ref"]["provider"], next);
        }
        let launches = fs::read_to_string(f.root.join("launches.jsonl")).unwrap();
        let args: Vec<String> = serde_json::from_str(launches.lines().last().unwrap()).unwrap();
        assert_eq!(
            args.iter().any(|v| v == "--resume" || v == "--session"),
            provider == next
        );
        fs::write(f.root.join("finish"), "").unwrap();
        assert_eq!(
            f.wait("SELECT state FROM worker_runs WHERE state='completed'"),
            "completed"
        );
    }
}

#[test]
#[ignore = "real authenticated Claude/Pi workers and isolated scratch issue stores"]
fn real_provider_workers_complete_their_claimed_issue() {
    use std::{
        io::{Read, Write},
        os::unix::net::UnixListener,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    for provider in ["claude", "pi"] {
        let root = PathBuf::from(format!(
            "/tmp/hb-provider-live-{}-{provider}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut f = Fixture { root, child: None };
        let checkout = f.root.join("checkout");
        fs::create_dir(&checkout).unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "Provider Test"],
            vec!["config", "user.email", "provider-test@example.invalid"],
        ] {
            assert!(
                Command::new("git")
                    .args(&args)
                    .current_dir(&checkout)
                    .stdout(Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        f.cli(&[
            "issue",
            "create",
            "--title",
            "Verify proof file",
            "--body",
            include_str!("prompts/goal-proof.md").trim(),
        ]);
        let listener = UnixListener::bind(f.root.join("inbox.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopping = stopped.clone();
        let inbox = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(20));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let reply = if request["command"] == "ask" {
                    json!({"task_id":"scratch-approval"})
                } else {
                    json!({"task_id":"scratch-approval","status":"ok","result":"Approve once"})
                };
                stream.write_all(reply.to_string().as_bytes()).unwrap();
            }
        });
        let mut c = f.command();
        c.current_dir(&checkout)
            .env_remove("HEY_BOSS_CLAUDE")
            .env_remove("HEY_BOSS_PI")
            .env("HEY_BOSS_INBOX_SOCKET", f.root.join("inbox.sock"))
            .args([
                "worker",
                "run",
                "--provider",
                provider,
                "--project",
                "Provider test",
                "--directory",
                checkout.to_str().unwrap(),
                "--prompt",
                &format!("/goal {}", hey_boss::issues::worker::DEFAULT_PROMPT),
            ])
            .stdout(Stdio::null());
        f.child = Some(c.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(240);
        let result = loop {
            let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
            if let Ok(value) = db.query_row(
                "SELECT state,summary FROM worker_runs WHERE finished_at IS NOT NULL",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            ) {
                break value;
            }
            if Instant::now() >= deadline {
                break ("timeout".into(), "No terminal worker result".into());
            }
            thread::sleep(Duration::from_millis(200));
        };
        stopped.store(true, Ordering::Relaxed);
        inbox.join().unwrap();
        assert_eq!(result.0, "completed", "{provider}: {}", result.1);
        assert_eq!(
            fs::read_to_string(checkout.join("proof.txt"))
                .unwrap()
                .trim(),
            "VERIFIED_GOAL"
        );
        assert_eq!(f.wait("SELECT state FROM issues WHERE number=1"), "closed");
        eprintln!(
            "PASS {provider}: real worker claimed and closed its issue; proof verified; {}",
            result.1
        );
    }
}
