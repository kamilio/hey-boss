use clap::Args;

#[derive(Args)]
pub struct Options {
    /// Build this checkout; remember it for future upgrades. Otherwise fetch upstream.
    #[arg(long)]
    source: Option<std::path::PathBuf>,
    /// Report outdated or unreachable machines without installing anything.
    #[arg(long)]
    check: bool,
    /// Reinstall even when the source build matches.
    #[arg(long)]
    force: bool,
    /// Update only this machine.
    #[arg(long, conflicts_with = "host")]
    local_only: bool,
    /// Update these SSH hosts instead of the registered companions (repeatable).
    #[arg(long)]
    host: Vec<String>,
    #[arg(long)]
    json: bool,
}

pub fn run(options: &Options) -> std::io::Result<()> {
    let mut command = std::process::Command::new("python3");
    command.args(["-c", include_str!("../tools/upgrade_hey_boss.py")]);
    command.env(
        "HEY_BOSS_UPGRADE_BINARY",
        std::env::current_exe()?.canonicalize()?,
    );
    if let Some(source) = &options.source {
        command.arg("--source").arg(source);
    }
    for (enabled, flag) in [
        (options.check, "--check"),
        (options.force, "--force"),
        (options.local_only, "--local-only"),
        (options.json, "--json"),
    ] {
        if enabled {
            command.arg(flag);
        }
    }
    for host in &options.host {
        command.arg("--host").arg(host);
    }
    std::process::exit(command.status()?.code().unwrap_or(1));
}
