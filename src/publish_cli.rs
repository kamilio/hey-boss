use clap::{Args, Subcommand};
use hey_boss::{
    issues::{self, Result},
    pages,
};
use std::path::PathBuf;

#[derive(Args)]
#[command(
    about = "Publish a private HTML page or static site to your hey-boss fleet. Nothing is uploaded to a public service.",
    after_help = "Works offline; files replicate to every fleet machine when connected. Reusing a name replaces that page. Dotfiles are excluded; symlinks are rejected. Site assets should use relative URLs."
)]
pub struct Publish {
    /// HTML file, or directory containing index.html (up to 256 MiB).
    source: PathBuf,
    #[arg(long)]
    name: String,
    #[arg(long)]
    pub json: bool,
}
#[derive(Args)]
#[command(about = "List, download, or delete private pages replicated within your hey-boss fleet.")]
pub struct Options {
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    /// List private pages on this machine, including pages still syncing.
    List {
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Restore a private published site into a new directory; never overwrite files.
    Download {
        name: String,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Remove a page from private web access on every machine as deletion syncs.
    Delete { name: String },
}
pub fn publish(options: &Publish) -> Result<()> {
    let page = pages::publish(&issues::database_path()?, &options.source, &options.name)?;
    let url = format!("http://hey-boss.test{}", page.path);
    if options.json {
        println!(
            "{}",
            serde_json::json!({"ok":true,"private":true,"url":url,"page":page,"replication":"Saved locally; fleet copies update when connected"})
        );
    } else {
        println!(
            "{url}\nPrivate · {} files · {} bytes\nSaved locally; fleet copies update when connected.",
            page.files, page.size
        );
    }
    Ok(())
}
pub fn run(options: &Options) -> Result<()> {
    let database = issues::database_path()?;
    let value = match &options.action {
        Action::List { offset } => {
            let entries = pages::list(&database, *offset)?;
            if !options.json {
                for page in &entries {
                    println!(
                        "{}  {} files  {}{}",
                        page.name,
                        page.files,
                        page.path,
                        if page.syncing { "  syncing" } else { "" }
                    );
                }
                if entries.is_empty() {
                    println!("No published pages");
                }
            }
            serde_json::json!({"ok":true,"private":true,"pages":entries,"next_offset":(entries.len()==100).then_some(offset+100)})
        }
        Action::Download { name, output } => {
            pages::download(&database, name, output)?;
            if !options.json {
                println!("{}", output.display());
            }
            serde_json::json!({"ok":true,"path":output})
        }
        Action::Delete { name } => {
            pages::delete(&database, name)?;
            if !options.json {
                println!("Deleted {name}. Other machines apply deletion when they sync.");
            }
            serde_json::json!({"ok":true,"deleted":name})
        }
    };
    if options.json {
        println!("{value}");
    }
    Ok(())
}
