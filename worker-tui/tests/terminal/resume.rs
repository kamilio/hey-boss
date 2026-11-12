use super::*;
use std::{
    collections::BTreeMap,
    process::{Child, Stdio},
};

struct Fleet {
    dir: Rc<tempfile::TempDir>,
    env: BTreeMap<String, String>,
    service: Option<Child>,
    added: bool,
}
impl Fleet {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.env["HEY_BOSS_TEST_CLI"]);
        cmd.current_dir(self.dir.path())
            .env_clear()
            .envs(&self.env)
            .args(args);
        cmd
    }
    fn cli(&self, args: &[&str]) -> Value {
        let o = self.cmd(args).output().unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn worker(&self) -> Value {
        self.cli(&["workers", "--json", "status"])["workers"][0].clone()
    }
    fn watch(&self, output: &Path) -> Terminal {
        Terminal::open(
            output.into(),
            self.dir.clone(),
            (120, 30),
            vec![
                self.env["HEY_BOSS_TEST_CLI"].clone(),
                "workers".into(),
                "watch".into(),
            ],
            self.env.clone().into_iter().collect(),
        )
    }
}
impl Drop for Fleet {
    fn drop(&mut self) {
        if self.added {
            let _ = fs::write(self.dir.path().join("release-agent"), "");
            let _ = self
                .cmd(&["workers", "--json", "remove", "resume-fixture"])
                .output();
            let end = Instant::now() + Duration::from_secs(15);
            while Instant::now() < end {
                let o = self.cmd(&["worker", "--json", "status"]).output();
                if o.ok()
                    .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
                    .is_some_and(|s| {
                        s["workers"].as_array().is_some_and(|ws| {
                            ws.iter().all(|w| {
                                w["pid"].is_null() && w["active"].as_u64().unwrap_or(0) == 0
                            })
                        })
                    })
                {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        if let Some(service) = &mut self.service {
            unsafe { libc::kill(service.id() as i32, libc::SIGTERM) };
            let _ = service.wait();
        }
    }
}

fn until(label: &str, mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(25);
    while !predicate() {
        assert!(Instant::now() < end, "timed out: {label}");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "builds the main CLI and starts a private supervisor"]
fn pause_and_resume_preserve_the_running_agent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let build = Command::new(env!("CARGO"))
        .current_dir(root.parent().unwrap())
        .args([
            "build",
            "--locked",
            "-p",
            "hey-boss",
            "--bin",
            "hey-boss",
            "--example",
            "codex-drain",
            "--message-format=json-render-diagnostics",
        ])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let executables = String::from_utf8(build.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            Some((
                event["target"]["name"].as_str()?.to_owned(),
                event["executable"].as_str()?.to_owned(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let dir = Rc::new(
        tempfile::Builder::new()
            .prefix(".r")
            .tempdir_in(root)
            .unwrap(),
    );
    let path = |name: &str| dir.path().join(name).to_str().unwrap().to_string();
    let output = root.join("target/resume-qa");
    fs::create_dir_all(&output).unwrap();
    fs::create_dir(path("home")).unwrap();
    fs::write(
        path("desired.json"),
        r#"{"machines":{"local":{"workers":[]}}}"#,
    )
    .unwrap();
    fs::write(path("inventory.json"), r#"{"ssh_hosts":[]}"#).unwrap();
    let env = [
        ("PATH", std::env::var("PATH").unwrap()),
        ("HOME", path("home")),
        ("GIT_CEILING_DIRECTORIES", root.display().to_string()),
        ("HEY_BOSS_ISSUE_DB", path("issues.db")),
        ("HEY_BOSS_FLEET_STATE", path("state")),
        ("HEY_BOSS_FLEET_DESIRED", path("desired.json")),
        ("HEY_BOSS_FLEET_CONFIG", path("inventory.json")),
        ("HEY_BOSS_FLEET_BINARY", executables["hey-boss"].clone()),
        ("HEY_BOSS_TEST_CLI", executables["hey-boss"].clone()),
        ("HEY_BOSS_CODEX", executables["codex-drain"].clone()),
        ("HEY_BOSS_INBOX_SOCKET", path("absent.sock")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let mut f = Fleet {
        dir: dir.clone(),
        env,
        added: false,
        service: None,
    };
    f.service = Some(
        f.cmd(&["fleet", "supervisor"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    until("private supervisor", || {
        dir.path().join("state/fleet.sock").exists()
    });
    f.cli(&[
        "project",
        "init",
        "--project",
        "Resume QA",
        "--prs",
        "false",
        "--worktree",
        "false",
        "--yes",
        "--json",
    ]);
    f.cli(&[
        "issue",
        "--json",
        "--agent",
        "human:resume-test",
        "create",
        "--title",
        "Keep building while pickup resumes",
    ]);
    f.cli(&[
        "workers",
        "--json",
        "add",
        "--id",
        "resume-fixture",
        "--name",
        "Builder",
        "-C",
        dir.path().to_str().unwrap(),
    ]);
    f.added = true;
    until("fixture agent", || {
        dir.path().join("agent-started").exists()
    });
    let before = f.worker();
    let agent: i32 = fs::read_to_string(path("agent-started"))
        .unwrap()
        .parse()
        .unwrap();
    f.cli(&["workers", "--json", "pause", "resume-fixture"]);
    assert_eq!(f.worker()["config"]["enabled"], false);
    let mut observer = f.watch(&output);
    observer.wait("0 available / 0 slots");
    observer.key("w");
    observer.wait("Finishing work before pause");
    observer.capture("paused-observer");
    observer.key("\x1b");
    observer.wait_for(|s| !s.contains("Finishing work before pause"));
    observer.quit();
    assert_eq!(f.worker()["config"]["enabled"], false);
    f.cli(&["workers", "--json", "resume", "resume-fixture"]);
    let mut t = f.watch(&output);
    t.wait("0 available / 1 slots");
    t.key("w");
    t.wait("BUSY");
    t.capture("resumed-worker");
    t.key("\x1b");
    for (c, r) in [(120, 30), (80, 24), (48, 16)] {
        t.resize(c, r);
        t.wait_for(|s| {
            s.contains("0 available / 1 slots")
                && s.lines()
                    .nth(r - 2)
                    .is_some_and(|l| l.contains("workers keep running"))
                && s.lines().nth(r - 1).is_some_and(|l| l.contains("help"))
        });
        let frame = t.capture(&format!("resumed-{c}"));
        assert!(frame.contains("1 active") && !frame.contains("Finishing work before pause"));
    }
    t.quit();
    let after = f.worker();
    assert!(before["pid"].is_number());
    assert_eq!(before["pid"], after["pid"]);
    assert_eq!(after["active"], 1);
    assert_eq!(
        before["config"]["concurrency"],
        after["config"]["concurrency"]
    );
    assert_eq!(after["config"]["enabled"], true);
    assert_eq!(after["intent"], "running");
    assert_eq!(unsafe { libc::kill(agent, 0) }, 0);
    fs::write(path("release-agent"), "").unwrap();
    until("agent completion", || f.worker()["active"] == 0);
    assert_eq!(
        f.cli(&["issue", "--json", "view", "1"])["issue"]["state"],
        "closed"
    );
    let history = f.cli(&[
        "worker",
        "--json",
        "--id",
        "resume-fixture",
        "--history",
        "20",
        "status",
    ]);
    let runs = history["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["state"], "completed");
    assert!(runs[0]["finished_at"].is_number());
}
