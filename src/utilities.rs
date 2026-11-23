//! Configured commands share an argv-safe launcher on the CLI and fleet peers.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStringExt,
        process::{CommandExt, ExitStatusExt},
    },
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const LIMIT: usize = 1024 * 1024;
pub const TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub global: bool,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "help"
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
impl Definition {
    pub fn validate(&self) -> io::Result<()> {
        if self.command.trim().is_empty() || self.command.contains('\0') {
            return Err(io::Error::other(
                "Utility command must be nonempty and contain no NUL bytes",
            ));
        }
        Ok(())
    }
    pub fn launch(&self, args: &[OsString]) -> io::Result<Command> {
        self.validate()?;
        // Fleet services have a minimal PATH. Resolve destination tools using
        // the user's terminal startup files, including Node version managers.
        let remote = self.destination.is_some();
        let shell = if remote {
            std::env::var_os("SHELL").filter(|shell| !shell.is_empty())
        } else {
            None
        };
        let mut command = Command::new(shell.unwrap_or_else(|| "/bin/sh".into()));
        if remote {
            // Keep configured commands' existing POSIX shell semantics.
            command.args(["-lic", "exec /bin/sh -c \"$@\"", "hey-boss-utils"]);
        } else {
            command.arg("-c");
        }
        command
            .args([&format!("{} \"$@\"", self.command), "hey-boss-utils"])
            .args(args);
        Ok(command)
    }
}

pub fn decode(value: &Value) -> io::Result<Vec<u8>> {
    let encoded = value
        .as_str()
        .ok_or_else(|| io::Error::other("Missing utility data"))?;
    if encoded.len() > LIMIT.div_ceil(3) * 4 {
        return Err(io::Error::other("Utility data exceeds 1 MiB"));
    }
    let bytes = STANDARD.decode(encoded).map_err(io::Error::other)?;
    if bytes.len() > LIMIT {
        return Err(io::Error::other("Utility data exceeds 1 MiB"));
    }
    Ok(bytes)
}
pub fn arguments(request: &Value) -> io::Result<Vec<OsString>> {
    let values = request["args"]
        .as_array()
        .ok_or_else(|| io::Error::other("Missing utility arguments"))?;
    let mut size = 0;
    values
        .iter()
        .map(|value| {
            let bytes = decode(value)?;
            size += bytes.len() + 1;
            if size > LIMIT || bytes.contains(&0) {
                return Err(io::Error::other("Invalid or oversized utility arguments"));
            }
            Ok(OsString::from_vec(bytes))
        })
        .collect()
}

pub fn execute(request: &Value) -> Value {
    static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    use std::sync::atomic::Ordering;
    if ACTIVE
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < 32).then_some(n + 1)
        })
        .is_err()
    {
        return json!({"ok":false,"error":"Utility destination is busy; nothing was queued"});
    }
    let result = execute_inner(request, TIMEOUT)
        .unwrap_or_else(|error| json!({"ok":false,"error":error.to_string()}));
    ACTIVE.fetch_sub(1, Ordering::AcqRel);
    result
}
fn execute_inner(request: &Value, timeout: Duration) -> io::Result<Value> {
    let definition: Definition = serde_json::from_value(request["utility"].clone())?;
    let args = arguments(request)?;
    let input = decode(&request["stdin"])?;
    let mut command = definition.launch(&args)?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // Destination commands run in the destination user's home, like a login shell.
    if let Some(home) = std::env::var_os("HOME") {
        command.current_dir(home);
    }
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    fn reader(pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<io::Result<Vec<u8>>> {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.take(LIMIT as u64 + 1).read_to_end(&mut bytes)?;
            Ok(bytes)
        })
    }
    let stdout = reader(child.stdout.take().unwrap());
    let stderr = reader(child.stderr.take().unwrap());
    let deadline = Instant::now() + timeout;
    let mut status = None;
    loop {
        if status.is_none() {
            status = child.try_wait()?;
        }
        if status.is_some() && writer.is_finished() && stdout.is_finished() && stderr.is_finished()
        {
            break;
        }
        if Instant::now() >= deadline {
            // Include descendants that inherited pipes, without stopping detached
            // background work after an otherwise successful command.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            status = None;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait()?;
    let _ = writer.join(); // Commands may deliberately stop reading stdin early.
    let stdout = stdout
        .join()
        .map_err(|_| io::Error::other("Utility output reader failed"))??;
    let stderr = stderr
        .join()
        .map_err(|_| io::Error::other("Utility error reader failed"))??;
    let status = status
        .ok_or_else(|| io::Error::other("Utility exceeded five minutes; process group stopped"))?;
    if stdout.len() > LIMIT || stderr.len() > LIMIT {
        return Err(io::Error::other("Utility output exceeds 1 MiB"));
    }
    Ok(
        json!({"ok":true,"done":true,"stdout":STANDARD.encode(stdout),"stderr":STANDARD.encode(stderr),"code":status.code().unwrap_or(128 + status.signal().unwrap_or(1))}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn destination_shell_resolves_updated_terminal_tools_and_preserves_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::current_dir()
            .unwrap()
            .join(format!(".utility-shell-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join(".zprofile"), "export UTILITY_LOGIN_READY=yes\n").unwrap();
        std::fs::write(root.join(".zshrc"), "export PATH=\"$ZDOTDIR/bin:$PATH\"\n").unwrap();
        let tool = root.join("bin/wire");
        let definition = Definition {
            command: "wire".into(),
            destination: Some("local".into()),
            global: false,
        };
        let args = ["two words", "", "$(touch injected)", "--help"].map(OsString::from);
        let launch = definition.launch(&args).unwrap();
        assert_eq!(
            launch.get_program(),
            std::env::var_os("SHELL")
                .filter(|shell| !shell.is_empty())
                .unwrap_or_else(|| "/bin/sh".into())
        );
        for version in ["first", "updated"] {
            std::fs::write(
                &tool,
                format!("#!/bin/sh\nprintf '%s\\0' {version} \"$UTILITY_LOGIN_READY\" \"$@\"\n"),
            )
            .unwrap();
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
            let output = Command::new("/bin/zsh")
                .args(launch.get_args())
                .env("ZDOTDIR", &root)
                .env("PATH", "/usr/bin:/bin")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                output.stdout,
                format!("{version}\0yes\0two words\0\0$(touch injected)\0--help\0").as_bytes()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn shell_function_keeps_fixed_suffix_after_caller_arguments() {
        let result = execute(&json!({
            "utility":{"command":"ans_ck() { printf '%s\\0' \"$@\" /home/kjopek/ans; }; ans_ck"},
            "args":[STANDARD.encode(b"--sem"),STANDARD.encode(b"agent API authentication"),STANDARD.encode(b""),STANDARD.encode(b"$(echo injected)")],
            "stdin":""
        }));
        assert_eq!(result["code"], 0);
        assert_eq!(
            decode(&result["stdout"]).unwrap(),
            b"--sem\0agent API authentication\0\0$(echo injected)\0/home/kjopek/ans\0"
        );
    }
    #[test]
    fn execution_bounds_output_and_stops_slow_process_groups() {
        let result = execute(&json!({"utility":{"command":"yes"},"args":[],"stdin":""}));
        assert_eq!(result["ok"], false);
        assert!(result["error"].as_str().unwrap().contains("1 MiB"));
        let start = Instant::now();
        let error = execute_inner(
            &json!({"utility":{"command":"sleep 30"},"args":[],"stdin":""}),
            Duration::from_millis(50),
        )
        .unwrap_err();
        assert!(error.to_string().contains("process group stopped"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn execution_keeps_argument_boundaries_and_binary_io() {
        let request = json!({"utility":{"command":"printf '%s\\0'; cat; printf error >&2; exit 37 #"},"args":[],"stdin":STANDARD.encode(b"bytes\0\xff")});
        let result = execute(&request);
        assert_eq!(result["code"], 37, "{result}");
        assert_eq!(decode(&result["stdout"]).unwrap(), b"\0bytes\0\xff");
        assert_eq!(decode(&result["stderr"]).unwrap(), b"error");
        let result = execute(
            &json!({"utility":{"command":"printf '%s\\0'"},"args":[STANDARD.encode(b"two words"),STANDARD.encode(b""),STANDARD.encode(b"$(touch injected)"),STANDARD.encode(b"\xff")],"stdin":""}),
        );
        assert_eq!(
            decode(&result["stdout"]).unwrap(),
            b"two words\0\0$(touch injected)\0\xff\0"
        );
    }
    #[test]
    fn rejects_invalid_and_oversized_payloads() {
        assert!(arguments(&json!({"args":[STANDARD.encode(b"a\0b")]})).is_err());
        assert!(decode(&json!(STANDARD.encode(vec![0; LIMIT + 1]))).is_err());
        assert!(
            Definition {
                command: " ".into(),
                destination: None,
                global: false,
            }
            .validate()
            .is_err()
        );
    }
}
