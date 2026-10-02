//! Observe small JSONL events and drain oversized telemetry without retaining it.
use crate::issues::{Error, Result};
use serde_json::Value;
use std::io::BufRead;

const MAX_EVENT_BYTES: usize = 1024 * 1024;

pub(super) fn read_event(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return if line.is_empty() && !oversized {
                Ok(None)
            } else {
                Err(Error::new(
                    "worker_error",
                    "Chief returned an incomplete event",
                ))
            };
        }
        let newline = memchr::memchr(b'\n', chunk);
        let count = newline.map_or(chunk.len(), |at| at + 1);
        if !oversized {
            if count > MAX_EVENT_BYTES - line.len() {
                oversized = true;
                line = Vec::new();
            } else {
                let needed = line.len() + count;
                if needed > line.capacity() {
                    let capacity = needed
                        .max(line.capacity().saturating_mul(2))
                        .min(MAX_EVENT_BYTES);
                    line.reserve_exact(capacity - line.len());
                }
                line.extend_from_slice(&chunk[..count]);
            }
        }
        reader.consume(count);
        if newline.is_some() {
            // The launcher needs lifecycle events, not arbitrary command output.
            // Final responses also have a separate --output-last-message file.
            return if oversized {
                Ok(Some(Value::Null))
            } else {
                serde_json::from_slice(&line)
                    .map(Some)
                    .map_err(|e| Error::new("worker_error", format!("Chief event rejected: {e}")))
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{self, Read};

    #[test]
    fn large_escaped_command_output_preserves_following_completion() {
        let event = json!({"type":"item.completed","item":{"type":"command_execution","aggregated_output":"\"\\\n🦀".repeat(200_000)}});
        let input = format!(
            "{event}\n{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"Done\"}}}}\n{{\"type\":\"turn.completed\"}}\n"
        );
        let mut reader = io::BufReader::new(input.as_bytes());
        let (skipped, allocated) =
            crate::test_allocations::measure(|| read_event(&mut reader).unwrap().unwrap());
        assert!(skipped.is_null());
        assert!(
            allocated < 3 * MAX_EVENT_BYTES,
            "Draining allocated {allocated} bytes"
        );
        assert_eq!(
            read_event(&mut reader).unwrap().unwrap()["item"]["text"],
            "Done"
        );
        assert_eq!(
            read_event(&mut reader).unwrap().unwrap()["type"],
            "turn.completed"
        );
        assert!(read_event(&mut reader).unwrap().is_none());
    }

    #[test]
    fn malformed_normal_events_and_truncated_streams_are_rejected() {
        for input in [
            "not json\n",
            "{\"type\":\"turn.completed\"}",
            "{\"type\":\"turn.completed\"} {}\n",
            "{\"ignored\":\"\\q\"}\n",
            "{\"ignored\":[1,]}\n",
        ] {
            assert!(read_event(&mut input.as_bytes()).is_err(), "{input}");
        }
        let mut truncated = io::BufReader::new(io::repeat(b'x').take(2 * MAX_EVENT_BYTES as u64));
        assert!(
            read_event(&mut truncated)
                .unwrap_err()
                .message
                .contains("incomplete")
        );
    }

    #[test]
    fn telemetry_beyond_64_mib_is_drained_in_bounded_memory() {
        let prefix = &b"{\"type\":\"item.completed\",\"item\":{\"aggregated_output\":\""[..];
        let suffix = &b"\",\"type\":\"command_execution\"}}\n{\"type\":\"turn.completed\"}\n"[..];
        let mut reader = io::BufReader::new(
            prefix
                .chain(io::repeat(b'x').take(65 * 1024 * 1024))
                .chain(suffix),
        );
        let (result, allocated) = crate::test_allocations::measure(|| read_event(&mut reader));
        assert!(result.unwrap().unwrap().is_null());
        assert!(
            allocated < 3 * MAX_EVENT_BYTES,
            "Draining allocated {allocated} bytes"
        );
        assert_eq!(
            read_event(&mut reader).unwrap().unwrap()["type"],
            "turn.completed"
        );
    }
}
