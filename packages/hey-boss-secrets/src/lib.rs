//! Local secret access. Only signed ciphertext enters the replication journal.
pub mod backend;
pub mod client;
mod crypto;
pub mod provision;
mod store;
pub use crypto::Enrollment;
use std::fmt;
pub use store::{Inventory, Record, Store};
use zeroize::Zeroizing;

pub const MAX_SECRET: usize = 1024 * 1024;
pub const KIND: &str = "secret_revision_v1";
pub type Result<T> = std::result::Result<T, Error>;

/// Fixed errors deliberately exclude command output, credential values and paths.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    NotInitialized,
    NotFound,
    StoreUnavailable,
    InvalidInput,
    InvalidRecord,
    UntrustedWriter,
    NotWriter,
    NotGranted,
    StaleRevision,
    Conflict,
    PendingHistory,
    StorageFailure,
    CryptoFailure,
    TooLarge,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}
impl std::error::Error for Error {}

/// Printing or debugging this value never reveals its bytes.
pub struct Secret(Zeroizing<Vec<u8>>);
impl Secret {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests;
