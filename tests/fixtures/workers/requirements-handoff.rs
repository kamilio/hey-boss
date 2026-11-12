// Installed worker/service compatibility check. The same executable acts as the synthetic Codex child.
mod protocol;
use clap::Parser;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Parser)]
struct Options {
    /// Installed service and CLI binary.
    binary: PathBuf,
    /// Expected substring in the installed version.
    build: Option<String>,
    #[arg(long)]
    worker_binary: Option<PathBuf>,
    /// Keep the resulting project available for visual inspection.
    #[arg(long)]
    serve: bool,
}

fn handoff_worker() {
    use protocol::*;
    let session = id();
    for msg in messages() {
        let (Some(method), Some(request)) = (msg["method"].as_str(), msg.get("id")) else {
            continue;
        };
        match method {
            "thread/start" => result(request, json!({"thread":{"id":session}})),
            "turn/start" => {
                let rest = msg["params"]["input"][0]["text"]
                    .as_str()
                    .unwrap()
                    .split_once("issue view ")
                    .unwrap()
                    .1;
                let number = &rest[..rest
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(rest.len())];
                let turn = id();
                issue(&session, &["claim", number]);
                let view = issue(&session, &["view", number]);
                let body = format!(
                    "Published handoff notes\n\n{}",
                    view["issue"]["body"].as_str().unwrap()
                );
                issue(&session, &["edit", number, "--body", &body]);
                issue(&session, &["ready", number, "--acknowledge-requirements"]);
                issue(
                    &session,
                    &[
                        "comment",
                        number,
                        "--body",
                        "Reviewed delivery details before watcher handoff.",
                    ],
                );
                issue(&session, &["assign", number, "github"]);
                issue(
                    &session,
                    &[
                        "comment",
                        number,
                        "--body",
                        "Published delivery details after watcher handoff.",
                    ],
                );
                result(request, json!({"turn":{"id":turn}}));
                finish(
                    &session,
                    &turn,
                    "Published delivery remains Ready for review.",
                );
            }
            _ => result(request, json!({})),
        }
    }
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
const PROJECT: &str = "Requirements handoff QA";

struct Process(Child);
impl Process {
    fn running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            unsafe {
                libc::kill(self.0.id() as i32, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while matches!(self.0.try_wait(), Ok(None)) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }
}

struct Fixture {
    root: PathBuf,
    socket: PathBuf,
    binary: PathBuf,
}
impl Fixture {
    fn new(binary: &Path) -> Self {
        let binary = binary.canonicalize().unwrap();
        let out = env::current_dir().unwrap().join("out");
        fs::create_dir_all(&out).unwrap();
        let root = out
            .canonicalize()
            .unwrap()
            .join(format!("handoff-{}", protocol::id()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let digest = format!(
            "{:x}",
            Sha256::digest(root.join("issues.db").as_os_str().as_encoded_bytes())
        );
        let socket = PathBuf::from(format!(
            "/tmp/hey-boss-db-{}/{}.sock",
            unsafe { libc::getuid() },
            &digest[..24]
        ));
        Self {
            root,
            socket,
            binary,
        }
    }
    fn cmd(&self, exe: &Path) -> Command {
        let mut cmd = Command::new(exe);
        cmd.current_dir(&self.root)
            .stdin(Stdio::null())
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", &self.root)
            .env("HEY_BOSS_INBOX_SOCKET", self.root.join("inbox.sock"))
            .env("HEY_BOSS_CODEX", env::current_exe().unwrap())
            .env("HEY_BOSS_TEST_CLI", &self.binary);
        for key in [
            "HEY_BOSS_ISSUE_HOST",
            "HEY_BOSS_ISSUE_PROJECT",
            "HEY_BOSS_STATE_DIR",
            "HEY_BOSS_AGENT_ID",
            "HEY_BOSS_WORKER_RUN",
            "CODEX_THREAD_ID",
        ] {
            cmd.env_remove(key);
        }
        cmd
    }
    fn run(&self, cmd: &mut Command) -> String {
        let path = self.root.join("cli.out");
        let mut child = Process(
            cmd.stdout(fs::File::create(&path).unwrap())
                .spawn()
                .unwrap(),
        );
        wait_for("CLI response", 30, || !child.running());
        let text = fs::read_to_string(path).unwrap();
        assert!(child.0.wait().unwrap().success(), "{cmd:?}: {text}");
        text
    }
    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.run(self.cmd(&self.binary).args(args))).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        for ext in ["sock", "lock", "startup", "log"] {
            let _ = fs::remove_file(self.socket.with_extension(ext));
        }
    }
}
fn wait_for(label: &str, seconds: u64, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !ready() {
        assert!(
            !INTERRUPTED.load(Ordering::Relaxed),
            "Interrupted waiting for {label}"
        );
        assert!(Instant::now() < deadline, "Timed out waiting for {label}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn main() {
    if env::args_os()
        .rev()
        .take(3)
        .eq(["stdio://", "--listen", "app-server"])
    {
        return handoff_worker();
    }
    let options = Options::parse();
    let f = Fixture::new(&options.binary);
    let worker = options
        .worker_binary
        .map(|p| p.canonicalize().unwrap())
        .unwrap_or_else(|| f.binary.clone());
    ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::Relaxed)).unwrap();
    let version = f.run(f.cmd(&f.binary).arg("--version")).trim().to_owned();
    let worker_version = f.run(f.cmd(&worker).arg("--version")).trim().to_owned();
    println!(
        "{}",
        json!({"service_version":version,"worker_version":worker_version})
    );
    if let Some(build) = options.build {
        assert!(
            version.contains(&build),
            "Installed build mismatch: {version}"
        );
    }
    assert!(
        f.run(f.cmd(&f.binary).args(["issue", "ready", "--help"]))
            .contains("--acknowledge-requirements")
    );
    let mut owner = Process(
        f.cmd(&f.binary)
            .args(["fleet", "companion"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for("database owner", 30, || {
        assert!(owner.running());
        f.socket.exists()
    });
    f.json(&[
        "project",
        "init",
        "--project",
        PROJECT,
        "--prs",
        "true",
        "--worktree",
        "false",
        "--yes",
        "--json",
    ]);
    let issue = |args: &[&str]| {
        let mut base = vec![
            "issue",
            "--project",
            PROJECT,
            "--agent",
            "human:verification",
            "--json",
        ];
        base.extend(args);
        f.json(&base)
    };
    issue(&[
        "create",
        "--title",
        "Published delivery awaiting review",
        "--body",
        "Original requirements remain unchanged.",
    ]);
    issue(&[
        "pr",
        "add",
        "1",
        "https://github.com/example/repo/pull/1",
        "--purpose",
        "fix",
    ]);
    let launcher = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/managed-worker.sh");
    let mut child = Process(
        f.cmd(&launcher)
            .arg(&worker)
            .args(["--project", PROJECT, "--directory"])
            .arg(&f.root)
            .arg("--prs")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let status = || {
        f.json(&[
            "worker",
            "--project",
            PROJECT,
            "--json",
            "--history",
            "20",
            "status",
        ])
    };
    wait_for("worker handoff", 60, || {
        assert!(child.running(), "Worker {worker_version} exited early");
        let snapshot = status();
        let Some(run) = snapshot["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["number"] == 1 && r["finished_at"].is_number())
        else {
            return false;
        };
        assert_eq!(run["state"], "completed", "{run}");
        assert_eq!(snapshot["eligible"], 0);
        true
    });
    let view = issue(&["view", "1"]);
    let task = &view["issue"];
    assert_eq!(task["state"], "ready");
    assert_eq!(task["assignee"], "watcher:github");
    assert_eq!(task["agent_launch_count"], 1);
    assert_eq!(task.get("closed_at"), Some(&Value::Null));
    assert!(
        task["body"]
            .as_str()
            .unwrap()
            .starts_with("Published handoff notes\n\n")
    );
    thread::sleep(Duration::from_millis(2500));
    let after = status();
    assert_eq!(after["eligible"], 0);
    assert_eq!(
        after["runs"].as_array().unwrap().len(),
        1,
        "Unexpected repeat run"
    );
    drop(child);
    println!(
        "{}",
        json!({"status":"passed","version":version,"worker_version":worker_version,"state":task["state"],"assignee":task["assignee"],"agent_launches":1,"redundant_runs":0})
    );
    if options.serve {
        let ready = f.root.join("web.out");
        let mut web = Process(
            f.cmd(&f.binary)
                .args([
                    "issue",
                    "web",
                    "--project",
                    PROJECT,
                    "--port",
                    "0",
                    "--no-discovery",
                    "--json",
                ])
                .stdout(fs::File::create(&ready).unwrap())
                .spawn()
                .unwrap(),
        );
        wait_for("web URL", 30, || {
            assert!(web.running() && owner.running());
            fs::read(&ready).unwrap().contains(&b'\n')
        });
        let info: Value =
            serde_json::from_str(fs::read_to_string(&ready).unwrap().lines().next().unwrap())
                .unwrap();
        println!(
            "{}",
            json!({"url":info["url"].as_str().unwrap(),"fixture_pid":std::process::id()})
        );
        while !INTERRUPTED.load(Ordering::Relaxed) {
            assert!(web.running() && owner.running());
            thread::sleep(Duration::from_millis(100));
        }
    }
}
