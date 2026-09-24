//! Local HTTP authorities and optional macOS launchd socket activation.
use std::io;
use std::net::TcpListener;

/// Normalize only the explicit loopback allowlist, not arbitrary URL syntax.
pub(super) fn authority(host: &str, ports: &[u16]) -> Option<String> {
    let (name, port) = match host.split_once(':') {
        Some((name, port)) => {
            let number = port.parse::<u16>().ok()?;
            if port != number.to_string() {
                return None;
            }
            (name, number)
        }
        None => (host, 80),
    };
    if !["127.0.0.1", "localhost", "hey-boss.test"].contains(&name) || !ports.contains(&port) {
        return None;
    }
    Some(if port == 80 {
        name.to_owned()
    } else {
        format!("{name}:{port}")
    })
}

pub(super) fn port_80() -> io::Result<(TcpListener, bool)> {
    if let Some(listener) = activated()? {
        return Ok((listener, true));
    }
    TcpListener::bind(("127.0.0.1", 80)).map(|listener| (listener, false))
}

#[cfg(not(target_os = "macos"))]
fn activated() -> io::Result<Option<TcpListener>> {
    Ok(None)
}

#[cfg(target_os = "macos")]
fn activated() -> io::Result<Option<TcpListener>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    unsafe extern "C" {
        fn launch_activate_socket(
            name: *const libc::c_char,
            fds: *mut *mut libc::c_int,
            count: *mut libc::size_t,
        ) -> libc::c_int;
    }
    let mut fds = std::ptr::null_mut();
    let mut count = 0;
    // launchd allocates the array and transfers ownership of its descriptors.
    let result = unsafe { launch_activate_socket(c"HTTP".as_ptr(), &mut fds, &mut count) };
    if result == libc::ENOENT || result == libc::ESRCH {
        return Ok(None);
    }
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    let listeners: Vec<_> = if fds.is_null() {
        Vec::new()
    } else {
        let listeners = unsafe { std::slice::from_raw_parts(fds, count) }
            .iter()
            .map(|fd| unsafe { TcpListener::from_raw_fd(*fd) })
            .collect();
        unsafe { libc::free(fds.cast()) };
        listeners
    };
    let mut listeners = listeners.into_iter();
    let listener = listeners
        .next()
        .ok_or_else(|| io::Error::other("launchd HTTP socket is missing"))?;
    if listeners.len() != 0 || listener.local_addr()? != ([127, 0, 0, 1], 80).into() {
        return Err(io::Error::other(
            "launchd HTTP must contain exactly one 127.0.0.1:80 socket",
        ));
    }
    // Never leak the privileged listener to Git, SSH, or agent subprocesses.
    let flags = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_GETFD) };
    if flags == -1
        || unsafe {
            libc::fcntl(
                listener.as_raw_fd(),
                libc::F_SETFD,
                flags | libc::FD_CLOEXEC,
            )
        } == -1
    {
        return Err(io::Error::last_os_error());
    }
    listener.set_nonblocking(false)?;
    Ok(Some(listener))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_allowlist_normalizes_only_default_http_ports() {
        for hostname in ["127.0.0.1", "localhost", "hey-boss.test"] {
            assert_eq!(authority(hostname, &[4781, 80]), Some(hostname.to_owned()));
            assert_eq!(
                authority(&format!("{hostname}:80"), &[4781, 80]),
                Some(hostname.to_owned())
            );
            assert_eq!(
                authority(&format!("{hostname}:4781"), &[4781, 80]),
                Some(format!("{hostname}:4781"))
            );
            assert_eq!(authority(hostname, &[49123]), None);
            assert_eq!(
                authority(&format!("{hostname}:49123"), &[49123]),
                Some(format!("{hostname}:49123"))
            );
        }
        for hostname in [
            "evil.test",
            "hey-boss.test.evil",
            "hey-boss.test.",
            "hey-boss.local",
            "hey-boss.test:81",
            "hey-boss.test:080",
            "hey-boss.test:+80",
            "hey-boss.test:",
            "hey-boss.test:80:80",
            "user@hey-boss.test",
            "hey-boss.test/path",
            "hey-boss.test?x",
            "hey-boss.test#x",
            "127.1",
            "0.0.0.0",
            "[::1]",
            "hey-boss.test\r\n",
        ] {
            assert_eq!(authority(hostname, &[4781, 80]), None, "{hostname:?}");
        }
    }
}
