//! Read-only companion forwarding with one caller budget and source-fenced cursors.
mod cursor;
use super::*;
use crate::shared_read::{self, Read, Reply, Request, Response, wire};
use tokio::time::Instant;

impl ApiClient {
    pub(super) async fn relay_read<T: DeserializeOwned>(
        &self,
        mut request: reqwest::Request,
    ) -> Result<T> {
        let get = request.method() == reqwest::Method::GET;
        if get
            && shared_read::supported_path(request.url().path())
            && let Some(reply) = self.try_relay(&request).await?
        {
            return reply.decode();
        }
        // A primary cursor is not a local cursor, even on routes (such as the
        // raw source feed) that deliberately do not support shared reads.
        let wrapped = get
            && request
                .url()
                .query_pairs()
                .any(|(key, value)| key == "cursor" && cursor::wrapped(&value));
        if !wrapped {
            return self
                .direct_read(RequestBuilder::from_parts(self.http.clone(), request))
                .await;
        }
        let identity = self
            .shared_identity()
            .await
            .map_err(|_| Error::CursorExpired)?;
        let source = cursor::Source::new(false, &identity);
        unwrap_cursors(request.url_mut(), &source, false)?;
        let mut body: Value = self
            .direct_read(RequestBuilder::from_parts(self.http.clone(), request))
            .await?;
        if self.shared_identity().await? != identity {
            return Err(Error::CursorExpired);
        }
        source.wrap(&mut body)?;
        serde_json::from_value(body)
            .map_err(|_| Error::Invalid("invalid local response body".into()))
    }

    #[cfg(unix)]
    async fn try_relay(&self, request: &reqwest::Request) -> Result<Option<Reply>> {
        let Some(path) = &self.relay_socket else {
            return Ok(None);
        };
        let deadline = self.read_deadline.expect("one outer read deadline");
        // Discovery must never turn an absent or obsolete companion into a
        // long wait. It uses only cache-only local and primary identities.
        let handshake = async {
            let mut socket = connect(path).await?;
            let local = self.shared_identity().await?;
            let response = exchange(
                &mut socket,
                &Request::Probe {
                    identity: local.clone(),
                },
            )
            .await?;
            let Response::Identity { identity: primary } = response else {
                return Ok(None);
            };
            primary.validate()?;
            if local.user_id != primary.user_id
                || !local.hostname.eq_ignore_ascii_case(&primary.hostname)
            {
                return Ok(None);
            }
            // Negotiation selects the supervisor's primary credential scope.
            // Equal GitHub user IDs do not imply equal token permissions; API
            // access errors from that primary must therefore remain visible.
            Ok::<_, Error>(Some((local, primary)))
        };
        let Ok(Ok(Some((local, primary)))) = tokio::time::timeout_at(
            deadline.min(Instant::now() + Duration::from_secs(1)),
            handshake,
        )
        .await
        else {
            return Ok(None);
        };
        let source = cursor::Source::new(true, &primary);
        let mut url = request.url().clone();
        unwrap_cursors(&mut url, &source, true)?;
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(shared_read::MAX_TIMEOUT_MS.into()) as u64;
        if remaining == 0 {
            return Err(Error::Deadline);
        }
        let read = Read {
            identity: primary,
            path: url.path().into(),
            query: url.query().map(str::to_owned),
            timeout_ms: remaining,
        };
        if read.validate().is_err() {
            return Ok(None);
        }
        let result = async {
            let mut socket = connect(path).await?;
            exchange(&mut socket, &Request::Read { read }).await
        }
        .await;
        let Ok(Response::Reply { mut reply }) = result else {
            return Ok(None);
        };
        if (200..300).contains(&reply.status) {
            if self.shared_identity().await? != local {
                return Err(Error::CursorExpired);
            }
            source.wrap(&mut reply.body)?;
        }
        Ok(Some(reply))
    }

    #[cfg(not(unix))]
    async fn try_relay(&self, _request: &reqwest::Request) -> Result<Option<Reply>> {
        Ok(None)
    }
}

fn unwrap_cursors(url: &mut Url, source: &cursor::Source, shared: bool) -> Result<()> {
    // Avoid re-encoding unrelated query options or changing their ordering.
    if !url.query_pairs().any(|(key, _)| key == "cursor") {
        return Ok(());
    }
    let pairs = url
        .query_pairs()
        .map(|(key, value)| {
            let value = if key == "cursor" {
                source.unwrap(&value, shared)?.to_owned()
            } else {
                value.into_owned()
            };
            Ok((key.into_owned(), value))
        })
        .collect::<Result<Vec<_>>>()?;
    url.set_query(None);
    url.query_pairs_mut().extend_pairs(pairs);
    Ok(())
}

#[cfg(unix)]
async fn connect(path: &std::path::Path) -> Result<tokio::net::UnixStream> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let owner = unsafe { libc::geteuid() };
    let private =
        |metadata: &std::fs::Metadata| metadata.uid() == owner && metadata.mode() & 0o077 == 0;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| Error::Stopped)?;
    let parent = path.parent().ok_or(Error::Stopped)?;
    let directory = std::fs::symlink_metadata(parent).map_err(|_| Error::Stopped)?;
    if !metadata.file_type().is_socket()
        || !directory.is_dir()
        || !private(&metadata)
        || !private(&directory)
    {
        return Err(Error::Stopped);
    }
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .map_err(|_| Error::Stopped)?;
    if !stream.peer_cred().is_ok_and(|peer| peer.uid() == owner) {
        return Err(Error::Stopped);
    }
    Ok(stream)
}

#[cfg(unix)]
async fn exchange(socket: &mut tokio::net::UnixStream, request: &Request) -> Result<Response> {
    wire::write(socket, request, shared_read::MAX_REQUEST_BYTES).await?;
    wire::read(socket, shared_read::MAX_RESPONSE_BYTES).await
}

#[cfg(all(test, unix))]
mod tests;
