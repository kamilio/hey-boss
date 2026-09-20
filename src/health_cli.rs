use clap::Subcommand;
use hey_boss::health::remote::hosts;
use hey_boss::health::{Snapshot, Store, readable_bytes};
use std::io;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Subcommand)]
pub enum Action {
    /// Open Machine Health in the native menu-bar app.
    Open,
    /// Show disk, memory, automation state, and the latest cleanup counts.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Browse recent maintenance activity, decisions, and errors.
    Logs {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
    },
    /// Inspect candidates without stopping processes or removing worktrees.
    Scan {
        #[arg(long)]
        json: bool,
    },
    /// Clean verified candidates (requires quiet observations across two scans).
    Clean {
        #[arg(long)]
        json: bool,
    },
    /// Remove one unused clean checkout, retaining its named branch.
    RemoveWorktree {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// List the configured and connected SSH health clients.
    Hosts {
        #[arg(long)]
        json: bool,
    },
    /// Enable scheduled maintenance at login and every five minutes.
    Enable,
    /// Stop scheduled maintenance.
    Disable,
    /// Run maintenance in the foreground at the configured interval.
    Watch,
    /// Add a directory containing repositories/worktrees to the cleaner.
    AddRoot { path: PathBuf },
    /// Remove a directory from the cleaner's scope.
    RemoveRoot { path: PathBuf },
    /// Independently enable or disable process/worktree cleanup.
    Configure {
        #[arg(long, action = clap::ArgAction::Set)]
        processes: Option<bool>,
        #[arg(long, action = clap::ArgAction::Set)]
        worktrees: Option<bool>,
    },
    #[command(hide = true)]
    Run,
}
pub fn run_remote(host: &str, action: &Action) -> io::Result<()> {
    let args: Vec<String> = match action {
        Action::Status { json } | Action::Scan { json } | Action::Clean { json } => {
            let name = match action {
                Action::Status { .. } => "status",
                Action::Scan { .. } => "scan",
                _ => "clean",
            };
            let mut args = vec![name.into()];
            if *json {
                args.push("--json".into());
            }
            args
        }
        Action::RemoveWorktree { path, json } => {
            let mut args = vec![
                "remove-worktree".into(),
                path.to_str()
                    .ok_or_else(|| io::Error::other("Path must be UTF-8"))?
                    .into(),
            ];
            if *json {
                args.push("--json".into());
            }
            args
        }
        Action::Logs { json, limit } => {
            let mut args = vec!["logs".into(), "--limit".into(), limit.to_string()];
            if *json {
                args.push("--json".into());
            }
            args
        }
        Action::Enable => vec!["enable".into()],
        Action::Disable => vec!["disable".into()],
        Action::Run => vec!["run".into()],
        Action::AddRoot { path } | Action::RemoveRoot { path } => vec![
            if matches!(action, Action::AddRoot { .. }) {
                "add-root"
            } else {
                "remove-root"
            }
            .into(),
            path.to_str()
                .ok_or_else(|| io::Error::other("Path must be UTF-8"))?
                .into(),
        ],
        Action::Configure {
            processes,
            worktrees,
        } => {
            let mut args = vec!["configure".into()];
            if let Some(value) = processes {
                args.extend(["--processes".into(), value.to_string()]);
            }
            if let Some(value) = worktrees {
                args.extend(["--worktrees".into(), value.to_string()]);
            }
            args
        }
        _ => {
            return Err(io::Error::other(
                "Use the local UI to select machines; remote health supports status, logs, scan, clean, remove-worktree, settings, and scheduling",
            ));
        }
    };
    let control = crate::autoconnect::control_path(host).ok();
    let data = hey_boss::health::remote::execute(host, &args, control)?;
    use std::io::Write;
    io::stdout().write_all(&data)
}

fn print(s: &Snapshot, json: bool) -> io::Result<()> {
    if json {
        println!("{}", serde_json::to_string(s)?);
        return Ok(());
    }
    let bytes = |n: Option<u64>| {
        n.map(readable_bytes)
            .unwrap_or_else(|| "unavailable".into())
    };
    println!(
        "Disk: {} available / {}",
        bytes(s.metrics.disk_available_bytes),
        bytes(s.metrics.disk_total_bytes)
    );
    println!(
        "Memory: {} · {} estimated available / {} · {} swap",
        s.metrics.memory_pressure,
        bytes(s.metrics.memory_available_bytes),
        bytes(s.metrics.memory_total_bytes),
        bytes(s.metrics.swap_used_bytes)
    );
    println!(
        "Automatic cleanup: {} · stopped {} processes · removed {} worktrees",
        if s.config.automatic { "on" } else { "off" },
        s.harvested_processes,
        s.removed_worktrees
    );
    println!(
        "{} process groups and {} worktrees inspected. Codex and active work are protected.",
        s.processes.len(),
        s.worktrees.len()
    );
    for e in &s.errors {
        eprintln!("{e}");
    }
    Ok(())
}
pub fn run(action: &Action) -> io::Result<()> {
    let store = Store::standard()?;
    match action {
        Action::Open => unreachable!("handled by native daemon dispatch"),
        Action::Hosts { json } => {
            let hosts = hosts()?;
            if *json {
                println!("{}", serde_json::to_string(&hosts)?);
            } else {
                for host in hosts {
                    println!("{host}");
                }
            }
            Ok(())
        }
        Action::RemoveWorktree { path, json } => {
            let result = store.remove_worktree(path)?;
            print(&result, *json)?;
            if !*json && !result.errors.is_empty() {
                return Err(io::Error::other(result.errors.join("; ")));
            }
            Ok(())
        }
        Action::Status { json } => print(&store.status()?, *json),
        Action::Logs { json, limit } => {
            let state = store.state()?;
            let events: Vec<_> = state
                .snapshot
                .activity
                .iter()
                .rev()
                .take(usize::from(*limit))
                .collect();
            if *json {
                println!("{}", serde_json::to_string(&events)?);
            } else {
                for event in events {
                    println!("{} · {} · {}", event.at, event.category, event.message);
                }
            }
            Ok(())
        }
        Action::Scan { json } => print(&store.cycle(false)?, *json),
        Action::Clean { json } => print(&store.cycle(true)?, *json),
        Action::Run => {
            if store.config()?.automatic {
                match store.cycle(true) {
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        }
        Action::Watch => loop {
            let c = store.config()?;
            if c.automatic {
                let _ = store.cycle(true);
            }
            std::thread::sleep(Duration::from_secs(c.interval_seconds));
        },
        Action::Enable => {
            let _lock = store.lock()?;
            let mut config = store.config()?;
            let old = config.clone();
            config.automatic = true;
            store.save("config.json", &config)?;
            if let Err(e) = schedule(&store, true, config.interval_seconds) {
                store.save("config.json", &old)?;
                return Err(e);
            }
            store.record_setting("Automatic cleanup enabled")?;
            println!(
                "Automatic health cleanup enabled. Checks every {} minutes; no reports or notifications.",
                config.interval_seconds / 60
            );
            Ok(())
        }
        Action::Disable => {
            let _lock = store.lock()?;
            // Disabling prevents the next mutation cycle even if launchd is unavailable.
            let mut config = store.config()?;
            config.automatic = false;
            store.save("config.json", &config)?;
            schedule(&store, false, config.interval_seconds)?;
            store.record_setting("Automatic cleanup disabled")?;
            println!("Automatic health cleanup disabled.");
            Ok(())
        }
        Action::AddRoot { path } | Action::RemoveRoot { path } => {
            let _lock = store.lock()?;
            let mut config = store.config()?;
            let path = path.canonicalize()?;
            if !path.is_dir() {
                return Err(io::Error::other("Workspace root must be a directory"));
            }
            config.workspace_roots.retain(|p| p != &path);
            if matches!(action, Action::AddRoot { .. }) {
                config.workspace_roots.push(path);
            }
            config.validate()?;
            store.save("config.json", &config)?;
            println!("Workspace roots updated.");
            store.record_setting("Workspace cleanup roots updated")?;
            Ok(())
        }
        Action::Configure {
            processes,
            worktrees,
        } => {
            let _lock = store.lock()?;
            let mut config = store.config()?;
            if let Some(v) = processes {
                config.harvest_processes = *v;
            }
            if let Some(v) = worktrees {
                config.clean_worktrees = *v;
            }
            store.save("config.json", &config)?;
            store.record_setting(&format!(
                "Process harvesting: {}; worktree cleanup: {}",
                config.harvest_processes, config.clean_worktrees
            ))?;
            println!("Cleanup settings updated.");
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
fn schedule(store: &Store, enable: bool, interval: u64) -> io::Result<()> {
    use std::fs;
    use std::process::Command;
    let uid = unsafe { libc::geteuid() };
    let domain = format!("gui/{uid}");
    let service = format!("{domain}/local.hey-boss.health");
    let home = PathBuf::from(
        std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is unavailable"))?,
    );
    let folder = home.join("Library/LaunchAgents");
    let path = folder.join("local.hey-boss.health.plist");
    if !enable {
        let _ = Command::new("/bin/launchctl")
            .args(["bootout", &service])
            .output()?;
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        return Ok(());
    }
    fs::create_dir_all(folder)?;
    let exe = std::env::current_exe()?.canonicalize()?;
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>local.hey-boss.health</string>
<key>ProgramArguments</key><array><string>{}</string><string>health</string><string>run</string></array>
<key>EnvironmentVariables</key><dict><key>HEY_BOSS_HEALTH_DIR</key><string>{}</string><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>
<key>RunAtLoad</key><true/><key>StartInterval</key><integer>{interval}</integer>
<key>ProcessType</key><string>Standard</string><key>Nice</key><integer>10</integer>
<key>StandardOutPath</key><string>/dev/null</string><key>StandardErrorPath</key><string>/dev/null</string>
</dict></plist>
"#,
        escape(&exe.to_string_lossy()),
        escape(&store.directory.to_string_lossy())
    );
    // Callers hold the maintenance lock, so no cycle is running. Keep a matching
    // registration; replace a stale one (old priority or binary path) in place.
    let registered = Command::new("/bin/launchctl")
        .args(["print", &service])
        .output()?
        .status
        .success();
    if registered {
        if fs::read_to_string(&path).is_ok_and(|current| current == plist) {
            return Ok(());
        }
        let _ = Command::new("/bin/launchctl")
            .args(["bootout", &service])
            .output()?;
    }
    fs::write(&path, plist)?;
    let out = Command::new("/bin/launchctl")
        .args(["bootstrap", &domain])
        .arg(&path)
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn schedule(store: &Store, enable: bool, interval: u64) -> io::Result<()> {
    use std::process::Command;
    fn systemctl(args: &[&str]) -> io::Result<()> {
        let result = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()?;
        if !result.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&result.stderr).trim().to_owned(),
            ));
        }
        Ok(())
    }
    if !enable {
        return systemctl(&["disable", "--now", "hey-boss-health.timer"]);
    }
    systemctl(&["show-environment"])?;
    let home = PathBuf::from(
        std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is unavailable"))?,
    );
    let units = home.join(".config/systemd/user");
    std::fs::create_dir_all(&units)?;
    let escape = |s: &str| {
        format!(
            "\"{}\"",
            s.replace('\\', "\\\\")
                .replace('\"', "\\\"")
                .replace('%', "%%")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
        )
    };
    let executable = std::env::current_exe()?.canonicalize()?;
    let service = format!(
        "[Unit]\nDescription=hey-boss machine health\n[Service]\nType=oneshot\nExecStart={} health run\nEnvironment={}\nEnvironment=\"PATH=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin\"\nUMask=0077\nTimeoutStartSec=300\nStandardOutput=null\nStandardError=journal\n",
        escape(&executable.to_string_lossy()).replace('$', "$$"),
        escape(&format!(
            "HEY_BOSS_HEALTH_DIR={}",
            store.directory.display()
        ))
    );
    let timer = format!(
        "[Unit]\nDescription=hey-boss periodic machine health\n[Timer]\nOnActiveSec=15\nOnUnitInactiveSec={interval}\nUnit=hey-boss-health.service\n[Install]\nWantedBy=timers.target\n"
    );
    std::fs::write(units.join("hey-boss-health.service"), service)?;
    std::fs::write(units.join("hey-boss-health.timer"), timer)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", "hey-boss-health.timer"])
}
