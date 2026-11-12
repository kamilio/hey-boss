//! Start a real loopback issue UI with disposable data, for interactive browser checks.
use clap::{Parser, ValueEnum};
use hey_boss::database::Owner;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};
mod archive;
mod seed;

#[derive(Clone, Copy, ValueEnum)]
enum Scenario {
    ArtifactDiagrams,
    PlanPrompt,
    LivePrompt,
    ProjectPicker,
    ProjectNames,
    LongTitle,
    StatusDot,
    AgentLaunches,
    CreatorModel,
    IssueListOrigins,
    IssueReopen,
    ImagePreview,
    ListPagination,
    QueuePlacement,
    Archive,
}
impl Scenario {
    fn destination(self) -> (&'static str, u16) {
        match self {
            Self::ArtifactDiagrams => ("Flowchart QA", 59549),
            Self::PlanPrompt => ("Plan QA", 59693),
            Self::LivePrompt => ("Prompt Studio", 4883),
            Self::ProjectPicker => ("Project 01", 59687),
            Self::ProjectNames => ("github.com/poe-internal/poe2", 59692),
            Self::IssueReopen => ("Reopen QA", 59661),
            Self::ImagePreview => ("Image Preview QA", 59661),
            Self::ListPagination => ("Pagination QA", 59671),
            Self::QueuePlacement => ("Queue placement QA", 59701),
            Self::Archive => ("Archive QA", 59662),
            Self::LongTitle => ("Long title QA", 59653),
            Self::StatusDot => ("Status Dot QA", 4798),
            Self::AgentLaunches => ("Launch QA", 4794),
            Self::CreatorModel => ("Creator model QA", 59734),
            Self::IssueListOrigins => ("List origin QA", 59676),
        }
    }
}
#[derive(Parser)]
#[command(about = "Start an isolated Hey Boss web fixture until Ctrl+C")]
struct Args {
    scenario: Scenario,
    /// Built hey-boss CLI (defaults to the debug binary next to Cargo's examples directory).
    binary: Option<PathBuf>,
}
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
struct Fixture {
    root: tempfile::TempDir,
    binary: PathBuf,
    db: PathBuf,
    owner: Owner,
}
impl Fixture {
    fn new(binary: &Path) -> Result<Self> {
        let out = std::env::current_dir()?.join("out");
        fs::create_dir_all(&out)?;
        let root = tempfile::Builder::new().prefix("web-").tempdir_in(out)?;
        let bin = root.path().join("hey-boss");
        fs::copy(binary, &bin).map_err(|e| format!("cannot copy CLI {}: {e}", binary.display()))?;
        fs::create_dir_all(root.path().join("codex/sessions"))?;
        fs::create_dir(root.path().join("home"))?;
        let db = root.path().join("issues.db");
        let owner = Owner::start(&db)?.ok_or("private database already has an owner")?;
        Ok(Self {
            root,
            binary: bin,
            db,
            owner,
        })
    }
    fn command(&self, args: &[&str]) -> Command {
        let root = self.root.path();
        let mut c = Command::new(&self.binary);
        c.current_dir(root)
            .env_clear()
            .env("HOME", root.join("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
            .env("HEY_BOSS_ISSUE_DB", &self.db)
            .env("HEY_BOSS_FLEET_STATE", root)
            .env("HEY_BOSS_INBOX_SOCKET", root.join("inbox.sock"))
            .env("CODEX_HOME", root.join("codex"))
            .env("HEY_BOSS_CODEX", "/usr/bin/false")
            .env("HEY_BOSS_AGENT_ID", "human:qa")
            .args(args);
        c
    }
    fn init(&self, project: &str) -> Result<()> {
        run(self.command(&[
            "project",
            "init",
            "--project",
            project,
            "--prs",
            "false",
            "--worktree",
            "false",
            "--yes",
            "--json",
        ]))?;
        Ok(())
    }
    fn issue(&self, project: &str, args: &[&str]) -> Command {
        let mut c = self.command(&["issue", "--project", project, "--json"]);
        c.args(args);
        c
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.owner.stop();
        if let Ok(db) = self.db.canonicalize() {
            let id = format!("{:x}", Sha256::digest(db.as_os_str().as_encoded_bytes()));
            let lock = PathBuf::from(format!(
                "/tmp/hey-boss-db-{}/{}.lock",
                unsafe { libc::getuid() },
                &id[..24]
            ));
            let _ = fs::remove_file(lock);
        }
    }
}
fn run(mut c: Command) -> Result<Value> {
    let o = c.output()?;
    if !o.status.success() {
        return Err(format!(
            "{:?}: {}{}",
            c.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&o.stderr),
            String::from_utf8_lossy(&o.stdout)
        )
        .into());
    }
    Ok(serde_json::from_slice(&o.stdout)?)
}
fn main() -> Result<()> {
    let args = Args::parse();
    let (project, port) = args.scenario.destination();
    let (stop, signal) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop.send(());
    })?;
    let binary = match args.binary {
        Some(path) => path,
        None => std::env::current_exe()?
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("hey-boss"),
    };
    let f = Fixture::new(&binary)?;
    f.init(project)?;
    seed::seed(&f, args.scenario, project)?;
    if matches!(args.scenario, Scenario::Archive) && !archive::wait(&f, &signal)? {
        return Ok(());
    }
    if signal.try_recv().is_ok() {
        return Ok(());
    }
    let mut web = f
        .command(&[
            "issue",
            "--project",
            project,
            "web",
            "--port",
            &port.to_string(),
            "--no-discovery",
            "--json",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = web.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = tx.send(reader.read_line(&mut line).map(|_| line));
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });
    let result = (|| -> Result<()> {
        let line = rx.recv_timeout(Duration::from_secs(15))??;
        if line.is_empty() {
            return Err("fixture server exited before reporting its URL".into());
        }
        let ready: Value = serde_json::from_str(&line)?;
        let url = ready["url"]
            .as_str()
            .ok_or_else(|| format!("fixture server did not start: {ready}"))?;
        let boot: Value = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()?
            .get(format!("{}/api/bootstrap", url.trim_end_matches('/')))
            .send()?
            .error_for_status()?
            .json()?;
        println!(
            "{}",
            json!({"fixture_pid":std::process::id(),"owner_pid":std::process::id(),"url":url,"native":url.trim_end_matches('/'),"project":boot["project"]["id"]})
        );
        loop {
            if let Some(status) = web.try_wait()? {
                return Err(format!("fixture server exited: {status}").into());
            }
            if signal.recv_timeout(Duration::from_millis(100)).is_ok() {
                return Ok(());
            }
        }
    })();
    if web.try_wait()?.is_none() {
        unsafe { libc::kill(web.id() as i32, libc::SIGTERM) };
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while web.try_wait()?.is_none() {
            if std::time::Instant::now() >= deadline {
                web.kill()?;
                web.wait()?;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    result
}
