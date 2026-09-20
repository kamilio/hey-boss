use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

// A concurrent fork can inherit a writable copy descriptor until exec, making
// Linux reject the new executable with ETXTBSY. Keep copies and launches apart.
static EXECUTABLE_SETUP: Mutex<()> = Mutex::new(());

struct Fixture {
    root: PathBuf,
    tasks: Arc<Mutex<BTreeMap<String, Value>>>,
    stop: Arc<AtomicBool>,
    inbox: Option<thread::JoinHandle<()>>,
    worker: Option<Child>,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = PathBuf::from(format!("/tmp/hb61-{}-{mode}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        // Pin the built inode across Cargo's atomic binary replacement. Unlike
        // copying in parallel tests, this opens no executable for writing that
        // another fork can briefly inherit and trigger Linux ETXTBSY.
        fs::hard_link(env!("CARGO_BIN_EXE_hey-boss"), root.join("hey-boss")).unwrap();
        fs::write(root.join("mode.txt"), mode).unwrap();
        let listener = UnixListener::bind(root.join("inbox.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let tasks = Arc::new(Mutex::new(BTreeMap::<String, Value>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (saved, finished) = (tasks.clone(), stop.clone());
        let mode = mode.to_owned();
        let inbox = thread::spawn(move || {
            let mut stalled = vec![];
            while !finished.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut bytes = Vec::new();
                if stream.read_to_end(&mut bytes).is_err() {
                    continue;
                }
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                if mode == "slow-status" && request["command"] == "status" {
                    stalled.push(stream);
                    continue;
                }
                let mut tasks = saved.lock().unwrap();
                let reply = match request["command"].as_str().unwrap() {
                    "ask" => {
                        let id = format!("notice-{}", tasks.len() + 1);
                        let mut task = request.clone();
                        task["status"] = json!("pending");
                        task["task_id"] = json!(id);
                        tasks.insert(id.clone(), task);
                        json!({"task_id":id})
                    }
                    "status" => tasks[request["task_id"].as_str().unwrap()].clone(),
                    "hide" => {
                        let task = tasks.get_mut(request["task_id"].as_str().unwrap()).unwrap();
                        if task["status"] == "pending" {
                            task["status"] = json!("cancelled");
                        }
                        task.clone()
                    }
                    command => panic!("Unexpected Inbox command {command}"),
                };
                stream
                    .write_all(&serde_json::to_vec(&reply).unwrap())
                    .unwrap();
            }
        });
        let mut f = Self {
            root,
            tasks,
            stop,
            inbox: Some(inbox),
            worker: None,
        };
        f.cli(&["issue", "create", "--title", "Approval fixture"]);
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-approvals.mjs");
        let fixture = f.root.join("codex");
        let _guard = EXECUTABLE_SETUP.lock().unwrap();
        fs::copy(source, &fixture).unwrap();
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o755)).unwrap();
        f.worker = Some(
            f.command(&["worker", "--directory", f.root.to_str().unwrap()])
                .env("HEY_BOSS_CODEX", fixture)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        f
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(self.root.join("hey-boss"));
        c.current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_ISSUE_PROJECT", "named:Approval QA")
            .env("HEY_BOSS_AGENT_ID", "human:approvals-test")
            .env("HEY_BOSS_FLEET_STATE", self.root.join("fleet-state"))
            .env("HEY_BOSS_FLEET_DESIRED", self.root.join("fleet.json"))
            .env("HEY_BOSS_INBOX_SOCKET", self.root.join("inbox.sock"))
            .env("HEY_BOSS_TEST_CLI", self.root.join("hey-boss"))
            .env_remove("HEY_BOSS_ISSUE_HOST");
        c.arg(args[0]).arg("--json");
        if args[0] == "worker" {
            c.args(["--project", "Approval QA"]);
        }
        c.args(&args[1..]);
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        let output = {
            let _guard = EXECUTABLE_SETUP.lock().unwrap();
            self.command(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        }
        .wait_with_output()
        .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn wait(&self, predicate: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !predicate() {
            assert!(
                Instant::now() < deadline,
                "Timed out: {}",
                self.cli(&["worker", "status"])
            );
            thread::sleep(Duration::from_millis(30));
        }
    }
    fn answer(&self, id: &str, answer: &str) {
        let mut tasks = self.tasks.lock().unwrap();
        let task = tasks.get_mut(id).unwrap();
        task["status"] = json!("ok");
        task["result"] = json!(answer);
    }
    fn replies(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("protocol.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|v| v.get("method").is_none() && v.get("result").is_some())
            .collect()
    }
    fn finished(&self) -> bool {
        self.cli(&["worker", "status"])["runs"][0]["finished_at"].is_number()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            unsafe {
                libc::kill(worker.id() as i32, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while worker.try_wait().unwrap().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            let _ = worker.kill();
            let _ = worker.wait();
        }
        self.stop.store(true, Ordering::Relaxed);
        let joined = self.inbox.take().unwrap().join();
        if !thread::panicking() {
            joined.unwrap();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn approvals_keep_the_claim_and_continue_the_original_session() {
    for mode in ["command", "files", "permissions", "network", "ack-race"] {
        let f = Fixture::new(mode);
        f.wait(|| f.tasks.lock().unwrap().len() == 1);
        assert!(f.replies().is_empty());
        assert_eq!(f.cli(&["worker", "status"])["active"], 1);
        assert!(
            f.cli(&["issue", "view", "1"])["issue"]["assignee"]
                .as_str()
                .unwrap()
                .starts_with("codex:")
        );
        let task = f.tasks.lock().unwrap()["notice-1"].clone();
        assert_eq!(task["project"], "Approval QA");
        assert_eq!(
            task["issue"],
            json!({"project":"named:Approval QA","number":1})
        );
        if mode == "files" {
            assert!(
                task["description"]
                    .as_str()
                    .unwrap()
                    .contains("-old\n    +new")
            );
        }
        let answer = if mode == "permissions" {
            "Approve for this turn"
        } else {
            "Approve once"
        };
        f.answer("notice-1", answer);
        f.wait(|| f.finished());
        assert_eq!(
            f.cli(&["worker", "status"])["runs"][0]["state"],
            "completed"
        );
        let replies = f.replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["id"], "approval-0");
        if mode == "permissions" {
            assert_eq!(replies[0]["result"]["scope"], "turn");
        } else {
            assert_eq!(replies[0]["result"], json!({"decision":"accept"}));
        }
    }
}

#[test]
fn concurrent_approvals_route_out_of_order_to_exact_callback_ids() {
    let f = Fixture::new("concurrent");
    f.wait(|| f.tasks.lock().unwrap().len() == 2);
    f.answer("notice-2", "Decline");
    f.wait(|| f.replies().len() == 1);
    assert_eq!(
        f.replies()[0],
        json!({"id":"approval-1","result":{"decision":"decline"}})
    );
    assert_eq!(f.cli(&["worker", "status"])["active"], 1);
    f.answer("notice-1", "Approve once");
    f.wait(|| f.finished());
    assert_eq!(f.replies()[1]["id"], "approval-0");
}

#[test]
fn dismissal_free_text_and_cancel_never_grant_approval() {
    for (mode, answer) in [
        ("dismiss", None),
        ("free-text", Some("yes")),
        ("cancel", Some("Cancel")),
    ] {
        let f = Fixture::new(mode);
        f.wait(|| f.tasks.lock().unwrap().len() == 1);
        if let Some(answer) = answer {
            f.answer("notice-1", answer);
        } else {
            f.tasks.lock().unwrap().get_mut("notice-1").unwrap()["status"] = json!("cancelled");
        }
        f.wait(|| f.finished());
        assert_eq!(f.replies()[0]["result"], json!({"decision":"cancel"}));
        assert_eq!(f.cli(&["worker", "status"])["runs"][0]["state"], "blocked");
        // Age beyond ordinary failure backoff: approval holds still require a human retry.
        rusqlite::Connection::open(f.root.join("issues.db"))
            .unwrap()
            .execute("UPDATE worker_runs SET finished_at=0", [])
            .unwrap();
        assert_eq!(f.cli(&["worker", "status"])["eligible"], 0);
    }
}

#[test]
fn stopping_worker_cancels_pending_inbox_questions_without_approval() {
    let f = Fixture::new("stop");
    f.wait(|| f.tasks.lock().unwrap().len() == 1);
    let status = f.cli(&["worker", "status"]);
    let id = status["worker_id"].as_str().unwrap();
    f.cli(&["worker", "stop", id]);
    f.wait(|| f.finished());
    f.wait(|| f.tasks.lock().unwrap()["notice-1"]["status"] == "cancelled");
    assert!(f.replies().is_empty());
}

#[test]
fn resolved_requests_are_dismissed_and_foreign_threads_are_not_prompted() {
    let f = Fixture::new("resolved");
    f.wait(|| f.finished());
    assert_eq!(f.tasks.lock().unwrap()["notice-1"]["status"], "cancelled");
    assert!(f.replies().is_empty());
    let f = Fixture::new("foreign");
    f.wait(|| f.finished());
    assert!(f.tasks.lock().unwrap().is_empty());
    assert!(f.replies().is_empty());
}

#[test]
fn slow_status_connection_does_not_delay_worker_stop() {
    let f = Fixture::new("slow-status");
    f.wait(|| f.tasks.lock().unwrap().len() == 1);
    thread::sleep(Duration::from_millis(1200));
    let status = f.cli(&["worker", "status"]);
    let started = Instant::now();
    f.cli(&["worker", "stop", status["worker_id"].as_str().unwrap()]);
    f.wait(|| f.finished());
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(f.replies().is_empty());
}
