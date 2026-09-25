use clap::Parser;
use hey_harvester::cli::{Action, run, run_remote};

#[derive(Parser)]
#[command(version, about = "Machine maintenance — Tab switches between machines")]
struct Options {
    /// Select a machine from the existing Hey Boss SSH inventory.
    #[arg(long, global = true)]
    host: Option<String>,
    #[command(subcommand)]
    action: Option<Action>,
}
fn main() {
    let options = Options::parse();
    let action = options.action.unwrap_or(Action::Open);
    let result = match options.host {
        Some(host) => run_remote(&host, &action),
        None => run(&action),
    };
    if let Err(error) = result {
        eprintln!("hey-harvester: {error}");
        std::process::exit(1);
    }
}
