//! Bounded snapshot transport; only a verified complete pull reaches replication.
use super::{Result, context::send, replica::invalid};
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, bufread::GzDecoder, write::GzEncoder};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
};

const CHUNK_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn send_pull(
    writer: &mut impl Write,
    payload: Value,
    receipts: Vec<Value>,
    supports_chunks: bool,
) -> Result<()> {
    let snapshot = payload["tables"].is_object();
    let mut frame = json!({"version":1,"kind":"pull"});
    frame["payload"] = payload;
    frame["receipts"] = Value::Array(receipts);
    if !supports_chunks || !snapshot {
        let bytes = serde_json::to_vec(&frame)?;
        if bytes.len() + 1 <= crate::issues::WIRE_LIMIT {
            writer.write_all(&bytes)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
            return Ok(());
        }
        if !supports_chunks {
            return Err(invalid(
                "Companion needs upgrading to receive pulls larger than 16 MiB",
            ));
        }
    }
    let transfer = crate::issues::worker::random_id()?;
    send(
        writer,
        json!({"kind":"pull_begin","transfer":transfer,"encoding":"gzip-base64"}),
    )?;
    let chunks = Chunks {
        writer,
        transfer,
        buffer: Vec::with_capacity(CHUNK_BYTES),
        parts: 0,
        bytes: 0,
        hash: Sha256::new(),
    };
    // JSON emits many tiny writes. Buffer them before invoking compression.
    let mut gzip = BufWriter::new(GzEncoder::new(chunks, Compression::fast()));
    serde_json::to_writer(&mut gzip, &frame)?;
    let mut chunks = gzip
        .into_inner()
        .map_err(|error| error.into_error())?
        .finish()?;
    chunks.emit()?;
    send(
        chunks.writer,
        json!({"kind":"pull_end","transfer":chunks.transfer,
        "parts":chunks.parts,"bytes":chunks.bytes,"sha256":format!("{:x}",chunks.hash.finalize())}),
    )
}

struct Chunks<'a, W> {
    writer: &'a mut W,
    transfer: String,
    buffer: Vec<u8>,
    parts: u64,
    bytes: u64,
    hash: Sha256,
}
impl<W: Write> Chunks<'_, W> {
    fn emit(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let mut frame = json!({"kind":"pull_chunk","transfer":self.transfer,"index":self.parts});
        frame["data"] = Value::String(STANDARD.encode(&self.buffer));
        send(self.writer, frame).map_err(io::Error::other)?;
        self.hash.update(&self.buffer);
        self.bytes += self.buffer.len() as u64;
        self.parts += 1;
        self.buffer.clear();
        Ok(())
    }
}
impl<W: Write> Write for Chunks<'_, W> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let written = bytes.len();
        while !bytes.is_empty() {
            let take = bytes.len().min(CHUNK_BYTES - self.buffer.len());
            self.buffer.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.buffer.len() == CHUNK_BYTES {
                self.emit()?;
            }
        }
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.emit()?;
        self.writer.flush()
    }
}

#[derive(Default)]
pub(super) struct PullReader {
    transfer: Option<Transfer>,
}
struct Transfer {
    id: String,
    file: File,
    parts: u64,
    bytes: u64,
    hash: Sha256,
}
impl PullReader {
    pub(super) fn receive(&mut self, frame: Value) -> Result<Option<Value>> {
        match frame["kind"].as_str() {
            Some("pull_begin") => {
                if self.transfer.is_some() || frame["encoding"] != "gzip-base64" {
                    return Err(invalid("Invalid or overlapping streamed pull"));
                }
                let id = frame["transfer"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 256)
                    .ok_or_else(|| invalid("Missing pull transfer identity"))?
                    .to_owned();
                let path = std::env::temp_dir().join(format!(
                    "hey-boss-pull-{}.gz",
                    crate::issues::worker::random_id()?
                ));
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)?;
                // Anonymous private staging survives only as long as this connection.
                fs::remove_file(path)?;
                self.transfer = Some(Transfer {
                    id,
                    file,
                    parts: 0,
                    bytes: 0,
                    hash: Sha256::new(),
                });
                Ok(None)
            }
            Some("pull_chunk") => {
                let transfer = self
                    .transfer
                    .as_mut()
                    .ok_or_else(|| invalid("Pull chunk arrived before its transfer"))?;
                if frame["transfer"].as_str() != Some(&transfer.id)
                    || frame["index"].as_u64() != Some(transfer.parts)
                {
                    return Err(invalid("Missing, reordered or duplicate pull chunk"));
                }
                let data = frame["data"]
                    .as_str()
                    .filter(|data| !data.is_empty() && data.len() <= CHUNK_BYTES.div_ceil(3) * 4)
                    .ok_or_else(|| invalid("Invalid pull chunk size"))?;
                let bytes = STANDARD.decode(data)?;
                if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
                    return Err(invalid("Invalid pull chunk size"));
                }
                transfer.file.write_all(&bytes)?;
                transfer.hash.update(&bytes);
                transfer.bytes += bytes.len() as u64;
                transfer.parts += 1;
                Ok(None)
            }
            Some("pull_end") => {
                let mut transfer = self
                    .transfer
                    .take()
                    .ok_or_else(|| invalid("Pull end arrived before its transfer"))?;
                if frame["transfer"].as_str() != Some(&transfer.id)
                    || frame["parts"].as_u64() != Some(transfer.parts)
                    || frame["bytes"].as_u64() != Some(transfer.bytes)
                    || frame["sha256"].as_str() != Some(&format!("{:x}", transfer.hash.finalize()))
                {
                    return Err(invalid("Incomplete or damaged streamed pull"));
                }
                transfer.file.seek(SeekFrom::Start(0))?;
                let mut decoder = BufReader::new(GzDecoder::new(BufReader::new(transfer.file)));
                let decoded: Value = serde_json::from_reader(&mut decoder)?;
                if !decoder.into_inner().into_inner().fill_buf()?.is_empty() {
                    return Err(invalid("Trailing data in streamed pull"));
                }
                if decoded["version"] != 1
                    || decoded["kind"] != "pull"
                    || !decoded["payload"].is_object()
                    || !decoded["receipts"].is_array()
                {
                    return Err(invalid("Invalid streamed pull payload"));
                }
                Ok(Some(decoded))
            }
            Some("pull") if self.transfer.is_some() => Err(invalid(
                "Pull arrived before its previous transfer finished",
            )),
            _ => Ok(Some(frame)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::context::read_frame;
    use super::*;
    use std::{
        io::Cursor,
        os::unix::fs::{MetadataExt, PermissionsExt},
        time::Instant,
    };

    fn frames(bytes: &[u8]) -> Vec<Value> {
        let mut reader = Cursor::new(bytes);
        let mut frames = Vec::new();
        while let Some(frame) = read_frame(&mut reader).unwrap() {
            frames.push(frame);
        }
        frames
    }

    #[test]
    fn small_pulls_keep_legacy_protocol_and_new_peers_accept_ordinary_frames() {
        let payload = json!({"changes":[],"allocations":[],"ranges":[],"cursor":9});
        for supported in [false, true] {
            let mut bytes = Vec::new();
            send_pull(
                &mut bytes,
                payload.clone(),
                vec![json!({"seq":7,"state":"accepted"})],
                supported,
            )
            .unwrap();
            let sent = frames(&bytes);
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0]["kind"], "pull");
            assert_eq!(sent[0]["payload"], payload);
            assert_eq!(
                PullReader::default().receive(sent[0].clone()).unwrap(),
                Some(sent[0].clone())
            );
        }
    }

    #[test]
    fn large_incompressible_rows_use_multiple_bounded_frames() {
        let mut random = 123456789u64;
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let body: String = (0..20 * 1024 * 1024)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                alphabet[random as usize & 63] as char
            })
            .collect();
        let payload =
            json!({"tables":{"issues":[{"body":body}]},"allocations":[],"ranges":[],"cursor":123});
        let mut bytes = Vec::new();
        send_pull(&mut bytes, payload.clone(), vec![], true).unwrap();
        assert!(
            bytes
                .split(|b| *b == b'\n')
                .all(|frame| frame.len() + 1 <= crate::issues::WIRE_LIMIT)
        );
        let sent = frames(&bytes);
        assert!(
            sent.iter()
                .filter(|frame| frame["kind"] == "pull_chunk")
                .count()
                > 1
        );
        let mut reader = PullReader::default();
        let mut completed = None;
        for (index, frame) in sent.into_iter().enumerate() {
            if index == 0 {
                assert!(reader.receive(frame).unwrap().is_none());
                let file = reader.transfer.as_ref().unwrap().file.metadata().unwrap();
                assert_eq!(file.permissions().mode() & 0o777, 0o600);
                assert_eq!(
                    file.nlink(),
                    0,
                    "Interrupted transfers must not leave staging paths"
                );
            } else if let Some(frame) = reader.receive(frame).unwrap() {
                assert!(completed.is_none());
                completed = Some(frame);
            }
        }
        assert_eq!(completed.unwrap()["payload"], payload);
        let mut legacy = Vec::new();
        assert!(
            send_pull(&mut legacy, payload, vec![], false)
                .unwrap_err()
                .to_string()
                .contains("upgrading")
        );
        assert!(
            legacy.is_empty(),
            "Do not emit partial oversized frames to legacy peers"
        );
    }

    #[test]
    fn damaged_reordered_and_overlapping_transfers_never_complete() {
        let payload = json!({"tables":{},"allocations":[],"ranges":[],"cursor":1});
        let mut bytes = Vec::new();
        send_pull(&mut bytes, payload, vec![], true).unwrap();
        let sent = frames(&bytes);
        assert_eq!(sent.len(), 3);
        let (begin, chunk, end) = (sent[0].clone(), sent[1].clone(), sent[2].clone());
        assert!(PullReader::default().receive(chunk.clone()).is_err());
        assert!(PullReader::default().receive(end.clone()).is_err());
        let mut reader = PullReader::default();
        reader.receive(begin.clone()).unwrap();
        assert!(reader.receive(begin.clone()).is_err());
        assert!(reader.receive(json!({"kind":"pull"})).is_err());
        for field in ["index", "transfer", "data"] {
            let mut reader = PullReader::default();
            reader.receive(begin.clone()).unwrap();
            let mut bad = chunk.clone();
            bad[field] = match field {
                "index" => json!(1),
                "transfer" => json!("other"),
                _ => json!("invalid base64!"),
            };
            assert!(reader.receive(bad).is_err(), "{field}");
        }
        for field in ["parts", "bytes", "sha256", "transfer"] {
            let mut reader = PullReader::default();
            reader.receive(begin.clone()).unwrap();
            reader.receive(chunk.clone()).unwrap();
            let mut bad = end.clone();
            bad[field] = match field {
                "parts" | "bytes" => json!(0),
                _ => json!("wrong"),
            };
            assert!(reader.receive(bad).is_err(), "{field}");
        }
        let mut reader = PullReader::default();
        reader.receive(begin).unwrap();
        reader.receive(chunk.clone()).unwrap();
        assert!(reader.receive(chunk).is_err());
    }

    #[test]
    fn valid_hash_does_not_hide_truncated_gzip_or_trailing_data() {
        let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
        gzip.write_all(br#"{"version":1,"kind":"pull","payload":{},"receipts":[]}"#)
            .unwrap();
        let complete = gzip.finish().unwrap();
        for damaged in [
            complete[..complete.len() - 4].to_vec(),
            [complete.as_slice(), b"junk"].concat(),
        ] {
            let mut reader = PullReader::default();
            reader
                .receive(json!({"kind":"pull_begin","transfer":"test","encoding":"gzip-base64"}))
                .unwrap();
            reader.receive(json!({"kind":"pull_chunk","transfer":"test","index":0,"data":STANDARD.encode(&damaged)})).unwrap();
            assert!(reader.receive(json!({"kind":"pull_end","transfer":"test","parts":1,"bytes":damaged.len(),"sha256":format!("{:x}",Sha256::digest(&damaged))})).is_err());
        }
    }

    #[test]
    #[ignore = "Profiles an explicitly supplied private database backup"]
    fn profile_snapshot_transport() {
        let path = std::path::PathBuf::from(
            std::env::var_os("HEY_BOSS_PROFILE_DB")
                .expect("Set HEY_BOSS_PROFILE_DB to a private backup"),
        );
        // Upgrade the isolated backup with the same schema as live stores.
        drop(crate::issues::Store::open(&path).unwrap());
        let db = rusqlite::Connection::open(&path).unwrap();
        let started = Instant::now();
        let payload = super::super::replica::snapshot(&db, "profile").unwrap();
        let cursor = payload["cursor"].clone();
        let build = started.elapsed();
        let started = Instant::now();
        let mut bytes = Vec::new();
        send_pull(&mut bytes, payload, vec![], true).unwrap();
        let encoding = started.elapsed();
        let started = Instant::now();
        let mut reader = PullReader::default();
        let mut input = Cursor::new(&bytes);
        let mut count = 0;
        let mut completed = None;
        while let Some(frame) = read_frame(&mut input).unwrap() {
            count += 1;
            if let Some(frame) = reader.receive(frame).unwrap() {
                completed = Some(frame);
            }
        }
        assert_eq!(completed.unwrap()["payload"]["cursor"], cursor);
        eprintln!(
            "Snapshot: {build:?} build; {encoding:?} encode; {:?} decode; {} bytes in {count} frames",
            started.elapsed(),
            bytes.len()
        );
    }
}
