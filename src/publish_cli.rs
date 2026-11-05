use clap::{Args, Subcommand};
use hey_boss::{
    issues::{self, Result},
    pages,
};
use std::path::PathBuf;

#[derive(Args)]
#[command(about = "Publish pages across your fleet and through configured external providers.")]
pub struct Options {
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Save an HTML page or static site across your fleet; update linked providers automatically.
    #[command(
        after_help = "Works offline for private fleet copies. Reusing a name replaces that page and auto-syncs linked external providers. New pages stay private unless --provider is supplied. Dotfiles are excluded; symlinks are rejected. Site assets should use relative URLs."
    )]
    Publish {
        /// HTML file, or directory containing index.html (up to 256 MiB).
        source: PathBuf,
        #[arg(long)]
        name: String,
        /// Also publish through this configured provider, remembering its link.
        #[arg(long)]
        provider: Option<String>,
    },
    /// List configured external publishers and their audiences.
    Providers,
    /// Publish an existing page externally, or link and update an existing remote artifact.
    Share {
        name: String,
        #[arg(long)]
        provider: String,
        /// Existing remote URL or ID. Its content will be replaced with this page.
        #[arg(long)]
        link: Option<String>,
        /// Automatically upload on subsequent pages publish calls.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        auto_sync: bool,
    },
    /// Retry uploads to all saved external targets, preserving their URLs.
    Sync { name: String },
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
    /// Permanently delete a private page and reclaim its unshared files and sync payloads across the fleet.
    Delete { name: String },
}
pub fn run(options: &Options) -> Result<()> {
    let database = issues::database_path()?;
    let value = match &options.action {
        Action::Publish {
            source,
            name,
            provider,
        } => {
            let mut page = pages::publish(&database, source, name)?;
            if let Some(provider) = provider {
                page = pages::publishing::share(&database, name, provider, None, true)?;
            }
            check_sync(&page, true)?;
            let url = format!("http://hey-boss.test{}", page.path);
            if !options.json {
                println!(
                    "{url}\nPrivate · {} files · {} bytes\nSaved locally; fleet copies update when connected.",
                    page.files, page.size
                );
                print_publications(&page);
            }
            serde_json::json!({"ok":true,"private":page.publications.is_empty(),"url":url,"page":page,"replication":"Saved locally; fleet copies update when connected"})
        }
        Action::Providers => {
            let providers = pages::publishing::providers()?;
            if !options.json {
                println!("Config: {}", pages::publishing::config_path()?.display());
                for (name, provider) in &providers {
                    println!("{name}  {}", provider.audience);
                }
            }
            serde_json::json!({"ok":true,"providers":providers,"config":pages::publishing::config_path()?})
        }
        Action::Share {
            name,
            provider,
            link,
            auto_sync,
        } => {
            let page =
                pages::publishing::share(&database, name, provider, link.as_deref(), *auto_sync)?;
            if !options.json {
                print_publications(&page);
            }
            serde_json::json!({"ok":true,"page":page})
        }
        Action::Sync { name } => {
            let page = pages::publishing::sync(&database, name)?;
            check_sync(&page, false)?;
            if !options.json {
                print_publications(&page);
            }
            serde_json::json!({"ok":true,"page":page})
        }
        Action::List { offset } => {
            let entries = pages::list(&database, *offset)?;
            if !options.json {
                for page in &entries {
                    println!(
                        "{}  {} files  {} bytes stored / {} bytes original  {}{}",
                        page.name,
                        page.files,
                        page.stored_size,
                        page.size,
                        page.path,
                        if page.syncing { "  syncing" } else { "" }
                    );
                    print_publications(page);
                }
                if entries.is_empty() {
                    println!("No published pages");
                }
            }
            serde_json::json!({"ok":true,"pages":entries,"next_offset":(entries.len()==100).then_some(offset+100)})
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
                println!(
                    "Deleted {name}. Unshared files and sync payloads are reclaimed as machines sync."
                );
            }
            serde_json::json!({"ok":true,"deleted":name})
        }
    };
    if options.json {
        println!("{value}");
    }
    Ok(())
}

fn print_publications(page: &pages::Page) {
    for publication in &page.publications {
        println!(
            "{} · {} · {} · {}{}",
            publication.provider,
            publication.audience,
            if publication.synced {
                "Synced"
            } else {
                "Not synced"
            },
            publication.url,
            publication
                .error
                .as_ref()
                .map(|e| format!("\n  {e}"))
                .unwrap_or_default()
        );
    }
}

fn check_sync(page: &pages::Page, automatic: bool) -> Result<()> {
    if let Some(p) = page
        .publications
        .iter()
        .find(|p| p.error.is_some() && (!automatic || p.auto_sync))
    {
        return Err(issues::Error::new(
            "publish_failed",
            format!(
                "Page {} is saved locally; {} is not synced. {} {}. Retry with pages sync {}",
                page.name,
                p.provider,
                p.url,
                p.error.as_deref().unwrap(),
                page.name
            ),
        ));
    }
    Ok(())
}
