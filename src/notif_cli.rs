//! Notification commands shared by the notif group and hidden root compatibility names.
use crate::{Metadata, Output, secret_cli};
use clap::Subcommand;
use hey_boss::Request;

#[derive(Subcommand)]
pub(super) enum Action {
    #[command(
        about = "Post a short update with a Markdown preview",
        after_help = r#"Keep SUMMARY to one short sentence. MARKDOWN is inline text; use --file PATH to load a Markdown document instead.
Headings, emphasis, lists, code, and http/https/file links are supported.
Read update opens the document and dismisses the card. History is kept. Returns a Task ID immediately.

Example (zsh/bash):
  hey-boss notif update --project Atlas --title 'Analysis ready' 'Three builds reviewed' $'# Results\n\n**Report complete.**\n\n[Read more](https://example.com/report)'
  hey-boss notif hide '<task_id>'"#
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
  hey-boss notif alert --project Atlas --title Build 'Checks passed' --autoclose 10
  hey-boss notif alert --project Atlas --title Report '**Analysis published**' --link-url https://example.com/report --link-label 'Open report'"#
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
  hey-boss notif ask --project Atlas --title 'Report name' 'What should the report be called?' '' --sync
  hey-boss notif ask --project Atlas --title Format 'Which format?' 'Choose the output.' --option Markdown --option PDF --async
  hey-boss notif wait '<task_id>'"#
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
        after_help = "Equivalent to ask --async without options. Returns a Task ID, not an answer.\n\nExample:\n  hey-boss notif prompt --project Atlas --title Name 'Name the report?' ''\n  hey-boss notif wait '<task_id>'"
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
        after_help = "Returns a Task ID immediately. Use --option repeatedly to replace Yes/No choices.\n\nExample:\n  hey-boss notif approval --project Atlas --title Publish 'Publish the report?' 'The draft is ready.'\n  hey-boss notif wait '<task_id>'"
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
        after_help = "Use when a notification becomes obsolete or is superseded. Does not hide questions\nor close an already open document preview.\n\nExample:\n  hey-boss notif hide '<task_id>'"
    )]
    Hide {
        task_id: String,
        #[command(flatten)]
        output: Output,
    },
    #[command(
        about = "Check a task",
        after_help = "pending = queued or displayed; ok = completed; cancelled = question dismissed by Close all.\nAn answered question includes result; cancellation has no result and is not approval.\nUse wait for a question answer instead of polling status.\n\nExample:\n  hey-boss notif status '<task_id>'"
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
        after_help = "Questions only; not alerts or updates. Returns immediately if answered or cancelled.\nOtherwise waits for an answer or Close all. Cancellation returns status cancelled\nwithout a result; it is not an answer or approval. Ctrl+C stops this caller and\nleaves the question available for a later wait.\n\nExample:\n  hey-boss notif wait '<task_id>'"
    )]
    Wait {
        task_id: String,
        #[command(flatten)]
        output: Output,
    },
    /// Open the web Inbox; --json lists notices without opening a browser.
    Inbox {
        #[arg(long)]
        json: bool,
    },
    /// Request one or two secrets without history or agent-visible output.
    Secret(secret_cli::Options),
}

impl Action {
    pub(super) fn into_request_with_project(
        self,
        resolve_project: impl FnOnce(Option<&str>) -> std::io::Result<hey_boss::issues::Project>,
    ) -> std::io::Result<(Request, Output)> {
        let invalid =
            |message: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
        let mut issue = None;
        let (project, title, severity, icon, icon_path) = match &self {
            Action::Update { metadata, .. }
            | Action::Alert { metadata, .. }
            | Action::Ask { metadata, .. }
            | Action::Prompt { metadata, .. }
            | Action::Approval { metadata, .. } => {
                if metadata
                    .project
                    .as_deref()
                    .is_some_and(|p| p.trim().is_empty())
                    || metadata.title.trim().is_empty()
                {
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
                    Some(resolve_project(metadata.project.as_deref())?.name),
                    Some(metadata.title.clone()),
                    metadata.severity,
                    metadata.icon.clone(),
                    metadata.icon_file.clone(),
                )
            }
            Action::Secret(_) | Action::Inbox { .. } => unreachable!(),
            Action::Hide { .. } | Action::Status { .. } | Action::Wait { .. } => {
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
        let output = match self {
            Action::Secret(_) | Action::Inbox { .. } => unreachable!(),
            Action::Update {
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
            Action::Alert {
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
            Action::Ask {
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
            Action::Prompt {
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
            Action::Approval {
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
            Action::Hide { task_id, output } => {
                request.command = "hide".into();
                request.task_id = Some(task_id);
                output
            }
            Action::Status {
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
            Action::Wait { task_id, output } => {
                request.command = "wait".into();
                request.task_id = Some(task_id);
                output
            }
        };
        Ok((request, output))
    }
}
