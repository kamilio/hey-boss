mod admin_cli;
mod agent_permissions;
mod artifact_cli;
mod attachment_cli;
mod auto_workers_cli;
mod autoconnect;
mod broker;
mod companion;
mod health_cli;
mod issue_cli;
mod lookup_cli;
mod mindmap_cli;
mod notif_cli;
mod secret_cli;
mod upgrade_cli;
mod upgrade_provenance;
mod worker_cli;
use clap::{Args, Parser, Subcommand};
use hey_boss::{Client, Request, Severity, resolve_icon_file};
use std::os::fd::AsRawFd;

fn initialize(executable: &std::path::Path) -> std::io::Result<()> {
    agent_permissions::configure_pending(executable)?;
    let pending = executable.with_file_name("hey-boss.setup");
    if !pending.exists() {
        return Ok(());
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(executable.with_file_name("hey-boss.setup.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if pending.exists() {
        let configuration = std::fs::read_to_string(&pending)?;
        let arguments: Vec<_> = configuration.lines().collect();
        if arguments.len() != 5 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid setup configuration",
            ));
        }
        if !std::process::Command::new(arguments[0])
            .args(&arguments[1..])
            .status()?
            .success()
        {
            return Err(std::io::Error::other("Setup failed"));
        }
        std::fs::remove_file(pending)?;
    }
    Ok(())
}

#[derive(Args)]
#[command(
    name = "hey-boss",
    version = concat!(env!("CARGO_PKG_VERSION"), " (build ", env!("HEY_BOSS_BUILD_ID"), ")"),
    about = "Project issues, native Mac updates, notifications, and questions",
    after_help = "Use hey-boss <command> --help for options. Notification commands live under hey-boss notif.

Examples:
  hey-boss issue list
  hey-boss notif alert --title Build 'Checks passed'
  hey-boss notif inbox
  hey-boss skill install"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// Build the compatibility forms from the same enum, but keep root help focused.
impl clap::CommandFactory for Cli {
    fn command() -> clap::Command {
        Self::augment_args(clap::Command::new("hey-boss")).mut_subcommands(|command| {
            let legacy = notif_cli::Action::has_subcommand(command.get_name());
            if legacy { command.hide(true) } else { command }
        })
    }
    fn command_for_update() -> clap::Command {
        Self::command()
    }
}
impl Parser for Cli {}

#[derive(Args)]
struct Output {
    #[arg(long, conflicts_with = "markdown", hide = true)]
    json: bool,
    #[arg(long, help = "Print human-readable text (the default output format)")]
    markdown: bool,
}

#[derive(Args)]
struct Metadata {
    /// Related issue number; only records a relationship.
    #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
    issue: Option<i64>,
    /// Full issue project ID (defaults to this Git repository/directory).
    #[arg(long, requires = "issue")]
    issue_project: Option<String>,
    /// SSH host owning the related issue.
    #[arg(long, requires = "issue")]
    issue_host: Option<String>,
    #[arg(
        long,
        help = "Full project ID or unambiguous name; defaults to this Git repository/directory"
    )]
    project: Option<String>,
    #[arg(
        long,
        help = "Short title describing this update or question; required"
    )]
    title: String,
    #[arg(
        long,
        value_enum,
        help = "Subtle status badge (default: neutral); warning needs attention, error means failure"
    )]
    severity: Option<Severity>,
    #[arg(
        long,
        conflicts_with = "icon_file",
        help = "SF Symbol name or alias: info, success, warning, error, build, code, test, review, deploy, docs, folder, bell, question"
    )]
    icon: Option<String>,
    #[arg(long, value_name = "PATH", conflicts_with = "icon", value_parser = parse_icon_file, help = "Local PNG, JPEG, TIFF, ICNS, or PDF icon up to 4 MiB; retains its colors")]
    icon_file: Option<std::path::PathBuf>,
}

fn read_markdown_file(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;
    const LIMIT: u64 = 2 * 1024 * 1024;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(
            "Markdown path must be a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(std::io::Error::other(
            "Rendered Markdown source exceeds 2 MiB",
        ));
    }
    String::from_utf8(bytes).map_err(|_| std::io::Error::other("Markdown file must be UTF-8"))
}

fn parse_icon_file(value: &str) -> Result<std::path::PathBuf, String> {
    resolve_icon_file(value)
}

#[derive(Subcommand)]
enum Command {
    /// Notifications, questions, secret requests, and Inbox.
    #[command(
        subcommand,
        after_help = "Use --title when creating a notification. Keep summaries short and put details in Markdown.\nQuestions: --sync waits; --async returns a Task ID. Use notif wait for the answer.\nProject context is inferred from Git or the directory; use --project to override it.\nExisting root notification commands remain accepted as compatibility aliases."
    )]
    Notif(notif_cli::Action),
    #[command(flatten)]
    LegacyNotif(notif_cli::Action),
    /// Review command help and isolated text/JSON output previews.
    Admin(admin_cli::Options),
    /// Show or install the canonical agent skill.
    Skill(admin_cli::SkillOptions),
    /// Resolve a web URL and read its issue, document, topic, notice or conversation.
    Lookup(lookup_cli::Options),
    /// Fleet supervisor, machine companions, durable replicas, and worker controls.
    Fleet {
        #[command(subcommand)]
        action: hey_boss::fleet::Action,
    },
    /// Project issues, Markdown comments, and atomic agent claims in SQLite.
    #[command(visible_alias = "issues")]
    Issue(issue_cli::Options),
    /// Persistent project Markdown artifacts, comments and resource links.
    #[command(visible_alias = "artifacts")]
    Artifact(artifact_cli::Options),
    /// Disk-backed files for issues, mindmap nodes and artifacts; SSH-aware downloads.
    #[command(visible_alias = "attachments")]
    Attachment(attachment_cli::Options),
    /// Project mindmaps with nested topics, live resources and cross-project links.
    #[command(visible_alias = "mindmap")]
    Mm(mindmap_cli::Options),
    /// Global profile settings shared across all projects.
    Settings(issue_cli::GlobalOptions),
    /// Run an independent Codex issue worker with its own slots and tag filter.
    Worker(worker_cli::Options),
    /// Run saved workers for this machine, with project tabs in one dashboard.
    AutoWorkers(auto_workers_cli::Options),
    /// Machine health, orphan process harvesting, and safe worktree cleanup.
    Health {
        /// Run on a configured SSH client (macOS or Linux).
        #[arg(long, global = true)]
        host: Option<String>,
        #[command(subcommand)]
        action: health_cli::Action,
    },
    /// Allow hey-boss globally in Codex and Claude Code, preserving existing settings.
    ConfigureAgents {
        /// Installed executable path to allow (repeatable; defaults to this executable).
        #[arg(long, value_name = "PATH")]
        binary: Vec<std::path::PathBuf>,
    },
    /// Control a loaded Codex thread on its configured owning server (JSON stdin/output).
    AgentControl {
        #[arg(long)]
        thread: String,
        #[arg(value_parser = ["inspect", "enable-goal", "disable-goal", "steer"])]
        action: String,
    },
    /// Invoke a versioned two-way desktop action; returns a JSON result.
    Action {
        method: String,
        #[arg(long, default_value = "{}")]
        params: String,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Open a website on the connected Mac or send HTTP requests to its session.
    Browser {
        #[command(subcommand)]
        action: BrowserAction,
    },
    #[command(hide = true)]
    RenderMarkdown {
        input: std::path::PathBuf,
        output: std::path::PathBuf,
        #[arg(long)]
        source_map: bool,
        #[arg(long, conflicts_with = "source_map")]
        native: bool,
    },
    /// List running Codex/Claude processes and matched session activity.
    Agents {
        #[arg(long)]
        json: bool,
    },
    /// Open the native agent overview window.
    Overview {
        /// Print the running overview's local/server snapshots and current view as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Install or connect the remote server companion over SSH.
    Companion {
        #[command(subcommand)]
        action: companion::Action,
    },
    /// Upgrade this installation and every registered companion from one source build.
    Upgrade(upgrade_cli::Options),
}

impl Cli {
    fn into_request(self) -> std::io::Result<(Request, Output)> {
        self.into_request_with_project(|override_id| {
            let cwd = std::env::current_dir()?;
            let machine = hey_boss::issues::identity::machine().map_err(std::io::Error::other)?;
            let detected = hey_boss::issues::identity::project(&cwd, &machine)
                .map_err(std::io::Error::other)?;
            let worker_project = std::env::var("HEY_BOSS_ISSUE_PROJECT")
                .ok()
                .filter(|value| !value.is_empty());
            let path = hey_boss::issues::database_path().map_err(std::io::Error::other)?;
            hey_boss::issues::Store::open(&path)
                .and_then(|mut store| {
                    store.notification_project(&detected, override_id.or(worker_project.as_deref()))
                })
                .map_err(std::io::Error::other)
        })
    }

    fn into_request_with_project(
        self,
        resolve_project: impl FnOnce(Option<&str>) -> std::io::Result<hey_boss::issues::Project>,
    ) -> std::io::Result<(Request, Output)> {
        self.into_notif().into_request_with_project(resolve_project)
    }

    fn into_notif(self) -> notif_cli::Action {
        match self.command {
            Command::Notif(action) | Command::LegacyNotif(action) => action,
            _ => unreachable!("not a notification command"),
        }
    }

    fn canonicalize(self) -> Self {
        Self {
            command: match self.command {
                Command::LegacyNotif(action) => Command::Notif(action),
                command => command,
            },
        }
    }
}

#[derive(Subcommand)]
enum BrowserAction {
    Open {
        url: String,
    },
    Request {
        session: String,
        #[arg(long, default_value = "/")]
        path: String,
        #[arg(long, default_value = "GET")]
        method: String,
        #[arg(long)]
        body: Option<String>,
        #[arg(long, default_value = "{}")]
        headers: String,
    },
    Close {
        session: String,
    },
}
fn desktop_action(
    method: &str,
    params: serde_json::Value,
    id: Option<&str>,
) -> std::io::Result<()> {
    let executable = std::env::current_exe()?.canonicalize()?;
    initialize(&executable)?;
    let state = std::fs::read_to_string(executable.with_file_name("hey-boss.state"))?;
    let request: Request = serde_json::from_value(
        serde_json::json!({"command":"action", "sync":false,
        "question":serde_json::json!({"version":1,"id":id.map(str::to_owned).unwrap_or_else(||format!("action-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos())),"method":method,"params":params}).to_string()}),
    )?;
    let response =
        Client::new(std::path::Path::new(&state).join("daemon.sock")).try_send(&request)?;
    println!("{}", response.result.as_deref().unwrap_or("{}"));
    if response.status.as_deref() != Some("ok") {
        let detail = response
            .result
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "desktop action failed".into());
        return Err(std::io::Error::other(detail));
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("hey-boss: {error}");
        std::process::exit(1);
    }
}

fn run() -> std::io::Result<()> {
    let cli = Cli::parse().canonicalize();
    // Review captures use private SQLite files without starting a database service.
    match &cli.command {
        Command::Admin(options) => return admin_cli::run(options),
        Command::Skill(options) => return admin_cli::skill(options),
        _ => {}
    }
    hey_boss::database::use_service();
    match &cli.command {
        Command::Lookup(options) => {
            if let Err(error) = lookup_cli::run(options) {
                if options.json {
                    println!("{}", serde_json::json!({"ok":false,"error":error}));
                } else {
                    eprintln!("hey-boss lookup: {error}");
                }
                std::process::exit(error.exit_code());
            }
            return Ok(());
        }
        Command::Upgrade(options) => return upgrade_cli::run(options),
        Command::Fleet { action } => return hey_boss::fleet::run(action),
        Command::Notif(notif_cli::Action::Secret(options)) => return secret_cli::run(options),
        Command::AutoWorkers(options) => {
            if let Err(error) = auto_workers_cli::run(options) {
                eprintln!("hey-boss auto-workers: {error}");
                std::process::exit(error.exit_code());
            }
            return Ok(());
        }
        Command::Worker(options) => {
            if let Err(error) = worker_cli::run(options) {
                eprintln!("hey-boss worker: {error}");
                std::process::exit(error.exit_code());
            }
            return Ok(());
        }
        Command::Settings(options) => {
            if let Err(error) = issue_cli::run_global(options) {
                if options.json {
                    println!("{}", serde_json::json!({"ok":false,"error":error}));
                } else {
                    eprintln!("hey-boss settings: {error}");
                }
                std::process::exit(error.exit_code());
            }
            return Ok(());
        }
        Command::Mm(options) => {
            if let Err(error) = mindmap_cli::run(options) {
                if options.json {
                    println!("{}", serde_json::json!({"ok":false,"error":error}));
                } else {
                    eprintln!("hey-boss mm: {error}");
                }
                std::process::exit(error.exit_code());
            }
            return Ok(());
        }
        Command::Attachment(options) => {
            if let Err(error) = attachment_cli::run(options) {
                if options.json {
                    println!("{}", serde_json::json!({"ok":false,"error":error}));
                } else {
                    eprintln!("hey-boss attachment: {error}");
                }
                std::process::exit(error.exit_code());
            }
            return Ok(());
        }
        Command::Artifact(options) => {
            if let Err(error) = artifact_cli::run(options) {
                if options.json {
                    println!("{}", serde_json::json!({"ok":false,"error":error}));
                } else {
                    eprintln!("hey-boss artifact: {error}");
                }
                std::process::exit(error.exit_code());
            }
            return Ok(());
        }
        Command::Issue(options) => {
            if let Err(error) = issue_cli::run(options) {
                if options.json_output() {
                    println!("{}", serde_json::json!({"ok":false,"error":error}));
                } else {
                    eprintln!("hey-boss issue: {error}");
                }
                std::process::exit(error.exit_code());
            }
            return Ok(());
        }
        Command::Health { action, host } => {
            return match host {
                Some(host) => health_cli::run_remote(host, action),
                None => health_cli::run(action),
            };
        }
        Command::ConfigureAgents { binary } => {
            if binary.is_empty()
                && agent_permissions::configure_pending(&std::env::current_exe()?.canonicalize()?)?
            {
                return Ok(());
            }
            return agent_permissions::configure(binary);
        }
        Command::AgentControl { thread, action } => {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::io::stdin().take(32769).read_to_end(&mut bytes)?;
            let result = if bytes.len() > 32768 {
                Err(std::io::Error::other("Control input exceeds limit"))
            } else {
                serde_json::from_slice::<serde_json::Value>(&bytes)
                    .map_err(std::io::Error::from)
                    .and_then(|input| hey_boss::agent_control::run(thread, action, &input))
            };
            println!(
                "{}",
                match result {
                    Ok(value) => value,
                    Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}),
                }
            );
            return Ok(());
        }
        Command::Action {
            method,
            params,
            request_id,
        } => return desktop_action(method, serde_json::from_str(params)?, request_id.as_deref()),
        Command::Browser { action } => {
            return match action {
                BrowserAction::Open { url } => {
                    desktop_action("browser.open", serde_json::json!({"url":url}), None)
                }
                BrowserAction::Close { session } => desktop_action(
                    "browser.close",
                    serde_json::json!({"session":session}),
                    None,
                ),
                BrowserAction::Request {
                    session,
                    path,
                    method,
                    body,
                    headers,
                } => desktop_action(
                    "browser.request",
                    serde_json::json!({"session":session,"path":path,"method":method,"body":body,"headers":serde_json::from_str::<serde_json::Value>(headers)?}),
                    None,
                ),
            };
        }
        _ => {}
    }
    if let Command::RenderMarkdown {
        input,
        output,
        source_map,
        native,
    } = &cli.command
    {
        let markdown = read_markdown_file(input)?;
        return std::fs::write(
            output,
            if *native {
                hey_boss::markdown::render_native_document(&markdown)
            } else if *source_map {
                hey_boss::markdown::render_review_document(&markdown)
            } else {
                hey_boss::markdown::render_document(&markdown)
            },
        );
    }
    if let Command::Agents { json } = &cli.command {
        let snapshot = hey_boss::agents::scan();
        if *json {
            println!("{}", serde_json::to_string(&snapshot)?);
        } else {
            println!(
                "{} · {} sessions/processes",
                snapshot.host,
                snapshot.agents.len()
            );
            for agent in snapshot.agents {
                println!(
                    "{} · PID {} · {} · {}\n  {}",
                    agent.kind,
                    agent.pid,
                    agent.state,
                    agent.cwd.as_deref().unwrap_or("Unknown project"),
                    agent.task.as_deref().unwrap_or("Task unavailable")
                );
            }
        }
        return Ok(());
    }
    if let Command::Overview { json } | Command::Notif(notif_cli::Action::Inbox { json }) =
        &cli.command
    {
        let executable = std::env::current_exe()?.canonicalize()?;
        initialize(&executable)?;
        let state = std::fs::read_to_string(executable.with_file_name("hey-boss.state"))?;
        if !executable.with_file_name("hey-boss.companion").exists() {
            hey_boss::require_protocol(std::path::Path::new(&state))?;
        }
        let mut request: Request =
            serde_json::from_value(serde_json::json!({"command":if matches!(cli.command,Command::Notif(notif_cli::Action::Inbox { .. })) { if *json { "inbox_list" } else { "inbox" } } else if *json { "overview_snapshot" } else { "overview" },"sync":false}))
                .map_err(std::io::Error::other)?;
        request.origin = None;
        let result = Client::new(std::path::Path::new(&state).join("daemon.sock"))
            .try_send(&request)
            .unwrap_or_else(|error| {
                eprintln!("hey-boss: daemon unavailable or request failed: {error}");
                std::process::exit(1);
            });
        if *json {
            let content = result
                .result
                .as_deref()
                .ok_or_else(|| std::io::Error::other("Overview response has no snapshot"))?;
            let snapshot: serde_json::Value =
                serde_json::from_str(content).map_err(std::io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&snapshot).map_err(std::io::Error::other)?
            );
        } else {
            println!("{}", result.status.unwrap_or_else(|| "ok".into()));
        }
        return Ok(());
    }
    if let Command::Companion { action } = &cli.command {
        if let Err(error) = companion::run(action) {
            eprintln!("Companion: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let (request, output) = cli.into_request()?;
    let executable = std::env::current_exe()?.canonicalize()?;
    initialize(&executable)?;
    let config = executable.with_file_name("hey-boss.state");
    let state = std::fs::read_to_string(config)?;
    if executable.with_file_name("hey-boss.companion").exists() && request.icon_path.is_some() {
        eprintln!("Remote companion supports --icon; --icon-file requires a file on the Mac.");
        std::process::exit(1);
    }
    if executable.with_file_name("hey-boss.companion").exists()
        && !std::path::Path::new(&state).join("daemon.sock").exists()
    {
        eprintln!("hey-boss is disconnected; connect the companion from your Mac first.");
        std::process::exit(1);
    }
    let result = Client::new(std::path::Path::new(&state).join("daemon.sock"))
        .try_send(&request)
        .unwrap_or_else(|error| {
            eprintln!("hey-boss: daemon unavailable or request failed: {error}");
            std::process::exit(1);
        });
    print_response(result, output.json)
}

fn print_response(result: hey_boss::Response, json: bool) -> std::io::Result<()> {
    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("Task ID: {}", result.task_id);
        if let Some(status) = result.status {
            println!("Status: {status}");
        }
        if let Some(name) = result.document_name {
            println!("Document: {name}");
        }
        if let Some(review) = result.review_status {
            println!("Review: {review}");
        }
        if let Some(comments) = result.comments {
            for comment in comments {
                if let Some(selection) = comment.selection {
                    println!("\nLines: {}–{}", selection.line_start, selection.line_end);
                    println!("Source:\n{}", selection.source_text);
                }
                if let Some(quote) = comment.quote {
                    println!("\nOn: {quote}");
                }
                println!("Comment: {}", comment.text);
            }
        }
        if let Some(answer) = result.result {
            println!("Result: {answer}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Cli {
        fn into_test_request(self) -> std::io::Result<(Request, Output)> {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "hb-notification-project-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            let result = self.into_request_with_project(|override_id| {
                let cwd = std::env::current_dir()?;
                let machine =
                    hey_boss::issues::identity::machine().map_err(std::io::Error::other)?;
                let detected = hey_boss::issues::identity::project(&cwd, &machine)
                    .map_err(std::io::Error::other)?;
                hey_boss::issues::Store::open(&root.join("issues.db"))
                    .and_then(|mut store| store.notification_project(&detected, override_id))
                    .map_err(std::io::Error::other)
            });
            let _ = std::fs::remove_dir_all(root);
            result
        }
    }

    #[test]
    fn notification_group_matches_legacy_wire_requests() {
        for args in [
            vec!["alert", "Ready", "--title", "Review"],
            vec!["update", "Ready", "# Report", "--title", "Review"],
            vec!["ask", "Proceed?", "Details", "--async", "--title", "Review"],
            vec!["prompt", "Name?", "Details", "--title", "Review"],
            vec!["approval", "Proceed?", "Details", "--title", "Review"],
            vec!["status", "task-1", "--sync"],
            vec!["wait", "task-1"],
            vec!["hide", "task-1"],
        ] {
            let parse = |prefix: &[&str]| {
                Cli::try_parse_from(
                    prefix
                        .iter()
                        .copied()
                        .chain(args.iter().copied())
                        .chain(["--json"]),
                )
                .unwrap()
                .into_request_with_project(|_| {
                    Ok(hey_boss::issues::Project {
                        id: "named:Review".into(),
                        name: "Review".into(),
                    })
                })
                .unwrap()
            };
            let (legacy, legacy_output) = parse(&["hey-boss"]);
            let (grouped, grouped_output) = parse(&["hey-boss", "notif"]);
            assert_eq!(
                serde_json::to_value(legacy).unwrap(),
                serde_json::to_value(grouped).unwrap()
            );
            assert!(legacy_output.json && grouped_output.json);
        }
    }

    #[test]
    fn notification_help_groups_commands_and_hides_root_compatibility_names() {
        use clap::CommandFactory;
        let mut root = Cli::command();
        root.build();
        let notif = root.find_subcommand("notif").unwrap();
        for name in [
            "alert", "update", "ask", "prompt", "approval", "status", "wait", "hide", "inbox",
            "secret",
        ] {
            assert!(
                notif.find_subcommand(name).is_some(),
                "missing notif {name}"
            );
            assert!(
                root.find_subcommand(name).unwrap().is_hide_set(),
                "visible root {name}"
            );
        }
        assert!(notif.find_subcommand("issue").is_none());
        assert!(
            root.find_subcommand("render-markdown")
                .unwrap()
                .is_hide_set()
        );
        for command in ["inbox", "secret"] {
            assert!(
                Cli::try_parse_from(["hey-boss", "notif", command, "--help"])
                    .err()
                    .unwrap()
                    .kind()
                    == clap::error::ErrorKind::DisplayHelp
            );
        }
        assert!(Cli::try_parse_from(["hey-boss", "notif", "alert", "Missing title"]).is_err());
    }

    #[test]
    fn inbox_and_secret_aliases_use_the_same_dispatch() {
        for prefix in [vec!["hey-boss"], vec!["hey-boss", "notif"]] {
            assert!(matches!(
                Cli::try_parse_from(prefix.iter().copied().chain(["inbox", "--json"]))
                    .unwrap()
                    .canonicalize()
                    .command,
                Command::Notif(notif_cli::Action::Inbox { json: true })
            ));
            assert!(matches!(
                Cli::try_parse_from(prefix.iter().copied().chain([
                    "secret",
                    "--field",
                    "TOKEN",
                    "--env-file",
                    "credentials.env"
                ]))
                .unwrap()
                .canonicalize()
                .command,
                Command::Notif(notif_cli::Action::Secret(_))
            ));
        }
    }

    #[test]
    fn appearance_options_reach_every_creation_command() {
        for args in [
            vec!["alert", "Ready"],
            vec!["update", "Summary", "Content"],
            vec!["ask", "Proceed?", "Details", "--async"],
            vec!["prompt", "Name?", "Details"],
            vec!["approval", "Proceed?", "Details"],
        ] {
            for severity in ["neutral", "info", "success", "warning", "error"] {
                let mut input = vec!["hey-boss"];
                input.extend(args.iter().copied());
                input.extend([
                    "--project",
                    "Atlas",
                    "--title",
                    "Review",
                    "--severity",
                    severity,
                    "--icon",
                    "hammer.fill",
                ]);
                let (request, _) = Cli::try_parse_from(input)
                    .unwrap()
                    .into_test_request()
                    .unwrap();
                let wire = serde_json::to_value(request).unwrap();
                assert_eq!(wire["severity"], severity);
                assert_eq!(wire["icon"], "hammer.fill");
                assert!(wire.get("icon_path").is_none());
            }
        }
    }

    #[test]
    fn invalid_appearance_is_rejected_during_parsing() {
        let base = [
            "hey-boss",
            "alert",
            "Ready",
            "--project",
            "Atlas",
            "--title",
            "Review",
        ];
        for extra in [
            vec!["--severity", "urgent"],
            vec!["--icon-file", "/missing/icon.png"],
            vec!["--icon", "build", "--icon-file", "/missing/icon.png"],
        ] {
            let mut input = base.to_vec();
            input.extend(extra);
            assert!(Cli::try_parse_from(input).is_err());
        }
    }

    #[test]
    fn local_icons_are_canonical_readable_files() {
        let root = std::env::current_dir()
            .unwrap()
            .join("out")
            .join(format!("icons-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let base = [
            "hey-boss",
            "alert",
            "Ready",
            "--project",
            "Atlas",
            "--title",
            "Review",
            "--icon-file",
        ];
        for extension in ["png", "PNG", "jpg", "jpeg", "tif", "tiff", "icns", "pdf"] {
            let path = root.join(format!("icon.{extension}"));
            std::fs::write(&path, b"decoded by AppKit").unwrap();
            let relative = path.strip_prefix(std::env::current_dir().unwrap()).unwrap();
            let mut input = base.to_vec();
            input.push(relative.to_str().unwrap());
            let (request, _) = Cli::try_parse_from(input)
                .unwrap()
                .into_test_request()
                .unwrap();
            assert_eq!(request.icon_path, Some(path.canonicalize().unwrap()));
            assert!(request.icon.is_none());
        }
        let target = root.join("extensionless");
        let alias = root.join("alias.png");
        std::fs::write(&target, b"image").unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let resolved = parse_icon_file(alias.to_str().unwrap()).unwrap();
        assert_eq!(resolve_icon_file(&resolved).unwrap(), resolved);
        let boundary = root.join("boundary.png");
        let file = std::fs::File::create(&boundary).unwrap();
        file.set_len(4 * 1024 * 1024).unwrap();
        assert!(parse_icon_file(boundary.to_str().unwrap()).is_ok());
        file.set_len(4 * 1024 * 1024 + 1).unwrap();
        assert_eq!(
            parse_icon_file(boundary.to_str().unwrap()).unwrap_err(),
            "icon file must be at most 4 MiB"
        );
        let mut oversized_input = base.to_vec();
        oversized_input.push(boundary.to_str().unwrap());
        assert!(Cli::try_parse_from(oversized_input).is_err());
        let directory = root.join("directory.png");
        std::fs::create_dir(&directory).unwrap();
        assert!(parse_icon_file(directory.to_str().unwrap()).is_err());
        let unsupported = root.join("icon.svg");
        std::fs::write(&unsupported, "<svg/>").unwrap();
        assert!(parse_icon_file(unsupported.to_str().unwrap()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_first_commands_register_once() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("out")
            .join(format!("setup-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("setup.sh");
        let calls = root.join("calls");
        std::fs::write(&script, "printf 'registered\\n' >> \"$1\"\n").unwrap();
        std::fs::write(
            root.join("hey-boss.setup"),
            format!(
                "/bin/sh\n{}\n{}\nunused\nunused",
                script.display(),
                calls.display()
            ),
        )
        .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let executable = root.join("hey-boss");
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    initialize(&executable).unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        initialize(&root.join("hey-boss")).unwrap();
        assert_eq!(std::fs::read_to_string(calls).unwrap(), "registered\n");
        assert!(!root.join("hey-boss.setup").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn creation_commands_require_title() {
        for args in [
            vec!["alert", "Ready"],
            vec!["update", "Summary", "Content"],
            vec!["ask", "Proceed?", "Details", "--async"],
            vec!["prompt", "Name?", "Details"],
            vec!["approval", "Proceed?", "Details"],
        ] {
            for metadata in [vec![], vec!["--project", "Atlas"]] {
                let mut input = vec!["hey-boss"];
                input.extend(args.iter().copied());
                input.extend(metadata);
                assert!(Cli::try_parse_from(input).is_err(), "{args:?}");
            }
        }
    }

    #[test]
    fn creation_requests_infer_project_without_override() {
        let cwd = std::env::current_dir().unwrap();
        let machine = hey_boss::issues::identity::machine().unwrap();
        let expected = hey_boss::issues::identity::project(&cwd, &machine).unwrap();
        for args in [
            vec!["alert", "Ready"],
            vec!["update", "Summary", "Content"],
            vec!["ask", "Proceed?", "Details", "--async"],
            vec!["prompt", "Name?", "Details"],
            vec!["approval", "Proceed?", "Details"],
        ] {
            let mut input = vec!["hey-boss"];
            input.extend(args);
            input.extend(["--title", "Review"]);
            let (request, _) = Cli::try_parse_from(input)
                .unwrap()
                .into_test_request()
                .unwrap();
            assert_eq!(request.project.as_deref(), Some(expected.name.as_str()));
            assert_eq!(request.title.as_deref(), Some("Review"));
        }
    }

    #[test]
    fn creation_requests_include_project_and_title() {
        for args in [
            vec!["alert", "Ready"],
            vec!["update", "Summary", "Content"],
            vec!["ask", "Proceed?", "Details", "--sync"],
            vec!["prompt", "Name?", "Details"],
            vec!["approval", "Proceed?", "Details"],
        ] {
            let mut input = vec!["hey-boss"];
            input.extend(args);
            input.extend(["--project", "Atlas", "--title", "Review", "--json"]);
            let (request, output) = Cli::try_parse_from(input)
                .unwrap()
                .into_test_request()
                .unwrap();
            let wire = serde_json::to_value(request).unwrap();
            assert_eq!(wire["project"], "Atlas");
            assert_eq!(wire["title"], "Review");
            assert!(output.json);
        }
    }

    #[test]
    fn creation_metadata_cannot_be_blank() {
        for field in ["--project", "--title"] {
            for blank in ["", " \t\n"] {
                let project = if field == "--project" { blank } else { "Atlas" };
                let title = if field == "--title" { blank } else { "Review" };
                let cli = Cli::try_parse_from([
                    "hey-boss",
                    "alert",
                    "Ready",
                    "--project",
                    project,
                    "--title",
                    title,
                ])
                .unwrap();
                assert!(cli.into_test_request().is_err());
            }
        }
    }

    #[test]
    fn invalid_alert_values_return_errors() {
        for args in [
            vec!["--autoclose", "NaN"],
            vec!["--autoclose", "0"],
            vec!["--autoclose", "1e308"],
            vec!["--link-url", "javascript:alert(1)", "--link-label", "Open"],
            vec!["--link-url", "https://example.com", "--link-label", " "],
        ] {
            let mut input = vec![
                "hey-boss",
                "alert",
                "Ready",
                "--project",
                "Atlas",
                "--title",
                "Build",
            ];
            input.extend(args);
            assert!(
                Cli::try_parse_from(input)
                    .unwrap()
                    .into_test_request()
                    .is_err()
            );
        }
    }

    #[test]
    fn existing_task_commands_do_not_require_creation_metadata() {
        for command in ["status", "hide", "wait"] {
            let (request, _) = Cli::try_parse_from(["hey-boss", command, "task-1"])
                .unwrap()
                .into_test_request()
                .unwrap();
            assert_eq!(request.command, command);
            assert_eq!(request.task_id.as_deref(), Some("task-1"));
            assert!(request.project.is_none());
            assert!(request.title.is_none());
        }
    }

    #[test]
    fn updates_accept_markdown_content_with_either_output_format() {
        for output in ["--json", "--markdown"] {
            let cli = Cli::try_parse_from([
                "hey-boss",
                "update",
                "--project",
                "Atlas",
                "--title",
                "Report",
                "Report ready",
                "# Report\n\n**Done**",
                output,
            ])
            .unwrap();
            let notif_cli::Action::Update {
                metadata,
                summary,
                content,
                ..
            } = cli.into_notif()
            else {
                panic!()
            };
            assert_eq!(metadata.project.as_deref(), Some("Atlas"));
            assert_eq!(summary, "Report ready");
            assert_eq!(content.as_deref(), Some("# Report\n\n**Done**"));
        }
    }
    #[test]
    fn markdown_file_input_snapshots_unicode_and_rejects_invalid_inputs() {
        let directory =
            std::env::temp_dir().join(format!("hb-markdown-file-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("report.md");
        std::fs::write(&path, "# Report 🌍\n\n| A | B |\n|---|---|\n|one|two|").unwrap();
        let args = [
            "hey-boss",
            "update",
            "--project",
            "Atlas",
            "--title",
            "Migration",
            "Ready for review",
            "--file",
            path.to_str().unwrap(),
        ];
        let (request, _) = Cli::try_parse_from(args)
            .unwrap()
            .into_test_request()
            .unwrap();
        std::fs::write(&path, "changed later").unwrap();
        assert!(request.question.unwrap().contains("Report 🌍"));
        assert!(Cli::try_parse_from(args.into_iter().chain(["inline content"])).is_err());
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(
            Cli::try_parse_from(args)
                .unwrap()
                .into_test_request()
                .is_err()
        );
        std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(
            Cli::try_parse_from(args)
                .unwrap()
                .into_test_request()
                .is_err()
        );
        std::fs::remove_file(&path).unwrap();
        assert!(
            Cli::try_parse_from(args)
                .unwrap()
                .into_test_request()
                .is_err()
        );
        assert!(read_markdown_file(&directory).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn comments_enabled_reviews_support_sync_and_async_status() {
        let (review, _) = Cli::try_parse_from([
            "hey-boss",
            "update",
            "--project",
            "Atlas",
            "--title",
            "Review",
            "Ready",
            "# Report",
            "--comments",
            "--sync",
        ])
        .unwrap()
        .into_test_request()
        .unwrap();
        assert!(review.comments_enabled && review.sync);
        for (flag, sync) in [("--sync", true), ("--async", false)] {
            let (request, _) = Cli::try_parse_from(["hey-boss", "status", "task", flag])
                .unwrap()
                .into_test_request()
                .unwrap();
            assert_eq!(request.command, "status");
            assert_eq!(request.sync, sync);
        }
        assert!(
            Cli::try_parse_from([
                "hey-boss",
                "update",
                "--project",
                "Atlas",
                "--title",
                "Review",
                "Ready",
                "# Report",
                "--sync"
            ])
            .is_err()
        );
    }
    #[test]
    fn notice_issue_relationship_flags_are_independent_of_notification_behavior() {
        let (request, _) = Cli::try_parse_from([
            "hey-boss",
            "alert",
            "Build ready",
            "--project",
            "Atlas",
            "--title",
            "Build",
            "--issue",
            "7",
            "--issue-project",
            "github.com/example/repo",
            "--issue-host",
            "devbox",
        ])
        .unwrap()
        .into_test_request()
        .unwrap();
        let issue = request.issue.unwrap();
        assert_eq!(issue.number, 7);
        assert_eq!(issue.project, "github.com/example/repo");
        assert_eq!(issue.host.as_deref(), Some("devbox"));
        assert_eq!(request.command, "alert");
        assert_eq!(request.project.as_deref(), Some("Atlas"));
        assert!(request.link_url.is_none());
        assert!(!request.sync);
        assert!(
            Cli::try_parse_from([
                "hey-boss",
                "alert",
                "Ready",
                "--project",
                "Atlas",
                "--title",
                "Build",
                "--issue",
                "0"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "hey-boss",
                "alert",
                "Ready",
                "--project",
                "Atlas",
                "--title",
                "Build",
                "--issue-project",
                "other"
            ])
            .is_err()
        );
        let (inferred, _) = Cli::try_parse_from([
            "hey-boss",
            "update",
            "Ready",
            "# Markdown",
            "--project",
            "Atlas",
            "--title",
            "Build",
            "--issue",
            "1",
        ])
        .unwrap()
        .into_test_request()
        .unwrap();
        let cwd = std::env::current_dir().unwrap();
        let machine = hey_boss::issues::identity::machine().unwrap();
        assert_eq!(
            inferred.issue.unwrap().project,
            hey_boss::issues::identity::project(&cwd, &machine)
                .unwrap()
                .id
        );
    }
}
