//! Health RPC over the existing SSH inventory. Arguments never become shell syntax.
use super::output;
use std::collections::BTreeSet;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-:[]".contains(&b))
}

pub fn hosts() -> io::Result<Vec<String>> {
    let home = PathBuf::from(
        std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is unavailable"))?,
    );
    let mut hosts = BTreeSet::new();
    for (file, key) in [
        (".hey-boss/config.json", "ssh_hosts"),
        (".local/share/hey-boss/connections.json", "machines"),
    ] {
        let path = home.join(file);
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if bytes.len() > 1024 * 1024 {
            return Err(io::Error::other("Host inventory exceeds 1 MiB"));
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        if let Some(entries) = value.get(key).and_then(|v| v.as_array()) {
            for entry in entries {
                if key == "machines"
                    && entry.get("state").and_then(|v| v.as_str()) != Some("connected")
                {
                    continue;
                }
                if let Some(host) = entry
                    .as_str()
                    .or_else(|| entry.get("host").and_then(|v| v.as_str()))
                    && valid_host(host)
                {
                    hosts.insert(host.to_owned());
                }
            }
        }
    }
    Ok(hosts.into_iter().take(32).collect())
}

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn script(args: &[String]) -> io::Result<String> {
    if args.iter().any(|s| s.contains('\0')) {
        return Err(io::Error::other("Invalid argument"));
    }
    let args = args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ");
    Ok(format!(
        r#"export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"; for worker in "$HOME/.local/bin/hey-harvester" "$HOME/.cargo/bin/hey-harvester" /opt/homebrew/bin/hey-harvester /usr/local/bin/hey-harvester; do if test -x "$worker"; then exec "$worker" {args}; fi; done; for worker in "$HOME/.local/bin/hey-boss-health" "$HOME/.local/bin/hey-boss" /opt/homebrew/bin/hey-boss /usr/local/bin/hey-boss; do if test -x "$worker"; then exec "$worker" health {args}; fi; done; echo 'Install hey-harvester on this machine.' >&2; exit 127"#
    ))
}

pub fn execute(host: &str, args: &[String], control: Option<PathBuf>) -> io::Result<Vec<u8>> {
    if !valid_host(host) {
        return Err(io::Error::other("Invalid SSH host"));
    }
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "ConnectionAttempts=1",
        "-o",
        "ServerAliveInterval=10",
        "-o",
        "ServerAliveCountMax=2",
    ]);
    if let Some(control) = control.filter(|p| p.exists()) {
        cmd.arg("-S").arg(control);
    }
    cmd.arg(host).arg(script(args)?);
    let timeout = if args.first().is_some_and(|a| a == "status" || a == "logs") {
        30
    } else {
        300
    };
    let result = output(&mut cmd, Duration::from_secs(timeout))?;
    if !result.status.success() {
        return Err(io::Error::other(format!(
            "{host}: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(result.stdout)
}

/// Reuse the companion's existing SSH multiplex connection when available.
pub fn control_path(host: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    host.hash(&mut hash);
    Some(
        PathBuf::from(std::env::var_os("HOME")?)
            .join(".local/share/hey-boss")
            .join(format!("ssh-{:016x}.sock", hash.finish())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_arguments_are_literal_and_hosts_cannot_inject_options() {
        for value in ["-oProxyCommand=x", "a b", "a\nwhoami", "a;id", "a$(id)", ""] {
            assert!(!valid_host(value));
        }
        assert!(valid_host("user@devbox"));
        let value = "/tmp/work tree ' $(touch /tmp/unwanted)\nnext";
        let result = Command::new("sh")
            .args(["-c", &format!("printf %s {}", quote(value))])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(result.stdout).unwrap(), value);
        let command = script(&["remove-worktree".into(), value.into(), "--json".into()]).unwrap();
        assert!(command.contains(&quote(value)));
        assert!(command.contains("hey-boss-health"));
    }
}
