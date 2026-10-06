//! Backend half of the fleet relay. Calls only this client's local daemon;
//! identities, headers and read budgets cannot be chosen by the remote URL.
use super::*;
use crate::shared_read::{Identity, MAX_RESPONSE_BYTES, Read, Reply};

impl ApiClient {
    /// Cache-only identity for a shared-read handshake. A cold identity is an
    /// explicit cache miss; the handshake can separately request a viewer read.
    pub async fn shared_identity(&self) -> Result<Identity> {
        let identity: Identity = self
            .bounded_shared_reply(self.http.get(self.url("v1/identity")), 1024)
            .await?
            .decode()?;
        identity.validate()?;
        Ok(identity)
    }

    /// Execute one validated read for an authenticated fleet peer. This method
    /// never discovers a relay or retries against another daemon/account.
    pub async fn shared_read(&self, read: &Read) -> Result<Reply> {
        read.validate()?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(read.timeout_ms);
        let client = self.clone().with_read_deadline(deadline);
        let deadline = client.read_deadline.unwrap();
        tokio::time::timeout_at(deadline, async {
            client.require_shared_identity(&read.identity).await?;
            let mut url = client.base.clone();
            url.set_path(&read.path);
            url.set_query(read.query.as_deref());
            let reply = client
                .bounded_shared_reply(client.http.get(url), MAX_RESPONSE_BYTES)
                .await?;
            // A local daemon can restart on the same port while this read is
            // pending. Discard its body if the identity changed at either end.
            client.require_shared_identity(&read.identity).await?;
            Ok(reply)
        })
        .await
        .unwrap_or(Err(Error::Deadline))
    }

    async fn bounded_shared_reply(
        &self,
        request: RequestBuilder,
        max_bytes: usize,
    ) -> Result<Reply> {
        let read = async {
            let mut request = self.authorize(request)?;
            if let Some(deadline) = self.read_deadline {
                let remaining = deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_millis()
                    .min(crate::shared_read::MAX_TIMEOUT_MS.into());
                if remaining == 0 {
                    return Err(Error::Deadline);
                }
                request = request.header(READ_TIMEOUT_HEADER, remaining.to_string());
            }
            let mut response = request.send().await.map_err(transport)?;
            let status = response.status().as_u16();
            let retry_after_seconds = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok());
            if response
                .content_length()
                .is_some_and(|n| n > max_bytes as u64)
            {
                return Err(Error::Invalid("shared response exceeds size limit".into()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(transport)? {
                if bytes.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(Error::Invalid("shared response exceeds size limit".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            let body = serde_json::from_slice(&bytes)
                .map_err(|_| Error::Invalid("invalid shared response body".into()))?;
            Ok(Reply {
                status,
                retry_after_seconds,
                body,
            })
        };
        if let Some(deadline) = self.read_deadline {
            tokio::time::timeout_at(deadline, read)
                .await
                .unwrap_or(Err(Error::Deadline))
        } else {
            read.await
        }
    }

    async fn require_shared_identity(&self, expected: &Identity) -> Result<()> {
        if &self.shared_identity().await? != expected {
            return Err(Error::Invalid("shared daemon identity changed".into()));
        }
        Ok(())
    }
}
