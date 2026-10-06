//! One bounded JSON request and reply per owner-private Unix socket connection.
use crate::{Error, Result};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub async fn read<T: DeserializeOwned>(
    input: &mut (impl AsyncRead + Unpin),
    limit: usize,
) -> Result<T> {
    let size = input.read_u32().await.map_err(transport)? as usize;
    if size == 0 || size > limit {
        return Err(Error::Invalid("shared frame exceeds size limit".into()));
    }
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes).await.map_err(transport)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Invalid("invalid shared frame".into()))
}

pub async fn write(
    output: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
    limit: usize,
) -> Result<()> {
    let mut buffer = Buffer {
        bytes: Vec::with_capacity(limit.min(4096)),
        limit: limit.min(u32::MAX as usize),
        overflow: false,
    };
    let result = serde_json::to_writer(&mut buffer, value);
    if buffer.overflow {
        return Err(Error::Invalid("shared frame exceeds size limit".into()));
    }
    result.map_err(|_| Error::Invalid("invalid shared frame".into()))?;
    output
        .write_u32(buffer.bytes.len() as u32)
        .await
        .map_err(transport)?;
    output.write_all(&buffer.bytes).await.map_err(transport)?;
    output.flush().await.map_err(transport)
}

struct Buffer {
    bytes: Vec<u8>,
    limit: usize,
    overflow: bool,
}
impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.overflow = true;
            return Err(std::io::Error::other("shared frame exceeds size limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn transport(error: std::io::Error) -> Error {
    Error::Transport(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn frames_round_trip_and_leave_the_next_message_intact() {
        let (mut left, mut right) = tokio::io::duplex(1024);
        write(&mut left, &json!({"one":1}), 100).await.unwrap();
        write(&mut left, &json!({"two":2}), 100).await.unwrap();
        assert_eq!(
            read::<Value>(&mut right, 100).await.unwrap(),
            json!({"one":1})
        );
        assert_eq!(
            read::<Value>(&mut right, 100).await.unwrap(),
            json!({"two":2})
        );
    }

    #[tokio::test]
    async fn declared_size_is_rejected_before_allocating_or_waiting_for_a_body() {
        let (mut left, mut right) = tokio::io::duplex(32);
        left.write_u32(u32::MAX).await.unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            read::<Value>(&mut right, 1024),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(Error::Invalid(_))));
    }

    #[tokio::test]
    async fn oversized_serialization_writes_no_partial_frame() {
        let (mut left, mut right) = tokio::io::duplex(32);
        assert!(matches!(
            write(&mut left, &json!({"body":"x".repeat(100)}), 32).await,
            Err(Error::Invalid(_))
        ));
        write(&mut left, &json!(true), 32).await.unwrap();
        assert_eq!(read::<Value>(&mut right, 32).await.unwrap(), json!(true));
    }
}
