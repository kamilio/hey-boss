//! A per-user owner, independent of the terminal or worker requesting startup.
use clap::Subcommand;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const UNIT: &str = "hey-gh.service";
const LABEL: &str = "local.hey-gh";
const ENDPOINT: &str = "http://127.0.0.1:8787/";

#[derive(Subcommand)]
pub enum Action {
    /// Install and start the user service; reuse any existing listener.
    Start,
    /// Check authenticated local API health and user-service ownership.
    Status,
    /// Recover/reload only this managed service, never another owner's daemon.
    Restart,
    #[command(hide = true)]
    Run,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn checked(command: &mut Command) -> io::Result<()> {
    let result = command.output().map_err(|e| io::Error::other(format!(
        "user service manager unavailable: {e}; run as the logged-in OS user; on Linux enable the systemd user manager (loginctl enable-linger USER)"
    )))?;
    if !result.status.success() {
        return Err(io::Error::other(format!(
            "user service manager failed ({}): {}; check {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim(),
            if cfg!(target_os = "macos") {
                "launchctl print gui/$(id -u)/local.hey-gh in the logged-in desktop session"
            } else {
                "systemctl --user status hey-gh.service and journalctl --user -u hey-gh.service; ensure a user manager is available (loginctl enable-linger USER)"
            }
        )));
    }
    Ok(())
}

fn domain() -> io::Result<String> {
    let output = Command::new("/usr/bin/id").arg("-u").output()?;
    if !output.status.success() {
        return Err(io::Error::other("cannot determine service user"));
    }
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

fn active(domain: &str) -> bool {
    let result = if cfg!(target_os = "macos") {
        Command::new("/bin/launchctl")
            .args(["print", &format!("{domain}/{LABEL}")])
            .output()
    } else {
        Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", UNIT])
            .output()
    };
    result.is_ok_and(|o| o.status.success())
}

fn bootstrap(launchctl: &Path, domain: &str, registration: &Path) -> io::Result<()> {
    // bootout returns before launchd has fully released the registration.
    // Retry only its transient I/O error; never unload or kill another owner.
    let mut last = None;
    for delay in [0, 100, 200, 400, 800, 1000, 2000, 2000, 2000] {
        std::thread::sleep(Duration::from_millis(delay));
        let result = Command::new(launchctl)
            .args(["bootstrap", domain])
            .arg(registration)
            .output()?;
        if result.status.success() {
            return Ok(());
        }
        let retry = result.status.code() == Some(5);
        last = Some(result);
        if !retry {
            break;
        }
    }
    let failure = last.unwrap();
    Err(io::Error::other(format!(
        "user service bootstrap failed ({}): {}; inspect launchctl print {domain}/{LABEL}, then retry hey-gh service start",
        failure.status,
        String::from_utf8_lossy(&failure.stderr).trim()
    )))
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn systemd(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}
fn definition(macos: bool, executable: &str, env: &[(String, String)]) -> String {
    if macos {
        let environment: String = env
            .iter()
            .map(|(k, v)| format!("<key>{}</key><string>{}</string>", xml(k), xml(v)))
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array><string>{}</string><string>service</string><string>run</string></array><key>EnvironmentVariables</key><dict>{environment}</dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>15</integer></dict></plist>\n",
            xml(executable)
        )
    } else {
        let environment: String = env
            .iter()
            .map(|(k, v)| {
                format!(
                    "Environment={}\n",
                    systemd(&format!("{k}={v}")).replace("$$", "$")
                )
            })
            .collect();
        format!(
            "[Unit]\nDescription=Shared cached GitHub API\nStartLimitIntervalSec=0\n\n[Service]\nExecStart={} service run\n{environment}Restart=always\nRestartSec=15\nKillMode=control-group\nTimeoutStopSec=10\n\n[Install]\nWantedBy=default.target\n",
            systemd(executable)
        )
    }
}

fn write_definition(path: &Path, content: &str) -> io::Result<bool> {
    if fs::read_to_string(path).ok().as_deref() == Some(content) {
        return Ok(false);
    }
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result.map(|()| true)
}

fn install(restart: bool) -> Result<()> {
    let home = dirs::home_dir().ok_or("cannot determine service home")?;
    let state = home.join(".local/share/hey-gh");
    fs::create_dir_all(&state)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("service.lock"))?;
    lock.lock()?;
    let domain = if cfg!(target_os = "macos") {
        domain()?
    } else {
        String::new()
    };
    // Fail before writing a registration when no durable owner is available.
    if cfg!(target_os = "macos") {
        checked(Command::new("/bin/launchctl").args(["print", &domain]))?;
    } else {
        checked(Command::new("systemctl").args(["--user", "show-environment"]))?;
    }
    let executable = std::env::current_exe()?;
    let mut paths: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    paths.extend([
        home.join(".local/bin"),
        home.join(".cargo/bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        "/usr/bin".into(),
        "/bin".into(),
    ]);
    let mut env = vec![(
        "PATH".into(),
        std::env::join_paths(paths)?.to_string_lossy().into_owned(),
    )];
    // Preserve location overrides, but never persist credential values in service files.
    for key in ["GH_CONFIG_DIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"] {
        if let Ok(value) = std::env::var(key) {
            env.push((key.into(), value));
        }
    }
    let path = if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents/local.hey-gh.plist")
    } else {
        dirs::config_dir()
            .ok_or("cannot determine user config directory")?
            .join("systemd/user/hey-gh.service")
    };
    let changed = write_definition(
        &path,
        &definition(
            cfg!(target_os = "macos"),
            executable
                .to_str()
                .ok_or("service executable path is not UTF-8")?,
            &env,
        ),
    )?;
    let running = active(&domain);
    if cfg!(target_os = "macos") {
        if running && restart {
            checked(
                Command::new("/bin/launchctl").args(["bootout", &format!("{domain}/{LABEL}")]),
            )?;
        }
        if !running || restart {
            bootstrap(Path::new("/bin/launchctl"), &domain, &path)?;
        }
    } else {
        if changed {
            checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        }
        checked(Command::new("systemctl").args(["--user", "enable", UNIT]))?;
        checked(Command::new("systemctl").args([
            "--user",
            if running && restart {
                "restart"
            } else {
                "start"
            },
            UNIT,
        ]))?;
    }
    Ok(())
}

async fn healthy() -> Result<()> {
    let client = hey_gh::ApiClient::new(ENDPOINT.parse()?)?;
    tokio::time::timeout(Duration::from_secs(2), client.status()).await??;
    Ok(())
}

// Stand by while any owner occupies the port, even during slow authentication.
// serve also binds before authentication and holds the cache lock throughout.
async fn occupied(address: &str) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(500),
            tokio::net::TcpStream::connect(address)
        )
        .await,
        Ok(Ok(_))
    )
}

async fn supervise(
    address: &str,
    mut command: tokio::process::Command,
    retry: Duration,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    tokio::pin!(shutdown);
    loop {
        if !occupied(address).await {
            let mut child = command.kill_on_drop(true).spawn()?;
            tokio::select! {
                result = child.wait() => { eprintln!("hey-gh service: daemon exited: {}; retrying in {} seconds", result?, retry.as_secs()); }
                _ = &mut shutdown => {
                    if let Some(id) = child.id() {
                        let _ = tokio::process::Command::new("/bin/kill")
                            .args(["-TERM", &id.to_string()]).status().await;
                    }
                    if tokio::time::timeout(Duration::from_secs(6), child.wait()).await.is_err() {
                        child.kill().await?;
                    }
                    return Ok(());
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(retry) => {},
            _ = &mut shutdown => return Ok(()),
        }
    }
}

pub async fn run(action: &Action) -> Result<()> {
    if matches!(action, Action::Run) {
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command.arg("serve");
        return supervise(
            "127.0.0.1:8787",
            command,
            Duration::from_secs(15),
            super::shutdown_signal(),
        )
        .await;
    }
    if matches!(action, Action::Start | Action::Restart) {
        install(matches!(action, Action::Restart))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            match healthy().await {
                Ok(()) => break,
                Err(e) if tokio::time::Instant::now() >= deadline => return Err(format!("service registered, but local API is not healthy: {e}; inspect hey-gh logs and the user service manager; run gh auth status as the service user, then hey-gh service restart. An occupied port is never replaced.").into()),
                Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
    }
    healthy().await.map_err(|e| {
        format!("local API unavailable: {e}; run hey-gh service start, then inspect hey-gh logs")
    })?;
    let domain = if cfg!(target_os = "macos") {
        domain()?
    } else {
        String::new()
    };
    let managed = active(&domain);
    if !managed && !matches!(action, Action::Status) {
        return Err("local API is healthy but the user service is not active; inspect the service manager and rerun hey-gh service start".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"endpoint": ENDPOINT, "healthy": true, "service_active": managed, "ownership": if managed { "managed or standing by for existing owner" } else { "existing external owner" }})
        )?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_retries_async_unload_but_reports_permanent_failures() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let command = dir.path().join("launchctl");
        let calls = dir.path().join("calls");
        fs::write(&command, format!(
            "#!/bin/sh\nif test -e '{}'; then echo retried >> '{}'; exit 0; fi\necho first > '{}'\necho 'Bootstrap failed: 5: Input/output error' >&2\nexit 5\n",
            calls.display(), calls.display(), calls.display()
        )).unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
        bootstrap(&command, "gui/123", &dir.path().join("job.plist")).unwrap();
        assert_eq!(fs::read_to_string(&calls).unwrap(), "first\nretried\n");
        fs::write(
            &command,
            "#!/bin/sh\necho invalid-registration >&2\nexit 78\n",
        )
        .unwrap();
        let error = bootstrap(&command, "gui/123", &dir.path().join("job.plist"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid-registration"));
        assert!(error.contains("retry hey-gh service start"));
    }
    #[test]
    fn unavailable_manager_returns_actionable_failure() {
        let missing = tempfile::tempdir().unwrap().path().join("missing-manager");
        let error = checked(&mut Command::new(missing)).unwrap_err().to_string();
        assert!(error.contains("user service manager unavailable"));
        assert!(error.contains("loginctl enable-linger"));
        let error = checked(Command::new("/bin/sh").args(["-c", "echo unavailable >&2; exit 1"]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("unavailable"));
        assert!(error.contains("check"));
    }

    #[tokio::test]
    async fn owner_waits_for_existing_listener_and_recovers_clean_or_failed_exits() {
        for exit in [0, 1] {
            let dir = tempfile::tempdir().unwrap();
            let calls = dir.path().join("calls");
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let mut command = tokio::process::Command::new("/bin/sh");
            command.args([
                "-c",
                &format!("echo start >> '{}' ; exit {exit}", calls.display()),
            ]);
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let owner = supervise(&address, command, Duration::from_millis(20), async {
                let _ = stopped.await;
            });
            let requester = async {
                tokio::time::sleep(Duration::from_millis(70)).await;
                assert!(
                    !calls.exists(),
                    "must not compete with an occupied listener"
                );
                drop(listener);
                let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                loop {
                    if fs::read_to_string(&calls)
                        .unwrap_or_default()
                        .lines()
                        .count()
                        >= 2
                    {
                        break;
                    }
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "owner did not recover exit {exit}"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                stop.send(()).unwrap();
            };
            let (result, ()) = tokio::join!(owner, requester);
            result.unwrap();
        }
    }
    #[test]
    fn service_definitions_survive_successful_exit_and_escape_paths() {
        let env = vec![("PATH".into(), "/test & space/bin".into())];
        let unit = definition(false, "/test % \"$/hey-gh", &env);
        assert!(unit.contains("Restart=always\n"));
        assert!(unit.contains("KillMode=control-group"));
        assert!(unit.contains("ExecStart=\"/test %% \\\"$$/hey-gh\" service run"));
        assert!(!unit.contains("GH_TOKEN"));
        let plist = definition(true, "/test & space/hey-gh", &env);
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
        assert!(plist.contains("/test &amp; space/hey-gh"));
    }
    #[test]
    fn registrations_are_idempotent_and_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user/hey-gh.service");
        assert!(write_definition(&path, "first").unwrap());
        assert!(!write_definition(&path, "first").unwrap());
        assert!(write_definition(&path, "second").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
}
