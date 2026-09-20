use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{Args, Subcommand};
use hey_boss::{
    attachments::{self, Kind, Operation, Target},
    issues::{self, Error, Result, Store},
};
use std::path::PathBuf;

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
#[group(required = true, multiple = false)]
struct Resource {
    #[arg(long)]
    issue: Option<i64>,
    #[arg(long)]
    node: Option<String>,
    #[arg(long)]
    artifact: Option<String>,
}
impl Resource {
    fn target(&self) -> Target {
        if let Some(n) = self.issue {
            Target {
                kind: Kind::Issue,
                id: n.to_string(),
            }
        } else if let Some(id) = &self.node {
            Target {
                kind: Kind::Node,
                id: id.clone(),
            }
        } else {
            Target {
                kind: Kind::Artifact,
                id: self.artifact.clone().unwrap(),
            }
        }
    }
}
#[derive(Subcommand)]
enum Action {
    /// Copy a file into the authoritative attachment store (at most 10 MiB).
    Upload {
        file: PathBuf,
        #[command(flatten)]
        resource: Resource,
    },
    List {
        #[command(flatten)]
        resource: Resource,
    },
    /// Download a local copy; defaults to a private temporary folder. Never overwrites.
    Download {
        id: String,
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
    Remove {
        id: String,
    },
}
pub fn run(options: &Options) -> Result<()> {
    let operation = match &options.action {
        Action::List { resource } => Operation::List {
            target: resource.target(),
        },
        Action::Upload { file, resource } => Operation::Upload {
            target: resource.target(),
            name: file
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| Error::invalid("File needs a UTF-8 filename"))?
                .into(),
            data: STANDARD.encode(attachments::read_file(file)?),
        },
        Action::Download { id, .. } => Operation::Download { id: id.clone() },
        Action::Remove { id } => Operation::Remove { id: id.clone() },
    };
    operation.validate()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let machine = issues::identity::machine()?;
    let request = issues::Request {
        version: 1,
        project: issues::identity::project(&cwd, &machine)?,
        project_override: options
            .project
            .clone()
            .or_else(|| std::env::var("HEY_BOSS_ISSUE_PROJECT").ok()),
        actor: if operation.writes() {
            Some(issues::identity::resolve(
                options.agent.as_deref(),
                &machine,
                &cwd,
            )?)
        } else {
            None
        },
        operation: issues::Operation::Attachment { operation },
        request_id: options.request_id.clone(),
    };
    let mut value = match options.host.clone().or_else(|| {
        std::env::var("HEY_BOSS_ISSUE_HOST")
            .ok()
            .filter(|s| !s.is_empty())
    }) {
        Some(host) => issues::remote::call(&host, &request)?,
        None => Store::open(&issues::database_path()?)?.execute(&request)?,
    };
    if let Action::Download { output, .. } = &options.action {
        let path = attachments::materialize(&value, output.as_deref())?;
        value.as_object_mut().unwrap().remove("data");
        value["path"] = serde_json::json!(path);
        if !options.json {
            println!("{}", path.display());
        }
    } else if !options.json {
        if let Some(entries) = value["attachments"].as_array() {
            for file in entries {
                println!(
                    "{}  {}  {} bytes",
                    file["id"].as_str().unwrap(),
                    file["name"].as_str().unwrap(),
                    file["size"]
                );
            }
            if entries.is_empty() {
                println!("No attachments");
            }
        } else {
            let file = &value["attachment"];
            println!(
                "{}  {}  {} bytes",
                file["id"].as_str().unwrap(),
                file["name"].as_str().unwrap(),
                file["size"]
            );
        }
    }
    if options.json {
        println!("{}", serde_json::to_string(&value)?);
    }
    Ok(())
}
