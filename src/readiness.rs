//! Recovery is allowed only before any application request bytes are sent.
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

// Covers SSH failure detection and the first reconnect cooldown.
pub const RECOVERY_WAIT: Duration = Duration::from_secs(120);
pub const CONTROL_REPLY_WAIT: Duration = Duration::from_secs(135);

pub fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
            | io::ErrorKind::Interrupted
    )
}

pub fn wait<T>(
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
    mut attempt: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    loop {
        check(deadline, cancelled)?;
        match attempt() {
            Ok(value) => {
                check(deadline, cancelled)?;
                return Ok(value);
            }
            Err(error) if transient(&error) => {}
            Err(error) => return Err(error),
        }
        std::thread::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

pub fn check(deadline: Instant, cancelled: &impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Readiness wait cancelled; no request was sent",
        ));
    }
    if Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Daemon readiness timed out; no request was sent",
        ));
    }
    Ok(())
}

// Nonblocking connect keeps a full Unix listen backlog from defeating the bound.
pub fn connect_once(path: &Path) -> io::Result<UnixStream> {
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.set_nonblocking(true)?;
    socket
        .connect(&socket2::SockAddr::unix(path)?)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::EINPROGRESS) {
                io::Error::new(io::ErrorKind::WouldBlock, error)
            } else {
                error
            }
        })?;
    socket.set_nonblocking(false)?;
    Ok(socket.into())
}

pub fn connect(
    path: &Path,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> io::Result<UnixStream> {
    wait(deadline, cancelled, || connect_once(path))
}

pub fn is_desktop_control(command: &str) -> bool {
    matches!(
        command,
        "overview" | "overview_snapshot" | "inbox" | "inbox_list" | "action" | "artifact_editor"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outage_cancellation_and_permanent_errors_are_distinct() {
        let start = Instant::now();
        let error = connect(
            Path::new("/tmp/hb-readiness-nonexistent/socket"),
            start + Duration::from_millis(150),
            &|| false,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(1));
        let start = Instant::now();
        let error = connect(
            Path::new("/tmp/hb-readiness-nonexistent/socket"),
            start + RECOVERY_WAIT,
            &|| start.elapsed() >= Duration::from_millis(100),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(start.elapsed() < Duration::from_secs(1));
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::Unsupported,
            io::ErrorKind::InvalidData,
        ] {
            let mut attempts = 0;
            let error = wait::<()>(Instant::now() + RECOVERY_WAIT, &|| false, || {
                attempts += 1;
                Err(io::Error::new(kind, "permanent"))
            })
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert_eq!(attempts, 1);
        }
    }
}
