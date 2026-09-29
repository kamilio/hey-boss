//! Commit ownership follows the checkout; issue ownership follows the worker.
use hey_boss::database::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const CONSUMER: &str = "github.com/example/consumer";
const UPSTREAM: &str = "github.com/example/upstream";

struct Fixture {
    root: PathBuf,
    owner: Option<Child>,
}

impl Fixture {
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", self.root.join("fleet"))
            .env("HEY_BOSS_INBOX_SOCKET", self.root.join("inbox.sock"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .env_remove("HEY_BOSS_ISSUE_NUMBER")
            .env_remove("HEY_BOSS_FLEET_SUPERVISED")
            .env_remove("HEY_BOSS_WORKER_RUN_ID");
        command
    }

    fn issue(&self, project: &str, args: &[&str], worker: bool) -> Value {
        let mut command = self.command();
        command.args(["issue", "--agent", "test:commits", "--json"]);
        if worker {
            command
                .env("HEY_BOSS_ISSUE_PROJECT", project)
                .env("HEY_BOSS_ISSUE_NUMBER", "1");
        } else {
            command.args(["--project", project]);
        }
        let output = command.args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if output.stdout.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.root)
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Commit QA",
                "-c",
                "user.email=qa@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("hey-boss-commit-qa-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut fixture = Self {
            root: root.canonicalize().unwrap(),
            owner: None,
        };
        fixture.owner = Some(
            fixture
                .command()
                .args(["fleet", "companion"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while Connection::connect(&fixture.root.join("issues.db")).is_err() {
            assert!(Instant::now() < deadline, "fixture owner did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        fixture.git(&["init", "--quiet"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "git@github.com:example/upstream.git",
        ]);
        fixture.git(&["commit", "--allow-empty", "-m", "Upstream fix"]);
        for project in [CONSUMER, UPSTREAM] {
            fixture.issue(
                project,
                &["create", "--title", "Commit provenance QA"],
                false,
            );
        }
        fixture
    }

    fn commits(&self, project: &str) -> Vec<Value> {
        self.issue(project, &["commit", "list", "1"], false)["commits"]
            .as_array()
            .unwrap()
            .clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut owner) = self.owner.take() {
            unsafe {
                libc::kill(owner.id() as i32, libc::SIGTERM);
            }
            let _ = owner.wait();
        }
        let identity = format!(
            "{:x}",
            Sha256::digest(self.root.join("issues.db").as_os_str().as_encoded_bytes())
        );
        let socket = format!(
            "/tmp/hey-boss-db-{}/{}",
            unsafe { libc::getuid() },
            &identity[..24]
        );
        for suffix in [".sock", ".lock", ".startup", ".log"] {
            let _ = fs::remove_file(format!("{socket}{suffix}"));
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn cross_repository_hook_and_explicit_attachments_keep_the_true_repository() {
    let f = Fixture::new();
    let first = f.git(&["rev-parse", "HEAD"]);
    let url = format!("https://{UPSTREAM}/commit/{first}");
    f.issue(CONSUMER, &["commit", "hook"], true);
    let commits = f.commits(CONSUMER);
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["url"], url);
    assert_eq!(commits[0]["title"], "Upstream fix");
    assert_eq!(commits[0]["origin"]["cwd"], f.root.to_str().unwrap());
    assert_eq!(commits[0]["added_by"], "test:commits");
    assert!(
        f.commits(UPSTREAM).is_empty(),
        "worker destination must not change"
    );

    // Manual HEAD attachment and repeated post-rewrite capture deduplicate.
    f.issue(CONSUMER, &["commit", "add", "1", "HEAD"], false);
    f.issue(CONSUMER, &["commit", "hook"], true);
    assert_eq!(f.commits(CONSUMER).len(), 1);
    assert_eq!(f.commits(CONSUMER)[0]["url"], url);
    f.issue(UPSTREAM, &["commit", "add", "1", "HEAD"], false);
    assert_eq!(f.commits(UPSTREAM)[0]["url"], url);

    // An explicit URL remains authoritative even if this checkout has its SHA.
    let explicit = format!("https://{CONSUMER}/commit/{first}");
    let resolved =
        hey_boss::issues::commits::resolve_commit_input(&explicit, UPSTREAM, Some(&f.root), None)
            .unwrap();
    assert_eq!(resolved.url, explicit);

    // An unresolved SHA retains the issue project's existing fallback.
    let missing = "0123456789012345678901234567890123456789";
    let resolved =
        hey_boss::issues::commits::resolve_commit_input(missing, CONSUMER, Some(&f.root), None)
            .unwrap();
    assert_eq!(resolved.url, format!("https://{CONSUMER}/commit/{missing}"));

    // Rewritten commits and HTTPS remotes use the same repository rules.
    f.git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/example/upstream.git",
    ]);
    f.git(&[
        "commit",
        "--amend",
        "--allow-empty",
        "-m",
        "Rebased upstream fix",
    ]);
    let rewritten = f.git(&["rev-parse", "HEAD"]);
    f.issue(CONSUMER, &["commit", "hook"], true);
    assert!(
        f.commits(CONSUMER)
            .iter()
            .any(|c| c["url"] == format!("https://{UPSTREAM}/commit/{rewritten}"))
    );

    // No remote: automatic capture must not invent a URL in the launching repo.
    f.git(&["remote", "remove", "origin"]);
    f.git(&["commit", "--allow-empty", "-m", "Unpublished local fix"]);
    let before = f.commits(CONSUMER);
    f.issue(CONSUMER, &["commit", "hook"], true);
    assert_eq!(f.commits(CONSUMER), before);
}
