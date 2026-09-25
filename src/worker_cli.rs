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
    /// Checkout to work in; repeat for multiple projects. Paths belong to --host when remote.
    #[arg(
        long = "cwd",
        short = 'C',
        visible_alias = "directory",
        conflicts_with = "all_projects"
    )]
    directory: Vec<std::path::PathBuf>,
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
    /// Use a dedicated Git worktree for each issue when the project allows it.
    #[arg(long, conflicts_with = "no_worktree")]
    worktree: bool,
    /// Use the existing checkout (the default workspace).
    #[arg(long, conflicts_with = "worktree")]
    no_worktree: bool,
    /// Enable the per-project hourly organizing agent, outside issue concurrency.
    #[arg(long, conflicts_with = "no_chief")]
    chief: bool,
    #[arg(long, conflicts_with = "chief")]
    no_chief: bool,
    /// Seconds allowed for Codex to claim its reserved issue.
    #[arg(long)]
    claim_timeout: Option<u32>,
    #[arg(long)]
    json: bool,
    /// Finished-attempt history (0 keeps only active or pending attempts).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=20))]
    history: u8,
    #[command(subcommand)]
    action: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    #[command(visible_alias = "list")]
    Status,
    /// Observe existing workers every two seconds; never start or control them.
    Watch {
        /// Stop after this many snapshots; omit to watch until Ctrl-C.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        count: Option<u32>,
    },
    /// Queue a durable restart through the fleet supervisor; keep the supervisor alive.
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
                    directory: o.directory.first().cloned(),
                    timeout: std::time::Duration::from_secs(10),
                },
                id: o.id.clone(),
                history: o.history > 0,
                owned_worker: false,
                project_tabs: false,
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
        .first()
        .cloned()
        .unwrap_or(std::env::current_dir()?)
        .canonicalize()?;
    let machine = issues::identity::machine()?;
    let base = issues::identity::project(&cwd, &machine)?;
    let actor_id = format!("worker-control:{machine}:{}", std::process::id());
    let actor = issues::identity::resolve(Some(&actor_id), &machine, &cwd)?;
    let path = issues::database_path()?;
    let mut store = if o.action.is_none() || matches!(o.action, Some(Action::Watch { .. })) {
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
    if let Some(Action::Watch { count }) = &o.action {
        use std::io::Write;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        issues::worker::install_signals(cancelled.clone())?;
        let mut output = std::io::stdout().lock();
        let mut emitted = 0u32;
        while !cancelled.load(Ordering::Relaxed) {
            let value = watch_snapshot(o.id.clone(), o.history as usize, |id| {
                issues::worker::retry_database_busy(|| {
                    store.execute(&request(
                        Operation::Workers {
                            worker_id: id.clone(),
                        },
                        None,
                    ))
                })
            })?;
            let result = if o.json {
                writeln!(output, "{value}")
            } else {
                write!(output, "{}", watch_text(&value))
            }
            .and_then(|_| output.flush());
            if let Err(error) = result {
                if error.kind() == std::io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(error.into());
            }
            emitted = emitted.saturating_add(1);
            if count.is_some_and(|limit| emitted >= limit) {
                break;
            }
            // Short waits make cancellation responsive without busy polling.
            for _ in 0..20 {
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        return Ok(());
    }
    if let Some(action) = &o.action {
        let operation = match action {
            Action::Restart { .. } => {
                unreachable!("Restart is routed through the fleet supervisor")
            }
            Action::Watch { .. } => unreachable!("Watch is handled before one-shot actions"),
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
            Action::Status | Action::Restart { .. } | Action::Watch { .. } => {}
        }
        value["store"] = serde_json::json!({"host":issues::identity::host(),"database":issues::database_path()?});
        if matches!(action, Action::Status) {
            issues::worker::limit_status_history(&mut value, o.history as usize);
        }
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
    if o.id.is_none() || o.all_projects || !o.project.is_empty() || !o.directory.is_empty() {
        let selected = o
            .project
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
            .collect::<Result<Vec<_>>>()?;
        let paths = o
            .directory
            .iter()
            .map(|path| {
                let path = path.canonicalize()?;
                if !path.is_dir() {
                    return Err(Error::invalid("Choose an existing checkout directory"));
                }
                Ok((issues::identity::project(&path, &machine)?, path))
            })
            .collect::<Result<Vec<_>>>()?;
        configure_scope(&mut c, &base, &cwd, selected, paths, o.all_projects)?;
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
    if o.worktree {
        c.worktree_enabled = Some(true);
    } else if o.no_worktree {
        c.worktree_enabled = Some(false);
    }
    if let Some(seconds) = o.claim_timeout {
        c.reservation_seconds = seconds;
    }
    c.enabled = true;
    if o.chief || o.no_chief {
        if c.projects.is_empty() {
            return Err(Error::invalid(
                "Select a project with --project before enabling or disabling its Chief",
            ));
        }
        for project in &c.projects {
            store.execute(&request(
                Operation::ConfigureProject {
                    subtask_scheduling: None,
                    chief_enabled: Some(o.chief),
                    chief_prompt: None,
                    prompt: None,
                    boss_name: None,
                    prs_enabled: None,
                    worktree_enabled: None,
                    prompt_overrides: None,
                    drafts_enabled: None,
                    plan_template: None,
                    if_version: None,
                },
                Some(project.clone()),
            ))?;
        }
    }
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
    !matches!(o.action, Some(Action::Watch { .. }))
        && !o.json
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
}

fn watch_snapshot(
    id: Option<String>,
    history: usize,
    mut fetch: impl FnMut(Option<String>) -> Result<serde_json::Value>,
) -> Result<serde_json::Value> {
    use serde_json::json;
    let inventory = fetch(id.clone())?;
    let ids: Vec<String> = if let Some(id) = id {
        vec![id]
    } else {
        inventory["workers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| w["id"].as_str().map(str::to_owned))
            .collect()
    };
    let mut snapshots = Vec::new();
    for id in ids {
        let mut value = if inventory["worker_id"].as_str() == Some(&id) {
            inventory.clone()
        } else {
            fetch(Some(id))?
        };
        issues::worker::limit_status_history(&mut value, history);
        // Inventory is emitted once rather than repeated for every worker.
        value.as_object_mut().unwrap().remove("workers");
        snapshots.push(value);
    }
    let observed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| Error::new("clock_error", e.to_string()))?
        .as_millis() as u64;
    Ok(json!({"ok":true,"observed_at":observed_at,
        "store":{"host":issues::identity::host(),"database":issues::database_path()?},
        "workers":inventory["workers"],"snapshots":snapshots}))
}

fn watch_text(value: &serde_json::Value) -> String {
    use hey_boss::worker_tui::text;
    use std::fmt::Write;
    let seconds = value["observed_at"].as_u64().unwrap_or(0) / 1000;
    let mut output = format!(
        "\nWorkers · {} · {:02}:{:02}:{:02} UTC · refresh 2s · Ctrl-C to exit\n",
        text(&value["store"]["host"]),
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    );
    let snapshots = value["snapshots"].as_array().unwrap();
    if snapshots.is_empty() {
        output.push_str("No workers registered.\n");
    }
    for snapshot in snapshots {
        let id = text(&snapshot["worker_id"]);
        let worker = value["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id);
        let state = worker
            .map(|w| {
                if w["upgrading"] == true {
                    "update drain"
                } else if w["pid"].is_null() {
                    "offline"
                } else if w["config"]["enabled"] != true {
                    "paused"
                } else {
                    "pickup on"
                }
            })
            .unwrap_or("unknown");
        let _ = writeln!(
            output,
            "{} · {} · {} · {}/{} busy · {} eligible",
            text(&snapshot["config"]["name"]),
            id,
            state,
            snapshot["active"],
            snapshot["config"]["concurrency"],
            snapshot["eligible"]
        );
        let runs = snapshot["runs"].as_array().unwrap();
        if !runs.iter().any(|run| run["finished_at"].is_null()) {
            output.push_str("  No active agents.\n");
        }
        for run in runs {
            let _ = writeln!(
                output,
                "  {} #{} · {} · {}{}",
                text(&run["project_name"]),
                run["number"],
                text(&run["title"]),
                text(&run["state"]),
                if run["finished_at"].is_null() {
                    ""
                } else {
                    " (history)"
                }
            );
            let _ = writeln!(
                output,
                "    {}",
                text(
                    if run["last_event"].as_str().is_some_and(|s| !s.is_empty()) {
                        &run["last_event"]
                    } else {
                        &run["summary"]
                    }
                )
            );
            if let Some(session) = run["session_id"].as_str() {
                let _ = writeln!(
                    output,
                    "    Session: {}",
                    text(&serde_json::Value::String(session.into()))
                );
            }
        }
    }
    output
}

fn configure_scope(
    c: &mut Settings,
    base: &issues::Project,
    cwd: &std::path::Path,
    selected: Vec<String>,
    paths: Vec<(issues::Project, std::path::PathBuf)>,
    all_projects: bool,
) -> Result<()> {
    c.directory.clear();
    c.directories.clear();
    c.projects = if all_projects {
        vec![]
    } else if !selected.is_empty() {
        selected
    } else if paths.is_empty() {
        vec![base.id.clone()]
    } else {
        paths.iter().map(|(p, _)| p.id.clone()).collect()
    };
    c.projects.sort();
    c.projects.dedup();
    if paths.len() == 1 && c.projects.len() == 1 {
        c.directory = paths[0].1.to_string_lossy().into_owned();
    } else {
        for (project, path) in paths {
            if !c.projects.contains(&project.id) {
                return Err(Error::invalid(format!(
                    "Checkout {} is not in the selected --project filters",
                    path.display()
                )));
            }
            let path = path.to_string_lossy().into_owned();
            if let Some(previous) = c.directories.insert(project.id, path.clone())
                && previous != path
            {
                return Err(Error::invalid(
                    "Choose only one checkout per project for this worker",
                ));
            }
        }
        if c.directories.is_empty() && c.projects == [base.id.clone()] {
            c.directory = cwd.to_string_lossy().into_owned();
        }
    }
    Ok(())
}

fn remote_arguments(o: &Options) -> Vec<String> {
    let mut args = vec!["worker".into()];
    args.extend(["--history".into(), o.history.to_string()]);
    for (flag, value) in [
        ("--concurrency", o.concurrency.map(|v| v.to_string())),
        ("--name", o.name.clone()),
        ("--id", o.id.clone()),
        ("--prompt", o.prompt.clone()),
        ("--claim-timeout", o.claim_timeout.map(|v| v.to_string())),
    ] {
        if let Some(value) = value {
            args.extend([flag.into(), value]);
        }
    }
    for path in &o.directory {
        args.extend(["--cwd".into(), path.to_string_lossy().into_owned()]);
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
        (o.worktree, "--worktree"),
        (o.no_worktree, "--no-worktree"),
        (o.chief, "--chief"),
        (o.no_chief, "--no-chief"),
        (o.json, "--json"),
    ] {
        if enabled {
            args.push(flag.into());
        }
    }
    match &o.action {
        Some(Action::Status) => args.push("status".into()),
        Some(Action::Watch { count }) => {
            args.push("watch".into());
            if let Some(count) = count {
                args.extend(["--count".into(), count.to_string()]);
            }
        }
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
    if o.action.is_none()
        && o.id.is_none()
        && o.directory.is_empty()
        && !o.all_projects
        && o.project.is_empty()
    {
        return Err(Error::invalid(
            "A remote worker needs --cwd PATH, --project, or --all-projects on the authoritative host; Codex sessions run on that host",
        ));
    }
    let status = std::process::Command::new("ssh")
        .args([
            if std::io::stdin().is_terminal()
                && !o.json
                && !matches!(o.action, Some(Action::Watch { .. }))
            {
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
    fn scope_project(name: &str) -> issues::Project {
        issues::Project {
            id: format!("repo/{name}"),
            name: name.into(),
        }
    }
    #[test]
    fn checkout_scope_infers_projects_and_replaces_saved_mappings() {
        let atlas = scope_project("Atlas");
        let beacon = scope_project("Beacon");
        let mut c = Settings {
            directory: "/old".into(),
            projects: vec!["old".into()],
            ..Settings::default()
        };
        configure_scope(
            &mut c,
            &atlas,
            std::path::Path::new("/shell"),
            vec![],
            vec![
                (atlas.clone(), "/work/atlas".into()),
                (beacon.clone(), "/work/beacon".into()),
            ],
            false,
        )
        .unwrap();
        assert_eq!(c.projects, [atlas.id.clone(), beacon.id.clone()]);
        assert!(c.directory.is_empty());
        assert_eq!(c.directories[&atlas.id], "/work/atlas");
        assert_eq!(c.directories[&beacon.id], "/work/beacon");
        configure_scope(
            &mut c,
            &atlas,
            std::path::Path::new("/shell"),
            vec![],
            vec![],
            true,
        )
        .unwrap();
        assert!(c.projects.is_empty() && c.directories.is_empty() && c.directory.is_empty());
    }
    #[test]
    fn checkout_scope_respects_project_filters_and_rejects_ambiguous_checkouts() {
        let atlas = scope_project("Atlas");
        let beacon = scope_project("Beacon");
        let mut c = Settings::default();
        configure_scope(
            &mut c,
            &atlas,
            std::path::Path::new("/shell"),
            vec![atlas.id.clone(), beacon.id.clone()],
            vec![(atlas.clone(), "/work/atlas".into())],
            false,
        )
        .unwrap();
        assert_eq!(c.directories.len(), 1);
        assert!(!c.directories.contains_key(&beacon.id));
        assert!(
            configure_scope(
                &mut c,
                &atlas,
                std::path::Path::new("/shell"),
                vec![atlas.id.clone()],
                vec![
                    (atlas.clone(), "/work/atlas".into()),
                    (beacon, "/work/beacon".into())
                ],
                false
            )
            .is_err()
        );
        assert!(
            configure_scope(
                &mut c,
                &atlas,
                std::path::Path::new("/shell"),
                vec![],
                vec![
                    (atlas.clone(), "/work/atlas".into()),
                    (atlas.clone(), "/work/atlas-other".into())
                ],
                false
            )
            .is_err()
        );
        configure_scope(
            &mut c,
            &atlas,
            std::path::Path::new("/shell"),
            vec!["named:Custom".into()],
            vec![(atlas.clone(), "/work/atlas".into())],
            false,
        )
        .unwrap();
        assert_eq!(c.directory, "/work/atlas");
        assert!(c.directories.is_empty());
    }
    #[test]
    fn explicit_checkouts_cannot_be_combined_with_all_projects() {
        assert!(
            TestCli::try_parse_from(["worker", "-C", "/work/atlas", "--all-projects"]).is_err()
        );
    }
    #[test]
    fn checkout_flags_are_repeatable_and_forwarded_to_the_remote_host() {
        let cli = TestCli::try_parse_from([
            "worker",
            "-C",
            "/work/atlas",
            "--cwd",
            "/work/beacon",
            "--directory",
            "/work/other project",
            "--project",
            "Atlas",
            "--project",
            "Beacon",
        ])
        .unwrap();
        let args = remote_arguments(&cli.options);
        let paths: Vec<_> = args
            .windows(2)
            .filter(|a| a[0] == "--cwd")
            .map(|a| a[1].as_str())
            .collect();
        assert_eq!(
            paths,
            ["/work/atlas", "/work/beacon", "/work/other project"]
        );
        assert_eq!(args.iter().filter(|a| *a == "--project").count(), 2);
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
            "--worktree",
            "--json",
            "status",
        ]);
        let args = remote_arguments(&cli.options);
        assert!(!args.contains(&"--host".into()));
        assert!(args.contains(&"--all-projects".into()));
        assert!(args.contains(&"--worktree".into()));
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

    #[test]
    fn watch_is_bounded_on_request_and_forwarded_without_starting_a_worker() {
        let cli = TestCli::try_parse_from([
            "worker", "--host", "devbox", "--id", "saved", "--json", "watch", "--count", "2",
        ])
        .unwrap();
        assert!(matches!(
            cli.options.action,
            Some(Action::Watch { count: Some(2) })
        ));
        assert_eq!(
            &remote_arguments(&cli.options)[remote_arguments(&cli.options).len() - 3..],
            ["watch", "--count", "2"]
        );
        assert!(!dashboard_enabled(&cli.options));
        assert!(TestCli::try_parse_from(["worker", "watch", "--count", "0"]).is_err());
    }

    #[test]
    fn watch_collects_all_workers_preserves_active_runs_and_limits_history() {
        use serde_json::json;
        let mut calls = Vec::new();
        let value = watch_snapshot(None, 1, |id| {
            calls.push(id.clone());
            Ok(json!({"ok":true,"worker_id":id.unwrap_or("first".into()),
                "workers":[{"id":"first"},{"id":"second"}],
                "runs":[{"id":"old1","finished_at":1},
                        {"id":"active","finished_at":null},
                        {"id":"old2","finished_at":2}]}))
        })
        .unwrap();
        assert_eq!(calls, [None, Some("second".into())]);
        assert_eq!(value["snapshots"].as_array().unwrap().len(), 2);
        for snapshot in value["snapshots"].as_array().unwrap() {
            assert_eq!(snapshot["runs"].as_array().unwrap().len(), 2);
            assert_eq!(snapshot["runs"][1]["id"], "active");
            assert!(snapshot.get("workers").is_none());
        }
    }

    #[test]
    fn watch_selection_empty_inventory_and_errors_are_explicit() {
        use serde_json::json;
        let selected = watch_snapshot(Some("chosen".into()), 0, |id| {
            assert_eq!(id.as_deref(), Some("chosen"));
            Ok(json!({"worker_id":"chosen","workers":[{"id":"chosen"}],
                "runs":[{"finished_at":1},{"finished_at":null}]}))
        })
        .unwrap();
        assert_eq!(
            selected["snapshots"][0]["runs"].as_array().unwrap().len(),
            1
        );
        let empty = watch_snapshot(None, 0, |_| Ok(json!({"workers":[],"runs":[]}))).unwrap();
        assert!(watch_text(&empty).contains("No workers registered."));
        assert!(watch_snapshot(None, 0, |_| Err(Error::invalid("disconnected"))).is_err());
    }

    #[test]
    fn watch_text_distinguishes_history_offline_and_empty_and_strips_controls() {
        use serde_json::json;
        let value = json!({"observed_at":0,"store":{"host":"test"},
            "workers":[{"id":"one","pid":null,"config":{"enabled":true}}],
            "snapshots":[{"worker_id":"one","config":{"name":"Demo\u{001b}[2J\nspoof",
                "concurrency":2},"active":0,"eligible":3,
                "runs":[{"project_name":"Atlas","number":7,"title":"Repair",
                    "state":"completed","finished_at":1,"last_event":"","summary":"Verified"}]}]});
        let text = watch_text(&value);
        assert!(text.contains("offline · 0/2 busy · 3 eligible"));
        assert!(text.contains("No active agents."));
        assert!(text.contains("completed (history)"));
        assert!(text.contains("Verified"));
        assert!(!text.contains('\u{001b}'));
        assert!(!text.contains("\nspoof"));
    }
}
