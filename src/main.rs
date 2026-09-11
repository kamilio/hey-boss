use clap::{Args, Parser, Subcommand};
use hey_boss::{Client, Request};
use std::os::fd::AsRawFd;

fn initialize(executable: &std::path::Path) {
    let pending = executable.with_file_name("hey-boss.setup");
    if !pending.exists() {
        return;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(executable.with_file_name("hey-boss.setup.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    if pending.exists() {
        let configuration = std::fs::read_to_string(&pending).unwrap();
        let arguments: Vec<_> = configuration.lines().collect();
        assert_eq!(arguments.len(), 5);
        assert!(
            std::process::Command::new(arguments[0])
                .args(&arguments[1..])
                .status()
                .unwrap()
                .success()
        );
        std::fs::remove_file(pending).unwrap();
    }
}

#[derive(Parser)]
#[command(
    name = "hey-boss",
    version,
    about = "Native Mac project updates, notifications, and questions",
    after_help = r#"Use --project and --title when creating an item. Keep summaries short;
put the details in Markdown. Ask only when requested or an answer is essential.

Questions: --sync waits; --async returns a Task ID. Use wait for the answer.
Save Task IDs and hide cards when they become obsolete.

Examples:
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
    #[arg(long, help = "Short project name shown in the heading; required")]
    project: String,
    #[arg(
        long,
        help = "Short title describing this update or question; required"
    )]
    title: String,
}

#[derive(Subcommand)]
enum Command {
    #[command(
        about = "Post a short update with a Markdown preview",
        after_help = r#"Keep SUMMARY to one short sentence. MARKDOWN is the full report text, not a file path.
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
        content: String,
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
        after_help = "pending = queued or displayed; ok = completed. An answered question includes result.\nUse wait for a question answer instead of polling status.\n\nExample:\n  hey-boss status '<task_id>'"
    )]
    Status {
        task_id: String,
        #[command(flatten)]
        output: Output,
    },
    #[command(
        about = "Wait for an answer",
        after_help = "Questions only; not alerts or updates. Returns immediately if already answered.\nOtherwise waits until answered. Ctrl+C stops this caller; the question\nremains available for a later wait. Returns the answer and Task ID.\n\nExample:\n  hey-boss wait '<task_id>'"
    )]
    Wait {
        task_id: String,
        #[command(flatten)]
        output: Output,
    },
}

impl Cli {
    fn into_request(self) -> (Request, Output) {
        let (project, title) = match &self.command {
            Command::Update { metadata, .. }
            | Command::Alert { metadata, .. }
            | Command::Ask { metadata, .. }
            | Command::Prompt { metadata, .. }
            | Command::Approval { metadata, .. } => {
                assert!(
                    !metadata.project.trim().is_empty() && !metadata.title.trim().is_empty(),
                    "creation project and title must not be blank"
                );
                (Some(metadata.project.clone()), Some(metadata.title.clone()))
            }
            Command::Hide { .. } | Command::Status { .. } | Command::Wait { .. } => (None, None),
        };
        let mut request = Request {
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
            origin: None,
        };
        let output = match self.command {
            Command::Update {
                summary,
                content,
                output,
                ..
            } => {
                request.command = "update".into();
                request.description = Some(summary);
                request.question = Some(content);
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
                if let Some(seconds) = autoclose {
                    assert!(seconds.is_finite() && seconds > 0.0);
                }
                if let Some(url) = &link_url {
                    assert!(
                        ["https://", "http://", "file://"]
                            .iter()
                            .any(|prefix| url.starts_with(prefix))
                    );
                    assert!(!link_label.as_ref().unwrap().trim().is_empty());
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
            Command::Status { task_id, output } => {
                request.command = "status".into();
                request.task_id = Some(task_id);
                output
            }
            Command::Wait { task_id, output } => {
                request.command = "wait".into();
                request.task_id = Some(task_id);
                output
            }
        };
        (request, output)
    }
}

fn main() {
    let (request, output) = Cli::parse().into_request();
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    initialize(&executable);
    let config = executable.with_file_name("hey-boss.state");
    let state = std::fs::read_to_string(config).unwrap();
    let result = Client::new(std::path::Path::new(&state).join("daemon.sock")).send(&request);
    if output.json {
        println!("{}", serde_json::to_string(&result).unwrap());
    } else {
        println!("Task ID: {}", result.task_id);
        if let Some(status) = result.status {
            println!("Status: {status}");
        }
        if let Some(answer) = result.result {
            println!("Result: {answer}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    initialize(&executable);
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        initialize(&root.join("hey-boss"));
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
            let (request, output) = Cli::try_parse_from(input).unwrap().into_request();
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
                assert!(std::panic::catch_unwind(|| cli.into_request()).is_err());
            }
        }
    }

    #[test]
    fn existing_task_commands_do_not_require_creation_metadata() {
        for command in ["status", "hide", "wait"] {
            let (request, _) = Cli::try_parse_from(["hey-boss", command, "task-1"])
                .unwrap()
                .into_request();
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
            assert_eq!(content, "# Report\n\n**Done**");
        }
    }
}
