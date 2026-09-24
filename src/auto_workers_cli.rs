use clap::{Args, Subcommand};
use hey_boss::{
    fleet, issues,
    worker_tui::{backend::Client, runtime},
};
use std::{
    io::IsTerminal,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

#[derive(Args)]
#[command(
    about = "Run this machine's saved workers and open project tabs. Repeated launches reuse the same workers."
)]
pub struct Options {
    /// Print one JSON snapshot instead of opening the dashboard.
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    action: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    /// Add and start a saved worker. Repeat --directory to share slots across projects.
    Add {
        /// Reuse an ID when retrying the same creation request.
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        name: String,
        #[arg(long, default_value_t = 1)]
        concurrency: u32,
        #[arg(long = "directory", short = 'C', required = true)]
        directories: Vec<std::path::PathBuf>,
    },
    /// Stop new pickup, then remove the worker once its agents and Chief finish.
    Remove { id: String },
    /// Show this machine's saved workers and the configuration file to edit.
    Config,
    /// Observe configured workers without starting or changing them.
    Status,
}
pub fn run(options: &Options) -> issues::Result<()> {
    if let Some(Action::Add {
        id,
        name,
        concurrency,
        directories,
    }) = &options.action
    {
        let machine = issues::identity::machine()?;
        let mut settings = issues::worker::Settings {
            name: name.clone(),
            concurrency: *concurrency,
            enabled: true,
            ..Default::default()
        };
        for path in directories {
            let path = path.canonicalize()?;
            let project = issues::identity::project(&path, &machine)?;
            if settings
                .directories
                .insert(project.id.clone(), path.to_string_lossy().into_owned())
                .is_some()
            {
                return Err(issues::Error::invalid(
                    "Use a separate worker for each checkout of the same project",
                ));
            }
            settings.projects.push(project.id);
        }
        if settings.directories.len() == 1 {
            settings.directory = settings.directories.pop_first().unwrap().1;
        }
        println!("{}", fleet::add_auto_worker(&settings, id.as_deref())?);
        return Ok(());
    }
    if let Some(Action::Remove { id }) = &options.action {
        println!("{}", fleet::remove_auto_worker(id)?);
        return Ok(());
    }
    let config_only = matches!(options.action, Some(Action::Config));
    let snapshot = fleet::auto_workers(options.action.is_none(), config_only)?;
    if options.json
        || config_only
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
    {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        return Ok(());
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    issues::worker::install_signals(cancelled.clone())?;
    runtime::run(
        runtime::Options {
            client: Client {
                binary: std::env::current_exe()?,
                host: None,
                directory: None,
                timeout: Duration::from_secs(10),
            },
            id: None,
            history: false,
            owned_worker: false,
            project_tabs: true,
        },
        cancelled,
    )
    .map_err(|e| issues::Error::new("worker_error", e.to_string()))?;
    Ok(())
}
