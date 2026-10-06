use crate::{Error, Result, shared_read::Identity};
use serde_json::Value;

#[derive(Clone, Debug)]
pub(super) struct Source(String);
impl Source {
    pub fn new(shared: bool, identity: &Identity) -> Self {
        Self(crate::digest(
            &serde_json::json!([shared, identity]).to_string(),
        ))
    }
    pub fn unwrap<'a>(&self, cursor: &'a str, shared: bool) -> Result<&'a str> {
        if !wrapped(cursor) {
            return if !shared && valid(cursor) {
                Ok(cursor)
            } else {
                Err(Error::CursorExpired)
            };
        }
        let (source, native) = cursor
            .strip_prefix("hgr1:")
            .and_then(|value| value.split_once(':'))
            .ok_or(Error::CursorExpired)?;
        if source != self.0 || !valid(native) || wrapped(native) {
            return Err(Error::CursorExpired);
        }
        Ok(native)
    }
    pub fn wrap(&self, body: &mut Value) -> Result<()> {
        for key in [
            "cursor",
            "next_cursor",
            "head_cursor",
            "nextCursor",
            "headCursor",
        ] {
            self.field(body.get_mut(key))?;
        }
        if let Some(changes) = body.get_mut("changes").and_then(Value::as_array_mut) {
            for change in changes {
                self.field(change.get_mut("cursor"))?;
            }
        }
        Ok(())
    }
    fn field(&self, field: Option<&mut Value>) -> Result<()> {
        let Some(value) = field.filter(|v| !v.is_null()) else {
            return Ok(());
        };
        let native = value
            .as_str()
            .filter(|s| valid(s) && !wrapped(s))
            .ok_or_else(|| Error::Invalid("invalid shared response cursor".into()))?;
        *value = Value::String(format!("hgr1:{}:{native}", self.0));
        Ok(())
    }
}
pub(super) fn wrapped(value: &str) -> bool {
    value.starts_with("hgr")
}
fn valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 8192
        && value.is_ascii()
        && !value.bytes().any(|b| b.is_ascii_control())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn identity() -> Identity {
        Identity {
            hostname: "github.com".into(),
            user_id: 42,
            instance: "a".repeat(32),
        }
    }

    #[test]
    fn cursor_sources_bind_route_user_host_and_daemon() {
        let expected = Source::new(true, &identity());
        let mut body = json!({"cursor":"pr1:original"});
        expected.wrap(&mut body).unwrap();
        let cursor = body["cursor"].as_str().unwrap();
        assert!(wrapped(cursor));
        assert_eq!(expected.unwrap(cursor, true).unwrap(), "pr1:original");
        for source in [
            Source::new(false, &identity()),
            Source::new(
                true,
                &Identity {
                    user_id: 43,
                    ..identity()
                },
            ),
            Source::new(
                true,
                &Identity {
                    hostname: "other.example.com".into(),
                    ..identity()
                },
            ),
            Source::new(
                true,
                &Identity {
                    instance: "b".repeat(32),
                    ..identity()
                },
            ),
        ] {
            assert!(matches!(
                source.unwrap(cursor, false),
                Err(Error::CursorExpired)
            ));
        }
    }
    #[test]
    fn legacy_cursors_continue_locally_but_cannot_enter_a_shared_feed() {
        let source = Source::new(false, &identity());
        assert_eq!(
            source.unwrap("native:cursor", false).unwrap(),
            "native:cursor"
        );
        assert!(matches!(
            source.unwrap("native:cursor", true),
            Err(Error::CursorExpired)
        ));
        for invalid in [
            "hgr1:",
            "hgr1:bad:value",
            "hgr2:unknown",
            "hgr1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:",
        ] {
            assert!(matches!(
                source.unwrap(invalid, false),
                Err(Error::CursorExpired)
            ));
        }
    }
    #[test]
    fn all_exposed_cursor_fields_are_fenced_without_rewriting_github_payloads() {
        let source = Source::new(true, &identity());
        let mut body = json!({"cursor":"one","next_cursor":"two","head_cursor":"three","nextCursor":"four","headCursor":"five","changes":[{"cursor":"six","pullRequest":{"body":"unchanged","cursor":"user-data"}}],"data":{"cursor":"also-user-data"}});
        source.wrap(&mut body).unwrap();
        for (field, value) in [
            ("cursor", "one"),
            ("next_cursor", "two"),
            ("head_cursor", "three"),
            ("nextCursor", "four"),
            ("headCursor", "five"),
        ] {
            assert_eq!(
                source.unwrap(body[field].as_str().unwrap(), true).unwrap(),
                value
            );
        }
        assert_eq!(
            source
                .unwrap(body["changes"][0]["cursor"].as_str().unwrap(), true)
                .unwrap(),
            "six"
        );
        assert_eq!(body["changes"][0]["pullRequest"]["cursor"], "user-data");
        assert_eq!(body["data"]["cursor"], "also-user-data");
        let mut absent = json!({"cursor":null});
        source.wrap(&mut absent).unwrap();
        assert_eq!(absent, json!({"cursor":null}));
    }
}
