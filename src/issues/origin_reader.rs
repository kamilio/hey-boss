//! Optional historical metadata must never prevent reading an issue roster.
use crate::database::Row;
use rusqlite::types::ValueRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Metadata {
    pub origin: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_error: Option<super::Error>,
}

impl Metadata {
    pub fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        let raw = match row.get_ref("origin") {
            Ok(raw) => raw,
            // Checkouts predating creation tracing do not select this column.
            Err(rusqlite::Error::InvalidColumnName(_)) => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        let parsed = match raw {
            ValueRef::Null => return Ok(Self::default()),
            ValueRef::Text(text) => serde_json::from_slice::<Value>(text)
                .map_err(|error| format!("Invalid issue origin JSON: {error}")),
            _ => Err("Invalid issue origin: expected JSON text or NULL".into()),
        };
        Ok(match parsed {
            Ok(Value::Null) => Self::default(),
            Ok(origin @ Value::Object(_)) => Self {
                origin: Some(origin),
                origin_error: None,
            },
            Ok(_) => {
                Self::invalid("Unsupported issue origin: expected a JSON object or null".into())
            }
            Err(message) => Self::invalid(message),
        })
    }

    fn invalid(message: String) -> Self {
        Self {
            origin: None,
            origin_error: Some(super::Error::new("invalid_origin", message)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nullable_and_invalid_metadata_are_read_without_exposing_raw_content() {
        let db = crate::database::Connection::open_in_memory().unwrap();
        for (raw, invalid) in [
            (Value::Null, false),
            (serde_json::json!({"session_id":"saved"}), false),
            (serde_json::json!([1]), true),
        ] {
            let metadata = db
                .query_row("SELECT ?1 AS origin", [raw.to_string()], Metadata::read)
                .unwrap();
            assert_eq!(metadata.origin_error.is_some(), invalid);
        }
        let metadata = db
            .query_row("SELECT NULL AS origin", [], Metadata::read)
            .unwrap();
        assert!(metadata.origin.is_none() && metadata.origin_error.is_none());
        let missing = db.query_row("SELECT 1", [], Metadata::read).unwrap();
        assert!(missing.origin.is_none() && missing.origin_error.is_none());
        for sql in [
            "SELECT '{private-broken' AS origin",
            "SELECT 123 AS origin",
            "SELECT x'FF' AS origin",
        ] {
            let metadata = db.query_row(sql, [], Metadata::read).unwrap();
            assert!(metadata.origin.is_none());
            assert!(
                !metadata
                    .origin_error
                    .unwrap()
                    .message
                    .contains("private-broken")
            );
        }
    }
}
