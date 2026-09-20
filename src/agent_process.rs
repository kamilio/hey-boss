//! Bounded JSONL transport for a process group owned by this client.
use serde_json::Value;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const RECORD_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct Process {
    child: Child,
    input: ChildStdin,
    inbox: mpsc::Receiver<io::Result<Value>>,
    stopped: bool,
}
impl Process {
    pub(crate) fn spawn(command: &mut Command) -> io::Result<Self> {
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let input = child.stdin.take().expect("piped stdin");
        let output = child.stdout.take().expect("piped stdout");
        let fd = input.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            let error = io::Error::last_os_error();
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let (send, inbox) = mpsc::sync_channel(16);
        thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                let result = match Read::take(&mut reader, RECORD_BYTES as u64 + 1)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) => break,
                    Ok(_) if line.len() > RECORD_BYTES => {
                        Err(io::Error::other("Agent event exceeded 8 MiB"))
                    }
                    Ok(_) if line.last() != Some(&b'\n') => Err(io::Error::other(
                        "Agent disconnected with an incomplete JSONL record",
                    )),
                    Ok(_) => serde_json::from_slice(&line).map_err(io::Error::from),
                    Err(error) => Err(error),
                };
                let bad = result.is_err();
                if send.send(result).is_err() || bad {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            inbox,
            stopped: false,
        })
    }
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }
    pub(crate) fn send(&mut self, value: &Value) -> io::Result<()> {
        if self.stopped {
            return Err(io::Error::other("Agent is stopped"));
        }
        let mut bytes = serde_json::to_vec(value)?;
        if bytes.len() > RECORD_BYTES {
            return Err(io::Error::other("Agent request exceeded 8 MiB"));
        }
        bytes.push(b'\n');
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut offset = 0;
        while offset < bytes.len() {
            match self.input.write(&bytes[offset..]) {
                Ok(0) => return Err(io::Error::other("Agent input disconnected")),
                Ok(size) => offset += size,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => {
                    return Err(io::Error::other(format!(
                        "Agent write failed: {error}. Inspect state before retrying."
                    )));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn receive(&mut self, timeout: Duration) -> io::Result<Option<Value>> {
        match self.inbox.recv_timeout(timeout) {
            Ok(value) => value.map(Some),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(
                "Agent disconnected before reporting completion",
            )),
        }
    }
    pub(crate) fn stop(&mut self) -> io::Result<()> {
        if self.stopped {
            return Ok(());
        }
        // The parent has not been reaped, so its PID cannot be reused. Signal the
        // group even when the parent exited: its tools may still be running.
        let group = -(self.child.id() as i32);
        let signal = |signal| -> io::Result<()> {
            if unsafe { libc::kill(group, signal) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH)
                || (error.raw_os_error() == Some(libc::EPERM) && group_has_no_live_processes(group))
            {
                Ok(())
            } else {
                Err(error)
            }
        };
        signal(libc::SIGTERM)?;
        thread::sleep(Duration::from_millis(100));
        signal(libc::SIGKILL)?;
        self.child.wait()?;
        self.stopped = true;
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn group_has_no_live_processes(group: i32) -> bool {
    // macOS can return EPERM when a group contains only the unreaped zombie.
    // Verify that condition rather than suppressing a real permission failure.
    let Ok(output) = Command::new("ps").args(["-axo", "pgid=,stat="]).output() else {
        return false;
    };
    output.status.success()
        && !String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            let mut fields = line.split_whitespace();
            fields.next().and_then(|v| v.parse::<i32>().ok()) == Some(-group)
                && fields.next().is_none_or(|state| !state.starts_with('Z'))
        })
}
