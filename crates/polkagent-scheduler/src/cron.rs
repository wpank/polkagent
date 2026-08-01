//! Simple cron expression parser and next-occurrence calculator.
//!
//! Supports the standard five-field cron format:
//!
//! ```text
//! ┌───────────── minute (0–59)
//! │ ┌───────────── hour (0–23)
//! │ │ ┌───────────── day of month (1–31)
//! │ │ │ ┌───────────── month (1–12)
//! │ │ │ │ ┌───────────── day of week (0–6, 0 = Sunday)
//! │ │ │ │ │
//! * * * * *
//! ```
//!
//! Each field supports:
//! - `*` — any value
//! - `N` — a specific value
//! - `N-M` — an inclusive range
//! - `N,M,O` — a list of values
//! - `*/N` — step values (every Nth from the start of the range)

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use crate::error::SchedulerError;

// ---------------------------------------------------------------------------
// CronField
// ---------------------------------------------------------------------------

/// A single field in a cron expression, expanded to an explicit set of allowed
/// values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronField {
    /// Allowed values for this field, sorted.
    values: BTreeSet<u32>,
    /// The minimum allowed value for this field type.
    min: u32,
    /// The maximum allowed value for this field type.
    max: u32,
}

impl CronField {
    /// Parse a single cron field string with bounds `[min, max]`.
    fn parse(field: &str, min: u32, max: u32) -> Result<Self, SchedulerError> {
        let mut values = BTreeSet::new();

        for part in field.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(SchedulerError::InvalidCronExpression {
                    message: "empty field segment".into(),
                });
            }

            if part == "*" {
                // All values.
                for v in min..=max {
                    values.insert(v);
                }
            } else if let Some(step_str) = part.strip_prefix("*/") {
                // Step: */N
                let step: u32 = step_str.parse().map_err(|_| {
                    SchedulerError::InvalidCronExpression {
                        message: format!("invalid step value: {step_str}"),
                    }
                })?;
                if step == 0 {
                    return Err(SchedulerError::InvalidCronExpression {
                        message: "step value must not be zero".into(),
                    });
                }
                let mut v = min;
                while v <= max {
                    values.insert(v);
                    v += step;
                }
            } else if part.contains('-') {
                // Range: N-M
                let parts: Vec<&str> = part.splitn(2, '-').collect();
                let lo: u32 = parts[0].parse().map_err(|_| {
                    SchedulerError::InvalidCronExpression {
                        message: format!("invalid range start: {}", parts[0]),
                    }
                })?;
                let hi: u32 = parts[1].parse().map_err(|_| {
                    SchedulerError::InvalidCronExpression {
                        message: format!("invalid range end: {}", parts[1]),
                    }
                })?;
                if lo > hi || lo < min || hi > max {
                    return Err(SchedulerError::InvalidCronExpression {
                        message: format!("range {lo}-{hi} out of bounds [{min},{max}]"),
                    });
                }
                for v in lo..=hi {
                    values.insert(v);
                }
            } else {
                // Single value.
                let v: u32 = part.parse().map_err(|_| {
                    SchedulerError::InvalidCronExpression {
                        message: format!("invalid value: {part}"),
                    }
                })?;
                if v < min || v > max {
                    return Err(SchedulerError::InvalidCronExpression {
                        message: format!("value {v} out of bounds [{min},{max}]"),
                    });
                }
                values.insert(v);
            }
        }

        Ok(Self { values, min, max })
    }

    /// Returns `true` if the given value matches this field.
    fn matches(&self, value: u32) -> bool {
        self.values.contains(&value)
    }

    /// Returns the next value >= `from` that matches this field, wrapping
    /// around if necessary. Returns `(value, wrapped)` where `wrapped` is
    /// `true` if the value wraps past `max`.
    fn next_from(&self, from: u32) -> (u32, bool) {
        // Find the first value >= from.
        if let Some(&v) = self.values.range(from..).next() {
            (v, false)
        } else {
            // Wrap around to the first value.
            let &v = self.values.iter().next().unwrap_or(&self.min);
            (v, true)
        }
    }
}

// ---------------------------------------------------------------------------
// CronExpr
// ---------------------------------------------------------------------------

/// A parsed five-field cron expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronExpr {
    /// The raw cron expression string.
    pub expression: String,
    /// Minute field (0-59).
    pub minute: CronField,
    /// Hour field (0-23).
    pub hour: CronField,
    /// Day-of-month field (1-31).
    pub day_of_month: CronField,
    /// Month field (1-12).
    pub month: CronField,
    /// Day-of-week field (0-6, 0 = Sunday).
    pub day_of_week: CronField,
}

impl CronExpr {
    /// Parse a cron expression string.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::InvalidCronExpression`] if the expression
    /// cannot be parsed.
    pub fn parse(expr: &str) -> Result<Self, SchedulerError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(SchedulerError::InvalidCronExpression {
                message: format!("expected 5 fields, got {}", fields.len()),
            });
        }

        Ok(Self {
            expression: expr.to_owned(),
            minute: CronField::parse(fields[0], 0, 59)?,
            hour: CronField::parse(fields[1], 0, 23)?,
            day_of_month: CronField::parse(fields[2], 1, 31)?,
            month: CronField::parse(fields[3], 1, 12)?,
            day_of_week: CronField::parse(fields[4], 0, 6)?,
        })
    }

    /// Calculate the next occurrence at or after the given time.
    ///
    /// Returns `None` if no valid occurrence can be found within 4 years
    /// (a safety bound to prevent infinite loops).
    #[must_use]
    pub fn next_occurrence(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        // Start from the next minute boundary.
        let start = after + Duration::minutes(1);
        let start = Utc
            .with_ymd_and_hms(
                start.year(),
                start.month(),
                start.day(),
                start.hour(),
                start.minute(),
                0,
            )
            .single()?;

        let max_iterations = 366 * 24 * 60 * 4; // ~4 years of minutes
        let mut candidate = start;

        for _ in 0..max_iterations {
            // Check month.
            if !self.month.matches(candidate.month()) {
                let (next_month, wrapped) = self.month.next_from(candidate.month());
                let year = if wrapped {
                    candidate.year() + 1
                } else {
                    candidate.year()
                };
                candidate = Utc.with_ymd_and_hms(year, next_month, 1, 0, 0, 0).single()?;
                continue;
            }

            // Check day of month.
            if !self.day_of_month.matches(candidate.day()) {
                let (next_day, wrapped) = self.day_of_month.next_from(candidate.day());
                if wrapped {
                    // Advance to next matching month.
                    candidate += Duration::days(1);
                    candidate = Utc
                        .with_ymd_and_hms(candidate.year(), candidate.month(), candidate.day(), 0, 0, 0)
                        .single()?;
                    continue;
                }
                // Try to set the day; if invalid (e.g. Feb 30), advance month.
                if let chrono::LocalResult::Single(dt) = Utc.with_ymd_and_hms(
                    candidate.year(),
                    candidate.month(),
                    next_day,
                    0,
                    0,
                    0,
                ) {
                    candidate = dt;
                    continue;
                }
                // Invalid date (e.g. Feb 30), advance to next month.
                let next_month_start = if candidate.month() == 12 {
                    Utc.with_ymd_and_hms(candidate.year() + 1, 1, 1, 0, 0, 0)
                        .single()?
                } else {
                    Utc.with_ymd_and_hms(
                        candidate.year(),
                        candidate.month() + 1,
                        1,
                        0,
                        0,
                        0,
                    )
                    .single()?
                };
                candidate = next_month_start;
                continue;
            }

            // Check day of week (chrono: Mon=0..Sun=6; cron: Sun=0..Sat=6).
            let cron_dow = candidate.weekday().num_days_from_sunday();
            if !self.day_of_week.matches(cron_dow) {
                candidate += Duration::days(1);
                candidate = Utc
                    .with_ymd_and_hms(candidate.year(), candidate.month(), candidate.day(), 0, 0, 0)
                    .single()?;
                continue;
            }

            // Check hour.
            if !self.hour.matches(candidate.hour()) {
                let (next_hour, wrapped) = self.hour.next_from(candidate.hour());
                if wrapped {
                    candidate += Duration::days(1);
                    candidate = Utc
                        .with_ymd_and_hms(
                            candidate.year(),
                            candidate.month(),
                            candidate.day(),
                            0,
                            0,
                            0,
                        )
                        .single()?;
                    continue;
                }
                candidate = Utc
                    .with_ymd_and_hms(
                        candidate.year(),
                        candidate.month(),
                        candidate.day(),
                        next_hour,
                        0,
                        0,
                    )
                    .single()?;
                continue;
            }

            // Check minute.
            if !self.minute.matches(candidate.minute()) {
                let (next_minute, wrapped) = self.minute.next_from(candidate.minute());
                if wrapped {
                    candidate += Duration::hours(1);
                    candidate = Utc
                        .with_ymd_and_hms(
                            candidate.year(),
                            candidate.month(),
                            candidate.day(),
                            candidate.hour(),
                            0,
                            0,
                        )
                        .single()?;
                    continue;
                }
                candidate = Utc
                    .with_ymd_and_hms(
                        candidate.year(),
                        candidate.month(),
                        candidate.day(),
                        candidate.hour(),
                        next_minute,
                        0,
                    )
                    .single()?;
                continue;
            }

            // All fields match.
            return Some(candidate);
        }

        None
    }
}

impl fmt::Display for CronExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.expression)
    }
}

impl FromStr for CronExpr {
    type Err = SchedulerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_every_minute() {
        let expr = CronExpr::parse("* * * * *").expect("valid cron");
        assert_eq!(expr.minute.values.len(), 60);
        assert_eq!(expr.hour.values.len(), 24);
    }

    #[test]
    fn parse_specific_values() {
        let expr = CronExpr::parse("30 9 * * *").expect("valid cron");
        assert_eq!(expr.minute.values.len(), 1);
        assert!(expr.minute.matches(30));
        assert_eq!(expr.hour.values.len(), 1);
        assert!(expr.hour.matches(9));
    }

    #[test]
    fn parse_range() {
        let expr = CronExpr::parse("0 9-17 * * *").expect("valid cron");
        assert_eq!(expr.hour.values.len(), 9);
        assert!(expr.hour.matches(9));
        assert!(expr.hour.matches(17));
        assert!(!expr.hour.matches(18));
    }

    #[test]
    fn parse_list() {
        let expr = CronExpr::parse("0,15,30,45 * * * *").expect("valid cron");
        assert_eq!(expr.minute.values.len(), 4);
        assert!(expr.minute.matches(0));
        assert!(expr.minute.matches(15));
        assert!(expr.minute.matches(30));
        assert!(expr.minute.matches(45));
    }

    #[test]
    fn parse_step() {
        let expr = CronExpr::parse("*/15 * * * *").expect("valid cron");
        assert_eq!(expr.minute.values.len(), 4);
        assert!(expr.minute.matches(0));
        assert!(expr.minute.matches(15));
        assert!(expr.minute.matches(30));
        assert!(expr.minute.matches(45));
    }

    #[test]
    fn parse_day_of_week() {
        let expr = CronExpr::parse("0 9 * * 1-5").expect("valid cron");
        assert_eq!(expr.day_of_week.values.len(), 5);
        assert!(!expr.day_of_week.matches(0)); // Sunday
        assert!(expr.day_of_week.matches(1));  // Monday
        assert!(expr.day_of_week.matches(5));  // Friday
        assert!(!expr.day_of_week.matches(6)); // Saturday
    }

    #[test]
    fn parse_invalid_field_count() {
        let result = CronExpr::parse("* * *");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("expected 5 fields"));
    }

    #[test]
    fn parse_invalid_value() {
        let result = CronExpr::parse("60 * * * *");
        assert!(result.is_err());
    }

    #[test]
    fn parse_invalid_range() {
        let result = CronExpr::parse("* 25-30 * * *");
        assert!(result.is_err());
    }

    #[test]
    fn parse_zero_step() {
        let result = CronExpr::parse("*/0 * * * *");
        assert!(result.is_err());
    }

    #[test]
    fn next_occurrence_every_minute() {
        let expr = CronExpr::parse("* * * * *").expect("valid");
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap();
        let next = expr.next_occurrence(now).expect("should find next");
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 1, 12, 1, 0).unwrap());
    }

    #[test]
    fn next_occurrence_specific_time() {
        // "At 09:30 every day"
        let expr = CronExpr::parse("30 9 * * *").expect("valid");
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 8, 0, 0).unwrap();
        let next = expr.next_occurrence(now).expect("should find next");
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 1, 9, 30, 0).unwrap());
    }

    #[test]
    fn next_occurrence_wraps_to_next_day() {
        // "At 09:30 every day"
        let expr = CronExpr::parse("30 9 * * *").expect("valid");
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 10, 0, 0).unwrap();
        let next = expr.next_occurrence(now).expect("should find next");
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 2, 9, 30, 0).unwrap());
    }

    #[test]
    fn next_occurrence_weekday_filter() {
        // "At 09:00 on Monday (1)"
        let expr = CronExpr::parse("0 9 * * 1").expect("valid");
        // 2025-01-01 is Wednesday
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let next = expr.next_occurrence(now).expect("should find next");
        // Next Monday is Jan 6
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 6, 9, 0, 0).unwrap());
    }

    #[test]
    fn next_occurrence_monthly() {
        // "At 00:00 on the 15th of every month"
        let expr = CronExpr::parse("0 0 15 * *").expect("valid");
        let now = Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap();
        let next = expr.next_occurrence(now).expect("should find next");
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 2, 15, 0, 0, 0).unwrap());
    }

    #[test]
    fn next_occurrence_year_wrap() {
        // "At 00:00 on Jan 1"
        let expr = CronExpr::parse("0 0 1 1 *").expect("valid");
        let now = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
        let next = expr.next_occurrence(now).expect("should find next");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn cron_expr_display() {
        let expr = CronExpr::parse("*/5 * * * *").expect("valid");
        assert_eq!(expr.to_string(), "*/5 * * * *");
    }

    #[test]
    fn cron_expr_from_str() {
        let expr: CronExpr = "0 12 * * *".parse().expect("valid");
        assert!(expr.hour.matches(12));
        assert!(expr.minute.matches(0));
    }

    #[test]
    fn cron_expr_serde_round_trip() {
        let expr = CronExpr::parse("30 9 * * 1-5").expect("valid");
        let json = serde_json::to_string(&expr).expect("serialize");
        let back: CronExpr = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(expr, back);
    }

    #[test]
    fn cron_field_next_from_wraps() {
        let field = CronField::parse("0,30", 0, 59).expect("valid");
        let (val, wrapped) = field.next_from(45);
        assert_eq!(val, 0);
        assert!(wrapped);
    }

    #[test]
    fn cron_field_next_from_no_wrap() {
        let field = CronField::parse("0,30", 0, 59).expect("valid");
        let (val, wrapped) = field.next_from(15);
        assert_eq!(val, 30);
        assert!(!wrapped);
    }
}
