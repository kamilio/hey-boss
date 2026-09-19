mod agent_permissions;
mod autoconnect;
mod broker;
mod companion;
mod health_cli;
mod issue_cli;
mod mindmap_cli;
mod secret_cli;
mod upgrade_cli;
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

#[derive(Parser)]
#[command(
    name = "hey-boss",
    version = concat!(env!("CARGO_PKG_VERSION"), " (build ", env!("HEY_BOSS_BUILD_ID"), ")"),
    about = "Project issues, native Mac updates, notifications, and questions",
    after_help = r#"Use --project and --title when creating an item. Keep summaries short;
put the details in Markdown. Ask only when requested or an answer is essential.

Questions: --sync waits; --async returns a Task ID. Use wait for the answer.
Save Task IDs and hide cards when they become obsolete.
Issues infer the project from Git or the directory. Use issue create --title TITLE --body MARKDOWN.

Examples:
  hey-boss issue list
  hey-boss issue claim 12
  hey-boss update --project Atlas --title Analysis 'Report ready' '# Findings'
  hey-boss alert --project Atlas --title Build 'Checks passed' --autoclose 10
  hey-boss ask --project Atlas --title Format 'Which format?' '' --option PDF --option Markdown --async
  hey-boss wait '<task_id>'
  hey-boss hide '<task_id>'

Three or more notifications form a collapsible project stack. The project × clears the stack. Read update/Open dismisses the card; history is kept.
Run hey-boss <command> --help for options and more examples."#
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

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
    #[arg(long, help = "Short project name shown in the heading; required")]
    project: String,
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
    /// Automatic controller, durable replicas, connected agents, and worker signals.
    Fleet {
        #[command(subcommand)]
        action: hey_boss::fleet::Action,
    },
    /// Request one or two secrets without history or agent-visible output.
    Secret(secret_cli::Options),
    /// Project issues, Markdown comments, and atomic agent claims in SQLite.
    #[command(visible_alias = "issues")]
    Issue(issue_cli::Options),
    /// Project mindmaps with nested topics, live resources and cross-project links.
    #[command(visible_alias = "mindmap")]
    Mm(mindmap_cli::Options),
    /// Global profile settings shared across all projects.
    Settings(issue_cli::GlobalOptions),
    /// Run an independent Codex issue worker with its own slots and tag filter.
    Worker(worker_cli::Options),
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
    },
    /// List running Codex/Claude processes and matched session activity.
    Agents {
        #[arg(long)]
        json: bool,
    },
    /// Open the web Inbox; --json lists notices without opening a browser.
    Inbox {
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
    #[command(
        about = "Post a short update with a Markdown preview",
        after_help = r#"Keep SUMMARY to one short sentence. MARKDOWN is inline text; use --file PATH to load a Markdown document instead.
Headings, emphasis, lists, code, and http/https/file links are supported.
Read update opens the document and dismisses the card. History is kept. Returns a Task ID immediately.

Example (zsh/bash):
  hey-boss update --project Atlas --title 'Analysis ready' 'Three builds reviewed' $'# Results\n\n**Report complete.**\n\n[Read more](https://example.com/report)'
  hey-boss hide '<task_id>'"#
    )]
    Update {
        #[command(flatten)]
        metadata: Metadata,
        #[arg(help = "Short sentence visible on the compact card")]
        summary: String,
        #[arg(
            value_name = "MARKDOWN",
            help = "Full Markdown text opened by Read update"
        )]
        #[arg(required_unless_present = "file", conflicts_with = "file")]
        content: Option<String>,
        #[arg(
            long,
            help = "Enable document comments; status includes feedback and wait completes when review finishes"
        )]
        comments: bool,
        #[arg(
            long,
            requires = "comments",
            help = "Wait until the enabled review is finished or cancelled"
        )]
        sync: bool,
        #[arg(
            long,
            alias = "markdown-file",
            value_name = "PATH",
            conflicts_with = "content",
            help = "Review Markdown, source/text (1 MiB), or PNG/JPEG/GIF/WebP images (4 MiB); snapshot file contents"
        )]
        file: Option<std::path::PathBuf>,
        #[command(flatten)]
        output: Output,
    },
    #[command(
        about = "Show a brief notification",
        after_help = r#"Returns a Task ID immediately. Stays visible until dismissed or hidden unless
--autoclose sets seconds on screen. Supply both --link-url and --link-label for a large
click target. The action button opens the link and dismisses the card; history is kept.

Examples:
  hey-boss alert --project Atlas --title Build 'Checks passed' --autoclose 10
  hey-boss alert --project Atlas --title Report '**Analysis published**' --link-url https://example.com/report --link-label 'Open report'"#
    )]
    Alert {
        #[command(flatten)]
        metadata: Metadata,
        message: String,
        #[arg(long, help = "Dismiss after this positive number of seconds on screen")]
        autoclose: Option<f64>,
        #[arg(
            long,
            requires = "link_label",
            help = "Destination for the action button: http, https, or file URL"
        )]
        link_url: Option<String>,
        #[arg(
            long,
            requires = "link_url",
            help = "Short action label, such as Open report"
        )]
        link_label: Option<String>,
        #[command(flatten)]
        output: Output,
    },
    #[command(
        about = "Ask for text or a choice",
        after_help = r#"Ask only when requested or an answer is essential to proceed. Without --option,
accepts free text. Repeat --option for choices. Pass '' for an empty description.
--sync waits for the answer. --async returns a Task ID immediately;
continue independent work, then use wait to receive the answer. status checks once.
Close all cancels unanswered questions: status is cancelled with no result.
Interrupting wait leaves the question available. hide does not dismiss questions.

Examples:
  hey-boss ask --project Atlas --title 'Report name' 'What should the report be called?' '' --sync
  hey-boss ask --project Atlas --title Format 'Which format?' 'Choose the output.' --option Markdown --option PDF --async
  hey-boss wait '<task_id>'"#
    )]
    Ask {
        #[command(flatten)]
        metadata: Metadata,
        question: String,
        description: String,
        #[arg(
            long = "option",
            help = "Choice button label; repeat for each choice (omit for free text)"
        )]
        options: Vec<String>,
        #[arg(
            long,
            help = "Wait for the answer before returning",
            required_unless_present = "asynchronous",
            conflicts_with = "asynchronous"
        )]
        sync: bool,
        #[arg(
            long = "async",
            required_unless_present = "sync",
            help = "Return a Task ID immediately; retrieve the answer with wait"
        )]
        asynchronous: bool,
        #[command(flatten)]
        output: Output,
    },
    #[command(
        about = "Ask for text asynchronously",
        after_help = "Equivalent to ask --async without options. Returns a Task ID, not an answer.\n\nExample:\n  hey-boss prompt --project Atlas --title Name 'Name the report?' ''\n  hey-boss wait '<task_id>'"
    )]
    Prompt {
        #[command(flatten)]
        metadata: Metadata,
        question: String,
        description: String,
        #[command(flatten)]
        output: Output,
    },
    #[command(
        about = "Ask for a choice asynchronously",
        after_help = "Returns a Task ID immediately. Use --option repeatedly to replace Yes/No choices.\n\nExample:\n  hey-boss approval --project Atlas --title Publish 'Publish the report?' 'The draft is ready.'\n  hey-boss wait '<task_id>'"
    )]
    Approval {
        #[command(flatten)]
        metadata: Metadata,
        question: String,
        description: String,
        #[arg(long = "option", default_values = ["Yes", "No"])]
        options: Vec<String>,
        #[command(flatten)]
        output: Output,
    },
    #[command(
        about = "Dismiss an alert or update",
        after_help = "Use when a notification becomes obsolete or is superseded. Does not hide questions\nor close an already open document preview.\n\nExample:\n  hey-boss hide '<task_id>'"
    )]
    Hide {
        task_id: String,
        #[command(flatten)]
        output: Output,
    },
    #[command(
        about = "Check a task",
        after_help = "pending = queued or displayed; ok = completed; cancelled = question dismissed by Close all.\nAn answered question includes result; cancellation has no result and is not approval.\nUse wait for a question answer instead of polling status.\n\nExample:\n  hey-boss status '<task_id>'"
    )]
    Status {
        task_id: String,
        #[arg(
            long,
            conflicts_with = "async",
            help = "Wait for available review comments or an answer"
        )]
        sync: bool,
        #[arg(
            long,
            help = "Return current state and available comments immediately (default)"
        )]
        r#async: bool,
        #[command(flatten)]
        output: Output,
    },
    #[command(
        about = "Wait for an answer",
        after_help = "Questions only; not alerts or updates. Returns immediately if answered or cancelled.\nOtherwise waits for an answer or Close all. Cancellation returns status cancelled\nwithout a result; it is not an answer or approval. Ctrl+C stops this caller and\nleaves the question available for a later wait.\n\nExample:\n  hey-boss wait '<task_id>'"
    )]
    Wait {
        task_id: String,
        #[command(flatten)]
        output: Output,
    },
}

impl Cli {
    fn into_request(self) -> std::io::Result<(Request, Output)> {
        let invalid =
            |message: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
        let mut issue = None;
        let (project, title, severity, icon, icon_path) = match &self.command {
            Command::Update { metadata, .. }
            | Command::Alert { metadata, .. }
            | Command::Ask { metadata, .. }
            | Command::Prompt { metadata, .. }
            | Command::Approval { metadata, .. } => {
                if metadata.project.trim().is_empty() || metadata.title.trim().is_empty() {
                    return Err(invalid("creation project and title must not be blank"));
                }
                if let Some(number) = metadata.issue {
                    let project = if let Some(project) = &metadata.issue_project {
                        project.clone()
                    } else {
                        let cwd = std::env::current_dir()?;
                        let machine =
                            hey_boss::issues::identity::machine().map_err(std::io::Error::other)?;
                        hey_boss::issues::identity::project(&cwd, &machine)
                            .map_err(std::io::Error::other)?
                            .id
                    };
                    let reference = hey_boss::notices::IssueReference {
                        project,
                        number,
                        host: metadata.issue_host.clone().or_else(|| {
                            std::env::var("HEY_BOSS_ISSUE_HOST")
                                .ok()
                                .filter(|s| !s.is_empty())
                        }),
                    };
                    reference.validate().map_err(std::io::Error::other)?;
                    issue = Some(reference);
                }
                (
                    Some(metadata.project.clone()),
                    Some(metadata.title.clone()),
                    metadata.severity,
                    metadata.icon.clone(),
                    metadata.icon_file.clone(),
                )
            }
            Command::Secret(_)
            | Command::Issue(_)
            | Command::Mm(_)
            | Command::Settings(_)
            | Command::Fleet { .. }
            | Command::Worker(_)
            | Command::Upgrade(_)
            | Command::Health { .. }
            | Command::ConfigureAgents { .. }
            | Command::AgentControl { .. }
            | Command::Action { .. }
            | Command::Browser { .. }
            | Command::RenderMarkdown { .. }
            | Command::Companion { .. }
            | Command::Agents { .. }
            | Command::Inbox { .. }
            | Command::Overview { .. } => {
                unreachable!()
            }
            Command::Hide { .. } | Command::Status { .. } | Command::Wait { .. } => {
                (None, None, None, None, None)
            }
        };
        let mut request = Request {
            issue,
            command: String::new(),
            question: None,
            project,
            title,
            description: None,
            options: None,
            autoclose: None,
            link_url: None,
            link_label: None,
            task_id: None,
            sync: false,
            comments_enabled: false,
            attachment: None,
            document_name: None,
            origin: None,
            severity,
            icon,
            icon_path,
        };
        let output = match self.command {
            Command::Secret(_)
            | Command::Issue(_)
            | Command::Mm(_)
            | Command::Settings(_)
            | Command::Fleet { .. }
            | Command::Worker(_)
            | Command::Upgrade(_)
            | Command::Health { .. }
            | Command::ConfigureAgents { .. }
            | Command::AgentControl { .. }
            | Command::Action { .. }
            | Command::Browser { .. }
            | Command::RenderMarkdown { .. }
            | Command::Companion { .. }
            | Command::Agents { .. }
            | Command::Inbox { .. }
            | Command::Overview { .. } => {
                unreachable!()
            }
            Command::Update {
                summary,
                content,
                file,
                comments,
                sync,
                output,
                ..
            } => {
                request.command = "update".into();
                request.comments_enabled = comments;
                request.sync = sync;
                request.description = Some(summary);
                request.question = Some(match file {
                    Some(path) => {
                        let (content, attachment) = hey_boss::document::read_file(&path)?;
                        request.attachment = attachment;
                        request.document_name = path.file_name().map(|name| {
                            name.to_string_lossy()
                                .chars()
                                .filter(|c| !c.is_control())
                                .take(256)
                                .collect()
                        });
                        content
                    }
                    None => {
                        content.ok_or_else(|| invalid("provide Markdown text or --file PATH"))?
                    }
                });
                output
            }
            Command::Alert {
                message,
                autoclose,
                link_url,
                link_label,
                output,
                ..
            } => {
                if autoclose.is_some_and(|seconds| {
                    !seconds.is_finite() || seconds <= 0.0 || seconds > 31_536_000.0
                }) {
                    return Err(invalid(
                        "autoclose must be positive and at most 31536000 seconds",
                    ));
                }
                if let Some(url) = &link_url {
                    if !["https://", "http://", "file://"]
                        .iter()
                        .any(|prefix| url.starts_with(prefix))
                    {
                        return Err(invalid("link URL must use https, http, or file"));
                    }
                    if link_label
                        .as_ref()
                        .is_none_or(|label| label.trim().is_empty())
                    {
                        return Err(invalid("link label must not be blank"));
                    }
                }
                request.command = "alert".into();
                request.question = Some(message);
                request.autoclose = autoclose;
                request.link_url = link_url;
                request.link_label = link_label;
                output
            }
            Command::Ask {
                question,
                description,
                options,
                sync,
                output,
                ..
            } => {
                request.command = "ask".into();
                request.question = Some(question);
                request.description = Some(description);
                request.options = Some(options);
                request.sync = sync;
                output
            }
            Command::Prompt {
                question,
                description,
                output,
                ..
            } => {
                request.command = "ask".into();
                request.question = Some(question);
                request.description = Some(description);
                request.options = Some(Vec::new());
                output
            }
            Command::Approval {
                question,
                description,
                options,
                output,
                ..
            } => {
                request.command = "ask".into();
                request.question = Some(question);
                request.description = Some(description);
                request.options = Some(options);
                output
            }
            Command::Hide { task_id, output } => {
                request.command = "hide".into();
                request.task_id = Some(task_id);
                output
            }
            Command::Status {
                task_id,
                output,
                sync,
                ..
            } => {
                request.command = "status".into();
                request.sync = sync;
                request.task_id = Some(task_id);
                output
            }
            Command::Wait { task_id, output } => {
                request.command = "wait".into();
                request.task_id = Some(task_id);
                output
            }
        };
        Ok((request, output))
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
    let cli = Cli::parse();
    match &cli.command {
        Command::Upgrade(options) => return upgrade_cli::run(options),
        Command::Fleet { action } => return hey_boss::fleet::run(action),
        Command::Secret(options) => return secret_cli::run(options),
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
        Command::Health {
            action: health_cli::Action::Open,
            host: None,
        } => {
            let executable = std::env::current_exe()?.canonicalize()?;
            initialize(&executable)?;
            let state = std::fs::read_to_string(executable.with_file_name("hey-boss.state"))?;
            let request: Request =
                serde_json::from_value(serde_json::json!({"command":"health", "sync":false}))?;
            let reply = Client::new(std::path::Path::new(state.trim()).join("daemon.sock"))
                .try_send(&request)?;
            if reply.status.as_deref() != Some("ok") {
                return Err(std::io::Error::other(
                    "Install the updated daemon to open Machine Health",
                ));
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
    } = &cli.command
    {
        let markdown = read_markdown_file(input)?;
        return std::fs::write(
            output,
            if *source_map {
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
    if let Command::Overview { json } | Command::Inbox { json } = &cli.command {
        let executable = std::env::current_exe()?.canonicalize()?;
        initialize(&executable)?;
        let state = std::fs::read_to_string(executable.with_file_name("hey-boss.state"))?;
        if !executable.with_file_name("hey-boss.companion").exists() {
            hey_boss::require_protocol(std::path::Path::new(&state))?;
        }
        let mut request: Request =
            serde_json::from_value(serde_json::json!({"command":if matches!(cli.command,Command::Inbox { .. }) { if *json { "inbox_list" } else { "inbox" } } else if *json { "overview_snapshot" } else { "overview" },"sync":false}))
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
    if output.json {
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
                let (request, _) = Cli::try_parse_from(input).unwrap().into_request().unwrap();
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
            let (request, _) = Cli::try_parse_from(input).unwrap().into_request().unwrap();
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
    fn creation_commands_require_project_and_title() {
        for args in [
            vec!["alert", "Ready"],
            vec!["update", "Summary", "Content"],
            vec!["ask", "Proceed?", "Details", "--async"],
            vec!["prompt", "Name?", "Details"],
            vec!["approval", "Proceed?", "Details"],
        ] {
            for metadata in [
                vec![],
                vec!["--project", "Atlas"],
                vec!["--title", "Review"],
            ] {
                let mut input = vec!["hey-boss"];
                input.extend(args.iter().copied());
                input.extend(metadata);
                assert!(Cli::try_parse_from(input).is_err(), "{args:?}");
            }
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
            let (request, output) = Cli::try_parse_from(input).unwrap().into_request().unwrap();
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
                assert!(cli.into_request().is_err());
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
            assert!(Cli::try_parse_from(input).unwrap().into_request().is_err());
        }
    }

    #[test]
    fn existing_task_commands_do_not_require_creation_metadata() {
        for command in ["status", "hide", "wait"] {
            let (request, _) = Cli::try_parse_from(["hey-boss", command, "task-1"])
                .unwrap()
                .into_request()
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
            let Command::Update {
                metadata,
                summary,
                content,
                ..
            } = cli.command
            else {
                panic!()
            };
            assert_eq!(metadata.project, "Atlas");
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
        let (request, _) = Cli::try_parse_from(args).unwrap().into_request().unwrap();
        std::fs::write(&path, "changed later").unwrap();
        assert!(request.question.unwrap().contains("Report 🌍"));
        assert!(Cli::try_parse_from(args.into_iter().chain(["inline content"])).is_err());
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(Cli::try_parse_from(args).unwrap().into_request().is_err());
        std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(Cli::try_parse_from(args).unwrap().into_request().is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(Cli::try_parse_from(args).unwrap().into_request().is_err());
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
        .into_request()
        .unwrap();
        assert!(review.comments_enabled && review.sync);
        for (flag, sync) in [("--sync", true), ("--async", false)] {
            let (request, _) = Cli::try_parse_from(["hey-boss", "status", "task", flag])
                .unwrap()
                .into_request()
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
        .into_request()
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
        .into_request()
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
