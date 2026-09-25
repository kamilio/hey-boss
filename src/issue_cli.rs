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
        boss_name: Option<String>,
        #[arg(long, action = clap::ArgAction::Set)]
        auto_close_merged_prs: Option<bool>,
        #[arg(long)]
        if_version: Option<i64>,
    },
}
pub fn run_global(options: &GlobalOptions) -> Result<()> {
    let operation = match &options.action {
        GlobalSettingsAction::Show => Operation::GlobalSettings,
        GlobalSettingsAction::Set {
            boss_name,
            auto_close_merged_prs,
            if_version,
        } => Operation::ConfigureGlobal {
            boss_name: boss_name.clone(),
            auto_close_merged_prs: *auto_close_merged_prs,
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
    /// Atomically update guarded labels and ownership; the entire array is one group.
    #[command(
        after_help = "JSON array (up to 100 issues / 1 MiB):\n  [{\"number\":1,\"if_version\":3,\"expected_assignee\":\"codex:session\",\"add_labels\":[\"rework needed\"],\"remove_labels\":[\"PR ready\"],\"assignment\":\"unassign\"}]\nexpected_assignee is required; use null for unassigned. assignment: keep (default), unassign, boss.\nApplying requires --request-id.\nGuard rejection returns applied:false and per-issue rejected/blocked results; no issue changes.\nThe caller assesses readiness; this command never closes issues or controls workers."
    )]
    Batch {
        /// JSON array file; '-' reads stdin.
        #[arg(long)]
        file: PathBuf,
    },
    /// Upgrade preflight; use the destination installation's state directory.
    #[command(hide = true)]
    Migrate {
        #[arg(long)]
        installation: PathBuf,
    },
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
    },
    #[command(skip)]
    GlobalSettings { operation: Operation },
    /// Shared and conditional project prompts, worktree choices, and PR behavior.
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
    #[command(visible_alias = "subtasks", after_help = SUBTASK_SCHEDULING_HELP)]
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
        /// Private Tailscale Serve HTTPS origin, e.g. https://mac.tailnet.ts.net:8443.
        /// Keeps the listener on loopback; configure Tailscale Serve separately.
        #[arg(long)]
        mobile_origin: Option<String>,
    },
    /// List projects by recent activity, with issue state counts.
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
        #[arg(long, default_value = "open", value_parser = ["open", "blocked", "ready", "closed", "all", "deleted"])]
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
        /// Retrieve every matching issue in queue order, without pagination.
        #[arg(long, conflicts_with_all = ["limit", "offset"])]
        all: bool,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// Move an inactive issue to an existing project, preserving its history.
    Transfer {
        number: i64,
        #[arg(long)]
        destination: String,
        #[arg(long)]
        if_version: i64,
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
    /// Inspect fleet allocation without claiming, synchronizing, or changing workers.
    Allocation { number: i64 },
    /// Read comments without audit events; newest first by default.
    Comments {
        number: i64,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
        #[arg(long, value_enum, default_value = "newest")]
        sort: issues::CommentSort,
    },
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
        /// Place the new issue at the front of the project queue (the default).
        #[arg(long, conflicts_with = "at_bottom")]
        at_top: bool,
        /// Append the new issue to the project queue.
        #[arg(long)]
        at_bottom: bool,
        #[arg(long)]
        draft: bool,
        #[arg(long)]
        interactive: bool,
        #[arg(long)]
        title: String,
        #[command(flatten)]
        body: Body,
        #[arg(long = "label")]
        labels: Vec<String>,
    },
    /// Replace supplied fields; omitted fields are preserved.
    Edit {
        #[arg(long)]
        draft: bool,
        #[arg(long)]
        interactive: bool,
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
    /// Make a draft runnable, syncing its bound plan first.
    Undraft { number: i64 },
    #[command(hide = true)]
    PlanSync,

    /// Show the calling session identity and resolved project.
    Whoami,
    /// Atomically assign an open issue to this session.
    #[command(
        visible_alias = "assign-to-myself",
        after_help = "To resume a released manual claim, retain its saved --agent ID.\nInspect first with `hey-boss issue allocation NUMBER` (add --json for structured reasons).\nCompanions sync automatically. With missing local allocation, inspect the supervisor using\n`hey-boss issue allocation NUMBER --host SUPERVISOR`; if unreserved or reserved for your\nmachine, claim using `hey-boss issue claim NUMBER --host SUPERVISOR --agent SAVED_ID`.\nThe supervisor reserves unallocated work atomically. Wait for local allocation before offline work.\nFor another machine's reservation, resume there or ask Boss for a handoff.\n--force is an explicit takeover override, never an allocation or synchronization recovery step."
    )]
    Claim {
        number: i64,
        /// Explicitly take over another session's claim.
        #[arg(long)]
        force: bool,
    },
    /// Mark an attached PR ready for Boss and unblock dependent tasks.
    Ready {
        number: i64,
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
    /// Publish a short progress update (green=on track, orange=at risk, red=in trouble).
    Status {
        number: i64,
        #[arg(value_enum)]
        level: issues::StatusLevel,
        /// One line of simple human language, up to 500 characters.
        #[arg(long)]
        comment: String,
    },
    /// Read progress updates, newest first, separately from durable comments.
    StatusHistory {
        number: i64,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// Resolve a comment while preserving its content.
    ResolveComment { number: i64, comment_id: i64 },
    /// Reopen a resolved comment.
    UnresolveComment { number: i64, comment_id: i64 },
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
    /// Block an issue and clear its claim. Use rarely: make every effort to resolve
    /// it first, ask the user for help via hey-boss ask, and describe the blocker.
    /// Set the issues blocking this issue; omit blockers to remove all links.
    BlockedBy {
        number: i64,
        blockers: Vec<i64>,
        #[arg(long)]
        if_version: Option<i64>,
        #[arg(long)]
        force: bool,
    },
    Block {
        number: i64,
        /// Issue that must be Ready (PR projects) or Closed first; repeat for multiple blockers.
        #[arg(long = "by")]
        blockers: Vec<i64>,
        #[arg(long)]
        comment: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Reopen a blocked, ready, or closed issue without assigning it.
    Reopen {
        number: i64,
        /// Clear only a reconciled manual hold; unresolved dependencies keep the issue Blocked.
        #[arg(long)]
        clear_manual_hold: bool,
        /// Reject reopening if another writer has changed this version.
        #[arg(long)]
        if_version: Option<i64>,
    },
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
    #[command(after_help = SUBTASK_SCHEDULING_HELP)]
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
    #[command(after_help = SUBTASK_SCHEDULING_HELP)]
    Add {
        number: i64,
        child: i64,
        #[arg(long)]
        if_version: Option<i64>,
        #[arg(long)]
        if_child_version: Option<i64>,
    },
    /// Unlink a subtask while preserving its issue and history.
    #[command(after_help = SUBTASK_SCHEDULING_HELP)]
    Remove {
        number: i64,
        child: i64,
        #[arg(long)]
        if_version: Option<i64>,
        #[arg(long)]
        if_child_version: Option<i64>,
    },
}

const SUBTASK_SCHEDULING_HELP: &str = "Default: sequential siblings. For independent branches with declared dependencies only:\n  hey-boss issue settings set --subtask-scheduling explicit\nDeclare intentional sequences with `issue blocked-by CHILD PREDECESSOR`.\nReady handoff and parent completion are unchanged.\nSubtasks affect scheduling: unfinished descendants put the parent in Blocked.\nChanges that would release an existing parent or ancestor claim are rejected atomically,\neven for the claim owner. For organization only, prefer ownership-preserving mindmap nesting:\n  hey-boss mm issue PARENT --id parent-work\n  hey-boss mm issue CHILD --under parent-work\nFor a scheduling dependency, have the owner explicitly unassign the affected issue first.";

#[derive(Subcommand)]
enum PrAction {
    /// Attach a PR; existing links keep their recorded purpose.
    Add {
        number: i64,
        url: String,
        #[arg(long, value_enum, default_value = "unspecified")]
        purpose: issues::PrPurpose,
    },
    /// Change an attached PR's purpose without removing its history.
    Classify {
        number: i64,
        url: String,
        #[arg(long, value_enum)]
        purpose: issues::PrPurpose,
    },
    Remove {
        number: i64,
        url: String,
    },
    List {
        number: i64,
    },
}
#[derive(Subcommand)]
enum SettingsAction {
    Show,
    Set {
        /// Sequential (default) waits for earlier siblings; explicit uses declared links only.
        #[arg(long, value_parser = ["sequential", "explicit"])]
        subtask_scheduling: Option<String>,
        /// Legacy spelling; global profile settings own this value.
        #[arg(long, hide = true)]
        boss_name: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        /// Enable an hourly organizing agent outside worker concurrency.
        #[arg(long, conflicts_with = "no_chief")]
        chief: bool,
        #[arg(long)]
        no_chief: bool,
        #[arg(long)]
        chief_prompt: Option<String>,
        #[arg(long, conflicts_with = "no_worktree")]
        worktree: bool,
        #[arg(long)]
        no_worktree: bool,
        #[arg(long, conflicts_with = "no_prs")]
        prs_enabled: bool,
        #[arg(long)]
        no_prs: bool,
        #[arg(long, conflicts_with = "no_drafts")]
        drafts_enabled: bool,
        #[arg(long)]
        no_drafts: bool,
        #[arg(long)]
        plan_template: Option<String>,
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
            Action::Batch { file } => {
                let raw = Body {
                    body: None,
                    file: Some(file.clone()),
                }
                .read()?
                .unwrap();
                Operation::Batch {
                    edits: serde_json::from_str(&raw)?,
                }
            }
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
                PrAction::Add {
                    number,
                    url,
                    purpose,
                } => Operation::AddPullRequest {
                    number: *number,
                    url: url.clone(),
                    purpose: *purpose,
                },
                PrAction::Classify {
                    number,
                    url,
                    purpose,
                } => Operation::ClassifyPullRequest {
                    number: *number,
                    url: url.clone(),
                    purpose: *purpose,
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
                    subtask_scheduling,
                    boss_name,
                    prompt,
                    chief,
                    no_chief,
                    chief_prompt,
                    prs_enabled,
                    no_prs,
                    worktree,
                    no_worktree,
                    drafts_enabled,
                    no_drafts,
                    plan_template,
                } => {
                    if boss_name.is_none()
                        && subtask_scheduling.is_none()
                        && !chief
                        && !no_chief
                        && chief_prompt.is_none()
                        && prompt.is_none()
                        && !worktree
                        && !no_worktree
                        && !prs_enabled
                        && !no_prs
                        && !drafts_enabled
                        && !no_drafts
                        && plan_template.is_none()
                    {
                        return Err(Error::invalid(
                            "Specify --subtask-scheduling, --prompt, --chief, --no-chief, --chief-prompt, --worktree, --no-worktree, --prs-enabled, --no-prs, --drafts-enabled, --no-drafts, or --plan-template",
                        ));
                    }
                    Operation::ConfigureProject {
                        subtask_scheduling: subtask_scheduling.clone(),
                        chief_enabled: if *chief {
                            Some(true)
                        } else if *no_chief {
                            Some(false)
                        } else {
                            None
                        },
                        chief_prompt: chief_prompt.clone(),
                        drafts_enabled: if *drafts_enabled {
                            Some(true)
                        } else if *no_drafts {
                            Some(false)
                        } else {
                            None
                        },
                        worktree_enabled: if *worktree {
                            Some(true)
                        } else if *no_worktree {
                            Some(false)
                        } else {
                            None
                        },
                        prompt_overrides: None,
                        plan_template: plan_template.clone(),
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
                all,
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
                all: *all,
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
            Action::Transfer {
                number,
                destination,
                if_version,
            } => Operation::Transfer {
                number: *number,
                destination: destination.clone(),
                if_version: *if_version,
            },
            Action::View { number } => Operation::View { number: *number },
            Action::Allocation { number } => Operation::Allocation {
                number: *number,
                machine: issues::identity::machine()?,
            },
            Action::Comments {
                number,
                limit,
                offset,
                sort,
            } => Operation::Comments {
                number: *number,
                limit: *limit,
                offset: *offset,
                sort: *sort,
            },
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
                at_top: _,
                at_bottom,
                title,
                body,
                labels,
                draft,
                interactive,
            } => {
                if *interactive && body.file.as_deref() == Some(std::path::Path::new("-")) {
                    return Err(Error::invalid(
                        "Interactive --file requires an existing file in the checkout",
                    ));
                }
                let imported = body.read()?.unwrap_or_default();
                let (title, body) = if *interactive && body.file.is_some() {
                    issues::planning::parse(&imported)?
                } else {
                    (title.clone(), imported)
                };
                Operation::Create {
                    draft: *draft || *interactive,
                    title,
                    body,
                    labels: labels.clone(),
                    at_top: !*at_bottom,
                }
            }
            Action::Edit {
                number,
                title,
                body,
                add_labels,
                remove_labels,
                if_version,
                draft,
                interactive,
            } => Operation::Edit {
                draft: if *draft { Some(true) } else { None },
                number: *number,
                title: title.clone(),
                body: if *interactive && body.file.is_some() {
                    None
                } else {
                    body.read()?
                },
                add_labels: add_labels.clone(),
                remove_labels: remove_labels.clone(),
                if_version: *if_version,
            },
            Action::Undraft { number } => Operation::Undraft { number: *number },
            Action::PlanSync => unreachable!(),
            Action::Claim { number, force } => Operation::Claim {
                number: *number,
                force: *force,
            },
            Action::Ready { number, force } => Operation::Ready {
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
            Action::Status {
                number,
                level,
                comment,
            } => Operation::Status {
                number: *number,
                level: *level,
                comment: comment.clone(),
            },
            Action::StatusHistory {
                number,
                limit,
                offset,
            } => Operation::StatusHistory {
                number: *number,
                limit: *limit,
                offset: *offset,
                before: None,
            },
            Action::ResolveComment { number, comment_id }
            | Action::UnresolveComment { number, comment_id } => Operation::ResolveComment {
                number: *number,
                comment_id: *comment_id,
                resolved: matches!(self.action, Action::ResolveComment { .. }),
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
            Action::BlockedBy {
                number,
                blockers,
                if_version,
                force,
            } => Operation::SetBlockers {
                number: *number,
                blockers: blockers.clone(),
                if_version: *if_version,
                force: *force,
            },
            Action::Block {
                number,
                blockers,
                comment,
                force,
            } => {
                eprintln!(
                    "Warning: blocking should be rare. Make every effort to resolve the issue first; raise questions and ask the user for help via hey-boss ask. Explain the blocker in --comment. Reopen when it can proceed."
                );
                Operation::Block {
                    number: *number,
                    blockers: if blockers.is_empty() {
                        None
                    } else {
                        Some(blockers.clone())
                    },
                    comment: comment.clone(),
                    force: *force,
                }
            }
            Action::Reopen {
                number,
                if_version,
                clear_manual_hold,
            } => Operation::Reopen {
                clear_manual_hold: *clear_manual_hold,
                number: *number,
                if_version: *if_version,
            },
            Action::Delete { number, force } => Operation::Delete {
                number: *number,
                force: *force,
            },
            Action::Restore { number } => Operation::Restore { number: *number },
            Action::Rpc
            | Action::Web { .. }
            | Action::DrainGithub { .. }
            | Action::Migrate { .. } => unreachable!(),
        })
    }
}

pub fn run(options: &Options) -> Result<()> {
    if let Action::Migrate { installation } = &options.action {
        let path = issues::database_path_for_installation(&installation.canonicalize()?)?;
        Store::migrate(&path)?;
        if options.json {
            println!("{}", json!({"ok": true}));
        }
        return Ok(());
    }
    if let Action::DrainGithub {
        repo,
        author,
        all_authors,
        state,
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
        if options.project.is_none()
            && let Some(project) = worker_project()
        {
            command.arg("--project").arg(project);
        }
        command.arg("--state").arg(state);
        for (enabled, flag) in [(*all_authors, "--all-authors"), (options.json, "--json")] {
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
    if let Action::Web {
        port,
        no_discovery,
        ref mobile_origin,
    } = options.action
    {
        return issues::web::serve(issues::web::Config {
            port,
            mobile_origin: mobile_origin.clone(),
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
    if matches!(options.action, Action::PlanSync) {
        return issues::planning::daemon();
    }
    let rpc = matches!(options.action, Action::Rpc);
    let mut value = if rpc {
        let raw = read_text(std::io::stdin(), issues::WIRE_LIMIT)?;
        let request: Request = serde_json::from_str(&raw)?;
        Store::open(&issues::database_path()?)?.execute(&request)?
    } else {
        let interactive = match &options.action {
            Action::Create { interactive, .. } | Action::Edit { interactive, .. } => *interactive,
            _ => false,
        };
        if interactive {
            issues::planning::require_terminal()?;
        }
        let operation = match &options.action {
            Action::Edit {
                number,
                interactive: true,
                ..
            } => Operation::View { number: *number },
            _ => options.operation()?,
        };
        let cwd = std::env::current_dir()?.canonicalize()?;
        let machine = issues::identity::machine()?;
        let project = issues::identity::project(&cwd, &machine)?;
        let inspection = matches!(operation, Operation::View { .. }) && !interactive;
        let mut actor = if operation.needs_actor() || interactive || inspection {
            Some(if inspection {
                issues::identity::resolve_inspection(options.agent.as_deref(), &machine, &cwd)?
            } else {
                issues::identity::resolve(options.agent.as_deref(), &machine, &cwd)?
            })
        } else {
            None
        };
        if matches!(
            operation,
            Operation::Create { .. } | Operation::CreateSubtask { .. }
        ) && let Some(actor) = actor.as_mut()
        {
            issues::identity::creation_context(actor);
        }
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
            Some(host) => issues::remote::call(host, &request).map_err(|mut error| {
                if let Some(info) = &mut error.details {
                    let original = info["inspect_command"].as_str().map(str::to_owned);
                    scope_allocation(info, host);
                    if let Some(original) = original {
                        error.message = error.message.replace(
                            &format!("Inspect: {original}\n"),
                            &format!("Inspect: {}\n", info["inspect_command"].as_str().unwrap()),
                        );
                    }
                }
                error
            })?,
            None => Store::open(&issues::database_path()?)?.execute(&request)?,
        };
        if interactive {
            let file = match &options.action {
                Action::Create { body, .. } | Action::Edit { body, .. } => body.file.as_deref(),
                _ => None,
            };
            let pending = match &options.action {
                Action::Edit {
                    draft,
                    title,
                    body,
                    add_labels,
                    remove_labels,
                    ..
                } if *draft
                    || title.is_some()
                    || body.body.is_some()
                    || !add_labels.is_empty()
                    || !remove_labels.is_empty() =>
                {
                    Some(options.operation()?)
                }
                _ => None,
            };
            value = issues::planning::interactive(&request, host.as_deref(), value, file, pending)?;
        }
        if let Some(host) = &host
            && let Some(info) = value.get_mut("allocation")
        {
            scope_allocation(info, host);
        }
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
    if !rpc && value["accepted"] == false && value.get("results").is_some() {
        // Preserve the compact group result instead of replacing it with the
        // ordinary single-error envelope. RPC carries this result successfully.
        std::process::exit(4);
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
fn command_context(value: &Value) -> String {
    let quote = |v: &Value| format!("'{}'", line(v).replace('\'', "'\\''"));
    let mut context = String::new();
    if value["project"]["id"].is_string() {
        context.push_str(&format!(" --project {}", quote(&value["project"]["id"])));
    }
    if let Some(host) = value["store"]["host"]
        .as_str()
        .filter(|host| *host != "local")
    {
        context.push_str(&format!(" --host {}", quote(&json!(host))));
    }
    context
}
fn scope_allocation(info: &mut Value, host: &str) {
    let scope = format!(" --host '{}'", host.replace('\'', "'\\''"));
    if let Some(command) = info["inspect_command"].as_str() {
        info["inspect_command"] = json!(format!("{command}{scope}"));
    }
    if matches!(
        info["reason"].as_str(),
        Some("allocated_here" | "unallocated")
    ) && let Some(recovery) = info["recovery"].as_str()
    {
        info["recovery"] = json!(recovery.replace(" --agent ", &format!("{scope} --agent ")));
    }
    info["store_host"] = json!(host);
}

pub(crate) fn print_text(value: &Value) {
    if let Some(warning) = value["ownership_warning"].as_str() {
        eprintln!("Warning: {}", line(&json!(warning)));
        if let Some(presence) = value["assignee_presence"].as_str() {
            eprintln!("Assignee process: {presence}.");
        }
    }
    if let Some(warnings) = value["project_warnings"].as_array() {
        for warning in warnings {
            eprintln!("Warning: {}", line(&warning["message"]));
        }
    }
    if let Some(destination) = value.get("moved_to") {
        println!(
            "Issue moved to {} ({}) #{}. Open it with --project {}.",
            destination["project"]["name"].as_str().unwrap_or(""),
            destination["project"]["id"].as_str().unwrap_or(""),
            destination["number"],
            destination["project"]["id"].as_str().unwrap_or("")
        );
        return;
    }
    let project = &value["project"];
    if value["scope"] != "global" {
        println!("{} ({})", line(&project["name"]), line(&project["id"]));
    }
    if value.get("issue").is_none()
        && let Some(allocation) = value.get("allocation")
    {
        println!(
            "{}\nStore: {} · machine {}\nInspect: {}\n{}",
            line(&allocation["summary"]),
            line(&allocation["role"]),
            line(&allocation["store_machine"]),
            line(&allocation["inspect_command"]),
            line(&allocation["recovery"])
        );
        return;
    }
    if let Some(results) = value["results"].as_array() {
        println!(
            "Batch: {}",
            if value["accepted"] != true {
                "rejected; no issue changes"
            } else {
                "applied"
            }
        );
        for result in results {
            println!(
                "#{} {}{}",
                result["number"],
                line(&result["status"]),
                result["error"]["message"]
                    .as_str()
                    .map(|m| format!(": {m}"))
                    .unwrap_or_default()
            );
            if let Some(after) = result.get("after") {
                println!(
                    "  v{} → v{}; assignee: {}; labels: {}",
                    result["before"]["version"],
                    after["version"],
                    after["assignee"].as_str().unwrap_or("unassigned"),
                    after["labels"]
                );
            }
        }
        return;
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
            println!(
                "PR [{}]: {}",
                pr["purpose"].as_str().unwrap_or("unspecified"),
                line(&pr["url"])
            );
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
    for (key, label) in [
        // Project permission does not imply that a worker selected a worktree.
        ("worktree_enabled", "Worktrees allowed"),
        ("prs_enabled", "PRs enabled"),
        ("chief_enabled", "Chief enabled"),
    ] {
        if value[key] == true {
            println!("{label}: true");
        }
    }
    for (key, label) in [
        ("chief_prompt", "Chief prompt"),
        ("prompt", "Shared prompt"),
    ] {
        let prompt = markdown(&value[key]);
        if !prompt.trim().is_empty() {
            println!("{label}: {prompt}");
        }
    }
    if let Some(mode) = value.get("subtask_scheduling") {
        println!("Subtask scheduling: {}", line(mode));
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
            "{:<28} {:>6} {:>8} {:>10} {:>8} {:>7} {:>7} {:>8}  PROJECT",
            "NAME", "OPEN", "CLAIMED", "UNASSIGNED", "BLOCKED", "READY", "CLOSED", "DELETED"
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
                "{:<28} {:>6} {:>8} {:>10} {:>8} {:>7} {:>7} {:>8}  {}",
                name,
                open,
                open - unassigned,
                unassigned,
                project["blocked"].as_i64().unwrap_or(0),
                project["ready"].as_i64().unwrap_or(0),
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
            println!(
                "Created: {} · Updated: {}",
                status_datetime(&issue["created_at"]),
                status_datetime(&issue["updated_at"])
            );
            if let Some(labels) = issue["labels"]
                .as_array()
                .filter(|labels| !labels.is_empty())
            {
                println!(
                    "Labels: {}",
                    labels.iter().map(line).collect::<Vec<_>>().join(", ")
                );
            }
            if let Some(origin) = issue["origin"].as_object() {
                if origin.get("model").is_some_and(Value::is_string) {
                    println!("Creator model: {}", line(&origin["model"]));
                }
                println!(
                    "Origin: {} · {}",
                    line(&origin["host"]),
                    line(&origin["cwd"])
                );
                if origin["session_id"].is_string() {
                    println!("Creator session: {}", line(&origin["session_id"]));
                }
            }
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
    if let Some(artifacts) = value["artifacts"]
        .as_array()
        .filter(|items| !items.is_empty())
    {
        println!("\nLinked artifacts:");
        for artifact in artifacts {
            println!(
                "  {} · {}{}",
                line(&artifact["id"]),
                line(&artifact["title"]),
                if artifact["archived"] == true {
                    " · archived"
                } else {
                    ""
                }
            );
        }
        println!(
            "Read: hey-boss artifact view ID{}\nSave Markdown: hey-boss artifact export ID --output PATH.md{}",
            command_context(value),
            command_context(value)
        );
    }
    if let Some(comments) = value["comments"].as_array() {
        println!(
            "\nComments: {} shown · {} total",
            comments.len(),
            value["comment_count"]
                .as_u64()
                .unwrap_or(comments.len() as u64)
        );
        for comment in comments {
            println!(
                "\nComment {} · {} · {}{}\n{}",
                comment["id"],
                line(&comment["author"]),
                status_datetime(&comment["created_at"]),
                if comment["resolved"] == true {
                    " · resolved"
                } else {
                    ""
                },
                markdown(&comment["body"])
            );
        }
        if value["more_comments"] == true {
            println!(
                "\nEarlier comments: hey-boss issue comments {}{} --offset {}",
                value["issue"]["number"],
                command_context(value),
                value["next_comment_offset"]
            );
        }
        if let Some(offset) = value["next_offset"].as_u64() {
            println!(
                "\nMore comments: hey-boss issue comments {}{} --offset {offset} --sort {}",
                value["number"],
                command_context(value),
                line(&value["sort"])
            );
        }
    }
    if let Some(updates) = value["updates"].as_array() {
        if updates.is_empty() {
            println!("No status updates yet.");
        }
        for update in updates {
            println!(
                "{} · {} · {}\n{}",
                line(&update["level"]),
                line(&update["author"]),
                status_datetime(&update["created_at"]),
                line(&update["comment"])
            );
        }
        if let Some(offset) = value["next_offset"].as_u64() {
            println!("More updates: use --offset {offset}");
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
    if let Some(offset) = value
        .get("next_offset")
        .filter(|n| !n.is_null() && value.get("comments").is_none())
    {
        println!("More results: --offset {offset}");
    }
    if let Some(id) = value.get("comment_id").filter(|n| !n.is_null()) {
        println!("Saved comment {id}.");
    }
}
fn status_datetime(value: &Value) -> String {
    let seconds = value.as_i64().unwrap_or(0) / 1000;
    let mut time = std::mem::MaybeUninit::<libc::tm>::uninit();
    let mut text = [0_u8; 64];
    // localtime_r writes the tm before strftime reads it; the output is bounded
    // and local to this call, so concurrent CLI requests share no static buffer.
    unsafe {
        if libc::localtime_r(&seconds, time.as_mut_ptr()).is_null() {
            return "Unknown time".into();
        }
        let size = libc::strftime(
            text.as_mut_ptr().cast(),
            text.len(),
            c"%Y-%m-%d %H:%M %Z".as_ptr(),
            time.as_ptr(),
        );
        String::from_utf8_lossy(&text[..size]).into_owned()
    }
}
fn print_issue_line(issue: &Value) {
    if issue["draft"] == true {
        print!("Draft · ");
    }
    if let Some(path) = issue["plan"]["path"].as_str() {
        println!("Plan: {path}");
    }
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
        "#{} [{}] {} · {} · {} agent launch{}{}",
        issue["number"],
        state,
        line(&issue["title"]),
        owner,
        issue["agent_launch_count"].as_u64().unwrap_or(0),
        if issue["agent_launch_count"] == 1 {
            ""
        } else {
            "es"
        },
        issue["origin"]["model"]
            .as_str()
            .filter(|model| issue["origin"]["kind"] == "codex" && !model.trim().is_empty())
            .map(|_| format!(" · created by Codex · {}", line(&issue["origin"]["model"])))
            .unwrap_or_default()
    );
    if issue["origin_error"]["message"].is_string() {
        println!(
            "  Origin unavailable: {}",
            line(&issue["origin_error"]["message"])
        );
    }
    if let Some(status) = issue["status"].as_object() {
        println!(
            "  Status [{}]: {}",
            line(&status["level"]),
            line(&status["comment"])
        );
    }
    if let Some(blockers) = issue["blocked_by"].as_array() {
        for blocker in blockers {
            println!(
                "  Blocked by: #{} {} [{}]",
                blocker["number"],
                line(&blocker["title"]),
                line(&blocker["state"])
            );
        }
    }
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
    if let Some(context) = issue["subtask_context"].as_object() {
        println!(
            "  Subtask {} of {} · {}",
            context["position"],
            context["total"],
            if context.get("scheduling").is_some_and(|v| v == "explicit") {
                "explicit dependencies"
            } else {
                "runs sequentially"
            }
        );
        for (key, label) in [("previous", "Previous"), ("next", "Next")] {
            let sibling = &context[key];
            if sibling["number"].is_number() {
                println!(
                    "  {label}: #{} {} [{}]",
                    sibling["number"],
                    line(&sibling["title"]),
                    line(&sibling["state"])
                );
            }
        }
        if context.get("scheduling").is_some_and(|v| v == "explicit") {
            println!(
                "  Read the parent requirements and declared prerequisite notes/PRs. Sibling order is context only; leave a handoff before completing your scope."
            );
        } else {
            println!(
                "  Read the parent and previous subtask with `hey-boss issue view NUMBER` for requirements, completion notes, and PRs. Leave a handoff before marking Ready (PR projects) or closing; later subtasks wait for this one."
            );
        }
    }
    if let Some(total) = issue["subtasks"]["total"].as_u64() {
        println!(
            "  Subtasks: {}/{} closed · {} open descendants",
            issue["subtasks"]["closed"], total, issue["subtasks"]["open_descendants"]
        );
    }
    if let Some(prs) = issue["pull_requests"].as_array() {
        for pr in prs {
            println!(
                "  PR [{}]: {}",
                pr["purpose"].as_str().unwrap_or("unspecified"),
                line(&pr["url"])
            );
        }
    }
}

fn worker_project() -> Option<String> {
    std::env::var("HEY_BOSS_ISSUE_PROJECT")
        .ok()
        .filter(|v| !v.is_empty())
}
