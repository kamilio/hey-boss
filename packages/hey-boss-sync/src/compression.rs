//! Gzip with a bounded decoded size for private protocol frames and page chunks.
use crate::{Result, invalid};
use std::io::{Read, Write};

pub fn encode(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(bytes)?;
    Ok(gzip.finish()?)
}

pub fn decode(bytes: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    flate2::read::MultiGzDecoder::new(bytes)
        .take(limit as u64 + 1)
        .read_to_end(&mut decoded)?;
    if decoded.len() > limit {
        return Err(invalid("Decompressed content exceeds limit"));
    }
    Ok(decoded)
}

pub(crate) fn stored(payload: &str) -> Result<rusqlite::types::Value> {
    if payload.len() >= 1024 {
        let bytes = encode(payload.as_bytes())?;
        if bytes.len() < payload.len() {
            return Ok(rusqlite::types::Value::Blob(bytes));
        }
    }
    Ok(payload.to_owned().into())
}

pub(crate) fn payload(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<String> {
    match row.get_ref(column)? {
        rusqlite::types::ValueRef::Blob(bytes) => {
            let decoded =
                decode(bytes, crate::MAX_PAYLOAD).and_then(|bytes| Ok(String::from_utf8(bytes)?));
            decoded.map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Blob, e)
            })
        }
        _ => row.get(column),
    }
}

pub(crate) fn register(db: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::{functions::FunctionFlags, types::ValueRef};
    db.create_scalar_function(
        "payload_header_contains",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let needle = ctx.get::<String>(1)?;
            let head = match ctx.get_raw(0) {
                ValueRef::Text(bytes) => bytes[..bytes.len().min(512)].to_vec(),
                ValueRef::Blob(bytes) => {
                    let mut head = Vec::new();
                    flate2::read::GzDecoder::new(bytes)
                        .take(512)
                        .read_to_end(&mut head)
                        .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))?;
                    head
                }
                _ => Vec::new(),
            };
            Ok(!needle.is_empty() && head.windows(needle.len()).any(|w| w == needle.as_bytes()))
        },
    )?;
    db.create_scalar_function(
        "payload_starts_with",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let prefix = ctx.get::<String>(1)?;
            match ctx.get_raw(0) {
                ValueRef::Text(bytes) => Ok(bytes.starts_with(prefix.as_bytes())),
                ValueRef::Blob(bytes) => {
                    // Tagged subscribers only inflate the prefix, never unrelated
                    // file bodies. Full reads still verify the gzip checksum.
                    let mut head = Vec::new();
                    flate2::read::GzDecoder::new(bytes)
                        .take(prefix.len() as u64)
                        .read_to_end(&mut head)
                        .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))?;
                    Ok(head == prefix.as_bytes())
                }
                _ => Ok(false),
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_expansion_corruption_and_truncation() {
        let bytes = vec![b'x'; 1024 * 1024];
        let mut compressed = encode(&bytes).unwrap();
        assert_eq!(decode(&compressed, bytes.len()).unwrap(), bytes);
        assert!(decode(&compressed, bytes.len() - 1).is_err());
        assert!(decode(&compressed[..compressed.len() - 1], bytes.len()).is_err());
        compressed[20] ^= 255;
        assert!(decode(&compressed, bytes.len()).is_err());
    }
}
