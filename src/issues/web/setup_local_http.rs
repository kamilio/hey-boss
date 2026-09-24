//! One-time macOS provisioning. This utility never executes Hey Boss as root.
//! rustc --edition 2024 src/issues/web/setup_local_http.rs -o /tmp/hey-boss-local-http-setup
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Command;

const PLIST: &str = "/Library/LaunchDaemons/local.hey-boss.web.plist";
const JOB: &str = "system/local.hey-boss.web";
const MARKER: &str = "# hey-boss local-http";
const MANAGED: &str = "<!-- Managed by hey-boss local-http setup -->";

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

fn hosts(contents: &str, install: bool) -> io::Result<String> {
    let mut result = String::new();
    let mut present = false;
    for line in contents.split_inclusive('\n') {
        if line.trim_end() == format!("127.0.0.1 hey-boss.test {MARKER}") {
            if install {
                result.push_str(line);
                present = true;
            }
            continue;
        }
        let fields: Vec<_> = line.split('#').next().unwrap().split_whitespace().collect();
        if fields.iter().skip(1).any(|name| {
            name.trim_end_matches('.')
                .eq_ignore_ascii_case("hey-boss.test")
        }) {
            if install && fields.first() != Some(&"127.0.0.1") {
                return Err(invalid(
                    "/etc/hosts already maps hey-boss.test elsewhere; resolve that conflict first",
                ));
            }
            present = true;
        }
        result.push_str(line);
    }
    if install && !present {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&format!("127.0.0.1 hey-boss.test {MARKER}\n"));
    }
    Ok(result)
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn valid_origin(origin: &str) -> bool {
    let Some(authority) = origin.strip_prefix("https://") else {
        return false;
    };
    let host = match authority.split_once(':') {
        Some((host, port))
            if port
                .parse::<u16>()
                .is_ok_and(|number| number != 0 && number != 443 && number.to_string() == port) =>
        {
            host
        }
        Some(_) => return false,
        None => authority,
    };
    host.len() <= 253
        && host.ends_with(".ts.net")
        && host.split('.').count() >= 4
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn plist(user: &str, home: &str, binary: &str, origin: Option<&str>) -> String {
    let origin = origin
        .map(|origin| {
            format!(
                "<string>--mobile-origin</string><string>{}</string>",
                xml(origin)
            )
        })
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
{MANAGED}
<plist version="1.0"><dict>
<key>Label</key><string>local.hey-boss.web</string>
<key>UserName</key><string>{user}</string>
<key>ProgramArguments</key><array><string>{binary}</string><string>issue</string><string>web</string><string>--json</string>{origin}</array>
<key>WorkingDirectory</key><string>{home}</string>
<key>EnvironmentVariables</key><dict>
<key>HOME</key><string>{home}</string>
<key>PATH</key><string>{home}/.local/bin:{home}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
</dict>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>5</integer>
<key>Sockets</key><dict><key>HTTP</key><dict>
<key>SockNodeName</key><string>127.0.0.1</string>
<key>SockServiceName</key><string>80</string>
<key>SockFamily</key><string>IPv4</string>
<key>SockType</key><string>stream</string>
<key>SockProtocol</key><string>TCP</string>
</dict></dict>
<key>StandardOutPath</key><string>/var/log/hey-boss-web.log</string>
<key>StandardErrorPath</key><string>/var/log/hey-boss-web.log</string>
</dict></plist>
"#,
        user = xml(user),
        home = xml(home),
        binary = xml(binary)
    )
}

fn output(command: &mut Command) -> io::Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(invalid(&String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn replace(path: &Path, contents: &str, mode: u32) -> io::Result<()> {
    let temporary = path.with_extension(format!("hey-boss-{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)?;
    let result = (|| {
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !cfg!(target_os = "macos")
        || !matches!(
            args.first().map(String::as_str),
            Some("install" | "uninstall")
        )
    {
        return Err(invalid(
            "macOS usage: sudo hey-boss-local-http-setup install /absolute/path/hey-boss [--mobile-origin HTTPS_ORIGIN]\n       sudo hey-boss-local-http-setup uninstall",
        ));
    }
    if output(Command::new("/usr/bin/id").arg("-u"))? != "0" {
        return Err(invalid(
            "Run this setup utility with sudo; never run the Hey Boss web service as root",
        ));
    }
    let original_hosts = fs::read_to_string("/etc/hosts")?;
    let existing = match fs::read_to_string(PLIST) {
        Ok(contents) if contents.contains(MANAGED) => Some(contents),
        Ok(_) => {
            return Err(invalid(
                "An unmanaged local.hey-boss.web.plist already exists; refusing to overwrite it",
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if args[0] == "uninstall" {
        if args.len() != 1 {
            return Err(invalid("uninstall takes no arguments"));
        }
        if existing.is_some()
            && Command::new("/bin/launchctl")
                .args(["print", JOB])
                .output()?
                .status
                .success()
        {
            output(Command::new("/bin/launchctl").args(["bootout", JOB]))?;
        }
        if existing.is_some() {
            fs::remove_file(PLIST)?;
        }
        let restored = hosts(&original_hosts, false)?;
        if restored != original_hosts {
            replace(
                Path::new("/etc/hosts"),
                &restored,
                fs::metadata("/etc/hosts")?.permissions().mode(),
            )?;
        }
        println!(
            "Removed local HTTP setup. Start hey-boss issue web as your normal user for port 4781."
        );
        return Ok(());
    }
    if !(args.len() == 2 || (args.len() == 4 && args[2] == "--mobile-origin")) {
        return Err(invalid(
            "install requires an absolute installed CLI path and optional --mobile-origin HTTPS_ORIGIN",
        ));
    }
    if args.get(3).is_some_and(|origin| !valid_origin(origin)) {
        return Err(invalid(
            "--mobile-origin must be a private HTTPS Tailscale origin without a path, matching hey-boss issue web",
        ));
    }
    let binary = Path::new(&args[1]);
    if !binary.is_absolute()
        || !binary.is_file()
        || fs::metadata(binary)?.permissions().mode() & 0o111 == 0
    {
        return Err(invalid(
            "The installed CLI must be an absolute executable file path",
        ));
    }
    let user = std::env::var("SUDO_USER")
        .map_err(|_| invalid("Invoke sudo from the account that owns the Hey Boss installation"))?;
    if user.is_empty()
        || !user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        || output(Command::new("/usr/bin/id").args(["-u", &user]))? == "0"
    {
        return Err(invalid("The web service must belong to a non-root user"));
    }
    let record = output(Command::new("/usr/bin/dscl").args([
        ".",
        "-read",
        &format!("/Users/{user}"),
        "NFSHomeDirectory",
    ]))?;
    let home = record
        .strip_prefix("NFSHomeDirectory: ")
        .filter(|home| Path::new(home).is_absolute())
        .ok_or_else(|| invalid("Cannot resolve the user's home directory"))?;
    let updated_hosts = hosts(&original_hosts, true)?;
    let definition = plist(&user, home, &args[1], args.get(3).map(String::as_str));
    if existing.as_ref().is_some_and(|old| old != &definition) {
        return Err(invalid(
            "Existing setup differs; uninstall it before changing its user, executable, or Tailscale origin",
        ));
    }
    if existing.is_none() {
        replace(Path::new(PLIST), &definition, 0o644)?;
    }
    if updated_hosts != original_hosts {
        replace(
            Path::new("/etc/hosts"),
            &updated_hosts,
            fs::metadata("/etc/hosts")?.permissions().mode(),
        )?;
    }
    output(Command::new("/usr/bin/plutil").args(["-lint", PLIST]))?;
    if Command::new("/bin/launchctl")
        .args(["print", JOB])
        .output()?
        .status
        .success()
    {
        println!("Local HTTP setup is already loaded. Verify both URLs.");
        return Ok(());
    }
    for port in [4781, 80] {
        if std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            std::time::Duration::from_secs(1),
        )
        .is_ok()
        {
            return Err(invalid(&format!(
                "Setup saved, but port {port} already has a listener. Stop that service normally, then run: sudo launchctl bootstrap system {PLIST}"
            )));
        }
    }
    output(Command::new("/bin/launchctl").args(["bootstrap", "system", PLIST]))?;
    println!(
        "Loaded an unprivileged Hey Boss service. Verify http://127.0.0.1:4781/ and http://hey-boss.test/."
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Local HTTP setup: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_proxy_configuration_is_rejected_before_installation() {
        for origin in [
            "https://mac.example.ts.net",
            "https://dev-box.example.ts.net:8443",
        ] {
            assert!(valid_origin(origin));
        }
        for origin in [
            "http://mac.example.ts.net",
            "https://evil.test",
            "https://mac.example.ts.net/",
            "https://mac.example.ts.net:443",
            "https://mac.example.ts.net:08443",
            "https://mac.example.ts.net:0",
            "https://mac.example.ts.net:65536",
            "https://user@mac.example.ts.net",
            "https://mac.example.ts.net?x",
            "https://mac.example.ts.net#x",
            "https://mac..ts.net",
        ] {
            assert!(!valid_origin(origin), "{origin}");
        }
    }

    #[test]
    fn hosts_install_is_idempotent_and_preserves_unrelated_content() {
        let original = "# local hosts\n127.0.0.1 localhost\n::1 localhost\n192.0.2.1 other.test\n";
        let installed = hosts(original, true).unwrap();
        assert!(installed.starts_with(original));
        assert_eq!(hosts(&installed, true).unwrap(), installed);
        assert_eq!(hosts(&installed, false).unwrap(), original);
    }

    #[test]
    fn existing_aliases_are_preserved_and_conflicts_are_rejected() {
        let original = "127.0.0.1 localhost HEY-BOSS.TEST # user-managed\n";
        assert_eq!(hosts(original, true).unwrap(), original);
        assert_eq!(hosts(original, false).unwrap(), original);
        for original in [
            "192.0.2.1 hey-boss.test\n",
            "::1 HEY-BOSS.TEST.\n",
            "127.0.0.1 hey-boss.test\n192.0.2.1 hey-boss.test\n",
        ] {
            assert!(hosts(original, true).is_err());
        }
        assert!(
            hosts("# hey-boss.test\n127.0.0.1 hey-boss.test.example", true)
                .unwrap()
                .ends_with(&format!("127.0.0.1 hey-boss.test {MARKER}\n"))
        );
    }

    #[test]
    fn launchd_runs_as_user_with_only_an_ipv4_loopback_socket() {
        let definition = plist(
            "test-user",
            "/Users/Test & User",
            "/Users/Test & User/.local/bin/hey-boss",
            Some("https://mac.example.ts.net"),
        );
        assert!(definition.contains("<key>UserName</key><string>test-user</string>"));
        assert!(definition.contains("<key>SockNodeName</key><string>127.0.0.1</string>"));
        assert!(definition.contains("<key>SockServiceName</key><string>80</string>"));
        assert!(definition.contains("<key>SockFamily</key><string>IPv4</string>"));
        assert!(definition.contains("/Users/Test &amp; User/.local/bin/hey-boss"));
        assert!(definition.contains(
            "<string>--mobile-origin</string><string>https://mac.example.ts.net</string>"
        ));
        assert!(!definition.contains("<string>root</string>"));
    }
}
