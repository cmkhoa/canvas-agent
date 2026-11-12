//! Deterministic priority policy. Every threshold lives in `config/policy.toml`.
//!
//! The score is explainable and additive:
//!   deadline proximity (0–120) + point value (0–30) + submission status (0–40)
//! It is a planning aid, not a prediction of a course grade.

use std::path::Path;

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::clock::{format_day, format_short};
use crate::error::{AgentError, Result};
use crate::model::{Assignment, WorkState};

#[derive(Debug, Clone, Deserialize)]
pub struct PolicyFile {
    pub version: String,
    pub default_horizon_days: u32,
    pub max_horizon_days: u32,
    pub risk_soon_hours: i64,
    pub score: ScoreConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScoreConfig {
    pub overdue_base: f64,
    pub overdue_per_day: f64,
    pub overdue_max_days: i64,
    pub due_within_24h: f64,
    pub due_within_72h: f64,
    pub due_within_7d: f64,
    pub due_within_14d: f64,
    pub due_later: f64,
    pub points_max: f64,
    pub points_cap: f64,
    pub missing_status: f64,
    pub not_submitted_status: f64,
    pub unknown_status: f64,
}

#[derive(Debug, Clone)]
pub struct Policy {
    file: PolicyFile,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoreBreakdown {
    pub deadline: f64,
    pub value: f64,
    pub status: f64,
    pub total: f64,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    Critical,
    High,
    Medium,
    Unknown,
    Outside,
    Done,
}

impl RiskTier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Unknown => "unknown",
            Self::Outside => "outside horizon",
            Self::Done => "done",
        }
    }

    pub fn is_reportable(self) -> bool {
        matches!(
            self,
            Self::Critical | Self::High | Self::Medium | Self::Unknown
        )
    }
}

impl Policy {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            AgentError::Config(format!("cannot read policy file {}: {e}", path.display()))
        })?;
        let file: PolicyFile = toml::from_str(&raw).map_err(|e| {
            AgentError::Config(format!("invalid policy file {}: {e}", path.display()))
        })?;
        let policy = Self { file };
        policy.validate()?;
        Ok(policy)
    }

    pub fn version(&self) -> &str {
        &self.file.version
    }

    pub fn default_horizon_days(&self) -> u32 {
        self.file.default_horizon_days
    }

    pub fn max_horizon_days(&self) -> u32 {
        self.file.max_horizon_days
    }

    pub fn risk_soon_hours(&self) -> i64 {
        self.file.risk_soon_hours
    }

    pub fn points_cap(&self) -> f64 {
        self.file.score.points_cap
    }

    fn validate(&self) -> Result<()> {
        let f = &self.file;
        if f.version.trim().is_empty() {
            return Err(AgentError::Config(
                "policy version must not be empty".into(),
            ));
        }
        if !(1..=365).contains(&f.max_horizon_days) {
            return Err(AgentError::Config(
                "policy max_horizon_days must be 1..=365".into(),
            ));
        }
        if !(1..=f.max_horizon_days).contains(&f.default_horizon_days) {
            return Err(AgentError::Config(
                "policy default_horizon_days must be 1..=max_horizon_days".into(),
            ));
        }
        if !(1..=720).contains(&f.risk_soon_hours) {
            return Err(AgentError::Config(
                "policy risk_soon_hours must be 1..=720".into(),
            ));
        }
        let s = &f.score;
        let values = [
            ("overdue_base", s.overdue_base),
            ("overdue_per_day", s.overdue_per_day),
            ("due_within_24h", s.due_within_24h),
            ("due_within_72h", s.due_within_72h),
            ("due_within_7d", s.due_within_7d),
            ("due_within_14d", s.due_within_14d),
            ("due_later", s.due_later),
            ("points_max", s.points_max),
            ("points_cap", s.points_cap),
            ("missing_status", s.missing_status),
            ("not_submitted_status", s.not_submitted_status),
            ("unknown_status", s.unknown_status),
        ];
        for (name, value) in values {
            if !value.is_finite() || !(0.0..=500.0).contains(&value) {
                return Err(AgentError::Config(format!(
                    "policy score.{name} must be a finite number in 0..=500"
                )));
            }
        }
        if s.points_cap <= 0.0 {
            return Err(AgentError::Config(
                "policy score.points_cap must be greater than zero".into(),
            ));
        }
        if !(0..=60).contains(&s.overdue_max_days) {
            return Err(AgentError::Config(
                "policy score.overdue_max_days must be 0..=60".into(),
            ));
        }
        if !(s.due_within_24h >= s.due_within_72h
            && s.due_within_72h >= s.due_within_7d
            && s.due_within_7d >= s.due_within_14d
            && s.due_within_14d >= s.due_later)
        {
            return Err(AgentError::Config(
                "policy deadline scores must be non-increasing as the deadline gets farther away"
                    .into(),
            ));
        }
        Ok(())
    }

    pub fn score(&self, assignment: &Assignment, now: DateTime<Utc>) -> ScoreBreakdown {
        let mut reasons = Vec::new();
        let deadline = match assignment.due_at {
            None => {
                reasons.push("no due date; urgency not scored".to_string());
                0.0
            }
            Some(due) if due < now => {
                let hours = (now - due).num_hours().max(1);
                let days = ((hours as f64) / 24.0).ceil() as i64;
                let reason = if hours < 24 {
                    format!("overdue by {hours} hours")
                } else {
                    format!("overdue by {days} day{}", if days == 1 { "" } else { "s" })
                };
                reasons.push(reason);
                self.file.score.overdue_base
                    + self.file.score.overdue_per_day
                        * (days.min(self.file.score.overdue_max_days) as f64)
            }
            Some(due) => {
                let hours = (due - now).num_hours();
                let (value, why) = if hours < 24 {
                    (self.file.score.due_within_24h, "due within 24 hours")
                } else if hours < 72 {
                    (self.file.score.due_within_72h, "due within 3 days")
                } else if hours < 168 {
                    (self.file.score.due_within_7d, "due within 7 days")
                } else if hours < 336 {
                    (self.file.score.due_within_14d, "due within 14 days")
                } else {
                    (self.file.score.due_later, "due later than 14 days")
                };
                reasons.push(why.to_string());
                value
            }
        };

        let value = match assignment.points_possible {
            Some(points) if points > 0.0 => {
                let capped = points.min(self.file.score.points_cap);
                reasons.push(format!("{} points", trim_number(points)));
                capped / self.file.score.points_cap * self.file.score.points_max
            }
            Some(_) => {
                reasons.push("0 points".to_string());
                0.0
            }
            None => {
                reasons.push("point value not returned by Canvas; value not scored".to_string());
                0.0
            }
        };

        let status = match assignment.state {
            WorkState::Missing => {
                reasons.push("Canvas flags this missing".to_string());
                self.file.score.missing_status
            }
            WorkState::NotSubmitted => {
                reasons.push("not submitted".to_string());
                self.file.score.not_submitted_status
            }
            WorkState::Unknown => {
                reasons.push("submission status unknown".to_string());
                self.file.score.unknown_status
            }
            _ => 0.0,
        };

        let total = round2(deadline + value + status);
        ScoreBreakdown {
            deadline: round2(deadline),
            value: round2(value),
            status: round2(status),
            total,
            reasons,
        }
    }

    pub fn risk_tier(
        &self,
        assignment: &Assignment,
        now: DateTime<Utc>,
        horizon_end: DateTime<Utc>,
    ) -> RiskTier {
        if assignment.state.is_done() {
            return RiskTier::Done;
        }
        if assignment.state.needs_verification() || assignment.due_at.is_none() {
            return RiskTier::Unknown;
        }
        let due = assignment.due_at.expect("checked");
        if assignment.state == WorkState::Missing || due < now {
            return RiskTier::Critical;
        }
        if due <= now + Duration::hours(self.file.risk_soon_hours) {
            return RiskTier::High;
        }
        if due <= horizon_end {
            return RiskTier::Medium;
        }
        RiskTier::Outside
    }

    /// The policy sentence shown to the model, without exposing mutable config.
    pub fn describe(&self) -> String {
        format!(
            "policy {}: urgency (deadline distance, overdue first), point value (capped at {}), \
             and Canvas submission status. Missing/past-due work is critical; unsubmitted work due \
             within {} hours is high. This is deadline risk, not a grade prediction.",
            self.file.version, self.file.score.points_cap, self.file.risk_soon_hours
        )
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HorizonArgs {
    #[serde(default)]
    pub days: Option<u32>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub weekday: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Horizon {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub days: u32,
    pub label: String,
}

impl Horizon {
    pub fn contains(&self, due: DateTime<Utc>) -> bool {
        due >= self.start && due <= self.end
    }
}

pub fn resolve_horizon(
    args: &HorizonArgs,
    now: DateTime<Utc>,
    tz: Tz,
    policy: &Policy,
) -> std::result::Result<Horizon, String> {
    let specified = [
        args.days.is_some(),
        args.until.as_ref().is_some_and(|s| !s.trim().is_empty()),
        args.weekday.as_ref().is_some_and(|s| !s.trim().is_empty()),
    ]
    .into_iter()
    .filter(|x| *x)
    .count();
    if specified > 1 {
        return Err("choose only one of days, until, or weekday".into());
    }

    let (end, label) = if let Some(days) = args.days {
        if !(1..=policy.max_horizon_days()).contains(&days) {
            return Err(format!(
                "days must be between 1 and {}",
                policy.max_horizon_days()
            ));
        }
        let end = now + Duration::days(i64::from(days));
        (
            end,
            format!("the next {days} days (through {})", format_short(end, tz)),
        )
    } else if let Some(raw) = args.until.as_ref().filter(|s| !s.trim().is_empty()) {
        let day = NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
            .map_err(|_| format!("unreadable date {raw:?}; use YYYY-MM-DD"))?;
        let end = end_of_day(day, tz)?;
        if end < now {
            return Err(format!("{raw} is in the past"));
        }
        if end > now + Duration::days(i64::from(policy.max_horizon_days())) {
            return Err(format!(
                "{raw} is beyond the maximum planning horizon ({} days)",
                policy.max_horizon_days()
            ));
        }
        (end, format!("through {}", format_short(end, tz)))
    } else if let Some(raw) = args.weekday.as_ref().filter(|s| !s.trim().is_empty()) {
        let weekday = parse_weekday(raw)?;
        let today = now.with_timezone(&tz).date_naive();
        let delta = (weekday.num_days_from_monday() as i64
            - today.weekday().num_days_from_monday() as i64)
            .rem_euclid(7);
        let day = today + Duration::days(delta);
        let end = end_of_day(day, tz)?;
        if end > now + Duration::days(i64::from(policy.max_horizon_days())) {
            return Err(format!(
                "{} is beyond the maximum planning horizon ({} days)",
                raw,
                policy.max_horizon_days()
            ));
        }
        (
            end,
            format!("through {} ({})", format_day(end, tz), weekday),
        )
    } else {
        let days = policy.default_horizon_days();
        let end = now + Duration::days(i64::from(days));
        (
            end,
            format!("the next {days} days (through {})", format_short(end, tz)),
        )
    };

    if end <= now {
        return Err("planning horizon ends before now".into());
    }
    let days = (((end - now).num_seconds() as f64) / 86_400.0)
        .ceil()
        .max(1.0) as u32;
    Ok(Horizon {
        start: now,
        end,
        days,
        label,
    })
}

fn end_of_day(day: NaiveDate, tz: Tz) -> std::result::Result<DateTime<Utc>, String> {
    let naive = day
        .and_hms_opt(23, 59, 59)
        .ok_or_else(|| "invalid end-of-day time".to_string())?;
    tz.from_local_datetime(&naive)
        .single()
        .map(|at| at.with_timezone(&Utc))
        .ok_or_else(|| format!("{day} at 23:59:59 does not exist in {tz}"))
}

fn parse_weekday(raw: &str) -> std::result::Result<Weekday, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "mon" | "monday" => Ok(Weekday::Mon),
        "tue" | "tues" | "tuesday" => Ok(Weekday::Tue),
        "wed" | "wednesday" => Ok(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Ok(Weekday::Thu),
        "fri" | "friday" => Ok(Weekday::Fri),
        "sat" | "saturday" => Ok(Weekday::Sat),
        "sun" | "sunday" => Ok(Weekday::Sun),
        _ => Err(format!("unreadable weekday {raw:?}; use monday..sunday")),
    }
}

fn trim_number(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::Chicago;

    fn policy() -> Policy {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/policy.toml");
        Policy::load(&path).unwrap()
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn assignment(state: WorkState, due: Option<DateTime<Utc>>, points: Option<f64>) -> Assignment {
        Assignment {
            course_label: "CS 101".into(),
            course_id: Some(101),
            assignment_id: 1,
            title: "Lab".into(),
            due_at: due,
            points_possible: points,
            state,
            kind: "assignment".into(),
            suspicious: false,
        }
    }

    #[test]
    fn due_proximity_and_points_change_the_score() {
        let p = policy();
        let now = at("2026-11-12T15:25:00Z");
        let soon = p.score(
            &assignment(
                WorkState::NotSubmitted,
                Some(now + Duration::hours(12)),
                Some(100.0),
            ),
            now,
        );
        let later = p.score(
            &assignment(
                WorkState::NotSubmitted,
                Some(now + Duration::days(20)),
                Some(100.0),
            ),
            now,
        );
        assert!(soon.total > later.total);
        assert_eq!(soon.value, 30.0);
        assert!(soon.reasons.iter().any(|r| r.contains("within 24")));
    }

    #[test]
    fn overdue_and_missing_outrank_everything_else() {
        let p = policy();
        let now = at("2026-11-12T15:25:00Z");
        let overdue = p.score(
            &assignment(
                WorkState::Missing,
                Some(now - Duration::days(3)),
                Some(10.0),
            ),
            now,
        );
        let soon = p.score(
            &assignment(
                WorkState::NotSubmitted,
                Some(now + Duration::hours(12)),
                Some(100.0),
            ),
            now,
        );
        assert!(overdue.total > soon.total, "{overdue:?} {soon:?}");
        assert_eq!(overdue.status, 40.0);
    }

    #[test]
    fn missing_points_are_not_guessed() {
        let p = policy();
        let now = at("2026-11-12T15:25:00Z");
        let score = p.score(
            &assignment(WorkState::NotSubmitted, Some(now + Duration::days(2)), None),
            now,
        );
        assert_eq!(score.value, 0.0);
        assert!(score.reasons.iter().any(|r| r.contains("not returned")));
    }

    #[test]
    fn undated_work_is_unknown_not_scored_as_urgent() {
        let p = policy();
        let now = at("2026-11-12T15:25:00Z");
        let a = assignment(WorkState::NotSubmitted, None, Some(50.0));
        let score = p.score(&a, now);
        assert_eq!(score.deadline, 0.0);
        assert_eq!(
            p.risk_tier(&a, now, now + Duration::days(7)),
            RiskTier::Unknown
        );
    }

    #[test]
    fn risk_tiers_follow_status_and_time() {
        let p = policy();
        let now = at("2026-11-12T15:25:00Z");
        let end = now + Duration::days(7);
        assert_eq!(
            p.risk_tier(
                &assignment(WorkState::Missing, Some(now + Duration::days(5)), Some(1.0)),
                now,
                end
            ),
            RiskTier::Critical
        );
        assert_eq!(
            p.risk_tier(
                &assignment(
                    WorkState::NotSubmitted,
                    Some(now + Duration::hours(20)),
                    Some(1.0)
                ),
                now,
                end
            ),
            RiskTier::High
        );
        assert_eq!(
            p.risk_tier(
                &assignment(
                    WorkState::NotSubmitted,
                    Some(now + Duration::days(4)),
                    Some(1.0)
                ),
                now,
                end
            ),
            RiskTier::Medium
        );
        assert_eq!(
            p.risk_tier(
                &assignment(
                    WorkState::Submitted,
                    Some(now + Duration::days(1)),
                    Some(1.0)
                ),
                now,
                end
            ),
            RiskTier::Done
        );
    }

    #[test]
    fn horizon_accepts_days_weekday_and_date() {
        let p = policy();
        let now = at("2026-11-12T15:25:00Z"); // Thursday
        let h = resolve_horizon(
            &HorizonArgs {
                weekday: Some("friday".into()),
                ..Default::default()
            },
            now,
            Chicago,
            &p,
        )
        .unwrap();
        assert!(h.label.contains("Fri Nov 13"));
        let h = resolve_horizon(
            &HorizonArgs {
                until: Some("2026-11-13".into()),
                ..Default::default()
            },
            now,
            Chicago,
            &p,
        )
        .unwrap();
        assert_eq!(h.days, 2);
        assert!(h.end > h.start);
    }

    #[test]
    fn horizon_rejects_ambiguous_and_out_of_bounds_requests() {
        let p = policy();
        let now = at("2026-11-12T15:25:00Z");
        assert!(
            resolve_horizon(
                &HorizonArgs {
                    days: Some(7),
                    weekday: Some("friday".into()),
                    ..Default::default()
                },
                now,
                Chicago,
                &p
            )
            .is_err()
        );
        assert!(
            resolve_horizon(
                &HorizonArgs {
                    days: Some(999),
                    ..Default::default()
                },
                now,
                Chicago,
                &p
            )
            .is_err()
        );
        assert!(
            resolve_horizon(
                &HorizonArgs {
                    until: Some("2026-01-01".into()),
                    ..Default::default()
                },
                now,
                Chicago,
                &p
            )
            .is_err()
        );
    }
}
