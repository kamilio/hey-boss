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
mod agents;
mod archive;
#[cfg(test)]
mod browser;

mod controls;
mod desktop;
mod fleet;
mod issue_design;
mod origin;
mod seed;

#[derive(Clone, Copy, ValueEnum)]
enum Scenario {
    AssignmentWatch,
    IssueArchitecture,
    IssueTimeline,
    IssueStatus,
    IssueListActions,
    ScheduledJobs,
    JobsUi,
    LocalHttp,
    NativeArtifactEditor,
    Agents,
    AutoWorkers,
    WorkerConfig,
    AgentsNavigation,
    AssignedAgent,
    Origin,
    ProjectSettings,
    QuietHours,
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
            Self::AssignmentWatch => ("Assignment QA", 48698),
            Self::IssueArchitecture => ("Issue Design QA", 48122),
            Self::IssueTimeline => ("Timeline QA", 48948),
            Self::IssueStatus => ("Progress Studio", 4796),
            Self::IssueListActions | Self::ScheduledJobs => ("Quick action QA", 59639),
            Self::JobsUi => ("Jobs UI QA", 59649),
            Self::LocalHttp => ("Local HTTP QA", 0),
            Self::NativeArtifactEditor => ("Native editor QA", 0),
            Self::Origin => ("Origin QA", 59675),
            Self::AssignedAgent => ("Assignment QA", 59668),
            Self::Agents => ("Atlas", 59641),
            Self::WorkerConfig => ("Atlas", 59648),
            Self::AutoWorkers => ("github.com/poe-platform/poe-code", 59688),
            Self::AgentsNavigation => ("Home", 59643),
            Self::ProjectSettings => ("Settings fixture", 59642),
            Self::QuietHours => ("Quiet hours QA", 59651),
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
struct Service(std::process::Child);
impl Drop for Service {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while self.0.try_wait().is_ok_and(|status| status.is_none()) {
                if std::time::Instant::now() >= deadline {
                    let _ = self.0.kill();
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            let _ = self.0.wait();
        }
    }
}
fn main() -> Result<()> {
    let args = Args::parse();
    let (project, port) = args.scenario.destination();
    let port = match (
        args.scenario,
        std::env::var("HEY_BOSS_SETTINGS_FIXTURE_PORT"),
    ) {
        (Scenario::ProjectSettings, Ok(value)) => value.parse::<u16>()?,
        _ => port,
    };
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
    let mut info = seed::seed(&f, args.scenario, project)?;
    if matches!(args.scenario, Scenario::Archive) && !archive::wait(&f, &signal)? {
        return Ok(());
    }
    if signal.try_recv().is_ok() {
        return Ok(());
    }
    let controls = match args.scenario {
        Scenario::JobsUi => Some(52049),
        Scenario::IssueStatus => Some(52070),
        Scenario::IssueListActions | Scenario::ScheduledJobs => Some(52039),
        _ => None,
    }
    .map(|port| {
        tiny_http::Server::http((std::net::Ipv4Addr::LOCALHOST, port))
            .map_err(std::io::Error::other)
    })
    .transpose()?;
    let records = info["records"].clone();
    let mut sequence = 0;
    let mut supervisor = if matches!(args.scenario, Scenario::WorkerConfig | Scenario::JobsUi) {
        Some(agents::supervisor(&f)?)
    } else {
        None
    };
    let mut web = Service(
        f.command(&[
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
        .spawn()?,
    );
    let stdout = web.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    (|| -> Result<()> {
        let mut starts = 0;
        let pid = web.0.id();
        let mut report = |line: String| -> Result<()> {
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
            info["fixture_pid"] = json!(std::process::id());
            info["owner_pid"] = info["fixture_pid"].clone();
            info["url"] = json!(url);
            info["native"] = json!(url.trim_end_matches('/'));
            info["project"] = boot["project"]["id"].clone();
            if matches!(args.scenario, Scenario::LocalHttp) {
                starts += 1;
                let mut state = ready;
                state["pid"] = json!(pid);
                state["starts"] = json!(starts);
                let pending = f.root.path().join("ready.new");
                fs::write(&pending, serde_json::to_vec(&state)?)?;
                fs::rename(pending, f.root.path().join("ready.json"))?;
                info["event"] = json!("ready");
                info["pid"] = json!(pid);
                info["starts"] = json!(starts);
            }
            println!("{info}");
            Ok(())
        };
        report(rx.recv_timeout(Duration::from_secs(15))??)?;
        loop {
            if let Some(ref controls) = controls
                && let Some(request) = controls.try_recv()?
            {
                controls::respond(request, &f, args.scenario, project, &records, &mut sequence);
            }
            if let Ok(line) = rx.try_recv() {
                report(line?)?;
            }
            if let Some(service) = supervisor.as_mut()
                && let Some(status) = service.0.try_wait()?
            {
                return Err(format!("fixture supervisor exited: {status}").into());
            }
            if let Some(status) = web.0.try_wait()? {
                return Err(format!("fixture server exited: {status}").into());
            }
            if signal.recv_timeout(Duration::from_millis(100)).is_ok() {
                return Ok(());
            }
        }
    })()
}
