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
    about = "Manage this machine's saved workers. Use run to start or resume, status for one snapshot, and watch for a live view.",
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Options {
    /// Print JSON instead of text (one record per refresh with watch).
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    action: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    /// Start or resume saved workers, print their status, and leave them running.
    Run,
    /// Watch saved workers without starting or resuming them.
    Watch {
        /// Stop after this many snapshots; omit for the live terminal dashboard.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        count: Option<u32>,
    },
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
    #[command(visible_alias = "list")]
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
        let value = fleet::add_auto_worker(&settings, id.as_deref())?;
        print_value(&value, options.json, "Started")?;
        return Ok(());
    }
    if let Some(Action::Remove { id }) = &options.action {
        let value = fleet::remove_auto_worker(id)?;
        print_value(&value, options.json, "Removal queued for")?;
        return Ok(());
    }
    if !matches!(options.action, Some(Action::Watch { .. })) {
        let snapshot = fleet::auto_workers(
            matches!(options.action, Some(Action::Run)),
            matches!(options.action, Some(Action::Config)),
        )?;
        return print_value(&snapshot, options.json, "");
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    issues::worker::install_signals(cancelled.clone())?;
    let Some(Action::Watch { count }) = options.action else {
        unreachable!()
    };
    if options.json
        || count.is_some()
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
    {
        use std::sync::atomic::Ordering;
        let mut emitted = 0;
        while !cancelled.load(Ordering::Relaxed) {
            let snapshot = fleet::auto_workers(false, false)?;
            print_value(&snapshot, options.json, "")?;
            emitted += 1;
            if count.is_some_and(|limit| emitted >= limit) {
                break;
            }
            for _ in 0..20 {
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        return Ok(());
    }
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

fn print_value(value: &serde_json::Value, json: bool, verb: &str) -> issues::Result<()> {
    use std::io::Write;
    let mut output = std::io::stdout().lock();
    if json {
        writeln!(output, "{value}")?;
    } else {
        write!(output, "{}", text(value, verb))?;
    }
    output.flush()?;
    Ok(())
}

fn text(value: &serde_json::Value, verb: &str) -> String {
    use hey_boss::worker_tui::text;
    use std::fmt::Write;
    if let Some(id) = value["worker_id"].as_str() {
        return format!("{verb} worker {id}.\n");
    }
    let mut output = format!("Saved workers · {}\n", text(&value["machine"]));
    for key in ["source", "fleet_source"] {
        if let Some(path) = value[key].as_str() {
            let _ = writeln!(output, "Configuration: {path}");
        }
    }
    let workers = value["workers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if workers.is_empty() {
        output.push_str("No saved workers.\n");
    }
    for worker in workers {
        let config = &worker["config"];
        let _ = writeln!(
            output,
            "{} · {} · {} · {} slots",
            text(&config["name"]),
            text(&worker["id"]),
            text(&worker["intent"]),
            config["concurrency"]
        );
        if let Some(path) = config["directory"].as_str().filter(|p| !p.is_empty()) {
            let _ = writeln!(output, "  {path}");
        }
        if let Some(paths) = config["directories"].as_object() {
            for (project, path) in paths {
                let _ = writeln!(output, "  {project}: {}", text(path));
            }
        }
    }
    output
}
