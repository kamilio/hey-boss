//! Shared, persisted daily notification schedule. Delivery uses the named zone.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuietHours {
    pub enabled: bool,
    pub start: String,
    pub end: String,
    pub time_zone: String,
}
impl Default for QuietHours {
    fn default() -> Self {
        static ZONE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let zone = ZONE
            .get_or_init(|| {
                std::fs::read_link("/etc/localtime")
                    .ok()
                    .and_then(|path| {
                        path.to_str()
                            .and_then(|s| s.split_once("zoneinfo/").map(|(_, z)| z.to_owned()))
                    })
                    .unwrap_or_else(|| "UTC".into())
            })
            .clone();
        Self {
            enabled: true,
            start: "22:00".into(),
            end: "07:00".into(),
            time_zone: zone,
        }
    }
}
impl QuietHours {
    pub fn validate(&self) -> crate::issues::Result<()> {
        let time = |s: &str| {
            s.len() == 5
                && s.as_bytes()[2] == b':'
                && s.bytes()
                    .enumerate()
                    .all(|(i, b)| i == 2 || b.is_ascii_digit())
                && s[..2].parse::<u8>().is_ok_and(|n| n < 24)
                && s[3..].parse::<u8>().is_ok_and(|n| n < 60)
        };
        if !time(&self.start) || !time(&self.end) || self.start == self.end {
            return Err(crate::issues::Error::invalid(
                "Choose different start and end times in HH:MM format",
            ));
        }
        if self.time_zone.is_empty()
            || self.time_zone.len() > 128
            || self.time_zone.starts_with('/')
            || self
                .time_zone
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == "..")
            || !self
                .time_zone
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
            || !std::fs::read(std::path::Path::new("/usr/share/zoneinfo").join(&self.time_zone))
                .is_ok_and(|data| data.starts_with(b"TZif"))
        {
            return Err(crate::issues::Error::invalid(
                "Choose a valid IANA time zone, such as America/Chicago",
            ));
        }
        Ok(())
    }
}
