//! Ephemeral credentials. Never use notification history or print credential values.
use clap::Args;
use serde::Deserialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Args)]
#[command(group(clap::ArgGroup::new("destination").required(true).args(["env_file", "stdout", "command"])))]
pub struct Options {
    #[arg(long, default_value = "Secrets")]
    project: String,
    #[arg(long, default_value = "Enter credentials")]
    title: String,
    /// Environment variable name; repeat once for a pair. Values are entered only in the UI.
    #[arg(long = "field", required = true, num_args = 1, value_parser = field_name)]
    fields: Vec<String>,
    /// Show the first field as a login; the second remains masked.
    #[arg(long)]
    login: bool,
    /// Append assignments to an owner-private file; existing requested keys are refused.
    #[arg(long, value_name = "PATH")]
    env_file: Option<PathBuf>,
    /// Emit assignments only when stdout is redirected to an empty owner-private regular file.
    #[arg(long)]
    stdout: bool,
    /// Inject fields into this child process. Child stdout/stderr are suppressed.
    #[arg(last = true, num_args = 1..)]
    command: Vec<String>,
}
fn field_name(s: &str) -> Result<String, String> {
    let mut chars = s.bytes();
    if !chars
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !chars.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || s.len() > 128
    {
        return Err("Use an environment variable name, not a value".into());
    }
    Ok(s.into())
}
fn error(message: &str) -> io::Error {
    io::Error::other(message)
}
#[derive(Debug)]
struct DeliveryError {
    delivery: &'static str,
    reason: &'static str,
    message: String,
}
impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Secret delivery={} reason={}: {}",
            self.delivery, self.reason, self.message
        )
    }
}
impl std::error::Error for DeliveryError {}
fn outcome(delivery: &'static str, reason: &'static str, message: &str) -> io::Error {
    io::Error::other(DeliveryError {
        delivery,
        reason,
        message: message.into(),
    })
}
fn not_delivered(reason: &'static str, message: &str) -> io::Error {
    outcome("not_delivered", reason, message)
}
fn private_regular(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
    {
        return Err(error(
            "Secret output requires an owned regular file with mode 600 and no hard links",
        ));
    }
    Ok(())
}
fn stdout_file() -> io::Result<File> {
    let fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if fd < 0 {
        return Err(error("Cannot open secret destination"));
    }
    use std::os::fd::FromRawFd;
    let file = unsafe { File::from_raw_fd(fd) };
    private_regular(&file).map_err(|_| error("Refusing secret output: redirect stdout to an empty private file with umask 077; terminals and pipes are forbidden"))?;
    if file.metadata()?.len() != 0 {
        return Err(error("Redirected secret destination must be empty"));
    }
    Ok(file)
}
struct EnvFile {
    path: PathBuf,
    original: Vec<u8>,
    identity: Option<(u64, u64)>,
}
impl EnvFile {
    fn open(path: &Path, fields: &[String]) -> io::Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| error("Expected a file destination"))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        let path = parent.join(name);
        // Tracked files can enter diffs/context. Never write credentials into them.
        if Command::new("git")
            .args(["ls-files", "--error-unmatch", "--"])
            .arg(name)
            .current_dir(&parent)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return Err(error("Refusing to put secrets in a Git-tracked file"));
        }
        let (original, identity) = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(mut file) => {
                private_regular(&file)?;
                let meta = file.metadata()?;
                if meta.len() > 1024 * 1024 {
                    return Err(error("Environment file is too large"));
                }
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| error("Environment file must be UTF-8"))?;
                for line in text.lines() {
                    let line = line
                        .trim_start()
                        .strip_prefix("export ")
                        .unwrap_or(line.trim_start());
                    if line
                        .split_once('=')
                        .is_some_and(|(key, _)| fields.iter().any(|f| f == key.trim()))
                    {
                        return Err(error(
                            "A requested key already exists; remove its old assignment before replacing it",
                        ));
                    }
                }
                (bytes, Some((meta.dev(), meta.ino())))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Vec::new(), None),
            Err(_) => {
                return Err(error(
                    "Cannot safely open secret destination (symlinks are refused)",
                ));
            }
        };
        Ok(Self {
            path,
            original,
            identity,
        })
    }
    fn write(self, assignments: &[u8]) -> io::Result<()> {
        let temporary = self.path.with_file_name(format!(
            ".hey-boss-secret-{}-{}",
            std::process::id(),
            SystemNonce::value()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(&self.original)?;
            if !self.original.is_empty() && !self.original.ends_with(b"\n") {
                file.write_all(b"\n")?;
            }
            file.write_all(assignments)?;
            file.sync_all()?;
            if let Some(identity) = self.identity {
                let mut current = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(&self.path)?;
                private_regular(&current)?;
                let m = current.metadata()?;
                if (m.dev(), m.ino()) != identity || m.len() != self.original.len() as u64 {
                    return Err(error(
                        "Destination changed while prompting; nothing replaced",
                    ));
                }
                let mut bytes = Vec::new();
                current.read_to_end(&mut bytes)?;
                if bytes != self.original {
                    return Err(error(
                        "Destination changed while prompting; nothing replaced",
                    ));
                }
                fs::rename(&temporary, &self.path)?;
            } else {
                // Atomic no-clobber publication when the destination did not exist.
                fs::hard_link(&temporary, &self.path)?;
                // Publication succeeded. Cleanup must not turn delivery into a failure.
            }
            Ok(())
        })();
        let _ = fs::remove_file(&temporary);
        result.map_err(|_| error("Could not safely save credentials; destination may have changed"))
    }
}
struct SystemNonce;
impl SystemNonce {
    fn value() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    }
}

#[derive(Deserialize)]
struct SecretResponse {
    status: String,
    // Decode the result only for success. Cancellation/rejection must not depend
    // on the shape of an unused (and potentially credential-bearing) payload.
    result: Option<Box<serde_json::value::RawValue>>,
}
fn receive(socket: &Path, request: &serde_json::Value) -> io::Result<Vec<String>> {
    let mut stream = UnixStream::connect(socket).map_err(|_| {
        not_delivered(
            "unavailable",
            "Connect an updated desktop companion; no request was sent.",
        )
    })?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    // Let the desktop's 900-second expiry and bounded reply write finish first.
    stream.set_read_timeout(Some(Duration::from_secs(915)))?;
    let payload = serde_json::to_vec(request)
        .map_err(|_| not_delivered("request_invalid", "Invalid secret request."))?;
    stream.write_all(&payload).and_then(|()| stream.shutdown(std::net::Shutdown::Write))
        .map_err(|_| not_delivered("transport_failed", "Request transport failed; dismiss any remaining prompt and reconnect before an explicit retry."))?;
    read_response(&mut stream)
}

fn read_response(stream: &mut UnixStream) -> io::Result<Vec<String>> {
    let mut bytes = Vec::new();
    stream
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| not_delivered("transport_failed", "Disconnected or timed out; dismiss any remaining prompt and reconnect before an explicit retry."))?;
    if bytes.len() > 1024 * 1024 {
        return Err(not_delivered(
            "response_too_large",
            "Response exceeded limit; update the desktop companion before an explicit retry.",
        ));
    }
    if bytes.is_empty() {
        return Err(not_delivered(
            "response_empty",
            "Connection closed without a reply; dismiss any remaining prompt and reconnect before an explicit retry.",
        ));
    }
    let malformed = || {
        not_delivered(
            "response_malformed",
            "Invalid response data; dismiss any remaining prompt and update the desktop companion before an explicit retry.",
        )
    };
    let reply: SecretResponse = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
    match reply.status.as_str() {
        "ok" => {}
        "cancelled" => {
            return Err(not_delivered(
                "cancelled",
                "Entry cancelled; do not request again without user authorization.",
            ));
        }
        "expired" => {
            return Err(not_delivered(
                "expired",
                "Entry expired; request again only with explicit authorization.",
            ));
        }
        "rejected" => {
            return Err(not_delivered(
                "rejected",
                "Request rejected; check field names and request metadata before retrying.",
            ));
        }
        "busy" => {
            return Err(not_delivered(
                "busy",
                "Desktop private-prompt limit reached; resolve existing prompts before retrying.",
            ));
        }
        "error" => {
            return Err(not_delivered(
                "unavailable",
                "Prompt unavailable; dismiss any remaining prompt and update/reconnect the desktop companion before an explicit retry.",
            ));
        }
        _ => return Err(malformed()),
    }
    let raw = reply.result.ok_or_else(|| {
        not_delivered(
            "response_missing",
            "Reply has no result; update the desktop companion before an explicit retry.",
        )
    })?;
    let result: String = serde_json::from_str(raw.get()).map_err(|_| malformed())?;
    serde_json::from_str(&result).map_err(|_| malformed())
}
// POSIX shell-compatible environment assignments; values are always literal.
fn assignments(fields: &[String], values: &[String]) -> Vec<u8> {
    fields
        .iter()
        .zip(values)
        .map(|(key, value)| format!("{key}='{}'\n", value.replace('\'', "'\\''")))
        .collect::<String>()
        .into_bytes()
}
pub fn run(options: &Options) -> io::Result<()> {
    run_inner(options).map_err(|err| {
        if err
            .get_ref()
            .is_some_and(|cause| cause.is::<DeliveryError>())
        {
            err
        } else {
            not_delivered("request_failed", &err.to_string())
        }
    })
}
fn run_inner(options: &Options) -> io::Result<()> {
    if !(1..=2).contains(&options.fields.len())
        || options.fields.len() == 2 && options.fields[0] == options.fields[1]
        || options.login && options.fields.len() != 2
    {
        return Err(error(
            "Request one field or two distinct fields; --login requires a pair",
        ));
    }
    if options.project.trim().is_empty()
        || options.title.trim().is_empty()
        || options.project.len() > 200
        || options.title.len() > 300
    {
        return Err(error("Use a short project and title"));
    }
    let env_file = options
        .env_file
        .as_deref()
        .map(|p| EnvFile::open(p, &options.fields))
        .transpose()?;
    let mut stdout = if options.stdout {
        Some(stdout_file()?)
    } else {
        None
    };
    let destination = if let Some(file) = &env_file {
        format!("Save to {}", file.path.display())
    } else if stdout.is_some() {
        "Save to the caller's redirected private file".into()
    } else {
        format!(
            "Use in child process: {} (output suppressed)",
            options.command[0]
        )
    };
    // Avoid credential-bearing core dumps of this short-lived client.
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    unsafe {
        libc::setrlimit(libc::RLIMIT_CORE, &limit);
    }
    let executable = std::env::current_exe()?.canonicalize()?;
    super::initialize(&executable)?;
    let state = fs::read_to_string(executable.with_file_name("hey-boss.state"))?;
    let request = serde_json::json!({"command":"secret","sync":true,"project":options.project,"title":options.title,
        "question":serde_json::json!({"fields":options.fields,"login":options.login,"destination":destination}).to_string()});
    let values = receive(&Path::new(state.trim()).join("daemon.sock"), &request)?;
    if values.len() != options.fields.len()
        || values
            .iter()
            .any(|v| v.is_empty() || v.len() > 65536 || v.contains('\0'))
    {
        return Err(not_delivered(
            "values_invalid",
            "Secret values must be nonempty, without NUL, and at most 64 KiB each",
        ));
    }
    if let Some(file) = env_file {
        file.write(&assignments(&options.fields, &values)).map_err(|_| not_delivered("destination_failed", "Could not safely save credentials; check destination permissions or concurrent changes before an explicit retry."))?;
    } else if let Some(file) = &mut stdout {
        file.write_all(&assignments(&options.fields, &values))
            .and_then(|()| file.sync_all())
            .map_err(|_| {
                outcome(
                    "unknown",
                    "destination_failed",
                    "Redirected output may be partial; do not repeat automatically.",
                )
            })?;
    } else {
        let mut child = Command::new(&options.command[0])
            .args(&options.command[1..])
            .envs(options.fields.iter().zip(&values))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| {
                not_delivered(
                    "child_start_failed",
                    "Could not start child process; check the executable before an explicit retry.",
                )
            })?;
        let status = child.wait().map_err(|_| {
            outcome(
                "delivered",
                "child_wait_failed",
                "Child started but completion is unknown; do not repeat automatically.",
            )
        })?;
        if !status.success() {
            return Err(outcome(
                "delivered",
                "child_failed",
                "Child process failed (output suppressed); do not repeat credential delivery automatically.",
            ));
        }
        eprintln!("Secret delivery=delivered: child process succeeded (output suppressed).");
        return Ok(());
    }
    eprintln!("Secret delivery=delivered: credentials saved.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn values_round_trip_without_shell_execution() {
        let fields = vec!["ONE".into(), "TWO".into()];
        let values = vec!["'\"\\$() `touch nope`\nline two".into(), "z".repeat(20000)];
        let data = assignments(&fields, &values);
        // Only the synthetic test runs a shell; production passes environment directly.
        let mut child=Command::new("sh").args(["-c",". /dev/stdin; test \"$ONE\" = \"$EXPECTED_ONE\" && test \"$TWO\" = \"$EXPECTED_TWO\""])
            .env("EXPECTED_ONE",&values[0]).env("EXPECTED_TWO",&values[1]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(&data).unwrap();
        assert!(child.wait().unwrap().success());
    }
    #[test]
    fn safe_file_permissions_and_existing_keys() {
        let root = std::env::temp_dir().join(format!("hb-secret-test-{}", SystemNonce::value()));
        fs::create_dir(&root).unwrap();
        let path = root.join(".env");
        let fields = vec!["KEY".into()];
        EnvFile::open(&path, &fields)
            .unwrap()
            .write(b"KEY='synthetic'\n")
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(EnvFile::open(&path, &fields).is_err());
        std::os::unix::fs::symlink(&path, root.join("link")).unwrap();
        assert!(EnvFile::open(&root.join("link"), &fields).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn secret_response_errors_are_redacted() {
        for (response, expected) in [
            (
                r#"{"status":"ok","result":"synthetic-secret-that-must-not-appear"}"#,
                "response_malformed",
            ),
            (r#"{"status":"synthetic-secret"}"#, "response_malformed"),
            (
                r#"{"status":"cancelled","result":"synthetic-secret"}"#,
                "cancelled",
            ),
            (r#"{"status":"ok"}"#, "response_missing"),
            ("synthetic-secret", "response_malformed"),
            ("", "response_empty"),
        ] {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            // Queue this small response and close the peer before reading. The
            // redaction test needs neither a request reader nor a human deadline.
            server.write_all(response.as_bytes()).unwrap();
            drop(server);
            let message = read_response(&mut client).unwrap_err().to_string();
            assert!(
                message.starts_with(&format!("Secret delivery=not_delivered reason={expected}:"))
            );
            assert!(!message.contains("synthetic-secret"));
        }
    }

    #[test]
    fn secret_response_round_trip() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        server
            .write_all(br#"{"status":"ok","result":"[\"synthetic-value\"]"}"#)
            .unwrap();
        drop(server);
        assert_eq!(read_response(&mut client).unwrap(), ["synthetic-value"]);
    }

    #[test]
    fn stalled_secret_response_is_redacted() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        server.write_all(b"synthetic-secret").unwrap();
        let message = read_response(&mut client).unwrap_err().to_string();
        assert!(message.starts_with("Secret delivery=not_delivered reason=transport_failed:"));
        assert!(!message.contains("synthetic-secret"));
    }

    #[test]
    fn oversized_secret_response_is_bounded_and_redacted() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let sender = std::thread::spawn(move || {
            let _ = server.write_all(&vec![b'x'; 1024 * 1024 + 1]);
        });
        let message = read_response(&mut client).unwrap_err().to_string();
        assert!(message.starts_with("Secret delivery=not_delivered reason=response_too_large:"));
        sender.join().unwrap();
    }
}
