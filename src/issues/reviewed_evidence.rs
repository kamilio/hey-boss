//! Lossless, bounded transport of the full reviewed reports, never a projection.
use super::{REVIEWED_EVIDENCE_LIMIT as JSON_LIMIT, ReviewedGithubEvidence};
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, read::MultiGzDecoder, write::GzEncoder};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io::{Read, Write};

const COMPRESSED_LIMIT: usize = 4 * 1024 * 1024;
// Preserve the original representation (and retry fingerprints) for inputs
// within the previous CLI limit. Only previously oversized arrays need packing.
const INLINE_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Packed {
    encoding: Encoding,
    data: String,
}

#[derive(Serialize, Deserialize)]
enum Encoding {
    #[serde(rename = "gzip-base64")]
    GzipBase64,
}

// Bound serialization too: callers can construct an operation without the CLI.
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > JSON_LIMIT - self.0.len() {
            return Err(std::io::Error::other("Reviewed evidence exceeds 64 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn serialize<S: Serializer>(
    evidence: &Option<Vec<ReviewedGithubEvidence>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let Some(evidence) = evidence else {
        return serializer.serialize_none();
    };
    let mut raw = Bounded(Vec::new());
    serde_json::to_writer(&mut raw, evidence).map_err(serde::ser::Error::custom)?;
    if raw.0.len() <= INLINE_LIMIT {
        return evidence.serialize(serializer);
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(&raw.0)
        .map_err(serde::ser::Error::custom)?;
    let compressed = encoder.finish().map_err(serde::ser::Error::custom)?;
    if compressed.len() > COMPRESSED_LIMIT {
        return Err(serde::ser::Error::custom(
            "Compressed reviewed evidence exceeds 4 MiB",
        ));
    }
    Packed {
        encoding: Encoding::GzipBase64,
        data: STANDARD.encode(compressed),
    }
    .serialize(serializer)
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<ReviewedGithubEvidence>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Inline(Vec<ReviewedGithubEvidence>),
        Packed(Packed),
    }
    match Option::<Wire>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Wire::Inline(evidence)) => Ok(Some(evidence)),
        Some(Wire::Packed(packed)) => {
            if packed.data.len() > COMPRESSED_LIMIT.div_ceil(3) * 4 {
                return Err(serde::de::Error::custom(
                    "Compressed reviewed evidence exceeds 4 MiB",
                ));
            }
            let compressed = STANDARD
                .decode(packed.data)
                .map_err(serde::de::Error::custom)?;
            if compressed.len() > COMPRESSED_LIMIT {
                return Err(serde::de::Error::custom(
                    "Compressed reviewed evidence exceeds 4 MiB",
                ));
            }
            let mut raw = Vec::new();
            MultiGzDecoder::new(compressed.as_slice())
                .take(JSON_LIMIT as u64 + 1)
                .read_to_end(&mut raw)
                .map_err(serde::de::Error::custom)?;
            if raw.len() > JSON_LIMIT {
                return Err(serde::de::Error::custom("Reviewed evidence exceeds 64 MiB"));
            }
            serde_json::from_slice(&raw)
                .map(Some)
                .map_err(serde::de::Error::custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn operation(evidence: Value) -> Result<super::super::Operation, serde_json::Error> {
        serde_json::from_value(json!({"action":"assign","number":1,"target":"github",
            "if_version":1,"reviewed_evidence":evidence}))
    }
    fn packed(raw: &[u8]) -> Value {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(raw).unwrap();
        json!({"encoding":"gzip-base64","data":STANDARD.encode(encoder.finish().unwrap())})
    }
    #[test]
    fn accepts_legacy_and_lossless_packed_arrays() {
        for value in [json!([]), packed(b"[]")] {
            let op = operation(value).unwrap();
            assert_eq!(
                serde_json::to_value(op).unwrap()["reviewed_evidence"],
                json!([])
            );
        }
        assert!(operation(Value::Null).is_ok());
    }
    #[test]
    fn rejects_corruption_trailing_data_and_wrong_schema() {
        for value in [
            json!({"encoding":"future","data":""}),
            json!({"encoding":"gzip-base64","data":"!"}),
            json!({"encoding":"gzip-base64","data":STANDARD.encode(b"not gzip")}),
            packed(b"[] trailing"),
            packed(b"null"),
            packed(b"{}"),
        ] {
            assert!(operation(value).is_err());
        }
        let mut valid = packed(b"[]");
        let mut bytes = STANDARD.decode(valid["data"].as_str().unwrap()).unwrap();
        bytes.pop();
        valid["data"] = json!(STANDARD.encode(bytes));
        assert!(operation(valid).is_err());
    }
    #[test]
    fn rejects_expansion_and_encoded_size_over_limits() {
        let mut raw = vec![b' '; JSON_LIMIT + 1];
        raw[..2].copy_from_slice(b"[]");
        assert!(
            operation(packed(&raw))
                .unwrap_err()
                .to_string()
                .contains("64 MiB")
        );
        assert!(
            operation(json!({"encoding":"gzip-base64",
            "data":"A".repeat(COMPRESSED_LIMIT.div_ceil(3)*4+1)}))
            .unwrap_err()
            .to_string()
            .contains("4 MiB")
        );
        let mut bounded = Bounded(Vec::new());
        assert!(bounded.write_all(&raw).is_err());
        assert!(bounded.0.is_empty());
    }
}
