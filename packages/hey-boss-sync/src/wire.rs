//! Length-delimited messages carried exclusively inside authenticated SSH.
use crate::{Result, compression, invalid};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{Read, Write};

// Payloads use base64 to bound expansion even for control-heavy text. Validate
// before allocating input; ordinary batches contain at most 8 MiB of facts.
pub const MAX_FRAME: usize = 48 * 1024 * 1024;
const GZIP: u32 = 1 << 31;

pub fn send(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() > MAX_FRAME {
        return Err(invalid("Replication frame exceeds limit"));
    }
    // Small handshakes stay readable to older peers so version fencing can
    // explain the required upgrade before any compressed facts are exchanged.
    let compressed = if data.len() >= 1024 {
        compression::encode(&data)?
    } else {
        Vec::new()
    };
    let (data, flag) = if !compressed.is_empty() && compressed.len() < data.len() {
        (compressed, GZIP)
    } else {
        (data, 0)
    };
    writer.write_all(&(data.len() as u32 | flag).to_be_bytes())?;
    writer.write_all(&data)?;
    writer.flush()?;
    Ok(())
}

pub fn receive<T: DeserializeOwned>(reader: &mut impl Read) -> Result<Option<T>> {
    let mut header = [0; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let header = u32::from_be_bytes(header);
    let length = (header & !GZIP) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(invalid("Invalid replication frame length"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    if header & GZIP != 0 {
        bytes = compression::decode(&bytes, MAX_FRAME)?;
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}
