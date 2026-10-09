//! Five-field cron in an explicit IANA zone. DOM/DOW follow Vixie cron semantics.
use crate::issues::{Error, Result};
use jiff::{
    Timestamp,
    civil::Date,
    tz::{AmbiguousOffset, TimeZone},
};

pub const MAX_PREVIEW: usize = 1000;
// A leap-day schedule can go eight years without a match across a century.
const SEARCH_DAYS: i64 = 366 * 9;
pub const MAX_RANGE_MS: i64 = SEARCH_DAYS * 86_400_000;

struct Field {
    values: Vec<i8>,
    star: bool,
}
impl Field {
    fn parse(text: &str, min: i8, max: i8, names: &[&str]) -> Result<Self> {
        let number = |s: &str| -> Result<i8> {
            let n = if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(s)) {
                i as i8 + min
            } else {
                if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
                    return Err(Error::invalid(
                        "Cron fields require numbers or supported month/day names",
                    ));
                }
                s.parse()
                    .map_err(|_| Error::invalid("Cron value out of range"))?
            };
            if !(min..=max).contains(&n) {
                return Err(Error::invalid("Cron value out of range"));
            }
            Ok(n)
        };
        let mut values = Vec::new();
        for part in text.split(',') {
            let (base, step) = match part.split_once('/') {
                Some((base, s)) => {
                    let step = s
                        .parse::<u8>()
                        .map_err(|_| Error::invalid("Invalid cron step"))?;
                    if step == 0
                        || step > (max - min + 1) as u8
                        || !s.bytes().all(|c| c.is_ascii_digit())
                    {
                        return Err(Error::invalid("Cron step out of range"));
                    }
                    (base, step)
                }
                None => (part, 1),
            };
            let (start, end) = if base == "*" {
                (min, max)
            } else if let Some((a, b)) = base.split_once('-') {
                (number(a)?, number(b)?)
            } else {
                let n = number(base)?;
                (n, if part.contains('/') { max } else { n })
            };
            if start > end {
                return Err(Error::invalid("Descending cron ranges are unsupported"));
            }
            values.extend((start..=end).step_by(step as usize));
        }
        values.sort_unstable();
        values.dedup();
        Ok(Self {
            values,
            star: text.starts_with('*'),
        })
    }
    fn has(&self, n: i8) -> bool {
        self.values.binary_search(&n).is_ok()
    }
}

pub struct Schedule {
    minute: Field,
    hour: Field,
    day: Field,
    month: Field,
    weekday: Field,
    zone: TimeZone,
}
impl Schedule {
    pub fn parse(cron: &str, zone: &str) -> Result<Self> {
        if cron.len() > 256 || zone.len() > 128 {
            return Err(Error::invalid("Cron expression or timezone too long"));
        }
        let parts: Vec<_> = cron.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(Error::invalid(
                "Cron requires five fields: minute hour day month weekday",
            ));
        }
        // Database lookup accepts IANA names only; never infer the machine's zone.
        let zone = jiff::tz::db()
            .get(zone)
            .map_err(|e| Error::invalid(format!("Invalid IANA timezone: {e}")))?;
        let mut weekday = Field::parse(
            parts[4],
            0,
            7,
            &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"],
        )?;
        for v in &mut weekday.values {
            if *v == 7 {
                *v = 0;
            }
        }
        weekday.values.sort_unstable();
        weekday.values.dedup();
        let s = Self {
            minute: Field::parse(parts[0], 0, 59, &[])?,
            hour: Field::parse(parts[1], 0, 23, &[])?,
            day: Field::parse(parts[2], 1, 31, &[])?,
            month: Field::parse(
                parts[3],
                1,
                12,
                &[
                    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV",
                    "DEC",
                ],
            )?,
            weekday,
            zone,
        };
        // All weekday/month/day combinations repeat within the Gregorian cycle.
        let mut date = Date::new(2000, 1, 1).unwrap();
        for _ in 0..146097 {
            if s.matches_date(date) {
                return Ok(s);
            }
            date = date.tomorrow().unwrap();
        }
        Err(Error::invalid(
            "Cron schedule never matches a calendar date",
        ))
    }
    fn matches_date(&self, date: Date) -> bool {
        let day = self.day.has(date.day());
        let weekday = self.weekday.has(date.weekday().to_sunday_zero_offset());
        self.month.has(date.month())
            && if self.day.star || self.weekday.star {
                day && weekday
            } else {
                day || weekday
            }
    }
    fn instant(&self, date: Date, hour: i8, minute: i8) -> Result<Option<i64>> {
        let ambiguous = self
            .zone
            .to_ambiguous_timestamp(date.at(hour, minute, 0, 0));
        if matches!(ambiguous.offset(), AmbiguousOffset::Gap { .. }) {
            return Ok(None);
        }
        Ok(Some(
            ambiguous
                .earlier()
                .map_err(|e| Error::invalid(e.to_string()))?
                .as_millisecond(),
        ))
    }
    fn date(&self, ms: i64) -> Result<Date> {
        let ts = Timestamp::from_millisecond(ms).map_err(|e| Error::invalid(e.to_string()))?;
        Ok(self.zone.to_datetime(ts).date())
    }
    /// UTC milliseconds in (after, through], at most limit, ordered ascending.
    pub fn preview(&self, after: i64, through: i64, limit: usize) -> Result<Vec<i64>> {
        if !(1..=MAX_PREVIEW).contains(&limit)
            || through
                .checked_sub(after)
                .is_none_or(|n| !(0..=MAX_RANGE_MS).contains(&n))
        {
            return Err(Error::invalid(
                "Preview requires 1–1000 results and a forward range of at most nine years",
            ));
        }
        let mut date = self.date(after)?;
        let end = self.date(through)?;
        let mut result = Vec::new();
        for _ in 0..=SEARCH_DAYS + 2 {
            if date > end {
                break;
            }
            if self.matches_date(date) {
                for &hour in &self.hour.values {
                    for &minute in &self.minute.values {
                        if let Some(at) = self.instant(date, hour, minute)?
                            && at > after
                            && at <= through
                        {
                            result.push(at);
                            if result.len() == limit {
                                return Ok(result);
                            }
                        }
                    }
                }
            }
            date = date.tomorrow().map_err(|e| Error::invalid(e.to_string()))?;
        }
        Ok(result)
    }
    pub fn next(&self, after: i64) -> Result<i64> {
        let through = after
            .checked_add(MAX_RANGE_MS)
            .ok_or_else(|| Error::invalid("Timestamp out of range"))?;
        self.preview(after, through, 1)?
            .first()
            .copied()
            .ok_or_else(|| Error::invalid("No occurrence within the supported nine-year horizon"))
    }
    /// Reverse calendar search coalesces downtime without enumerating missed minutes.
    pub fn latest(&self, after: i64, through: i64) -> Result<Option<i64>> {
        let mut date = self.date(through)?;
        self.date(after)?;
        if through <= after {
            return Ok(None);
        }
        for _ in 0..=SEARCH_DAYS + 2 {
            if self.matches_date(date) {
                for &hour in self.hour.values.iter().rev() {
                    for &minute in self.minute.values.iter().rev() {
                        if let Some(at) = self.instant(date, hour, minute)?
                            && at <= through
                        {
                            return Ok((at > after).then_some(at));
                        }
                    }
                }
            }
            date = date
                .yesterday()
                .map_err(|e| Error::invalid(e.to_string()))?;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(s: &str) -> i64 {
        s.parse::<jiff::Timestamp>().unwrap().as_millisecond()
    }
    #[test]
    fn daily_and_hourly_use_explicit_zone() {
        let s = Schedule::parse("0 8 * * *", "America/Chicago").unwrap();
        assert_eq!(
            s.next(at("2026-10-09T12:59:00Z")).unwrap(),
            at("2026-10-09T13:00:00Z")
        );
        assert_eq!(
            s.next(at("2026-10-09T13:00:00Z")).unwrap(),
            at("2026-10-10T13:00:00Z")
        );
        let h = Schedule::parse("0 * * * *", "UTC").unwrap();
        assert_eq!(
            h.preview(at("2026-10-09T13:00:00Z"), at("2026-10-09T16:00:00Z"), 2)
                .unwrap(),
            vec![at("2026-10-09T14:00:00Z"), at("2026-10-09T15:00:00Z")]
        );
    }
    #[test]
    fn gaps_are_skipped_and_folds_execute_only_the_first_instant() {
        let gap = Schedule::parse("30 2 * * *", "America/New_York").unwrap();
        assert_eq!(
            gap.next(at("2026-03-08T00:00:00Z")).unwrap(),
            at("2026-03-09T06:30:00Z")
        );
        let fold = Schedule::parse("30 1 * * *", "America/New_York").unwrap();
        assert_eq!(
            fold.next(at("2026-11-01T00:00:00Z")).unwrap(),
            at("2026-11-01T05:30:00Z")
        );
        assert_eq!(
            fold.next(at("2026-11-01T05:30:00Z")).unwrap(),
            at("2026-11-02T06:30:00Z")
        );
        assert_eq!(
            fold.latest(at("2026-11-01T05:30:00Z"), at("2026-11-01T07:00:00Z"))
                .unwrap(),
            None
        );
        let half = Schedule::parse("45 1 * * *", "Australia/Lord_Howe").unwrap();
        assert_eq!(
            half.preview(at("2026-04-04T13:00:00Z"), at("2026-04-04T17:00:00Z"), 10)
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn lists_ranges_steps_and_standard_day_semantics() {
        let s = Schedule::parse("5,20-40/10 8-9 * JAN,MAR MON-FRI", "UTC").unwrap();
        assert_eq!(
            s.preview(at("2026-01-02T08:00:00Z"), at("2026-01-02T08:59:00Z"), 10)
                .unwrap(),
            [5, 20, 30, 40].map(|m| at(&format!("2026-01-02T08:{m:02}:00Z")))
        );
        // Both restricted: either day field can match, including an impossible DOM.
        let or = Schedule::parse("0 8 31 FEB MON", "UTC").unwrap();
        assert_eq!(
            or.next(at("2026-02-01T00:00:00Z")).unwrap(),
            at("2026-02-02T08:00:00Z")
        );
        let sunday = Schedule::parse("0 8 * * 7", "UTC").unwrap();
        assert_eq!(
            sunday.next(at("2026-10-09T00:00:00Z")).unwrap(),
            at("2026-10-11T08:00:00Z")
        );
        let star = Schedule::parse("0 8 */2 * MON", "UTC").unwrap();
        assert_eq!(
            star.next(at("2026-10-12T00:00:00Z")).unwrap(),
            at("2026-10-19T08:00:00Z")
        );
    }
    #[test]
    fn invalid_and_impossible_inputs_are_bounded() {
        for c in [
            "",
            "* * * * * *",
            "60 * * * *",
            "*/0 * * * *",
            "0 0 31 2 *",
            "0 0 30 2 *",
            "0 0 * * L",
            "1-0 * * * *",
            "1,,2 * * * *",
            "999999999999 * * * *",
            "0 0 * * ?",
        ] {
            assert!(Schedule::parse(c, "UTC").is_err(), "{c}");
        }
        assert!(Schedule::parse("0 8 * * *", "+02:00").is_err());
        assert!(Schedule::parse("0 8 * * *", "Imaginary/Zone").is_err());
        let leap = Schedule::parse("0 0 29 2 *", "UTC").unwrap();
        assert_eq!(
            leap.next(at("2096-02-29T00:00:00Z")).unwrap(),
            at("2104-02-29T00:00:00Z")
        );
        assert!(leap.preview(0, 1, 0).is_err());
        assert!(leap.preview(0, 1, 1001).is_err());
        assert!(leap.preview(1, 0, 1).is_err());
        assert!(leap.preview(i64::MIN, i64::MAX, 1).is_err());
    }
    #[test]
    fn latest_coalesces_decades_without_enumerating_occurrences() {
        let s = Schedule::parse("* * * * *", "UTC").unwrap();
        assert_eq!(
            s.latest(at("2000-01-01T00:00:00Z"), at("2026-10-09T13:25:39Z"))
                .unwrap(),
            Some(at("2026-10-09T13:25:00Z"))
        );
    }
}
