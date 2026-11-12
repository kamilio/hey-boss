use super::*;
use std::{
    os::unix::{fs::symlink, net::UnixStream},
    time::Instant,
};

const PROJECT: &str = "github.com/acme/checkout-fixture";
const URL: &str = "https://github.com/acme/checkout-fixture.git";

fn rpc(root: &Path, request: Value) -> Value {
    let mut socket = UnixStream::connect(root.join("fleet.sock")).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    writeln!(socket, "{request}").unwrap();
    let mut line = String::new();
    BufReader::new(socket).read_line(&mut line).unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_ne!(reply["ok"], false, "{request}: {reply}");
    reply
}

// The supervisor deliberately leaves workers running on exit. On a failed check,
// stop only the process still holding this private worker's recorded identity.
struct WorkerCleanup<'a>(&'a Path);
impl Drop for WorkerCleanup<'_> {
    fn drop(&mut self) {
        let Ok(db) = rusqlite::Connection::open(self.0) else {
            return;
        };
        if let Ok((pid, start)) = db.query_row(
            "SELECT owner_pid,owner_start FROM issue_workers WHERE id='fixture-worker'",
            [],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?)),
        ) && hey_boss::agents::process_identity(pid).as_deref() == Some(&start)
        {
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }
}

#[test]
#[ignore = "runs real workers and a Git clone lasting over a minute"]
fn installed_project_checkouts_and_worker_controls() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let root = f.root.path();
    let home = root.join("home");
    let source = root.join("source");
    let git = |dir: &Path, args: &[&str]| {
        let r = Command::new("/usr/bin/git")
            .current_dir(dir)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .unwrap();
        assert!(
            r.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&r.stderr)
        );
    };
    let redirect = |src: &Path| {
        fs::write(home.join(".gitconfig"),format!("[protocol]\n allow = never\n[protocol \"file\"]\n allow = always\n[url \"{}\"]\n insteadOf = {URL}\n",src.display())).unwrap()
    };
    redirect(&source);
    fs::create_dir(&source).unwrap();
    git(&source, &["init", "--quiet"]);
    fs::write(source.join("README.md"), "fixture\n").unwrap();
    git(&source, &["add", "README.md"]);
    git(
        &source,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "Fixture",
        ],
    );
    let existing = home.join("checkout-fixture");
    git(&home, &["clone", "--quiet", URL, "checkout-fixture"]);
    fs::write(existing.join("local-work"), "keep").unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    symlink(
        worker_fixtures::binary("checkout-git"),
        root.join("bin/git"),
    )
    .unwrap();
    fs::write(
        root.join("fleet.yaml"),
        "machines: {local: {workers: []}}\n",
    )
    .unwrap();
    let _worker_cleanup = WorkerCleanup(&f.db);
    let mut supervisor = Service(
        f.command(&["fleet", "supervisor"])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "PATH",
                std::env::join_paths([root.join("bin"), "/usr/bin".into(), "/bin".into()]).unwrap(),
            )
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    while !root.join("fleet.sock").exists() {
        assert!(
            supervisor.0.try_wait().unwrap().is_none()
                && started.elapsed() < Duration::from_secs(15),
            "Supervisor failed to start"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let edit = |update: Value| {
        let config = rpc(root, json!({"kind":"configuration"}));
        rpc(
            root,
            json!({"kind":"configuration","save":true,"revision":config["revision"],"machine_update":update}),
        )
    };
    let checkout = |path: &Path, url: &str| {
        edit(json!({"host":"local","action":"project","workspace":"~","path":path,"git":url}))
    };
    let wait = |label: &str, seconds: u64, ok: &dyn Fn(&Value) -> bool| {
        let start = Instant::now();
        loop {
            let state = rpc(root, json!({"kind":"status"}));
            let m = state["machines"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["host"] == "local")
                .unwrap();
            if ok(m) {
                break m.clone();
            }
            assert!(
                start.elapsed() < Duration::from_secs(seconds),
                "{label}: {m}"
            );
            thread::sleep(Duration::from_millis(200));
        }
    };
    let applied = |path: &Path, seconds| {
        wait("checkout did not apply", seconds, &|m| {
            m["configuration_error"].is_null()
                && m["applied_revision"] == m["desired_revision"]
                && m["projects"][PROJECT]["resolved_path"].as_str() == path.to_str()
        })
    };
    let retry = || {
        rpc(
            root,
            json!({"kind":"configuration","retry_project":{"host":"local","project":PROJECT}}),
        )
    };

    let saved = edit(json!({"host":"local","action":"project","workspace":"~","git":URL}));
    assert_eq!(saved["document"]["machines"]["local"]["workspace"], "~");
    assert_eq!(
        saved["document"]["machines"]["local"]["projects"][PROJECT]["path"],
        "~/checkout-fixture"
    );
    applied(&existing, 30);
    let calls = home.join("clone-calls");
    assert!(!calls.exists());
    let separate = home.join("separate-clone");
    redirect(&root.join("unavailable-source"));
    checkout(&separate, URL);
    wait("missing clone source", 30, &|m| {
        m["configuration_error"].is_string()
    });
    redirect(&source);
    retry();
    applied(&separate, 30);
    wait("pending retry cleared", 30, &|m| {
        m["project_retries"]
            .as_object()
            .is_none_or(|v| v.is_empty())
    });
    assert_eq!(fs::read(separate.join("README.md")).unwrap(), b"fixture\n");
    assert_eq!(fs::read(existing.join("local-work")).unwrap(), b"keep");

    let wrong = home.join("wrong");
    fs::create_dir(&wrong).unwrap();
    git(&wrong, &["init", "--quiet"]);
    git(
        &wrong,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/other/repo.git",
        ],
    );
    fs::write(wrong.join("local-work"), "keep").unwrap();
    checkout(&wrong, URL);
    wait("wrong repository refused", 30, &|m| {
        m["configuration_error"]
            .as_str()
            .is_some_and(|s| s.contains("another repository"))
    });
    assert_eq!(fs::read(wrong.join("local-work")).unwrap(), b"keep");
    git(&wrong, &["remote", "set-url", "origin", URL]);
    retry();
    applied(&wrong, 30);

    let fallback = home.join("fallback");
    let ssh = "git@github.com:acme/checkout-fixture.git";
    fs::write(&calls, "").unwrap();
    checkout(&fallback, ssh);
    applied(&fallback, 30);
    assert_eq!(
        fs::read_to_string(&calls)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [ssh, URL]
    );
    assert!(fallback.join("README.md").is_file());
    let slow = home.join("slow");
    fs::write(home.join("slow-clone"), "").unwrap();
    let start = Instant::now();
    checkout(&slow, URL);
    applied(&slow, 100);
    assert!(start.elapsed() >= Duration::from_secs(60) && slow.join("README.md").is_file());
    fs::remove_file(home.join("slow-clone")).unwrap();

    edit(json!({"host":"local","action":"add","id":"fixture-worker","concurrency":1}));
    let worker = |m: &Value| {
        m["workers"]
            .as_array()?
            .iter()
            .find(|w| w["id"] == "fixture-worker")
            .cloned()
    };
    let running = wait("worker startup", 30, &|m| {
        worker(m).is_some_and(|w| w["pid"].as_u64().is_some_and(|p| p > 0))
    });
    let config = &worker(&running).unwrap()["config"];
    assert_eq!(config["projects"], json!([PROJECT]));
    assert_eq!(config["directories"][PROJECT], json!(slow));
    fs::write(slow.join("local-work"), "keep").unwrap();
    let saved = edit(
        json!({"host":"local","action":"project","workspace":home,"path":slow,"git":URL,"worker":"fixture-worker"}),
    );
    assert_eq!(
        saved["document"]["machines"]["local"]["workspace"],
        json!(home)
    );
    applied(&slow, 30);
    assert_eq!(fs::read(slow.join("local-work")).unwrap(), b"keep");
    edit(json!({"host":"local","action":"remove","id":"fixture-worker"}));
    wait("idle worker drained", 30, &|m| {
        worker(m).is_some_and(|w| w["retiring"] == true && w["pid"].is_null())
    });
    let saved = rpc(root, json!({"kind":"configuration"}));
    let w = &saved["document"]["machines"]["local"]["workers"][0];
    assert_eq!(w["intent"], "drain");
    assert_eq!(w["retiring"], true);
}
