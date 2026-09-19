//! Synchronous issue RPC. Requests travel on stdin, never as remote shell text.
use super::{Error, Request, Result, WIRE_LIMIT};
use serde_json::Value;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SCRIPT: &str = r#"export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"; exec hey-boss issue rpc"#;

pub fn call(host: &str, request: &Request) -> Result<Value> {
    if !crate::health::remote::valid_host(host) {
        return Err(Error::invalid("Invalid SSH host"));
    }
    let input = serde_json::to_vec(request)?;
    if input.len() > WIRE_LIMIT {
        return Err(Error::invalid("Issue request exceeds transport limit"));
    }
    let mut child = Command::new("ssh")
        .args([
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
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (status, bytes, errors, sent) = std::thread::scope(|scope| -> Result<_> {
        let writer = scope.spawn(move || stdin.write_all(&input));
        let reader = scope.spawn(move || bounded(stdout, 64 * 1024 * 1024));
        let error_reader = scope.spawn(move || bounded(stderr, 64 * 1024));
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        Ok((
            status,
            reader.join().unwrap(),
            error_reader.join().unwrap(),
            writer.join().unwrap(),
        ))
    })?;
    let hint = "No local fallback was used. A remote write may have completed; retry with the same --request-id to avoid duplicates.";
    let Some(status) = status else {
        return Err(Error::new(
            "transport_error",
            format!("Issue host timed out. {hint}"),
        ));
    };
    let bytes = bytes?;
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
        if value["ok"] == false {
            return Err(serde_json::from_value(value["error"].clone())?);
        }
        if status.success() && sent.is_ok() && value["ok"] == true {
            return Ok(value);
        }
    }
    let detail = String::from_utf8_lossy(&errors.unwrap_or_default())
        .trim()
        .to_owned();
    Err(Error::new(
        "transport_error",
        format!("Issue host {host} failed or returned an incompatible response: {detail}. {hint}"),
    ))
}

fn bounded(reader: impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::new(
            "transport_error",
            "Issue response exceeds transport limit; use a smaller --limit for history",
        ));
    }
    Ok(bytes)
}
