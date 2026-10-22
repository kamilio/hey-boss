//! Synchronous issue RPC. Requests travel on stdin, never as remote shell text.
use super::{Error, Request, Result, WIRE_LIMIT};
use serde_json::Value;
use std::process::Command;
use std::time::Duration;

const SCRIPT: &str = r#"export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"; exec hey-boss issue rpc"#;

pub fn call(host: &str, request: &Request) -> Result<Value> {
    if !crate::health::remote::valid_host(host) {
        return Err(Error::invalid("Invalid SSH host"));
    }
    let input = serde_json::to_vec(request)?;
    if input.len() > WIRE_LIMIT {
        return Err(Error::invalid("Issue request exceeds transport limit"));
    }
    let mut command = Command::new("ssh");
    command.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "ConnectionAttempts=1",
        "-o",
        "ServerAliveInterval=5",
        "-o",
        "ServerAliveCountMax=2",
        host,
        SCRIPT,
    ]);
    exchange(host, &mut command, &input, Duration::from_secs(30))
}

fn exchange(host: &str, command: &mut Command, input: &[u8], timeout: Duration) -> Result<Value> {
    let captured = crate::ssh::capture::exchange(
        command,
        input,
        Some(timeout),
        (64 * 1024 * 1024, 64 * 1024),
    )?;
    let result = captured.output;
    let hint = "No local fallback was used. A remote write may have completed; retry with the same --request-id to avoid duplicates.";
    if captured.timed_out {
        return Err(Error::new(
            "transport_error",
            format!("Issue host timed out. {hint}"),
        ));
    };
    if captured.stdout_exceeded {
        return Err(Error::new(
            "transport_error",
            format!(
                "Issue response exceeds transport limit; use a smaller --limit for history. {hint}"
            ),
        ));
    }
    let bytes = result.stdout;
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
        if value["ok"] == false {
            return Err(serde_json::from_value(value["error"].clone())?);
        }
        if result.status.success() && captured.sent.is_ok() && value["ok"] == true {
            return Ok(value);
        }
    }
    let detail = String::from_utf8_lossy(&result.stderr).trim().to_owned();
    Err(Error::new(
        "transport_error",
        format!("Issue host {host} failed or returned an incompatible response: {detail}. {hint}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn rpc_reaps_exited_child_with_inherited_pipes() {
        let start = Instant::now();
        let value = exchange(
            "fixture",
            Command::new("sh").args(["-c", "cat >/dev/null; sleep 2 & printf '{\"ok\":true}'"]),
            b"request",
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(value["ok"], true);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn rpc_timeout_does_not_join_inherited_pipe_readers() {
        let start = Instant::now();
        let result = exchange(
            "fixture",
            Command::new("sh").args(["-c", "sleep 2 & wait"]),
            b"request",
            Duration::from_millis(50),
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("timed out"), "{error}");
        assert!(error.contains("same --request-id"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn rpc_failure_preserves_diagnostics_and_uncertain_write_guidance() {
        let start = Instant::now();
        let error = exchange(
            "fixture",
            Command::new("sh").args([
                "-c",
                "cat >/dev/null; sleep 2 >&2 & printf 'authentication failed' >&2; exit 255",
            ]),
            b"request",
            Duration::from_secs(1),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("authentication failed"), "{error}");
        assert!(error.contains("No local fallback"), "{error}");
        assert!(error.contains("same --request-id"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
