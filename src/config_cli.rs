use clap::{Args, Subcommand};
use hey_boss::issues::{self, Operation, Request, Result, project_config};
use serde_json::json;

#[derive(Args)]
pub struct Options {
    #[arg(long, global = true)]
    project: Option<String>,
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Create or reconcile .hey-boss/config.yaml and prompts/*.md; enable automatic sync.
    Sync,
    /// Show configuration activity, newest first.
    Log {
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// Show what a revision changed and what undo would change now.
    Diff { revision: String },
    /// Reverse the fields changed by a revision, keeping unrelated later edits.
    Undo { revision: String },
    /// Restore the complete configuration saved by a revision; retain all history.
    Restore { revision: String },
}
pub fn run(options: &Options) -> Result<()> {
    let cwd = std::env::current_dir()?.canonicalize()?;
    let machine = issues::identity::machine()?;
    let mut actor = issues::identity::resolve_inspection(None, &machine, &cwd)?;
    actor.source = "config-cli".into();
    let mut request = Request {
        version: 1,
        project: issues::identity::project(&cwd, &machine)?,
        project_override: options.project.clone(),
        actor: Some(actor.clone()),
        request_id: None,
        operation: Operation::ProjectSettings,
    };
    let settings = crate::cli_request::execute(&request, None, false)?;
    request.project = serde_json::from_value(settings["project"].clone())?;
    let value = if matches!(options.action, Action::Sync) {
        issues::config_files::sync(&issues::database_path()?, &cwd, &request.project)?
    } else {
        request.operation = Operation::ProjectConfig {
            operation: match &options.action {
                Action::Log { offset } => project_config::Operation::History { offset: *offset },
                Action::Diff { revision } => project_config::Operation::View {
                    revision: revision.clone(),
                },
                Action::Undo { revision } => project_config::Operation::Undo {
                    revision: revision.clone(),
                    if_digest: None,
                },
                Action::Restore { revision } => project_config::Operation::Restore {
                    revision: revision.clone(),
                    if_digest: None,
                },
                Action::Sync => unreachable!(),
            },
        };
        crate::cli_request::execute(&request, None, false)?
    };
    if options.json {
        println!("{}", serde_json::to_string(&value)?);
    } else if let Some(directory) = value["directory"].as_str() {
        println!("Synced {directory}\nAutomatic sync is active while the fleet service runs.");
    } else if let Some(revisions) = value["revisions"].as_array() {
        if revisions.is_empty() {
            println!("No configuration changes recorded yet.");
        }
        for revision in revisions {
            println!(
                "{}  {}  {} on {}\n  {}",
                revision["id"].as_str().unwrap(),
                revision["action"].as_str().unwrap(),
                revision["actor"]["id"].as_str().unwrap_or("unknown"),
                revision["actor"]["host"].as_str().unwrap_or("unknown"),
                revision["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|f| f.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if !value["next_offset"].is_null() {
            println!(
                "More: hey-boss config log --offset {}",
                value["next_offset"]
            );
        }
    } else if value.get("changes").is_some() {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"changes":value["changes"],"undo_changes":value["undo_changes"]})
            )?
        );
    } else {
        println!("Configuration saved. History retained.");
    }
    Ok(())
}
