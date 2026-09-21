use clap::{Args, Subcommand};
use hey_boss::{
    artifacts::Operation,
    issues::{self, Error, Result, Store},
};
use serde_json::Value;
use std::{io::Read, path::PathBuf};

#[derive(Args)]
pub struct Options {
    #[arg(long, global = true)]
    project: Option<String>,
    #[arg(long, global = true)]
    host: Option<String>,
    #[arg(long, global = true)]
    agent: Option<String>,
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true)]
    request_id: Option<String>,
    #[command(subcommand)]
    action: Action,
}
#[derive(Args)]
struct Text {
    /// Markdown text, or '-' for stdin.
    #[arg(long, conflicts_with = "file")]
    body: Option<String>,
    /// Import a UTF-8 Markdown file, or '-' for stdin.
    #[arg(long, conflicts_with = "body")]
    file: Option<PathBuf>,
}
impl Text {
    fn read(&self) -> Result<Option<String>> {
        let mut bytes = Vec::new();
        if self.body.as_deref() == Some("-")
            || self.file.as_deref() == Some(std::path::Path::new("-"))
        {
            std::io::stdin()
                .take(issues::BODY_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)?;
        } else if let Some(path) = &self.file {
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)?;
            if !file.metadata()?.is_file() {
                return Err(Error::invalid("Markdown file must be a regular file"));
            }
            file.take(issues::BODY_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)?;
        } else {
            return Ok(self.body.clone());
        }
        if bytes.len() > issues::BODY_LIMIT {
            return Err(Error::invalid("Markdown exceeds 1 MiB"));
        }
        Ok(Some(
            String::from_utf8(bytes).map_err(|_| Error::invalid("Markdown must be UTF-8"))?,
        ))
    }
}
#[derive(Args)]
struct Target {
    #[arg(long, conflicts_with = "node")]
    issue: Option<i64>,
    #[arg(long, conflicts_with = "issue")]
    node: Option<String>,
}
#[derive(Subcommand)]
enum Action {
    List {
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        archived: bool,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    View {
        id: String,
    },
    /// Export the saved Markdown to stdout.
    Export {
        id: String,
    },
    Create {
        #[arg(long)]
        title: String,
        #[command(flatten)]
        text: Text,
        #[command(flatten)]
        target: Target,
    },
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        text: Text,
        #[arg(long)]
        if_version: i64,
    },
    Archive {
        id: String,
        #[arg(long)]
        if_version: i64,
    },
    Restore {
        id: String,
        #[arg(long)]
        if_version: i64,
    },
    Comment {
        id: String,
        #[arg(long)]
        body: String,
        #[arg(long)]
        quote: Option<String>,
        #[arg(long)]
        parent: Option<i64>,
    },
    Resolve {
        id: String,
        comment_id: i64,
        #[arg(long)]
        reopen: bool,
    },
    Link {
        id: String,
        #[command(flatten)]
        target: Target,
    },
    Unlink {
        id: String,
        #[command(flatten)]
        target: Target,
    },
    Links {
        #[command(flatten)]
        target: Target,
    },
}
pub fn run(options: &Options) -> Result<()> {
    let op = match &options.action {
        Action::List {
            query,
            archived,
            offset,
        } => Operation::List {
            query: query.clone(),
            archived: *archived,
            offset: *offset,
        },
        Action::View { id } | Action::Export { id } => Operation::View { id: id.clone() },
        Action::Create {
            title,
            text,
            target,
        } => Operation::Create {
            title: title.clone(),
            body: text.read()?.unwrap_or_default(),
            issue: target.issue,
            node: target.node.clone(),
        },
        Action::Edit {
            id,
            title,
            text,
            if_version,
        } => Operation::Edit {
            id: id.clone(),
            title: title.clone(),
            body: text.read()?,
            if_version: *if_version,
        },
        Action::Archive { id, if_version } | Action::Restore { id, if_version } => {
            Operation::Archive {
                id: id.clone(),
                archived: matches!(options.action, Action::Archive { .. }),
                if_version: *if_version,
            }
        }
        Action::Comment {
            id,
            body,
            quote,
            parent,
        } => Operation::Comment {
            id: id.clone(),
            body: body.clone(),
            quote: quote.clone(),
            prefix: None,
            suffix: None,
            parent: *parent,
        },
        Action::Resolve {
            id,
            comment_id,
            reopen,
        } => Operation::Resolve {
            id: id.clone(),
            comment_id: *comment_id,
            resolved: !*reopen,
        },
        Action::Link { id, target } => Operation::Link {
            id: id.clone(),
            issue: target.issue,
            node: target.node.clone(),
        },
        Action::Unlink { id, target } => Operation::Unlink {
            id: id.clone(),
            issue: target.issue,
            node: target.node.clone(),
        },
        Action::Links { target } => Operation::Links {
            issue: target.issue,
            node: target.node.clone(),
        },
    };
    op.validate()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let machine = issues::identity::machine()?;
    let mut actor = if op.writes() {
        Some(issues::identity::resolve(
            options.agent.as_deref(),
            &machine,
            &cwd,
        )?)
    } else {
        None
    };
    if matches!(op, Operation::Create { .. })
        && let Some(actor) = actor.as_mut()
    {
        issues::identity::creation_context(actor);
    }
    let request = issues::Request {
        version: 1,
        project: issues::identity::project(&cwd, &machine)?,
        project_override: options
            .project
            .clone()
            .or_else(|| std::env::var("HEY_BOSS_ISSUE_PROJECT").ok()),
        actor,
        operation: issues::Operation::Artifact { operation: op },
        request_id: options.request_id.clone(),
    };
    let value = match options.host.clone().or_else(|| {
        std::env::var("HEY_BOSS_ISSUE_HOST")
            .ok()
            .filter(|h| !h.is_empty())
    }) {
        Some(host) => issues::remote::call(&host, &request)?,
        None => Store::open(&issues::database_path()?)?.execute(&request)?,
    };
    if matches!(options.action, Action::Export { .. }) {
        print!("{}", value["artifact"]["body"].as_str().unwrap());
    } else if options.json {
        println!("{}", serde_json::to_string(&value)?);
    } else if let Some(doc) = value.get("artifact") {
        println!(
            "{} · {} · revision {}{}\n\n{}",
            doc["id"].as_str().unwrap(),
            doc["title"].as_str().unwrap(),
            doc["version"],
            if doc["archived"] == true {
                " · archived"
            } else {
                ""
            },
            doc["body"].as_str().unwrap()
        );
    } else {
        for doc in value["artifacts"].as_array().unwrap() {
            println!(
                "{}  {}{}",
                doc["id"].as_str().unwrap(),
                doc["title"].as_str().unwrap(),
                if doc["archived"] == true {
                    " (archived)"
                } else {
                    ""
                }
            );
        }
        if value["more"] == Value::Bool(true) {
            println!("More results: use --offset for the next page");
        }
    }
    Ok(())
}
