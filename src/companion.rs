use clap::Subcommand;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Subcommand)]
pub enum Action {
    #[command(hide = true)]
    Forward { host: String, remote_port: u16 },
    #[command(hide = true)]
    CancelForward {
        host: String,
        local_port: u16,
        remote_port: u16,
    },
    /// Import the hey-proxy machine inventory and enable config-driven connections.
    Setup,
    /// Save automatic connection settings and enable login startup on macOS.
    Configure {
        #[arg(default_value = "devbox")]
        host: String,
        #[arg(long, default_value = "quora.net")]
        vpn_domain: String,
        #[arg(long)]
        disable: bool,
    },
    /// Run the VPN-aware automatic connector (normally started at login).
    Auto {
        /// Maintain an additional host without changing the saved default.
        #[arg(long)]
        host: Option<String>,
        /// A .local LAN host does not need the default VPN.
        #[arg(long, requires = "host")]
        local_network: bool,
        #[arg(long, requires = "host")]
        vpn_domain: Option<String>,
    },
    /// Show saved settings and the automatic connector's last status.
    Status,
    /// Run the durable server queue (normally started by installation).
    Serve {
        #[arg(long)]
        state: PathBuf,
    },
    #[command(hide = true)]
    ClearStaleBridge {
        #[arg(long)]
        state: PathBuf,
    },
    /// Distribute the embedded canonical skill to this machine and an SSH server.
    SyncSkill {
        /// Omit to update every host previously installed or connected.
        host: Option<String>,
    },
    /// Install the CLI, Codex skill, and global agent permissions on a Unix SSH host.
    Install {
        #[arg(default_value = "devbox")]
        host: String,
        /// Source checkout containing Cargo.toml and skills/hey-boss.
        #[arg(long, default_value = ".")]
        source: PathBuf,
    },
    /// Keep a two-way connection open; Ctrl+C disconnects. Authenticate SSH first.
    Connect {
        #[arg(default_value = "devbox")]
        host: String,
        #[arg(long, hide = true)]
        unattended: bool,
    },
}

fn ssh(host: &str) -> Result<Command, String> {
    if host.is_empty() || host.starts_with('-') || host.chars().any(char::is_whitespace) {
        return Err("host must be an SSH host or configuration alias".into());
    }
    let mut command = Command::new("ssh");
    if std::env::var_os("HEY_BOSS_UNATTENDED").is_some() {
        unattended_ssh(&mut command);
    }
    command.args([
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
    ]);
    Ok(command)
}

fn unattended_ssh(command: &mut Command) {
    command
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "StrictHostKeyChecking=yes",
        ])
        .env("SSH_ASKPASS_REQUIRE", "never")
        // ScaleFT is the configured SSH proxy on this Mac. Its installed
        // client explicitly honors this variable instead of opening login UI.
        .env("SFT_NO_BROWSER", "1");
}

fn clear_stale_bridge(state: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::FileTypeExt;
    let path = state.join("bridge.sock");
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
        Ok(m) if !m.file_type().is_socket() => return Err("bridge path is not a socket".into()),
        Ok(_) => {}
    }
    match std::os::unix::net::UnixStream::connect(&path) {
        Ok(_) => Err("a bridge is already active".into()),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            std::fs::remove_file(path).map_err(|e| e.to_string())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot check bridge: {e}")),
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn run(action: &Action) -> Result<(), String> {
    match action {
        Action::Forward { host, remote_port } => {
            println!("{}", forward_port(host, *remote_port)?);
            return Ok(());
        }
        Action::CancelForward {
            host,
            local_port,
            remote_port,
        } => return cancel_forward(host, *local_port, *remote_port),
        Action::Setup => return super::autoconnect::setup_machines(),
        Action::Configure {
            host,
            vpn_domain,
            disable,
        } => return super::autoconnect::configure(host, vpn_domain, !disable),
        Action::Auto {
            host,
            local_network,
            vpn_domain,
        } => {
            return super::autoconnect::run(host.as_deref(), *local_network, vpn_domain.as_deref());
        }
        Action::Status => return super::autoconnect::status(),
        Action::Serve { state } => return super::broker::serve(state.clone()),
        Action::ClearStaleBridge { state } => return clear_stale_bridge(state),
        Action::SyncSkill { host } => {
            if let Some(host) = host {
                return sync_skill(host);
            }
            let hosts = std::fs::read_to_string(registry()?).unwrap_or_default();
            if hosts.is_empty() {
                return Err("no registered servers; specify an SSH host".into());
            }
            for host in hosts.lines() {
                sync_skill(host)?;
            }
            return Ok(());
        }
        Action::Install { host, source } => {
            let mut remote = ssh(host)?;
            // Archive just the companion sources, including uncommitted changes.
            let mut archive = Command::new("tar")
                .env("COPYFILE_DISABLE", "1")
                .arg("-czf")
                .arg("-")
                .arg("-C")
                .arg(source)
                .args([
                    "Cargo.toml",
                    "Cargo.lock",
                    "build.rs",
                    "src",
                    "tests",
                    "skills/hey-boss",
                    "packages/hey-gh",
                    "README.md",
                    "LICENSE",
                    "tools/upgrade_hey_boss.py",
                    "tools/drain_github_issues.py",
                    "hey_boss_daemon.swift",
                    "package_hey_boss.swift",
                    "setup_hey_boss.swift",
                    "assets",
                ])
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
            remote
                .arg(host)
                .arg(
                    r#"set -eu
umask 077
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT HUP INT TERM
tar -xzf - -C "$stage"
export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null || { echo 'Install Rust on the server first.' >&2; exit 1; }
cargo test --locked --manifest-path "$stage/Cargo.toml" --bin hey-boss --test secret_cli -- --test-threads=1
cargo build --locked --release --manifest-path "$stage/Cargo.toml"
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/hey-boss" "$HOME/.codex/skills/hey-boss"
chmod 700 "$HOME/.local/share/hey-boss"
cp "$stage/target/release/hey-gh" "$HOME/.local/bin/hey-gh.new"
chmod 755 "$HOME/.local/bin/hey-gh.new"
mv "$HOME/.local/bin/hey-gh.new" "$HOME/.local/bin/hey-gh"
if [ -e "$HOME/.cargo/bin/hey-gh" ]; then
cp "$stage/target/release/hey-gh" "$HOME/.cargo/bin/hey-gh.new"
chmod 755 "$HOME/.cargo/bin/hey-gh.new"
mv "$HOME/.cargo/bin/hey-gh.new" "$HOME/.cargo/bin/hey-gh"
fi
cp "$stage/target/release/hey-boss" "$HOME/.local/bin/hey-boss.new"
mv "$HOME/.local/bin/hey-boss.new" "$HOME/.local/bin/hey-boss"
if [ ! -e "$HOME/.local/bin/hb" ] && [ ! -L "$HOME/.local/bin/hb" ]; then
ln -s hey-boss "$HOME/.local/bin/hb"
fi
"$HOME/.local/bin/hey-boss" configure-agents --binary "$HOME/.local/bin/hey-boss"
printf '%s' "$HOME/.local/share/hey-boss" > "$HOME/.local/bin/hey-boss.state"
touch "$HOME/.local/bin/hey-boss.companion"
if command -v systemctl >/dev/null && systemctl --user show-environment >/dev/null 2>&1; then
mkdir -p "$HOME/.config/systemd/user"
cat > "$HOME/.config/systemd/user/hey-boss-companion.service" <<'UNIT'
[Unit]
Description=hey-boss durable notification companion
[Service]
ExecStart=%h/.local/bin/hey-boss companion serve --state %h/.local/share/hey-boss
Restart=on-failure
UMask=0077
[Install]
WantedBy=default.target
UNIT
systemctl --user daemon-reload
systemctl --user enable --now hey-boss-companion.service
systemctl --user restart hey-boss-companion.service
elif [ "$(uname -s)" = Darwin ]; then
mkdir -p "$HOME/Library/LaunchAgents"
python3 - <<'BROKER_PLIST'
import pathlib,plistlib
home=pathlib.Path.home()
content={"Label":"local.hey-boss-broker","ProgramArguments":[str(home/".local/bin/hey-boss"),"companion","serve","--state",str(home/".local/share/hey-boss")],"RunAtLoad":True,"KeepAlive":True,"ThrottleInterval":60,"StandardOutPath":str(home/".local/share/hey-boss/broker.log"),"StandardErrorPath":str(home/".local/share/hey-boss/broker.log")}
(home/"Library/LaunchAgents/local.hey-boss-broker.plist").write_bytes(plistlib.dumps(content))
BROKER_PLIST
domain="gui/$(id -u)"
launchctl bootout "$domain/local.hey-boss-broker" >/dev/null 2>&1 || true
launchctl bootstrap "$domain" "$HOME/Library/LaunchAgents/local.hey-boss-broker.plist"
launchctl kickstart "$domain/local.hey-boss-broker"
else
nohup "$HOME/.local/bin/hey-boss" companion serve --state "$HOME/.local/share/hey-boss" > "$HOME/.local/share/hey-boss/broker.log" 2>&1 < /dev/null &
fi
"$HOME/.local/bin/hey-boss" skill install
echo 'Installed CLI and skill. Add ~/.local/bin to PATH; connect from your Mac.'
"#,
                )
                .stdin(Stdio::from(archive.stdout.take().unwrap()));
            let status = remote.status().map_err(|e| e.to_string())?;
            let archived = archive.wait().map_err(|e| e.to_string())?;
            if !status.success() || !archived.success() {
                return Err("remote installation failed".into());
            }
            register(host)?;
        }
        Action::Connect { host, unattended } => {
            if *unattended {
                // Child process only; never change the parent's SSH behavior.
                unsafe {
                    std::env::set_var("HEY_BOSS_UNATTENDED", "1");
                }
            }
            let _connection_lock = super::autoconnect::connection_lock(host)?;
            let executable = std::env::current_exe()
                .and_then(|p| p.canonicalize())
                .map_err(|e| e.to_string())?;
            super::initialize(&executable).map_err(|error| error.to_string())?;
            let state = std::fs::read_to_string(executable.with_file_name("hey-boss.state"))
                .map_err(|e| format!("install the Mac app first: {e}"))?;
            hey_boss::require_protocol(std::path::Path::new(&state)).map_err(|e| e.to_string())?;
            let local = PathBuf::from(state).join("daemon.sock");
            let control = super::autoconnect::control_path(host)?;
            if control.exists() {
                let mut check = ssh(host)?;
                let status = check
                    .args(["-S"])
                    .arg(&control)
                    .args(["-O", "check"])
                    .arg(host)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map_err(|e| e.to_string())?;
                if status.success() {
                    return Err("an SSH companion master is already active for this host".into());
                }
                // The host connection lock excludes another managed connector.
                // Never unlink a live master or a non-socket filesystem entry.
                use std::os::unix::fs::FileTypeExt;
                let metadata = std::fs::symlink_metadata(&control).map_err(|e| e.to_string())?;
                if !metadata.file_type().is_socket() {
                    return Err("SSH control path is not a socket".into());
                }
                std::fs::remove_file(&control).map_err(|e| e.to_string())?;
            }
            let remote = prepare(host)?;
            if !remote.starts_with('/')
                || remote.contains([':', '\n', '\r'])
                || local.to_string_lossy().contains(':')
            {
                return Err(
                    "SSH socket paths must be absolute and contain no colons or newlines".into(),
                );
            }
            let mut connection = ssh(host)?;
            connection
                .args(["-o", "ControlMaster=yes", "-o", "ControlPersist=no", "-o"])
                .arg(format!(
                    "ControlPath={}",
                    super::autoconnect::control_path(host)?.display()
                ));
            // The initial bridge is owner-private; browser forwards are loopback-only.
            connection.args(["-o", "ExitOnForwardFailure=yes", "-o", "StreamLocalBindUnlink=yes", "-o", "StreamLocalBindMask=0177", "-R"])
                .arg(format!("{remote}:{}", local.display()))
                .arg(host)
                .arg(format!("trap {} EXIT HUP INT TERM; echo 'hey-boss connected. Keep this session open; Ctrl+C disconnects.'; cat >/dev/null", quote(&format!("rm -f -- {} {}", quote(&remote), quote(&PathBuf::from(&remote).with_file_name("bridge-protocol").to_string_lossy())))))
                .stdin(Stdio::inherit());
            let status = connection.status().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err("SSH connection ended with an error; reconnect to resume".into());
            }
        }
    }
    Ok(())
}

fn multiplex(host: &str, operation: &str, local: u16, remote: u16) -> Result<(), String> {
    let mut command = ssh(host)?;
    let output = command
        .args(["-S"])
        .arg(super::autoconnect::control_path(host)?)
        .args([
            "-O",
            operation,
            "-L",
            &format!("127.0.0.1:{local}:127.0.0.1:{remote}"),
        ])
        .arg(host)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "established SSH connection unavailable: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}
fn forward_port(host: &str, remote: u16) -> Result<u16, String> {
    if remote == 0 {
        return Err("remote port must be nonzero".into());
    }
    // Reserve an ephemeral loopback port; SSH reports a bind race rather than
    // attaching to someone else's listener. Retry locally, with no new login.
    for _ in 0..3 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let local = listener.local_addr().map_err(|e| e.to_string())?.port();
        drop(listener);
        match multiplex(host, "forward", local, remote) {
            Ok(()) => return Ok(local),
            Err(error)
                if error.contains("Port forwarding failed") || error.contains("cannot listen") =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    Err("could not allocate loopback browser forward".into())
}
fn cancel_forward(host: &str, local: u16, remote: u16) -> Result<(), String> {
    multiplex(host, "cancel", local, remote)
}

// Combine skill distribution and readiness lookup into one SSH setup request.
fn prepare(host: &str) -> Result<String, String> {
    let skill = hey_boss::skill::archive().map_err(|e| e.to_string())?;
    let home = std::env::var_os("HOME").ok_or("HOME is missing")?;
    hey_boss::skill::install(&PathBuf::from(home)).map_err(|e| e.to_string())?;
    let mut command = ssh(host)?;
    let mut child = command
        .arg(host)
        .arg(format!(
            r#"{}
{}
printf '1' > "$HOME/.local/share/hey-boss/bridge-protocol"
printf '%s' {} > "$HOME/.local/share/hey-boss/bridge-host"
printf '%s' {} > "$HOME/.local/share/hey-boss/bridge-generation"
printf '%s' "$HOME/.local/share/hey-boss/bridge.sock""#,
            r#"set -eu
umask 077
test -f "$HOME/.local/bin/hey-boss.companion"
test -x "$HOME/.local/bin/hey-boss"
test -S "$HOME/.local/share/hey-boss/daemon.sock"
"$HOME/.local/bin/hey-boss" companion clear-stale-bridge --state "$HOME/.local/share/hey-boss""#,
            hey_boss::skill::remote_install_script(),
            quote(host),
            quote(&format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )),
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&skill)
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(
            "SSH setup failed; authenticate manually and check the server queue service".into(),
        );
    }
    register(host)?;
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn sync_skill(host: &str) -> Result<(), String> {
    let skill = hey_boss::skill::archive().map_err(|e| e.to_string())?;
    let home = std::env::var_os("HOME").ok_or("HOME is missing")?;
    hey_boss::skill::install(&PathBuf::from(home)).map_err(|e| e.to_string())?;
    let mut command = ssh(host)?;
    let mut child = command
        .arg(host)
        .arg(hey_boss::skill::remote_install_script())
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&skill)
        .map_err(|e| e.to_string())?;
    if !child.wait().map_err(|e| e.to_string())?.success() {
        return Err("skill sync failed".into());
    }
    register(host)?;
    Ok(())
}

fn registry() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is missing")?;
    let directory = PathBuf::from(home).join(".local/share/hey-boss");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    Ok(directory.join("companion-hosts"))
}
fn register(host: &str) -> Result<(), String> {
    let path = registry()?;
    let mut hosts: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    if !hosts.iter().any(|h| h == host) {
        hosts.push(host.into());
    }
    std::fs::write(path, hosts.join("\n") + "\n").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_bridge_is_removed_but_live_sockets_and_regular_files_are_preserved() {
        let root = std::env::temp_dir().join(format!("hb-stale-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("bridge.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert!(clear_stale_bridge(&root).is_err());
        assert!(path.exists());
        drop(listener.accept().unwrap());
        drop(listener);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if clear_stale_bridge(&root).is_ok() {
                break;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!path.exists());
        std::fs::write(&path, b"keep").unwrap();
        assert!(clear_stale_bridge(&root).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unattended_proxy_cannot_open_login_browser() {
        let mut command = Command::new("ssh");
        unattended_ssh(&mut command);
        let envs: Vec<_> = command.get_envs().collect();
        assert!(
            envs.iter().any(|(key, value)| *key == "SFT_NO_BROWSER"
                && *value == Some(std::ffi::OsStr::new("1")))
        );
        assert!(envs.iter().any(|(key, value)| *key == "SSH_ASKPASS_REQUIRE"
            && *value == Some(std::ffi::OsStr::new("never"))));
        assert!(command.get_args().any(|arg| arg == "BatchMode=yes"));
        assert!(command.get_args().any(|arg| arg == "ConnectTimeout=8"));
    }

    #[test]
    fn ssh_hosts_cannot_inject_options_and_shell_paths_are_quoted() {
        for host in ["", "-oProxyCommand=bad", "devbox bad"] {
            assert!(ssh(host).is_err());
        }
        assert!(ssh("user@devbox").is_ok());
        assert_eq!(quote("/home/a'b"), "'/home/a'\\''b'");
    }
}
