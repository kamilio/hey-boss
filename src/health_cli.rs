//! Delegate maintenance to the separately installed executable.
use std::{io, path::PathBuf, process::Command};

pub fn run(host: Option<&str>, args: &[String]) -> io::Result<()> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let binary = home
        .iter()
        .flat_map(|home| {
            [
                home.join(".local/bin/hey-harvester"),
                home.join(".cargo/bin/hey-harvester"),
            ]
        })
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("hey-harvester"));
    let mut command = Command::new(binary);
    if let Some(host) = host {
        command.args(["--host", host]);
    }
    let status = command.args(args).status().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Run the separately installed hey-harvester: {error}"),
        )
    })?;
    std::process::exit(status.code().unwrap_or(1));
}
