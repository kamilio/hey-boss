use clap::{Args, Subcommand};
use hey_boss::issues::{self, Error, Operation, Request, Result, Store, worker::Settings};
#[derive(Args)]
pub struct Options {
    /// Run on the authoritative SSH host; Codex sessions run there too.
    #[arg(long)]
    host: Option<String>,
    /// Parallel Codex sessions owned by this worker; there is no shared pool cap.
    #[arg(long)]
    concurrency: Option<u32>,
    /// Only pick issues matching every selected tag; omit for unrestricted tags.
    #[arg(long = "tag")]
    tags: Vec<String>,
    /// Project ID/name; repeat to scan several projects. Defaults to this checkout.
    #[arg(long, conflicts_with = "all_projects")]
    project: Vec<String>,
    /// Scan all visible projects with known local checkout directories.
    #[arg(long)]
    all_projects: bool,
    #[arg(long)]
    directory: Option<std::path::PathBuf>,
    #[arg(long)]
    name: Option<String>,
    /// Restore this worker's saved settings; concurrent use of an ID is rejected.
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long, conflicts_with = "no_prs")]
    prs: bool,
    #[arg(long, conflicts_with = "prs")]
    no_prs: bool,
    /// Seconds allowed for Codex to claim its reserved issue.
    #[arg(long)]
    claim_timeout: Option<u32>,
    #[arg(long)]
    json: bool,
    /// Finished-attempt history (0 starts with active work only).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=20))]
    history: u8,
    #[command(subcommand)]
    action: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    #[command(visible_alias = "list")]
    Status,
    /// Queue a durable restart through the fleet controller; keep the controller alive.
    Restart {
        id: String,
    },
    Stop {
        id: String,
    },
    Pause {
        id: String,
    },
}
pub fn run(o: &Options) -> Result<()> {
    if matches!(o.action, Some(Action::Status)) && dashboard_enabled(o) {
        use hey_boss::worker_tui::{backend::Client, runtime};
        use std::sync::{Arc, atomic::AtomicBool};
        let cancelled = Arc::new(AtomicBool::new(false));
        issues::worker::install_signals(cancelled.clone())?;
        runtime::run(
            runtime::Options {
                client: Client {
                    binary: std::env::current_exe()?,
                    host: o.host.clone(),
                    directory: o.directory.clone(),
                    timeout: std::time::Duration::from_secs(10),
                },
                id: o.id.clone(),
                history: o.history > 0,
                owned_worker: false,
            },
            cancelled,
        )
        .map_err(|e| Error::new("worker_error", e.to_string()))?;
        return Ok(());
    }
    if let Some(Action::Restart { id }) = &o.action {
        let host = o
            .host
            .clone()
            .or_else(|| {
                std::env::var("HEY_BOSS_ISSUE_HOST")
                    .ok()
                    .filter(|v| !v.is_empty())
            })
            .unwrap_or_else(|| "local".into());
        let value = hey_boss::fleet::call(
            &serde_json::json!({"kind":"signal", "host":host, "worker":id, "signal":"restart"}),
        )?;
        if o.json {
            println!("{value}");
        } else {
            println!(
                "Restart queued for worker {id} on {host} (signal {}). Check hey-boss fleet status for acknowledgment.",
                value["id"].as_str().unwrap_or("unknown")
            );
        }
        return Ok(());
    }
    if let Some(host) = o.host.clone().or_else(|| {
        std::env::var("HEY_BOSS_ISSUE_HOST")
            .ok()
            .filter(|v| !v.is_empty())
    }) {
        return run_remote(o, &host);
    }
    let cwd = o
        .directory
        .clone()
        .unwrap_or(std::env::current_dir()?)
        .canonicalize()?;
    let machine = issues::identity::machine()?;
    let base = issues::identity::project(&cwd, &machine)?;
    let controller = format!("worker-controller:{machine}:{}", std::process::id());
    let actor = issues::identity::resolve(Some(&controller), &machine, &cwd)?;
    let path = issues::database_path()?;
    let mut store = if o.action.is_none() {
        issues::worker::retry_database_busy(|| Store::open(&path))?
    } else {
        Store::open(&path)?
    };
    let request = |operation, override_id| Request {
        version: 1,
        project: base.clone(),
        project_override: override_id,
        actor: Some(actor.clone()),
        operation,
        request_id: None,
    };
    if let Some(action) = &o.action {
        let operation = match action {
            Action::Restart { .. } => {
                unreachable!("Restart is routed through the fleet controller")
            }
            Action::Status => Operation::Workers {
                worker_id: o.id.clone(),
            },
            Action::Stop { id } => Operation::ControlWorker {
                worker_id: id.clone(),
                command: "stop_worker".into(),
                run_id: None,
            },
            Action::Pause { id } => Operation::ControlWorker {
                worker_id: id.clone(),
                command: "pause".into(),
                run_id: None,
            },
        };
        let mut value = store.execute(&request(operation, None))?;
        match action {
            Action::Stop { id } => {
                hey_boss::fleet::record_local_worker(id, None, "stop")?;
            }
            Action::Pause { id } => {
                hey_boss::fleet::record_local_worker(id, None, "pause")?;
            }
            Action::Status | Action::Restart { .. } => {}
        }
        value["store"] = serde_json::json!({"host":issues::identity::host(),"database":issues::database_path()?});
        if o.json {
            println!("{value}");
        } else {
            issues::worker::print_status_with_history(&value, false, o.history as usize);
        }
        return Ok(());
    }
    let mut c = if let Some(id) = &o.id {
        serde_json::from_value(
            store.execute(&request(
                Operation::Workers {
                    worker_id: Some(id.clone()),
                },
                None,
            ))?["config"]
                .clone(),
        )?
    } else {
        Settings {
            name: format!("Worker {}", std::process::id()),
            ..Settings::default()
        }
    };
    if o.id.is_none() || o.all_projects || !o.project.is_empty() {
        c.projects = if o.all_projects {
            vec![]
        } else if o.project.is_empty() {
            vec![base.id.clone()]
        } else {
            o.project
                .iter()
                .map(|p| {
                    store
                        .execute(&request(
                            Operation::Projects {
                                include_hidden: true,
                            },
                            Some(p.clone()),
                        ))
                        .map(|v| v["project"]["id"].as_str().unwrap().to_owned())
                })
                .collect::<Result<_>>()?
        };
        c.directory = if c.projects == vec![base.id.clone()] {
            cwd.to_string_lossy().into()
        } else {
            String::new()
        };
    }
    if o.directory.is_some() {
        c.directory = cwd.to_string_lossy().into();
    }
    if let Some(n) = o.concurrency {
        c.concurrency = n;
    }
    if !o.tags.is_empty() {
        c.tags = o.tags.clone();
    }
    if let Some(name) = &o.name {
        c.name = name.clone();
    }
    if let Some(prompt) = &o.prompt {
        c.prompt = Some(prompt.clone());
    }
    if o.prs {
        c.prs_enabled = Some(true);
    } else if o.no_prs {
        c.prs_enabled = Some(false);
    }
    if let Some(seconds) = o.claim_timeout {
        c.reservation_seconds = seconds;
    }
    c.enabled = true;
    issues::worker::serve_instance_with_history(
        c,
        o.id.as_deref(),
        base,
        o.json,
        o.history as usize,
    )
}

fn dashboard_enabled(o: &Options) -> bool {
    use std::io::IsTerminal;
    !o.json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

fn remote_arguments(o: &Options) -> Vec<String> {
    let mut args = vec!["worker".into()];
    args.extend(["--history".into(), o.history.to_string()]);
    for (flag, value) in [
        ("--concurrency", o.concurrency.map(|v| v.to_string())),
        (
            "--directory",
            o.directory
                .as_ref()
                .map(|v| v.to_string_lossy().into_owned()),
        ),
        ("--name", o.name.clone()),
        ("--id", o.id.clone()),
        ("--prompt", o.prompt.clone()),
        ("--claim-timeout", o.claim_timeout.map(|v| v.to_string())),
    ] {
        if let Some(value) = value {
            args.extend([flag.into(), value]);
        }
    }
    for (flag, values) in [("--project", &o.project), ("--tag", &o.tags)] {
        for value in values {
            args.extend([flag.into(), value.clone()]);
        }
    }
    for (enabled, flag) in [
        (o.all_projects, "--all-projects"),
        (o.prs, "--prs"),
        (o.no_prs, "--no-prs"),
        (o.json, "--json"),
    ] {
        if enabled {
            args.push(flag.into());
        }
    }
    match &o.action {
        Some(Action::Status) => args.push("status".into()),
        Some(Action::Restart { id }) => args.extend(["restart".into(), id.clone()]),
        Some(Action::Stop { id }) => args.extend(["stop".into(), id.clone()]),
        Some(Action::Pause { id }) => args.extend(["pause".into(), id.clone()]),
        None => {}
    }
    args
}

fn remote_script(args: &[String]) -> String {
    let quoted = args
        .iter()
        .map(|value| format!("'{}'", value.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "export PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH\"; unset HEY_BOSS_ISSUE_HOST; exec hey-boss {quoted}"
    )
}

fn run_remote(o: &Options, host: &str) -> Result<()> {
    use std::io::IsTerminal;
    if !hey_boss::health::remote::valid_host(host) {
        return Err(Error::invalid("Invalid authoritative SSH host"));
    }
    if o.action.is_none() && o.id.is_none() && o.directory.is_none() && !o.all_projects {
        return Err(Error::invalid(
            "A remote worker needs --directory PATH pointing to its checkout on the authoritative host; Codex sessions run on that host",
        ));
    }
    let status = std::process::Command::new("ssh")
        .args([
            if std::io::stdin().is_terminal() {
                "-t"
            } else {
                "-T"
            },
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            host,
        ])
        .arg(remote_script(&remote_arguments(o)))
        .env("SFT_NO_BROWSER", "1")
        .status()?;
    if !status.success() {
        return Err(Error::new(
            "transport_error",
            format!("Worker on {host} exited with {status}; no local database fallback was used"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        options: Options,
    }
    #[test]
    fn remote_arguments_preserve_filters_and_put_status_after_options() {
        let cli = TestCli::parse_from([
            "worker",
            "--host",
            "devbox",
            "--all-projects",
            "--concurrency",
            "1",
            "--tag",
            "ready",
            "--id",
            "saved",
            "--json",
            "status",
        ]);
        let args = remote_arguments(&cli.options);
        assert!(!args.contains(&"--host".into()));
        assert!(args.contains(&"--all-projects".into()));
        assert!(args.windows(2).any(|a| a == ["--tag", "ready"]));
        assert!(args.windows(2).any(|a| a == ["--concurrency", "1"]));
        assert_eq!(args.last().unwrap(), "status");
        assert!(args.contains(&"--json".into()));
    }
    #[test]
    fn remote_prompt_and_paths_are_shell_literals() {
        let args = vec![
            "worker".into(),
            "--prompt".into(),
            "literal ' $(touch /tmp/bad)\nnext line".into(),
            "--directory".into(),
            "/repo with spaces".into(),
        ];
        let script = remote_script(&args).replace("exec hey-boss", "printf '%s\\n'");
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            args.join("\n") + "\n"
        );
    }

    #[test]
    fn json_status_does_not_enable_the_dashboard() {
        let cli = TestCli::parse_from(["worker", "--json", "status"]);
        assert!(!dashboard_enabled(&cli.options));
    }
}
