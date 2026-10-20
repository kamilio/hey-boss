use clap::{Args, Subcommand};
use hey_boss_secrets::{Error, MAX_SECRET, Result, Secret, backend};
use std::{
    io::{IsTerminal, Read, Write},
    path::PathBuf,
};

#[derive(Args)]
pub struct Options {
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Initialize local storage and install the Python/JavaScript SDK sources.
    Install,
    /// Create this machine's local identity. Linux creates a GPG key if needed.
    Init {
        #[arg(long)]
        label: Option<String>,
    },
    /// Print only this machine's signed public enrollment card.
    Identity,
    /// Exchange public enrollment cards over an already trusted SSH connection.
    Enroll { host: String },
    /// Pin a public enrollment card, after verifying its source (for example SSH).
    Trust {
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Metadata only; never decrypts secret values.
    List,
    /// Read bytes from stdin or a file; print only resulting metadata.
    Put {
        name: String,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long,default_value="value",value_parser=["value","bundle","blob","cookies"])]
        kind: String,
        #[arg(long)]
        expected: Option<String>,
    },
    /// Export to a private file, or explicitly to a pipe with --stdout.
    Get {
        name: String,
        #[arg(long, required_unless_present = "stdout", conflicts_with = "stdout")]
        file: Option<PathBuf>,
        #[arg(long)]
        stdout: bool,
    },
    /// Allow an enrolled machine to read and write this record.
    Grant { name: String, machine: String },
    /// Remove a machine from future revisions (old copies cannot be erased).
    Revoke { name: String, machine: String },
    /// Publish a signed deletion. Offline peers learn it when they reconnect.
    Delete { name: String },
    /// Import a field from the Agents vault, with optional local Keychain fallback.
    Import {
        name: String,
        #[arg(long)]
        reference: String,
        #[arg(long,default_value="value",value_parser=["value","bundle","blob","cookies"])]
        kind: String,
        #[arg(long, requires = "keychain_account")]
        keychain_service: Option<String>,
        #[arg(long, requires = "keychain_service")]
        keychain_account: Option<String>,
    },
    /// Version-one private SDK protocol. Never connect stdout to a log.
    #[command(hide = true)]
    Rpc,
}
fn input(file: Option<&PathBuf>, limit: usize) -> Result<Secret> {
    let reader: Box<dyn Read> = match file {
        Some(path) => Box::new(std::fs::File::open(path).map_err(|_| Error::StorageFailure)?),
        None => {
            if std::io::stdin().is_terminal() {
                return Err(Error::InvalidInput);
            }
            Box::new(std::io::stdin())
        }
    };
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::StorageFailure)?;
    let secret = Secret::new(bytes);
    if secret.expose().len() > limit {
        return Err(Error::TooLarge);
    }
    Ok(secret)
}
fn json(value: impl serde::Serialize) -> Result<()> {
    serde_json::to_writer(std::io::stdout().lock(), &value).map_err(|_| Error::StorageFailure)?;
    println!();
    Ok(())
}
fn execute(options: &Options) -> Result<()> {
    let mut store = hey_boss::secrets::open()?;
    match &options.action {
        Action::Install => {
            hey_boss::secrets::install()?;
            json(serde_json::json!({"installed":true}))
        }
        Action::Init { label } => {
            backend::prepare_native(&hey_boss::secrets::home()?)?;
            let label = label
                .clone()
                .unwrap_or_else(|| std::env::var("HOSTNAME").unwrap_or_else(|_| "local".into()));
            json(store.initialize(&label)?)
        }
        Action::Identity => json(store.enrollment()?),
        Action::Trust { file } => {
            let bytes = input(file.as_ref(), 16384)?;
            let card = serde_json::from_slice(bytes.expose()).map_err(|_| Error::InvalidInput)?;
            store.trust(&card)?;
            json(serde_json::json!({"trusted":true}))
        }
        Action::List => json(store.inventory()?),
        Action::Enroll { host } => {
            if host.is_empty()
                || host.starts_with('-')
                || !host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-@".contains(&b))
            {
                return Err(Error::InvalidInput);
            }
            let local = store.enrollment()?;
            let ssh = |command: String| {
                let mut cmd = std::process::Command::new("ssh");
                cmd.args([
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=yes",
                    "-o",
                    "ConnectTimeout=10",
                    host,
                    &command,
                ]);
                cmd
            };
            backend::command(ssh("~/.local/bin/hey-boss secrets install".into()), &[])?;
            let output =
                backend::command(ssh("~/.local/bin/hey-boss secrets identity".into()), &[])?;
            let remote =
                serde_json::from_slice(output.expose()).map_err(|_| Error::InvalidRecord)?;
            store.trust(&remote)?;
            let card = serde_json::to_vec(&local).map_err(|_| Error::InvalidRecord)?;
            backend::command(ssh("~/.local/bin/hey-boss secrets trust".into()), &card)?;
            json(remote)
        }
        Action::Put {
            name,
            file,
            kind,
            expected,
        } => {
            let bytes = input(file.as_ref(), MAX_SECRET)?;
            json(store.put(name, bytes.expose(), kind, expected.as_deref())?)
        }
        Action::Get { name, file, stdout } => {
            if *stdout && std::io::stdout().is_terminal() {
                return Err(Error::InvalidInput);
            }
            let bytes = store.get(name)?;
            if let Some(file) = file {
                backend::write_private(file, bytes.expose())
            } else {
                std::io::stdout()
                    .lock()
                    .write_all(bytes.expose())
                    .map_err(|_| Error::StorageFailure)
            }
        }
        Action::Grant { name, machine } => json(store.grant(name, machine, true)?),
        Action::Revoke { name, machine } => json(store.grant(name, machine, false)?),
        Action::Delete { name } => json(store.delete(name)?),
        Action::Import {
            name,
            reference,
            kind,
            keychain_service,
            keychain_account,
        } => {
            let bytes = hey_boss_secrets::provision::read(
                &hey_boss::secrets::home()?,
                reference,
                keychain_service.as_deref().zip(keychain_account.as_deref()),
            )?;
            json(store.put(name, bytes.expose(), kind, None)?)
        }
        Action::Rpc => {
            if std::io::stdout().is_terminal() {
                return Err(Error::InvalidInput);
            }
            let bytes = input(None, 2 * MAX_SECRET)?;
            let reply = hey_boss_secrets::client::serve(&mut store, bytes.expose());
            std::io::stdout()
                .lock()
                .write_all(reply.expose())
                .map_err(|_| Error::StorageFailure)
        }
    }
}
pub fn run(options: &Options) -> std::io::Result<()> {
    execute(options).map_err(std::io::Error::other)
}
