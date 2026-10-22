//! Capture a direct child's output without depending on EOF from its descendants.
use std::{
    io::{self, Read, Write},
    os::fd::AsRawFd,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

const EXIT_DRAIN: Duration = Duration::from_millis(100);
const POLL_MS: i32 = 20;

pub struct Captured {
    pub output: Output,
    pub sent: io::Result<()>,
    pub timed_out: bool,
    pub stdout_exceeded: bool,
    pub stderr_exceeded: bool,
}

pub fn output(command: &mut Command) -> io::Result<Output> {
    Ok(exchange(command, &[], None, (usize::MAX, usize::MAX))?.output)
}

// Only the direct child belongs to this operation. Never signal its process group:
// SSH authentication helpers and remote deployment builds can outlive it.
struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Reader<T> {
    pipe: Option<T>,
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl<T: Read + AsRawFd> Reader<T> {
    fn new(pipe: T, limit: usize) -> io::Result<Self> {
        nonblocking(&pipe)?;
        Ok(Self {
            pipe: Some(pipe),
            bytes: Vec::new(),
            limit,
            exceeded: false,
        })
    }

    fn drain(&mut self) -> io::Result<()> {
        let Some(pipe) = self.pipe.as_mut() else {
            return Ok(());
        };
        let mut buffer = [0; 16 * 1024];
        // A continuously writing descendant must not starve status/deadline checks
        // or the other stream. Discard excess bytes while still draining the pipe.
        for _ in 0..16 {
            match pipe.read(&mut buffer) {
                Ok(0) => {
                    self.pipe = None;
                    break;
                }
                Ok(n) => {
                    let keep = n.min(self.limit - self.bytes.len());
                    self.bytes.extend_from_slice(&buffer[..keep]);
                    self.exceeded |= keep < n;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn pollfd(&self) -> libc::pollfd {
        pollfd(self.pipe.as_ref(), libc::POLLIN)
    }
}

fn pollfd(pipe: Option<&impl AsRawFd>, events: i16) -> libc::pollfd {
    libc::pollfd {
        fd: pipe.map_or(-1, AsRawFd::as_raw_fd),
        events,
        revents: 0,
    }
}

/// Service stdin/stdout/stderr together, reaping promptly even when a descendant
/// inherits a pipe. After exit, drain buffered output for at most 100 ms. No reader
/// threads or temporary files survive this call. A timeout kills only this child.
pub fn exchange(
    command: &mut Command,
    input: &[u8],
    timeout: Option<Duration>,
    limits: (usize, usize),
) -> io::Result<Captured> {
    let start = Instant::now();
    let mut child = Reap(
        command
            .stdin(if input.is_empty() {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    let mut stdin = child.0.stdin.take();
    if let Some(pipe) = &stdin {
        nonblocking(pipe)?;
    }
    let mut stdout = Reader::new(child.0.stdout.take().unwrap(), limits.0)?;
    let mut stderr = Reader::new(child.0.stderr.take().unwrap(), limits.1)?;
    let mut written = 0;
    let mut sent = Ok(());
    let mut exited = None;
    let mut timed_out = false;
    let status = loop {
        if exited.is_none() {
            if let Some(status) = child.0.try_wait()? {
                exited = Some((status, Instant::now()));
            } else if timeout.is_some_and(|limit| start.elapsed() >= limit) {
                child.0.kill()?;
                exited = Some((child.0.wait()?, Instant::now()));
                timed_out = true;
            }
            if exited.is_some() {
                stdin = None;
                if written != input.len() && sent.is_ok() {
                    sent = Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Child exited before receiving input",
                    ));
                }
            }
        }
        stdout.drain()?;
        stderr.drain()?;
        if let Some(pipe) = stdin.as_mut() {
            let end = input.len().min(written.saturating_add(64 * 1024));
            match pipe.write(&input[written..end]) {
                Ok(0) => {
                    sent = Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Child stdin closed",
                    ));
                    stdin = None;
                }
                Ok(n) => {
                    written += n;
                    if written == input.len() {
                        stdin = None;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => {
                    sent = Err(e);
                    stdin = None;
                }
            }
        }
        if let Some((status, at)) = exited {
            if (stdout.pipe.is_none() && stderr.pipe.is_none()) || at.elapsed() >= EXIT_DRAIN {
                break status;
            }
        }
        let mut descriptors = [
            stdout.pollfd(),
            stderr.pollfd(),
            pollfd(stdin.as_ref(), libc::POLLOUT),
        ];
        let result =
            unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, POLL_MS) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    };
    Ok(Captured {
        output: Output {
            status,
            stdout: stdout.bytes,
            stderr: stderr.bytes,
        },
        sent,
        timed_out,
        stdout_exceeded: stdout.exceeded,
        stderr_exceeded: stderr.exceeded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_large_bidirectional_output_without_pipe_deadlock() {
        let input = vec![b'x'; 2 * 1024 * 1024];
        let captured = exchange(
            Command::new("sh").args([
                "-c",
                "dd if=/dev/zero bs=65536 count=32 2>/dev/null | cat >&2; cat",
            ]),
            &input,
            Some(Duration::from_secs(5)),
            (input.len(), input.len()),
        )
        .unwrap();
        assert!(captured.output.status.success());
        captured.sent.unwrap();
        assert!(!captured.timed_out);
        assert!(!captured.stdout_exceeded && !captured.stderr_exceeded);
        assert_eq!(captured.output.stdout, input);
        assert_eq!(captured.output.stderr, vec![0; input.len()]);
    }

    #[test]
    fn limits_retained_bytes_but_keeps_draining_to_child_exit() {
        let captured = exchange(
            Command::new("sh").args([
                "-c",
                "dd if=/dev/zero bs=65536 count=32 2>/dev/null; printf diagnostic >&2; exit 7",
            ]),
            &[],
            Some(Duration::from_secs(5)),
            (100, 4),
        )
        .unwrap();
        assert_eq!(captured.output.status.code(), Some(7));
        assert_eq!(captured.output.stdout, vec![0; 100]);
        assert_eq!(captured.output.stderr, b"diag");
        assert!(captured.stdout_exceeded && captured.stderr_exceeded);
        assert!(!captured.timed_out);
    }

    #[test]
    fn waits_for_live_child_even_after_both_output_pipes_close() {
        let start = Instant::now();
        let result =
            output(Command::new("sh").args(["-c", "exec 1>&- 2>&-; sleep 0.2; exit 9"])).unwrap();
        assert_eq!(result.status.code(), Some(9));
        assert!(start.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn exited_child_is_reaped_and_inherited_stdin_cannot_block_sender() {
        let start = Instant::now();
        let captured = exchange(
            Command::new("sh").args([
                "-c",
                "sleep 2 <&0 >&2 & printf '%s %s' $$ $!; printf diagnostic >&2; exit 7",
            ]),
            &vec![b'x'; 2 * 1024 * 1024],
            Some(Duration::from_secs(1)),
            (1024, 1024),
        )
        .unwrap();
        let pids: Vec<i32> = String::from_utf8(captured.output.stdout)
            .unwrap()
            .split_whitespace()
            .map(|pid| pid.parse().unwrap())
            .collect();
        // The direct child is already reaped; the inherited-pipe holder is alive.
        assert_eq!(
            unsafe { libc::waitpid(pids[0], std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        let holder_alive = unsafe { libc::kill(pids[1], 0) } == 0;
        // Only this synthetic test helper; production never signals descendants.
        unsafe {
            libc::kill(pids[1], libc::SIGTERM);
        }
        assert!(holder_alive);
        assert_eq!(captured.output.status.code(), Some(7));
        assert_eq!(captured.output.stderr, b"diagnostic");
        assert!(captured.sent.is_err());
        assert!(!captured.timed_out);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn timeout_also_bounds_a_blocked_stdin_writer() {
        let start = Instant::now();
        let captured = exchange(
            Command::new("sleep").arg("2"),
            &vec![b'x'; 2 * 1024 * 1024],
            Some(Duration::from_millis(50)),
            (1024, 1024),
        )
        .unwrap();
        assert!(captured.timed_out);
        assert!(captured.sent.is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn continuous_descendant_output_cannot_starve_exit_deadline() {
        let start = Instant::now();
        let captured = exchange(
            Command::new("sh").args([
                "-c",
                "(while printf noise; do :; done) 2>/dev/null & exit 0",
            ]),
            &[],
            Some(Duration::from_secs(1)),
            (1024, 1024),
        )
        .unwrap();
        assert!(captured.output.status.success());
        assert!(captured.stdout_exceeded);
        assert!(!captured.timed_out);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
