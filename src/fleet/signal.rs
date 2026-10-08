//! A CLI mutation has one identity, known before any transport can accept it.
use crate::issues::{Error, Result};
use serde_json::{Value, json};
use std::{io::Write, os::unix::net::UnixStream, time::Duration};

pub fn queue_signal(
    host: &str,
    worker: &str,
    action: &str,
    request_id: Option<&str>,
) -> Result<Value> {
    let id = match request_id {
        Some(id)
            if !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)) =>
        {
            id.to_owned()
        }
        Some(_) => {
            return Err(Error::invalid(
                "Request ID must contain 1–128 ASCII letters, digits, or -_.:",
            ));
        }
        None => crate::issues::worker::random_id()?,
    };
    // Keep stdout machine-readable and make the key available even if the
    // process is interrupted while waiting for an acknowledgment.
    let mut stderr = std::io::stderr().lock();
    writeln!(stderr, "Request ID: {id}")?;
    stderr.flush()?;
    drop(stderr);
    let request = json!({"kind":"signal","id":id,"host":host,"worker":worker,"signal":action});
    let result = (|| {
        let socket = super::socket_path().map_err(not_sent)?;
        match UnixStream::connect(socket) {
            Ok(stream) => exchange(stream, &request),
            // Only route discovery happens here. Never retry a failed send.
            Err(_) => super::native::signal_request(request.clone()),
        }
    })();
    result
        .and_then(|value| {
            if value["id"] != id {
                return Err(unknown("Supervisor returned a different signal ID"));
            }
            Ok(value)
        })
        .map_err(|mut error| {
            let (outcome, message) = match error.code.as_str() {
                "signal_not_sent" => ("not_sent", "Not sent"),
                "signal_outcome_unknown" => {
                    ("unknown", "Outcome unknown; the signal may have been saved")
                }
                _ => ("rejected", "Signal rejected"),
            };
            let guidance = if outcome == "rejected" {
                format!("Request ID: {id}. Reuse this ID only with its original arguments.")
            } else {
                format!("Retry identical arguments with --request-id {id}.")
            };
            error.message = format!("{message}: {}.\n{guidance}", error.message);
            error.details = Some(json!({"request_id":id,"outcome":outcome}));
            error
        })
}

pub(in crate::fleet) fn not_sent(error: impl std::fmt::Display) -> Error {
    Error::new("signal_not_sent", error.to_string())
}
pub(in crate::fleet) fn unknown(error: impl std::fmt::Display) -> Error {
    Error::new("signal_outcome_unknown", error.to_string())
}

pub(in crate::fleet) fn exchange(mut stream: UnixStream, request: &Value) -> Result<Value> {
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(not_sent)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(not_sent)?;
    let mut bytes = serde_json::to_vec(request).map_err(not_sent)?;
    bytes.push(b'\n');
    // A partial write can still reach the server. Everything from this point
    // through an incomplete/malformed reply must retain the same retry key.
    stream.write_all(&bytes).map_err(unknown)?;
    // The newline terminates the request. A fast relay may close before a
    // half-close, so read its buffered acknowledgment without calling shutdown.
    let bytes = super::native::read_control_body(&mut stream)
        .map_err(unknown)?
        .ok_or_else(|| unknown("Fleet response exceeds 16 MiB"))?;
    if bytes.is_empty() {
        return Err(unknown("Connection closed before acknowledgment"));
    }
    let result: Value = serde_json::from_slice(&bytes).map_err(unknown)?;
    if result["ok"] == false {
        let error = if let Some(message) = result["error"].as_str() {
            Error::new("fleet_error", message)
        } else {
            serde_json::from_value::<Error>(result["error"].clone()).map_err(unknown)?
        };
        return Err(
            if error
                .details
                .as_ref()
                .is_some_and(|details| details["sent"] == false)
            {
                not_sent(error)
            } else if error.code == "fleet_unavailable" {
                unknown(error)
            } else {
                error
            },
        );
    }
    if result["ok"] != true {
        return Err(unknown("Fleet supervisor returned an incomplete response"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn malformed_and_incomplete_responses_leave_an_unknown_outcome() {
        for reply in ["", "not-json", "{}", r#"{"ok":false,"error":null}"#] {
            let (client, mut server) = UnixStream::pair().unwrap();
            let thread = std::thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(&mut server).read_line(&mut line).unwrap();
                assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["id"], "retry");
                server.write_all(reply.as_bytes()).unwrap();
            });
            let error = exchange(client, &json!({"kind":"signal","id":"retry"})).unwrap_err();
            thread.join().unwrap();
            assert_eq!(error.code, "signal_outcome_unknown");
        }
    }

    #[test]
    fn relay_errors_distinguish_pre_forwarding_failure_from_lost_acknowledgment() {
        for (error, expected) in [
            (
                json!({"code":"fleet_handshake_pending","message":"Waiting for handshake","details":{"sent":false}}),
                "signal_not_sent",
            ),
            (
                json!({"code":"fleet_unavailable","message":"Lost acknowledgment"}),
                "signal_outcome_unknown",
            ),
            (
                json!({"code":"fleet_error","message":"Signal ID already has a different payload"}),
                "fleet_error",
            ),
        ] {
            let (client, mut server) = UnixStream::pair().unwrap();
            let thread = std::thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(&mut server).read_line(&mut line).unwrap();
                writeln!(server, "{}", json!({"ok":false,"error":error})).unwrap();
            });
            assert_eq!(
                exchange(client, &json!({"kind":"worker_signal","id":"retry"}))
                    .unwrap_err()
                    .code,
                expected
            );
            thread.join().unwrap();
        }
    }
}
