use clap::Parser;
use hey_boss_worker_tui::{backend::Client, runtime};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Parser)]
#[command(about = "Live hey-boss worker dashboard. Exiting leaves workers running.")]
struct Options {
    /// hey-boss executable supplying worker status JSON.
    #[arg(long, default_value = "hey-boss")]
    binary: PathBuf,
    /// Authoritative SSH queue host, forwarded to hey-boss.
    #[arg(long)]
    host: Option<String>,
    /// Checkout directory used to resolve the queue project.
    #[arg(long)]
    directory: Option<PathBuf>,
    /// Initially selected worker.
    #[arg(long)]
    id: Option<String>,
    /// Show recent attempts on startup.
    #[arg(long)]
    history: bool,
    /// Show all configured workers grouped into project tabs.
    #[arg(long, conflicts_with_all = ["host", "directory", "id"])]
    projects: bool,
}

fn main() {
    let options = Options::parse();
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let result = ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })
        .and_then(|()| {
            runtime::run(
                runtime::Options {
                    client: Client {
                        binary: options.binary,
                        host: options.host,
                        directory: options.directory,
                        timeout: Duration::from_secs(10),
                    },
                    id: options.id,
                    history: options.history,
                    owned_worker: false,
                    project_tabs: options.projects,
                },
                cancelled,
            )
        });
    if let Err(error) = result {
        eprintln!("hey-boss-worker-tui: {error}");
        std::process::exit(1);
    }
}
