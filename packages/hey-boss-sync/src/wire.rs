//! Length-delimited messages carried exclusively inside authenticated SSH.
use crate::{Result, invalid};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{Read, Write};

// Payloads use base64 to bound expansion even for control-heavy text. Validate
// before allocating input; ordinary batches contain at most 2 MiB of facts.
pub const MAX_FRAME: usize = 48 * 1024 * 1024;

pub fn send(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() > MAX_FRAME {
        return Err(invalid("Replication frame exceeds limit"));
    }
    writer.write_all(&(data.len() as u32).to_be_bytes())?;
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
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(invalid("Invalid replication frame length"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}
