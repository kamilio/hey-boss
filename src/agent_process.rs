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
    input: Option<ChildStdin>,
    inbox: Option<mpsc::Receiver<io::Result<Value>>>,
    stopped: bool,
    #[cfg(test)]
    reader: thread::JoinHandle<()>,
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
        let _reader = thread::spawn(move || {
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
            input: Some(input),
            inbox: Some(inbox),
            stopped: false,
            #[cfg(test)]
            reader: _reader,
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
        if bytes.len() >= RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Agent request exceeded 8 MiB including its newline",
            ));
        }
        bytes.push(b'\n');
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut offset = 0;
        while offset < bytes.len() {
            match self
                .input
                .as_mut()
                .expect("running process input")
                .write(&bytes[offset..])
            {
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
        let inbox = self
            .inbox
            .as_ref()
            .ok_or_else(|| io::Error::other("Agent is stopped"))?;
        match inbox.recv_timeout(timeout) {
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
        self.input.take();
        // A full bounded channel otherwise keeps the reader and stdout alive
        // for as long as the caller retains the stopped process's state.
        self.inbox.take();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor_identity(fd: i32) -> Option<(libc::dev_t, libc::ino_t)> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
            return None;
        }
        let stat = unsafe { stat.assume_init() };
        Some((stat.st_dev, stat.st_ino))
    }

    #[test]
    fn stopping_closes_the_owned_input_descriptor() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let mut process = Process::spawn(&mut command).unwrap();
        let input_fd = process.input.as_ref().unwrap().as_raw_fd();
        let identity = descriptor_identity(input_fd).unwrap();
        process.stop().unwrap();
        // Other parallel tests may reuse the numeric descriptor after close.
        assert_ne!(descriptor_identity(input_fd), Some(identity));
    }

    #[test]
    fn jsonl_record_limit_includes_the_newline_in_both_directions() {
        let mut command = Command::new("/bin/cat");
        let mut process = Process::spawn(&mut command).unwrap();
        let maximum = Value::String("x".repeat(RECORD_BYTES - 3));
        process.send(&maximum).unwrap();
        assert_eq!(
            process.receive(Duration::from_secs(3)).unwrap(),
            Some(maximum)
        );
        let oversized = Value::String("x".repeat(RECORD_BYTES - 2));
        assert!(
            process.send(&oversized).is_err(),
            "Request exceeded the round-trip record limit"
        );
        assert!(
            process
                .receive(Duration::from_millis(20))
                .unwrap()
                .is_none(),
            "Rejected request was delivered"
        );
    }

    #[test]
    fn stopping_releases_a_reader_blocked_by_backpressure() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "i=0; while [ $i -lt 100 ]; do printf '{}\\n'; i=$((i+1)); done; sleep 30",
        ]);
        let mut process = Process::spawn(&mut command).unwrap();
        // More than the channel's capacity fits in the output pipe. The reader
        // cannot drain it while the stopped session retains its receiver.
        thread::sleep(Duration::from_millis(100));
        process.stop().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !process.reader.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            process.reader.is_finished(),
            "Stopped process retained its output reader"
        );
        assert!(
            process.receive(Duration::ZERO).is_err(),
            "Stopped transport returned stale output"
        );
    }

    #[test]
    #[ignore = "manual transport endurance check; launches 1000 owned process groups"]
    fn retained_stopped_sessions_keep_resource_usage_bounded() {
        let descriptors = || std::fs::read_dir("/dev/fd").unwrap().count();
        let baseline = descriptors();
        let started = Instant::now();
        let mut retained = Vec::new();
        for n in 0..1000 {
            let mut command = Command::new("/bin/sh");
            command.args([
                "-c",
                "i=0; while [ $i -lt 100 ]; do printf '{}\\n'; i=$((i+1)); done; sleep 30",
            ]);
            let mut process = Process::spawn(&mut command).unwrap();
            // Wait for real output, then retain the stopped transport rather
            // than relying on Drop to free its pipes and blocked reader.
            process.receive(Duration::from_secs(2)).unwrap().unwrap();
            process.stop().unwrap();
            retained.push(process);
            if (n + 1) % 100 == 0 {
                eprintln!(
                    "Retained {} stopped sessions; open descriptors {}",
                    n + 1,
                    descriptors()
                );
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while retained.iter().any(|process| !process.reader.is_finished())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(retained.iter().all(|process| process.reader.is_finished()));
        let final_count = descriptors();
        assert!(
            final_count <= baseline + 2,
            "Descriptors grew from {baseline} to {final_count}"
        );
        eprintln!(
            "PASS 1000 retained sessions in {:?}; descriptors {baseline} -> {final_count}",
            started.elapsed()
        );
    }
}
