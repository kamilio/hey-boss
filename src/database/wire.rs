use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};
pub(super) const VERSION: u32 = 1;
const LIMIT: usize = crate::issues::WIRE_LIMIT;
#[derive(Serialize, Deserialize, Debug)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(#[serde(with = "text_bytes")] String),
    Blob(#[serde(with = "binary_bytes")] Vec<u8>),
}

mod binary_bytes {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(value))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        STANDARD
            .decode(String::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}
mod text_bytes {
    use serde::{Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
        super::binary_bytes::serialize(value.as_bytes(), serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        String::from_utf8(super::binary_bytes::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}
impl From<rusqlite::types::Value> for SqlValue {
    fn from(v: rusqlite::types::Value) -> Self {
        use rusqlite::types::Value as V;
        match v {
            V::Null => Self::Null,
            V::Integer(n) => Self::Integer(n),
            V::Real(n) => Self::Real(n),
            V::Text(s) => Self::Text(s),
            V::Blob(b) => Self::Blob(b),
        }
    }
}
impl SqlValue {
    pub fn as_ref(&self) -> rusqlite::types::ValueRef<'_> {
        use rusqlite::types::ValueRef as V;
        match self {
            Self::Null => V::Null,
            Self::Integer(n) => V::Integer(*n),
            Self::Real(n) => V::Real(*n),
            Self::Text(s) => V::Text(s.as_bytes()),
            Self::Blob(b) => V::Blob(b),
        }
    }
}
impl rusqlite::ToSql for SqlValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(self.as_ref()))
    }
}
#[derive(Serialize, Deserialize)]
pub(super) enum Command {
    Hello,
    CheckSchema,
    ExclusiveSession,
    ReadTransaction,
    Prepare { sql: String },
    Execute { sql: String, values: Vec<SqlValue> },
    Query { sql: String, values: Vec<SqlValue> },
    Batch { sql: String },
    Backup { path: PathBuf },
}
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Reply {
    pub version: u32,
    pub pid: u32,
    pub transaction: bool,
    pub last_id: i64,
    pub changes: usize,
    pub parameters: usize,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlValue>>,
    pub steps: i32,
    pub error: Option<Failure>,
    pub more: bool,
    pub application: i64,
    pub schema: i64,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Failure {
    code: Option<i32>,
    message: String,
    domain: Option<crate::issues::Error>,
}
impl Failure {
    pub fn capture(e: rusqlite::Error) -> Self {
        let code = if let rusqlite::Error::SqliteFailure(code, _) = &e {
            Some(code.extended_code)
        } else {
            None
        };
        Self {
            code,
            message: e.to_string(),
            domain: if let rusqlite::Error::ToSqlConversionFailure(source) = &e {
                source.downcast_ref::<crate::issues::Error>().cloned()
            } else {
                None
            },
        }
    }
    pub fn restore(&self) -> rusqlite::Error {
        if let Some(domain) = &self.domain {
            return rusqlite::Error::ToSqlConversionFailure(Box::new(domain.clone()));
        }
        match self.code {
            Some(code) => rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                Some(self.message.clone()),
            ),
            None => super::error(&self.message),
        }
    }
}
pub(super) fn write<T: Serialize>(output: &mut impl Write, value: &T) -> io::Result<()> {
    // The encoder is capped while serializing, before any bytes reach the peer.
    struct Capped(Vec<u8>);
    impl Write for Capped {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > LIMIT.saturating_sub(self.0.len()) {
                return Err(io::Error::other("Database frame exceeds limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut encoded = Capped(Vec::new());
    serde_json::to_writer(&mut encoded, value)?;
    output.write_all(&(encoded.0.len() as u32).to_be_bytes())?;
    output.write_all(&encoded.0)?;
    output.flush()
}
pub(super) fn read<T: for<'a> Deserialize<'a>>(input: &mut impl BufRead) -> io::Result<Option<T>> {
    if input.fill_buf()?.is_empty() {
        return Ok(None);
    }
    let mut header = [0u8; 4];
    input
        .read_exact(&mut header)
        .map_err(|e| io::Error::other(format!("Incomplete database frame: {e}")))?;
    let size = u32::from_be_bytes(header) as usize;
    if size > LIMIT {
        return Err(io::Error::other("Database frame exceeds limit"));
    }
    let mut encoded = vec![0; size];
    input
        .read_exact(&mut encoded)
        .map_err(|e| io::Error::other(format!("Incomplete database frame: {e}")))?;
    Ok(Some(serde_json::from_slice(&encoded)?))
}
