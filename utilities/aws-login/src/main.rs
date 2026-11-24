use serde_json::Value;
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{fs::DirBuilderExt, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// The run directory contains only a browser session ID or an error, never tokens.
struct Run(PathBuf);
impl Drop for Run {
    fn drop(&mut self) {
        if let Ok(session) = fs::read_to_string(self.0.join("session")) {
            let _ = Command::new("hey-boss")
                .args(["browser", "close", session.trim()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("aws-login: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let [mode, url] = args.as_slice() {
        if mode == "--open" {
            let directory = PathBuf::from(std::env::var("HEY_AWS_LOGIN_RUN")?);
            let result = open(url, &directory);
            if let Err(error) = &result {
                fs::write(directory.join("error"), error.to_string())?;
            }
            return result;
        }
    }
    if args == ["--help"] || args == ["-h"] {
        println!(
            "Usage: hey-boss utils aws-login [PROFILE]\n\nRun AWS CLI login on this machine, approve in your connected Mac's browser,\nand receive the callback automatically. Uses AWS_PROFILE or default when omitted.\nExisting profile identity changes are declined. AWS CLI manages credentials and refresh."
        );
        return Ok(());
    }
    if args.len() > 1
        || args
            .first()
            .is_some_and(|arg| arg.is_empty() || arg.starts_with('-'))
    {
        return Err("Usage: hey-boss utils aws-login [PROFILE]".into());
    }
    let profile = args
        .first()
        .cloned()
        .or_else(|| std::env::var("AWS_PROFILE").ok())
        .unwrap_or_else(|| "default".into());
    let stopped = Arc::new(AtomicBool::new(false));
    let signal = stopped.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    let directory = std::env::current_dir()?.join(format!(
        ".aws-login-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::DirBuilder::new().mode(0o700).create(&directory)?;
    let run = Run(directory);
    let executable = std::env::current_exe()?;
    let browser = format!(
        "'{}' --open %s",
        executable.to_string_lossy().replace('\'', "'\\''")
    );
    eprintln!("Opening AWS login for profile {profile} on your connected Mac…");
    let mut login = aws(&profile);
    login
        .arg("login")
        .env("BROWSER", browser)
        .env("HEY_AWS_LOGIN_RUN", &run.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = login.spawn()?;
    // Never silently replace an existing profile with the browser's other identity.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"n\n");
    }
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let output = std::thread::spawn(move || read(stdout));
    let errors = std::thread::spawn(move || read(stderr));
    let began = Instant::now();
    let status = loop {
        if stopped.load(Ordering::Relaxed)
            || began.elapsed() >= Duration::from_secs(300)
            || run.0.join("error").exists()
        {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            break Err(fs::read_to_string(run.0.join("error"))
                .unwrap_or_else(|_| "Login cancelled or timed out".into()));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                break Err(error.to_string());
            }
        }
    };
    let stdout = output
        .join()
        .map_err(|_| "Cannot read AWS login output")??;
    let stderr = errors
        .join()
        .map_err(|_| "Cannot read AWS login errors")??;
    let status = status?;
    if !status.success() {
        return Err(format!(
            "AWS login failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        )
        .into());
    }
    // AWS returns zero even when a profile change is declined. Require its save
    // confirmation as well as a live identity check; old cached access isn't proof.
    if !String::from_utf8_lossy(&stdout).contains(&format!("Updated profile {profile} to use ")) {
        return Err("AWS did not update the profile. Select the identity already configured for this profile in your browser and retry, or use a different profile for another organization.".into());
    }
    drop(run);
    let identity = aws(&profile)
        .args(["sts", "get-caller-identity", "--output", "json"])
        .output()?;
    if !identity.status.success() {
        return Err(format!(
            "Login completed, but identity verification failed: {}",
            String::from_utf8_lossy(&identity.stderr).trim()
        )
        .into());
    }
    let identity: Value = serde_json::from_slice(&identity.stdout)?;
    let account = identity["Account"]
        .as_str()
        .ok_or("AWS returned no account")?;
    let arn = identity["Arn"]
        .as_str()
        .ok_or("AWS returned no identity ARN")?;
    println!("Authenticated profile {profile}: {arn} (account {account})");
    Ok(())
}

fn aws(profile: &str) -> Command {
    let mut command = Command::new("aws");
    command
        .args(["--profile", profile, "--no-cli-pager"])
        .env("AWS_CLI_AUTO_PROMPT", "off");
    command
}

fn read(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(1024 * 1024).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn open(url: &str, directory: &std::path::Path) -> Result<()> {
    let params = serde_json::json!({"url":url}).to_string();
    let response = Command::new("hey-boss")
        .args(["action", "browser.authorize", "--params", &params])
        .output()?;
    if !response.status.success() {
        return Err(format!(
            "Cannot open login: {}",
            String::from_utf8_lossy(&response.stderr).trim()
        )
        .into());
    }
    let response: Value = serde_json::from_slice(&response.stdout)?;
    if response["result"]["opened"] != true {
        return Err("Browser did not acknowledge opening login".into());
    }
    let session = response["result"]["session"]
        .as_str()
        .ok_or("Browser returned no session")?;
    if let Err(error) = fs::write(directory.join("session"), session) {
        let _ = Command::new("hey-boss")
            .args(["browser", "close", session])
            .output();
        return Err(error.into());
    }
    Ok(())
}
