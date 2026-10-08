//! Deserialize accounting metadata while skipping prompts and generated content.
//! Serde still validates the complete JSON; ignored messages/text/tool deltas do
//! not allocate a second copy just to update accounting.
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::fmt;

// Unlike serde's IgnoredAny, this validates UTF-8, surrogate escapes, numeric
// ranges and nesting depth just as Value does, without retaining skipped values.
struct Discard;
impl<'de> Deserialize<'de> for Discard {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Validate;
        impl<'de> Visitor<'de> for Validate {
            type Value = Discard;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("valid JSON")
            }
            fn visit_unit<E>(self) -> Result<Discard, E> {
                Ok(Discard)
            }
            fn visit_bool<E>(self, _: bool) -> Result<Discard, E> {
                Ok(Discard)
            }
            fn visit_i64<E>(self, _: i64) -> Result<Discard, E> {
                Ok(Discard)
            }
            fn visit_u64<E>(self, _: u64) -> Result<Discard, E> {
                Ok(Discard)
            }
            fn visit_f64<E>(self, _: f64) -> Result<Discard, E> {
                Ok(Discard)
            }
            fn visit_str<E>(self, _: &str) -> Result<Discard, E> {
                Ok(Discard)
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Discard, M::Error> {
                while map.next_entry::<Discard, Discard>()?.is_some() {}
                Ok(Discard)
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut items: S) -> Result<Discard, S::Error> {
                while items.next_element::<Discard>()?.is_some() {}
                Ok(Discard)
            }
        }
        deserializer.deserialize_any(Validate)
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum Field {
    Type,
    Id,
    Model,
    Speed,
    InferenceGeo,
    Status,
    Usage,
    #[serde(rename = "usageMetadata")]
    UsageMetadata,
    Response,
    Message,
    Error,
    IncompleteDetails,
    Code,
    Reason,
    Choices,
    #[serde(other)]
    Other,
}

pub(super) struct Metadata(pub Value);
impl<'de> Deserialize<'de> for Metadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Metadata;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("response metadata")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut value = Map::new();
                while let Some(field) = map.next_key::<Field>()? {
                    let (key, item) = match field {
                        Field::Type => ("type", map.next_value()?),
                        Field::Id => ("id", map.next_value()?),
                        Field::Model => ("model", map.next_value()?),
                        Field::Speed => ("speed", map.next_value()?),
                        Field::InferenceGeo => ("inference_geo", map.next_value()?),
                        Field::Status => ("status", map.next_value()?),
                        Field::Usage => ("usage", map.next_value()?),
                        Field::UsageMetadata => ("usageMetadata", map.next_value()?),
                        Field::Code => ("code", map.next_value()?),
                        Field::Reason => ("reason", map.next_value()?),
                        Field::Response => ("response", map.next_value::<Metadata>()?.0),
                        Field::Message => ("message", map.next_value::<Metadata>()?.0),
                        Field::Error => ("error", map.next_value::<Metadata>()?.0),
                        Field::IncompleteDetails => {
                            ("incomplete_details", map.next_value::<Metadata>()?.0)
                        }
                        Field::Choices => ("choices", map.next_value::<Choices>()?.0),
                        Field::Other => {
                            map.next_value::<Discard>()?;
                            continue;
                        }
                    };
                    value.insert(key.into(), item);
                }
                Ok(Metadata(Value::Object(value)))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Metadata(Value::Null))
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(Metadata(Value::Bool(true)))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Metadata(Value::Bool(true)))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(Metadata(Value::Bool(true)))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Metadata(Value::Bool(true)))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Metadata(Value::Bool(true)))
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut items: S) -> Result<Self::Value, S::Error> {
                while items.next_element::<Discard>()?.is_some() {}
                Ok(Metadata(Value::Bool(true)))
            }
        }
        deserializer.deserialize_any(Fields)
    }
}
struct Choices(Value);
impl<'de> Deserialize<'de> for Choices {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Items;
        impl<'de> Visitor<'de> for Items {
            type Value = Choices;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("choices array")
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut items: S) -> Result<Self::Value, S::Error> {
                let mut nonempty = false;
                while items.next_element::<Discard>()?.is_some() {
                    nonempty = true;
                }
                Ok(Choices(Value::Array(if nonempty {
                    vec![Value::Null]
                } else {
                    Vec::new()
                })))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Choices(Value::Null))
            }
        }
        deserializer.deserialize_any(Items)
    }
}

pub(super) fn parse(bytes: &[u8]) -> serde_json::Result<Value> {
    // Unusual metadata shapes retain the previous reader's permissive behavior.
    // Well-formed provider responses stay on the selective path.
    serde_json::from_slice::<Metadata>(bytes)
        .map(|metadata| metadata.0)
        .or_else(|_| serde_json::from_slice(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_usage_terminal_metadata_and_skips_generated_content() {
        let payload = json!({"type":"response.completed", "response": {
            "id":"resp_1", "status":"incomplete", "output":[{"text":"private"}],
            "usage":{"input_tokens":12,"output_tokens":4,"input_tokens_details":{"cached_tokens":3}},
            "error":{"code":"test","message":"private"}, "incomplete_details":{"reason":"max_output_tokens"}}});
        let value = parse(&serde_json::to_vec(&payload).unwrap()).unwrap();
        assert_eq!(value["response"]["usage"], payload["response"]["usage"]);
        assert_eq!(value["response"]["error"]["code"], "test");
        assert_eq!(
            value["response"]["incomplete_details"]["reason"],
            "max_output_tokens"
        );
        assert!(!value.to_string().contains("private"));
        assert_eq!(
            parse(br#"{"choices":[{"delta":{"content":"private"}}],"usage":{"prompt_tokens":1}}"#)
                .unwrap()["choices"],
            json!([null])
        );
    }

    #[test]
    fn escaped_keys_and_unusual_metadata_do_not_hide_usage_or_errors() {
        let value =
            parse(br#"{"\u0075sage":{"input_tokens":5},"choices":0,"error":"failed"}"#).unwrap();
        assert_eq!(value["usage"]["input_tokens"], 5);
        assert_eq!(value["error"], "failed");
        assert!(parse(br#"{"type":"response.output_text.delta","delta":"unterminated}"#).is_err());
    }

    #[test]
    fn native_request_metadata_skips_conversation_and_preserves_last_duplicate_key() {
        let value = parse(br#"{"model":"old","model":"claude-sonnet-4-6","speed":"fast","inference_geo":"us","system":"private","messages":[{"role":"user","content":[{"type":"text","text":"private"}]}],"tools":[{"name":"private"}]}"#).unwrap();
        assert_eq!(
            value,
            json!({"model":"claude-sonnet-4-6","speed":"fast","inference_geo":"us"})
        );
        assert!(parse(br#"{"model":"claude-sonnet-4-6","messages":[{"content":"broken}"#).is_err());
    }

    #[test]
    fn skipped_values_still_validate_unicode_numbers_and_nesting() {
        for payload in [
            br#"{"model":"claude","messages":[{"content":"\ud800"}]}"#.as_slice(),
            b"{\"model\":\"claude\",\"messages\":[{\"content\":\"\xff\"}]}",
            br#"{"model":"claude","messages":[{"content":1e999}]}"#,
        ] {
            assert!(serde_json::from_slice::<Value>(payload).is_err());
            assert!(parse(payload).is_err());
        }
        let deep = format!("{{\"messages\":{}0{}}}", "[".repeat(150), "]".repeat(150));
        assert!(serde_json::from_str::<Value>(&deep).is_err());
        assert!(parse(deep.as_bytes()).is_err());
        assert!(parse(br#"{"model":"claude","messages":[{"content":"\ud83d\ude00"}]}"#).is_ok());
    }
}

#[cfg(test)]
mod native_accounting_tests {
    #[test]
    fn native_gemini_usage_survives_metadata_only_parsing() {
        let value=super::parse(br#"{"candidates":[{"content":{"parts":[{"text":"not retained"}]}}],"usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":5,"thoughtsTokenCount":15}}"#).unwrap();
        assert!(value.get("candidates").is_none());
        assert_eq!(value["usageMetadata"]["promptTokenCount"], 100);
    }
}
