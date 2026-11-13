// A system LaunchDaemon cannot read the user's login Keychain. Ask native gh
// in the desktop session, returning its output over a private socket, never a file.
use super::*;
use std::{
    io::{BufRead, BufReader, Read},
    os::unix::net::UnixListener,
    time::Instant,
};

pub(super) fn desktop_token() -> Result<String> {
    let dir = crate::admin::Temporary::new()?;
    let socket = dir.0.join("gh.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let label = format!(
        "local.hey-boss.review-auth.{}",
        dir.0.file_name().unwrap().to_string_lossy()
    );
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let args = [
        "/bin/sh",
        "-c",
        r#"unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN; { HEY_GH_NATIVE_CALL=1 gh auth token --hostname github.com; printf '\n'; } | /usr/bin/nc -U "$1""#,
        "review-auth",
        socket
            .to_str()
            .ok_or_else(|| Error::invalid("Invalid credential socket path"))?,
    ];
    let args = args
        .iter()
        .map(|s| format!("<string>{}</string>", html_escape::encode_text(s)))
        .collect::<String>();
    let path = std::env::var("PATH").unwrap_or_default();
    let plist = dir.0.join("auth.plist");
    std::fs::write(
        &plist,
        format!(
            "<plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{args}</array><key>EnvironmentVariables</key><dict><key>PATH</key><string>{}</string></dict><key>RunAtLoad</key><true/></dict></plist>",
            html_escape::encode_text(&path)
        ),
    )?;
    if !Command::new("/bin/launchctl")
        .args(["bootstrap", &domain])
        .arg(plist)
        .output()?
        .status
        .success()
    {
        return Err(Error::invalid(
            "Code Reviews could not access your logged-in Mac session. Log in to the desktop before publishing.",
        ));
    }
    let start = Instant::now();
    let result = loop {
        match listener.accept() {
            Ok((stream, _)) => {
                break (|| -> Result<String> {
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
                    let mut token = String::new();
                    BufReader::new(stream.take(4096)).read_line(&mut token)?;
                    if token.trim().is_empty() {
                        return Err(Error::invalid(
                            "No personal github.com login was available in the Mac desktop session. Check gh auth status there.",
                        ));
                    }
                    Ok(token)
                })();
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && start.elapsed() < Duration::from_secs(10) =>
            {
                std::thread::sleep(Duration::from_millis(25))
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                break Err(Error::invalid(
                    "Timed out waiting for the desktop GitHub credential.",
                ));
            }
            Err(e) => break Err(e.into()),
        }
    };
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &format!("{domain}/{label}")])
        .output();
    result
}
