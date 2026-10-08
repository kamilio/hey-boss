//! Retry only failures that prove no authority request has been forwarded.
use super::*;
use std::io::{self, Read};

const POLL: Duration = Duration::from_millis(50);

pub(super) fn exchange(
    state: &Path,
    database: &Path,
    request: Value,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> crate::issues::Result<Value> {
    let envelope = json!({"version":1,"database":database,"request":request});
    let bytes = encode_frame(&envelope, crate::issues::WIRE_LIMIT - 1)
        .map_err(|error| failure_before_send(error, "encoding the authority request"))?
        .ok_or_else(|| {
            failure_before_send(
                "Fleet frame exceeds 16 MiB",
                "encoding the authority request",
            )
        })?;
    let mut last = "waiting for the companion relay".to_owned();
    loop {
        check(deadline, cancelled).map_err(|error| failure_before_send(error, &last))?;
        let stream = match connect(
            &state.join(SOCKET),
            POLL.min(deadline.saturating_duration_since(Instant::now()))
                .max(Duration::from_millis(1)),
        ) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::NotConnected
                ) =>
            {
                last = error.to_string();
                pause(deadline);
                continue;
            }
            Err(error) => {
                return Err(failure_before_send(
                    error,
                    "connecting to the companion relay",
                ));
            }
        };
        check(deadline, cancelled).map_err(|error| failure_before_send(error, &last))?;
        let mut stream = BoundedStream {
            stream,
            deadline,
            cancelled,
        };
        // No transport error after this point authorizes replay. The relay may
        // have forwarded even a partial/failed local write before disconnect.
        let mut result = (|| -> Result<Value> {
            stream.write_all(&bytes)?;
            stream.write_all(b"\n")?;
            read_frame(&mut BufReader::new(stream))?
                .ok_or_else(|| unavailable("connection closed before acknowledgment").into())
        })()
        .map_err(|error| unknown(unavailable(error), &envelope))?;
        if result["ok"] == false
            && result["error"]["code"] == "fleet_handshake_pending"
            && result["error"]["details"]["sent"] == false
        {
            last = "supervisor handshake has not completed".into();
            pause(deadline);
            continue;
        }
        if result["ok"] == false
            && result["error"]["code"] == "fleet_unavailable"
            && result["error"]["details"]["sent"] != false
        {
            let error: Error = serde_json::from_value(result["error"].clone())?;
            result["error"] = serde_json::to_value(unknown(error, &envelope))?;
        } else if result["ok"] != true && result["ok"] != false {
            return Err(unknown(unavailable("incomplete response"), &envelope));
        }
        return Ok(result);
    }
}

fn unknown(mut error: Error, envelope: &Value) -> Error {
    let details = error.details.get_or_insert_with(|| json!({}));
    if let Some(details) = details.as_object_mut() {
        for (key, value) in json!({"route":"supervisor_tunnel","sent":true,
            "outcome":"unknown","request_id":envelope["request"]["request"]["request_id"]})
        .as_object()
        .unwrap()
        {
            details.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    error
}

fn connect(path: &Path, timeout: Duration) -> io::Result<UnixStream> {
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(path)?, timeout)?;
    // A full Unix accept queue can report writable without connecting on Linux.
    socket.peer_addr()?;
    let stream: UnixStream = socket.into();
    // Set these before sending. On macOS, changing a timeout after the peer
    // closes can fail with EINVAL even though its reply is buffered for reading.
    stream.set_read_timeout(Some(POLL))?;
    stream.set_write_timeout(Some(POLL))?;
    Ok(stream)
}

fn failure_before_send(error: impl std::fmt::Display, phase: &str) -> Error {
    let mut error = unavailable(format!("{phase}: {error}"));
    error.details = Some(json!({"route":"supervisor_tunnel","sent":false}));
    error
}

fn check(deadline: Instant, cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::other("authority request cancelled"))
    } else if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "authority request deadline expired",
        ))
    } else {
        Ok(())
    }
}

fn pause(deadline: Instant) {
    thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
}

// Check the same deadline between partial reads/writes, retaining buffered
// frame bytes. Socket timeouts alone reset on every byte and are not a bound.
struct BoundedStream<'a> {
    stream: UnixStream,
    deadline: Instant,
    cancelled: &'a dyn Fn() -> bool,
}
fn pending(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}
impl Read for BoundedStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            check(self.deadline, self.cancelled)?;
            match self.stream.read(bytes) {
                Err(error) if pending(&error) => {}
                result => return result,
            }
        }
    }
}
impl Write for BoundedStream<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            check(self.deadline, self.cancelled)?;
            match self.stream.write(bytes) {
                Err(error) if pending(&error) => {}
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
