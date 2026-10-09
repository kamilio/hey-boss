//! Display ranges are civil dates in an explicit zone, never schedule mutations.
use crate::issues::{Error, Result};
use jiff::{civil::Date, tz::TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub at: i64,
    pub key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub start: String,
    pub days: usize,
    pub timezone: String,
    pub job_id: Option<String>,
    pub cursor: Option<Cursor>,
}
pub struct Day {
    pub date: String,
    pub start: i64,
    pub end: i64,
}
impl Query {
    pub fn validate(&self, entries: bool) -> Result<()> {
        if self.start.len() != 10 || self.timezone.len() > 128 {
            return Err(Error::invalid("Use a YYYY-MM-DD date and IANA timezone"));
        }
        if !(1..=42).contains(&self.days)
            || (entries && self.days != 1)
            || (!entries && self.cursor.is_some())
        {
            return Err(Error::invalid(
                "Calendar requires 1–42 days; entry pages require one day",
            ));
        }
        if let Some(id) = &self.job_id {
            super::validate_id(id)?;
        }
        if let Some(cursor) = &self.cursor
            && (cursor.key.len() > 160 || cursor.key.is_empty())
        {
            return Err(Error::invalid("Invalid calendar cursor"));
        }
        self.bounds()?;
        Ok(())
    }
    pub fn bounds(&self) -> Result<Vec<Day>> {
        let zone = jiff::tz::db()
            .get(&self.timezone)
            .map_err(|e| Error::invalid(e.to_string()))?;
        let mut date: Date = self
            .start
            .parse()
            .map_err(|_| Error::invalid("Calendar start must be YYYY-MM-DD"))?;
        let midnight = |date: Date, zone: &TimeZone| -> Result<i64> {
            // Compatible disambiguation handles a midnight gap or fold, too.
            Ok(zone
                .to_ambiguous_timestamp(date.at(0, 0, 0, 0))
                .compatible()
                .map_err(|e| Error::invalid(e.to_string()))?
                .as_millisecond())
        };
        let mut days = Vec::new();
        for _ in 0..self.days {
            let next = date.tomorrow().map_err(|e| Error::invalid(e.to_string()))?;
            days.push(Day {
                date: date.to_string(),
                start: midnight(date, &zone)?,
                end: midnight(next, &zone)?,
            });
            date = next;
        }
        Ok(days)
    }
}
