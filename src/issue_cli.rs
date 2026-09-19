use clap::{Args, Subcommand};
use hey_boss::issues::{self, Error, Operation, Request, Result, Store};
use serde_json::{Value, json};
use std::io::Read;
use std::path::PathBuf;

#[derive(Args)]
#[command(
    after_help = "Project defaults to the Git repository (shared by worktrees), or current directory.\nMarkdown bodies and comments are stored in SQLite. Use --body - for stdin.\nUse --agent ID or HEY_BOSS_AGENT_ID if your session cannot be detected.\nRun `hey-boss issue <command> --help` for details."
)]
pub struct Options {
    /// Full project ID or an unambiguous short name; defaults to this checkout.
    #[arg(long, global = true)]
    project: Option<String>,
    /// Stable session identity (also HEY_BOSS_AGENT_ID).
    #[arg(long, global = true)]
    agent: Option<String>,
    /// Authoritative SSH host (also HEY_BOSS_ISSUE_HOST); never falls back locally.
    #[arg(long, global = true)]
    host: Option<String>,
    /// Print structured results and operational errors.
    #[arg(long, global = true)]
    pub json: bool,
    /// Deduplicate a retried mutation; reuse only for the identical operation.
    #[arg(long, global = true)]
    request_id: Option<String>,
    #[command(subcommand)]
    action: Action,
}

#[derive(Args)]
pub struct GlobalOptions {
    #[arg(long, global = true)]
    pub json: bool,
    /// Authoritative issue store; omit for this machine.
    #[arg(long, global = true)]
    host: Option<String>,
    #[arg(long, global = true)]
    request_id: Option<String>,
    #[command(subcommand)]
    action: GlobalSettingsAction,
}
#[derive(Subcommand)]
enum GlobalSettingsAction {
    Show,
    Set {
        #[arg(long)]
        boss_name: String,
        #[arg(long)]
        if_version: Option<i64>,
    },
}
pub fn run_global(options: &GlobalOptions) -> Result<()> {
    let operation = match &options.action {
        GlobalSettingsAction::Show => Operation::GlobalSettings,
        GlobalSettingsAction::Set {
            boss_name,
            if_version,
        } => Operation::ConfigureGlobal {
            boss_name: boss_name.clone(),
            if_version: *if_version,
        },
    };
    run(&Options {
        project: None,
        agent: Some("human:boss".into()),
        host: options.host.clone(),
        json: options.json,
        request_id: options.request_id.clone(),
        action: Action::GlobalSettings { operation },
    })
}

#[derive(Args)]
struct Body {
    /// Markdown text; '-' reads UTF-8 from stdin (up to 1 MiB).
    #[arg(long, conflicts_with = "file")]
    body: Option<String>,
    /// Copy a UTF-8 Markdown file into SQLite; '-' reads stdin.
    #[arg(long, visible_alias = "markdown-file", conflicts_with = "body")]
    file: Option<PathBuf>,
}
impl Body {
    fn read(&self) -> Result<Option<String>> {
        let value = if self.body.as_deref() == Some("-")
            || self.file.as_deref() == Some(std::path::Path::new("-"))
        {
            Some(read_text(std::io::stdin(), issues::BODY_LIMIT)?)
        } else if let Some(path) = &self.file {
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)?;
            if !file.metadata()?.is_file() {
                return Err(Error::invalid("Markdown path must be a regular file"));
            }
            Some(read_text(file, issues::BODY_LIMIT)?)
        } else {
            self.body.clone()
        };
        if value.as_ref().is_some_and(|s| s.len() > issues::BODY_LIMIT) {
            return Err(Error::invalid("Markdown exceeds 1 MiB"));
        }
        Ok(value)
    }
}

#[derive(Subcommand)]
enum Action {
    /// Import GitHub issues and delete the originals only after verifying the copy.
    DrainGithub {
        /// Source GitHub repository; defaults to this checkout's GitHub repository.
        #[arg(long)]
        repo: Option<String>,
        /// GitHub login of the creator; defaults to the authenticated gh user.
        #[arg(long, conflicts_with = "all_authors")]
        author: Option<String>,
        /// Include issues created by anyone instead of the default author filter.
        #[arg(long)]
        all_authors: bool,
        #[arg(long, default_value = "open", value_parser = ["open", "closed", "all"])]
        state: String,
        /// Preview matching issues without creating or deleting anything.
        #[arg(long)]
        dry_run: bool,
    },
    #[command(skip)]
    GlobalSettings { operation: Operation },
    /// Project instructions and whether agents should create and attach PRs.
    Settings {
        #[command(subcommand)]
        command: SettingsAction,
    },
    /// Attach, list, or remove first-class pull request links.
    Pr {
        #[command(subcommand)]
        command: PrAction,
    },
    /// Create, link, list, or unlink full issues as subtasks.
    #[command(visible_alias = "subtasks")]
    Subtask {
        #[command(subcommand)]
        command: SubtaskAction,
    },
    /// Serve the fast issue web app on localhost. Ctrl+C stops the server.
    Web {
        /// Local HTTP port; use 0 to pick an available port.
        #[arg(long, default_value_t = 4781)]
        port: u16,
        /// Disable background discovery of projects used by running local agents.
        #[arg(long)]
        no_discovery: bool,
    },
    /// List projects by recent activity, with open/closed issue counts.
    Projects {
        /// Include hidden projects.
        #[arg(long)]
        all: bool,
    },
    /// Configure and control automatic Codex issue workers.
    Worker {
        #[command(subcommand)]
        command: WorkerAction,
    },
    /// Hide this project from the switcher; keep all issues and history.
    HideProject,
    /// Show a hidden project again.
    RestoreProject,
    /// List open issues in the current project.
    List {
        #[arg(long, default_value = "open", value_parser = ["open", "closed", "all", "deleted"])]
        state: String,
        #[arg(long, conflicts_with = "unassigned")]
        mine: bool,
        #[arg(long)]
        unassigned: bool,
        /// Filter by an exact session ID, or boss.
        #[arg(long, conflicts_with_all = ["mine", "unassigned"])]
        assignee: Option<String>,
        /// Require each supplied label (repeatable).
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Literal substring in the title or body.
        #[arg(long)]
        search: Option<String>,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// Move an issue before/after another issue; omit both to move to the end.
    Move {
        number: i64,
        #[arg(long, conflicts_with = "after")]
        before: Option<i64>,
        #[arg(long)]
        after: Option<i64>,
        #[arg(long)]
        if_order_version: Option<i64>,
    },
    /// Show the complete Markdown body and the latest 20 comments.
    View { number: i64 },
    /// Show the audit trail, including every comment and previous body revisions.
    History {
        number: i64,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// Create an open, unassigned issue. The body defaults to empty.
    Create {
        #[arg(long)]
        title: String,
        #[command(flatten)]
        body: Body,
        #[arg(long = "label")]
        labels: Vec<String>,
    },
    /// Replace supplied fields; omitted fields are preserved.
    Edit {
        number: i64,
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        body: Body,
        #[arg(long = "label")]
        add_labels: Vec<String>,
        #[arg(long = "remove-label")]
        remove_labels: Vec<String>,
        /// Reject the edit if another writer has changed this version.
        #[arg(long)]
        if_version: Option<i64>,
    },
    /// Show the calling session identity and resolved project.
    Whoami,
    /// Atomically assign an open issue to this session.
    #[command(visible_alias = "assign-to-myself")]
    Claim {
        number: i64,
        /// Explicitly take over another session's claim.
        #[arg(long)]
        force: bool,
    },
    /// Assign an open issue to Boss.
    #[command(visible_alias = "assign-boss")]
    AssignToBoss {
        number: i64,
        #[arg(long)]
        force: bool,
    },
    /// Remove your assignment, leaving the issue open.
    Unassign {
        number: i64,
        /// Explicitly remove another session's claim.
        #[arg(long)]
        force: bool,
    },
    /// Append a Markdown comment.
    Comment {
        number: i64,
        #[command(flatten)]
        body: Body,
    },
    /// Complete an issue and clear its claim, preserving attribution.
    Close {
        number: i64,
        /// Save a final Markdown comment in the same transaction.
        #[arg(long)]
        comment: Option<String>,
        /// Close even when another session owns the claim.
        #[arg(long)]
        force: bool,
    },
    /// Reopen a closed issue without assigning it.
    Reopen { number: i64 },
    /// Soft-delete an issue; its number and history are retained.
    Delete {
        number: i64,
        /// Delete even when another session owns the claim.
        #[arg(long)]
        force: bool,
    },
    /// Recover a deleted issue without restoring its former claim.
    Restore { number: i64 },
    #[command(hide = true)]
    Rpc,
}

#[derive(Subcommand)]
enum SubtaskAction {
    /// List immediate subtasks in the project queue order.
    List {
        number: i64,
        /// Include deleted subtasks so they can be unlinked.
        #[arg(long)]
        all: bool,
    },
    /// Create and link a child issue atomically.
    Create {
        number: i64,
        #[arg(long)]
        title: String,
        #[command(flatten)]
        body: Body,
        #[arg(long = "label")]
        labels: Vec<String>,
        #[arg(long)]
        if_version: Option<i64>,
    },
    /// Link an existing issue; unlink its previous parent first.
    Add {
        number: i64,
        child: i64,
        #[arg(long)]
        if_version: Option<i64>,
        #[arg(long)]
        if_child_version: Option<i64>,
    },
    /// Unlink a subtask while preserving its issue and history.
    Remove {
        number: i64,
        child: i64,
        #[arg(long)]
        if_version: Option<i64>,
        #[arg(long)]
        if_child_version: Option<i64>,
    },
}

#[derive(Subcommand)]
enum PrAction {
    Add { number: i64, url: String },
    Remove { number: i64, url: String },
    List { number: i64 },
}
#[derive(Subcommand)]
enum SettingsAction {
    Show,
    Set {
        /// Legacy spelling; global profile settings own this value.
        #[arg(long, hide = true)]
        boss_name: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long, conflicts_with = "no_prs")]
        prs_enabled: bool,
        #[arg(long)]
        no_prs: bool,
    },
}
#[derive(Subcommand)]
enum WorkerAction {
    /// List independent workers. Prefer `hey-boss worker status`.
    Status,
    /// Monitor enabled managed workers without the web interface.
    Run,
}

fn read_text(reader: impl Read, limit: usize) -> Result<String> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::invalid(format!("Input exceeds {limit} bytes")));
    }
    String::from_utf8(bytes).map_err(|_| Error::invalid("Input must be UTF-8"))
}

impl Options {
    pub fn json_output(&self) -> bool {
        self.json || matches!(self.action, Action::Rpc)
    }
    fn operation(&self) -> Result<Operation> {
        Ok(match &self.action {
            Action::Subtask { command } => match command {
                SubtaskAction::List { number, all } => Operation::Subtasks {
                    number: *number,
                    include_deleted: *all,
                },
                SubtaskAction::Create {
                    number,
                    title,
                    body,
                    labels,
                    if_version,
                } => Operation::CreateSubtask {
                    number: *number,
                    title: title.clone(),
                    body: body.read()?.unwrap_or_default(),
                    labels: labels.clone(),
                    at_top: false,
                    if_version: *if_version,
                },
                SubtaskAction::Add {
                    number,
                    child,
                    if_version,
                    if_child_version,
                } => Operation::AddSubtask {
                    number: *number,
                    child: *child,
                    if_version: *if_version,
                    if_child_version: *if_child_version,
                },
                SubtaskAction::Remove {
                    number,
                    child,
                    if_version,
                    if_child_version,
                } => Operation::RemoveSubtask {
                    number: *number,
                    child: *child,
                    if_version: *if_version,
                    if_child_version: *if_child_version,
                },
            },
            Action::Pr { command } => match command {
                PrAction::Add { number, url } => Operation::AddPullRequest {
                    number: *number,
                    url: url.clone(),
                },
                PrAction::Remove { number, url } => Operation::RemovePullRequest {
                    number: *number,
                    url: url.clone(),
                },
                PrAction::List { number } => Operation::PullRequests { number: *number },
            },
            Action::GlobalSettings { operation } => operation.clone(),
            Action::Settings { command } => match command {
                SettingsAction::Show => Operation::ProjectSettings,
                SettingsAction::Set {
                    boss_name,
                    prompt,
                    prs_enabled,
                    no_prs,
                } => {
                    if boss_name.is_none() && prompt.is_none() && !prs_enabled && !no_prs {
                        return Err(Error::invalid(
                            "Specify --boss-name, --prompt, --prs-enabled or --no-prs",
                        ));
                    }
                    Operation::ConfigureProject {
                        boss_name: boss_name.clone(),
                        prompt: prompt.clone(),
                        prs_enabled: if *prs_enabled {
                            Some(true)
                        } else if *no_prs {
                            Some(false)
                        } else {
                            None
                        },
                        if_version: None,
                    }
                }
            },
            Action::Worker { command } => match command {
                WorkerAction::Status => Operation::Workers { worker_id: None },
                WorkerAction::Run => unreachable!(),
            },
            Action::Projects { all } => Operation::Projects {
                include_hidden: *all,
            },
            Action::HideProject => Operation::HideProject,
            Action::RestoreProject => Operation::RestoreProject,
            Action::Whoami => Operation::Whoami,
            Action::List {
                state,
                mine,
                unassigned,
                assignee,
                labels,
                search,
                limit,
                offset,
            } => Operation::List {
                state: state.clone(),
                mine: *mine,
                unassigned: *unassigned,
                assignee: assignee.as_ref().map(|id| {
                    if id == "boss" {
                        "human:boss".into()
                    } else {
                        id.clone()
                    }
                }),
                labels: labels.clone(),
                search: search.clone(),
                limit: *limit,
                offset: *offset,
                all: false,
            },
            Action::Move {
                number,
                before,
                after,
                if_order_version,
            } => Operation::Move {
                number: *number,
                before: *before,
                after: *after,
                if_order_version: *if_order_version,
            },
            Action::View { number } => Operation::View { number: *number },
            Action::History {
                number,
                limit,
                offset,
            } => Operation::History {
                number: *number,
                limit: *limit,
                offset: *offset,
            },
            Action::Create {
                title,
                body,
                labels,
            } => Operation::Create {
                title: title.clone(),
                body: body.read()?.unwrap_or_default(),
                labels: labels.clone(),
                at_top: false,
            },
            Action::Edit {
                number,
                title,
                body,
                add_labels,
                remove_labels,
                if_version,
            } => Operation::Edit {
                number: *number,
                title: title.clone(),
                body: body.read()?,
                add_labels: add_labels.clone(),
                remove_labels: remove_labels.clone(),
                if_version: *if_version,
            },
            Action::Claim { number, force } => Operation::Claim {
                number: *number,
                force: *force,
            },
            Action::AssignToBoss { number, force } => Operation::AssignBoss {
                number: *number,
                force: *force,
            },
            Action::Unassign { number, force } => Operation::Unassign {
                number: *number,
                force: *force,
            },
            Action::Comment { number, body } => Operation::Comment {
                number: *number,
                body: body
                    .read()?
                    .ok_or_else(|| Error::invalid("comment requires --body or --file"))?,
            },
            Action::Close {
                number,
                comment,
                force,
            } => Operation::Close {
                number: *number,
                comment: comment.clone(),
                force: *force,
            },
            Action::Reopen { number } => Operation::Reopen { number: *number },
            Action::Delete { number, force } => Operation::Delete {
                number: *number,
                force: *force,
            },
            Action::Restore { number } => Operation::Restore { number: *number },
            Action::Rpc | Action::Web { .. } | Action::DrainGithub { .. } => unreachable!(),
        })
    }
}

pub fn run(options: &Options) -> Result<()> {
    if let Action::DrainGithub {
        repo,
        author,
        all_authors,
        state,
        dry_run,
    } = &options.action
    {
        if options.request_id.is_some() {
            return Err(Error::invalid(
                "drain-github generates stable per-source request IDs; omit --request-id",
            ));
        }
        let mut command = std::process::Command::new("python3");
        command.args(["-c", include_str!("../tools/drain_github_issues.py")]);
        command.env("HEY_BOSS_DRAIN_BINARY", std::env::current_exe()?);
        for (flag, value) in [
            ("--repo", repo.as_deref()),
            ("--author", author.as_deref()),
            ("--project", options.project.as_deref()),
            ("--host", options.host.as_deref()),
        ] {
            if let Some(value) = value {
                command.arg(flag).arg(value);
            }
        }
        if options.project.is_none() {
            if let Some(project) = worker_project() {
                command.arg("--project").arg(project);
            }
        }
        command.arg("--state").arg(state);
        for (enabled, flag) in [
            (*all_authors, "--all-authors"),
            (*dry_run, "--dry-run"),
            (options.json, "--json"),
        ] {
            if enabled {
                command.arg(flag);
            }
        }
        std::process::exit(command.status()?.code().unwrap_or(1));
    }
    if let Action::Worker {
        command: WorkerAction::Run,
    } = options.action
    {
        if options.host.is_some()
            || std::env::var_os("HEY_BOSS_ISSUE_HOST").is_some_and(|v| !v.is_empty())
        {
            return Err(Error::invalid(
                "Start the worker service on the authoritative host; worker run uses the local database",
            ));
        }
        return issues::worker::serve();
    }
    if let Action::Web { port, no_discovery } = options.action {
        return issues::web::serve(issues::web::Config {
            port,
            discover: !no_discovery,
            project: options.project.clone().or_else(worker_project),
            actor: options.agent.clone(),
            host: options.host.clone().or_else(|| {
                std::env::var("HEY_BOSS_ISSUE_HOST")
                    .ok()
                    .filter(|s| !s.is_empty())
            }),
            json: options.json,
            mindmap: false,
        });
    }
    let rpc = matches!(options.action, Action::Rpc);
    let mut value = if rpc {
        let raw = read_text(std::io::stdin(), issues::WIRE_LIMIT)?;
        let request: Request = serde_json::from_str(&raw)?;
        Store::open(&issues::database_path()?)?.execute(&request)?
    } else {
        let operation = options.operation()?;
        let cwd = std::env::current_dir()?.canonicalize()?;
        let machine = issues::identity::machine()?;
        let project = issues::identity::project(&cwd, &machine)?;
        let actor = if operation.needs_actor() {
            Some(issues::identity::resolve(
                options.agent.as_deref(),
                &machine,
                &cwd,
            )?)
        } else {
            None
        };
        let request = Request {
            version: 1,
            project,
            project_override: options.project.clone().or_else(worker_project),
            actor,
            operation,
            request_id: options.request_id.clone(),
        };
        let host = options.host.clone().or_else(|| {
            std::env::var("HEY_BOSS_ISSUE_HOST")
                .ok()
                .filter(|s| !s.is_empty())
        });
        let mut value = match &host {
            Some(host) => issues::remote::call(host, &request)?,
            None => Store::open(&issues::database_path()?)?.execute(&request)?,
        };
        value["store"] = if let Some(host) = host {
            json!({"host":host})
        } else {
            json!({"host":"local","database":issues::database_path()?})
        };
        value
    };
    if let Some(actor) = value.get("assignee_agent").filter(|a| !a.is_null()) {
        let actor: issues::Actor = serde_json::from_value(actor.clone())?;
        value["assignee_presence"] = json!(issues::identity::presence(
            &actor,
            &issues::identity::machine()?
        ));
    }
    if options.json_output() {
        println!("{}", serde_json::to_string(&value)?);
    } else {
        print_text(&value);
    }
    Ok(())
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
fn line(value: &Value) -> String {
    text(value)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn markdown(value: &Value) -> String {
    text(value)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}
fn print_text(value: &Value) {
    let project = &value["project"];
    if value["scope"] != "global" {
        println!("{} ({})", line(&project["name"]), line(&project["id"]));
    }
    if value.get("workers").is_some() {
        issues::worker::print_status(value, false);
        return;
    }
    if let Some(instructions) = value["instructions"].as_str() {
        println!("Instructions: {instructions}");
    }
    if let Some(prs) = value["pull_requests"].as_array() {
        for pr in prs {
            println!("PR: {}", line(&pr["url"]));
        }
    }
    if value["scope"] == "global" {
        println!(
            "Boss: {} (human:boss)\nRevision: {}",
            value["boss_name"].as_str().unwrap_or("Boss"),
            value["version"]
        );
        return;
    }
    if value.get("prs_enabled").is_some() {
        println!(
            "PRs enabled: {}\nPrompt: {}",
            value["prs_enabled"],
            markdown(&value["prompt"])
        );
    }
    if let Some(config) = value.get("config") {
        println!(
            "Worker: {} · project limit {} · global {}/{} · {} eligible\nDirectory: {}\nRequired labels: {}\n/goal: {}",
            if config["enabled"] == true {
                "enabled"
            } else {
                "paused"
            },
            config["concurrency"],
            value["pool"]["active"],
            value["pool"]["concurrency"],
            value["eligible"],
            line(&config["cwd"]),
            config["labels"],
            config["use_goal"]
        );
        if let Some(runs) = value["runs"].as_array() {
            for run in runs {
                println!(
                    "{} · #{} · {} · {}",
                    line(&run["id"]),
                    run["number"],
                    line(&run["state"]),
                    line(&run["last_event"])
                );
            }
        }
    }
    if let Some(projects) = value["projects"].as_array() {
        println!(
            "{:<28} {:>6} {:>8} {:>10} {:>7} {:>8}  PROJECT",
            "NAME", "OPEN", "CLAIMED", "UNASSIGNED", "CLOSED", "DELETED"
        );
        for project in projects {
            let name = format!(
                "{}{}",
                line(&project["name"]),
                if project["hidden_at"].is_null() {
                    ""
                } else {
                    " [hidden]"
                }
            );
            let open = project["open"].as_i64().unwrap_or(0);
            let unassigned = project["unassigned"].as_i64().unwrap_or(0);
            println!(
                "{:<28} {:>6} {:>8} {:>10} {:>7} {:>8}  {}",
                name,
                open,
                open - unassigned,
                unassigned,
                project["closed"].as_i64().unwrap_or(0),
                project["deleted"].as_i64().unwrap_or(0),
                line(&project["id"])
            );
        }
    }

    if let Some(agent) = value.get("agent") {
        println!(
            "Agent: {}\nSource: {}\nHost: {}",
            line(&agent["id"]),
            line(&agent["source"]),
            line(&agent["host"])
        );
        if !agent["pid"].is_null() {
            println!("PID: {}", agent["pid"]);
        }
    }
    if let Some(issues) = value["issues"].as_array() {
        if let Some(parent) = value.get("parent_issue") {
            println!(
                "Subtasks of #{}: {}",
                parent["number"],
                line(&parent["title"])
            );
        }
        if issues.is_empty() {
            println!("No matching issues.");
        }
        for issue in issues {
            print_issue_line(issue);
        }
    }
    if let Some(issue) = value.get("issue") {
        print_issue_line(issue);
        if value.get("changed").is_none() {
            println!(
                "Version: {} · Created by: {}",
                issue["version"],
                line(&issue["created_by"])
            );
            if let Some(presence) = value.get("assignee_presence") {
                println!(
                    "Assignee process: {} (claims persist until explicitly cleared)",
                    text(presence)
                );
            }
            if !issue["closed_by"].is_null() {
                println!("Closed by: {}", line(&issue["closed_by"]));
            }
            if !text(&issue["body"]).is_empty() {
                println!("\n{}", markdown(&issue["body"]));
            }
        } else if value["changed"] == false {
            println!("Already in the requested state.");
        }
    }
    if let Some(children) = value["subtasks"].as_array()
        && !children.is_empty()
    {
        println!("\nSubtasks:");
        for child in children {
            print_issue_line(child);
        }
    }
    if let Some(comments) = value["comments"].as_array() {
        for comment in comments {
            println!(
                "\nComment {} · {}\n{}",
                comment["id"],
                line(&comment["author"]),
                markdown(&comment["body"])
            );
        }
        if value["more_comments"] == true {
            println!("\nShowing recent comments; use history for earlier comments.");
        }
    }
    if let Some(events) = value["events"].as_array() {
        for event in events {
            println!(
                "\n{} · {} · {}\n{}",
                event["id"],
                line(&event["action"]),
                line(&event["actor"]),
                serde_json::to_string_pretty(&event["data"]).unwrap()
            );
        }
    }
    if let Some(offset) = value.get("next_offset").filter(|n| !n.is_null()) {
        println!("More results: --offset {offset}");
    }
    if let Some(id) = value.get("comment_id").filter(|n| !n.is_null()) {
        println!("Saved comment {id}.");
    }
}
fn print_issue_line(issue: &Value) {
    let state = if issue["deleted_at"].is_null() {
        text(&issue["state"])
    } else {
        "deleted"
    };
    let owner = issue["assignee_name"]
        .as_str()
        .or_else(|| issue["assignee"].as_str())
        .unwrap_or("unassigned");
    println!(
        "#{} [{}] {} · {}",
        issue["number"],
        state,
        line(&issue["title"]),
        owner
    );
    if let Some(number) = issue["parent"]["number"].as_i64() {
        println!(
            "  Parent: #{} {}{}",
            number,
            line(&issue["parent"]["title"]),
            if issue["parent"]["deleted_at"].is_null() {
                ""
            } else {
                " [deleted]"
            }
        );
    }
    if let Some(total) = issue["subtasks"]["total"].as_u64() {
        println!(
            "  Subtasks: {}/{} closed · {} open descendants",
            issue["subtasks"]["closed"], total, issue["subtasks"]["open_descendants"]
        );
    }
    if let Some(prs) = issue["pull_requests"].as_array() {
        for pr in prs {
            println!("  PR: {}", line(&pr["url"]));
        }
    }
}

fn worker_project() -> Option<String> {
    std::env::var("HEY_BOSS_ISSUE_PROJECT")
        .ok()
        .filter(|v| !v.is_empty())
}
