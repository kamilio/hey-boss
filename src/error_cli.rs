use clap::{Args, Subcommand};
use hey_boss::{
    error_inbox::{Event, Inbox, now},
    issues::{self, Result},
};
use std::io::Read;
#[derive(Args)]
pub struct Options {
    /// Full project ID. Error collection is local to this machine.
    #[arg(long)]
    project: String,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Ranked by observed occurrences in the last 24 hours.
    List,
    /// Read a JSON array of normalized events from stdin (maximum 500).
    Ingest,
    /// Collection is off until explicitly enabled for a project.
    Enable {
        #[arg(action=clap::ArgAction::Set)]
        enabled: bool,
    },
    Show {
        id: String,
    },
}
pub fn run(options: &Options) -> Result<()> {
    let mut inbox = Inbox::open(&issues::database_path()?)?;
    let value = match &options.action {
        Action::List => inbox.list(&options.project, now())?,
        Action::Enable { enabled } => inbox.enable(&options.project, *enabled)?,
        Action::Show { id } => inbox.detail(&options.project, id, now())?,
        Action::Ingest => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(issues::Error::invalid("Batch exceeds 16 MiB"));
            }
            let events: Vec<Event> = serde_json::from_slice(&bytes)?;
            inbox.ingest(&options.project, &events, now())?
        }
    };
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}
