//! Injectable clock and date formatting. All date-sensitive behavior takes a `Clock`.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

use crate::error::{AgentError, Result};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Clone)]
pub struct FixedClock {
    at: DateTime<Utc>,
}

impl FixedClock {
    pub fn new(at: DateTime<Utc>) -> Self {
        Self { at }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.at
    }
}

/// Accepts an RFC 3339 instant with an explicit offset, or `YYYY-MM-DD`
/// (midnight in the student's zone). A date without a zone is rejected.
pub fn parse_clock_override(raw: &str, tz: Tz) -> Result<DateTime<Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(AgentError::Config("clock override is empty".into()));
    }
    if let Ok(at) = DateTime::parse_from_rfc3339(raw) {
        return Ok(at.with_timezone(&Utc));
    }
    let day = NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        AgentError::Config(format!(
            "invalid clock override {raw:?}: use RFC 3339 with an offset \
             (2026-11-12T09:25:00-06:00) or YYYY-MM-DD"
        ))
    })?;
    let naive = day
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| AgentError::Config("invalid clock date".into()))?;
    tz.from_local_datetime(&naive)
        .single()
        .map(|at| at.with_timezone(&Utc))
        .ok_or_else(|| {
            AgentError::Config(format!(
                "{raw} is ambiguous or does not exist in {tz}; pass an RFC 3339 instant instead"
            ))
        })
}

/// `Thu Nov 12, 2026 at 9:25 AM CST`
pub fn format_at(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz)
        .format("%a %b %-d, %Y at %-I:%M %p %Z")
        .to_string()
}

/// `Thu Nov 12, 2026, 9:25 AM CST`
pub fn format_short(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz)
        .format("%a %b %-d, %Y, %-I:%M %p %Z")
        .to_string()
}

/// `2026-11-14T23:59:00-06:00` — stable, unambiguous, used in JSON and citations.
pub fn format_rfc3339(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz).to_rfc3339()
}

/// `2026-11-14 23:59 CST`
pub fn format_compact(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz)
        .format("%Y-%m-%d %H:%M %Z")
        .to_string()
}

pub fn format_day(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz).format("%a %b %-d, %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::Chicago;

    #[test]
    fn parses_rfc3339_and_rejects_naive_timestamps() {
        let at = parse_clock_override("2026-11-12T09:25:00-06:00", Chicago).unwrap();
        assert_eq!(at.to_rfc3339(), "2026-11-12T15:25:00+00:00");
        assert!(parse_clock_override("2026-11-12T09:25:00", Chicago).is_err());
    }

    #[test]
    fn parses_a_day_in_the_students_zone() {
        let at = parse_clock_override("2026-11-12", Chicago).unwrap();
        assert_eq!(format_at(at, Chicago), "Thu Nov 12, 2026 at 12:00 AM CST");
    }

    #[test]
    fn formats_are_stable_and_explicit() {
        let at = parse_clock_override("2026-11-12T09:25:00-06:00", Chicago).unwrap();
        assert_eq!(format_short(at, Chicago), "Thu Nov 12, 2026, 9:25 AM CST");
        assert_eq!(format_rfc3339(at, Chicago), "2026-11-12T09:25:00-06:00");
    }
}
