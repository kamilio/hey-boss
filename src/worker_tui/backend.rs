//! Bounded, cancellable CLI adapter. All commands run outside the UI thread.
use serde_json::Value;
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Client {
    pub binary: PathBuf,
    pub host: Option<String>,
    pub directory: Option<PathBuf>,
    pub timeout: Duration,
}

#[derive(Clone, Debug)]
pub enum Request {
    Refresh(Option<String>),
    Control { worker_id: String, stop: bool },
}

impl Client {
    pub fn execute(&self, request: &Request, cancelled: &Arc<AtomicBool>) -> Result<Value, String> {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let mut command = Command::new(&self.binary);
        command.args(["worker", "--json"]);
        if let Some(host) = &self.host {
            command.args(["--host", host]);
        }
        if let Some(directory) = &self.directory {
            command.arg("--directory").arg(directory);
        }
        match request {
            Request::Refresh(id) => {
                if let Some(id) = id {
                    command.args(["--id", id]);
                }
                command.arg("status");
            }
            Request::Control { worker_id, stop } => {
                command.args([if *stop { "stop" } else { "pause" }, worker_id]);
            }
        }
        // Remote status must not allocate a pseudo-terminal and corrupt JSON.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Cannot run {}: {e}", self.binary.display()))?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        fn reader(
            mut stream: impl Read + Send + 'static,
        ) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
            thread::spawn(move || {
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let n = stream.read(&mut chunk)?;
                    if n == 0 {
                        break;
                    }
                    // Drain excess output without retaining unbounded data.
                    let keep = n.min((4 * 1024 * 1024usize).saturating_sub(bytes.len()));
                    bytes.extend_from_slice(&chunk[..keep]);
                }
                Ok(bytes)
            })
        }
        let out = reader(stdout);
        let err = reader(stderr);
        let start = Instant::now();
        let status = loop {
            if cancelled.load(Ordering::Relaxed) || start.elapsed() >= self.timeout {
                terminate(&mut child);
                return Err(if cancelled.load(Ordering::Relaxed) {
                    "Cancelled".into()
                } else {
                    "Queue request timed out; press r to retry".into()
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(e) => {
                    terminate(&mut child);
                    return Err(e.to_string());
                }
            }
        };
        // Don't wait indefinitely on pipes inherited by a descendant.
        while !out.is_finished() || !err.is_finished() {
            if cancelled.load(Ordering::Relaxed) || start.elapsed() >= self.timeout {
                terminate(&mut child);
                return Err("Queue output timed out".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        let output = out
            .join()
            .map_err(|_| "Output reader failed")?
            .map_err(|e| e.to_string())?;
        let error = err
            .join()
            .map_err(|_| "Error reader failed")?
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!(
                "Queue command failed ({status}): {}",
                String::from_utf8_lossy(&error)
            ));
        }
        let value: Value =
            serde_json::from_slice(&output).map_err(|e| format!("Invalid worker status: {e}"))?;
        if value["ok"] != true {
            return Err(format!("Queue rejected request: {value}"));
        }
        if matches!(request, Request::Refresh(_))
            && (!value["workers"].is_array() || !value["runs"].is_array())
        {
            return Err("Worker status is missing workers/runs; upgrade hey-boss".into());
        }
        Ok(value)
    }
}

fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    // The command owns a new process group. Kill its SSH/pipe descendants too.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}
