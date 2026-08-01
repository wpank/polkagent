//! Schedule types for defining when tasks should run.
//!
//! Three schedule variants are supported:
//!
//! - [`Schedule::Once`] — a single execution at a specific time.
//! - [`Schedule::Interval`] — repeating at a fixed duration.
//! - [`Schedule::Cron`] — cron-expression-based scheduling.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::cron::CronExpr;
use crate::error::SchedulerError;

// ---------------------------------------------------------------------------
// Schedule
// ---------------------------------------------------------------------------

/// Defines when and how often a scheduled task should run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Schedule {
    /// Run exactly once at the specified time.
    Once {
        /// The time at which the task should execute.
        at: DateTime<Utc>,
    },

    /// Run repeatedly at a fixed interval.
    Interval {
        /// The duration between consecutive runs.
        #[serde(with = "duration_secs")]
        every: Duration,
        /// When the first run should occur.
        start: DateTime<Utc>,
    },

    /// Run according to a cron expression.
    Cron(CronExpr),
}

impl Schedule {
    /// Calculate the next occurrence at or after `after`.
    ///
    /// Returns `None` if the schedule has no future occurrences (e.g. a
    /// `Once` schedule in the past).
    #[must_use]
    pub fn next_occurrence(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Self::Once { at } => {
                if *at > after {
                    Some(*at)
                } else {
                    None
                }
            }
            Self::Interval { every, start } => {
                if *start > after {
                    return Some(*start);
                }
                // Calculate how many intervals have passed since start.
                let elapsed = after - *start;
                let interval_millis = every.num_milliseconds();
                if interval_millis <= 0 {
                    return None;
                }
                let intervals_passed = elapsed.num_milliseconds() / interval_millis;
                let next = *start + Duration::milliseconds(interval_millis * (intervals_passed + 1));
                Some(next)
            }
            Self::Cron(expr) => expr.next_occurrence(after),
        }
    }

    /// Calculate the next occurrence, returning an error if the schedule is
    /// exhausted.
    pub fn next_occurrence_or_err(
        &self,
        after: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, SchedulerError> {
        self.next_occurrence(after)
            .ok_or(SchedulerError::ScheduleExhausted)
    }
}

impl std::fmt::Display for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Once { at } => write!(f, "once at {at}"),
            Self::Interval { every, start } => {
                write!(f, "every {}s starting {start}", every.num_seconds())
            }
            Self::Cron(expr) => write!(f, "cron: {expr}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Serde helper for Duration as seconds
// ---------------------------------------------------------------------------

mod duration_secs {
    use chrono::Duration;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(duration.num_seconds())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = i64::deserialize(deserializer)?;
        Ok(Duration::seconds(secs))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn once_future() {
        let future = Utc.with_ymd_and_hms(2030, 6, 15, 12, 0, 0).unwrap();
        let schedule = Schedule::Once { at: future };
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(schedule.next_occurrence(now), Some(future));
    }

    #[test]
    fn once_past_returns_none() {
        let past = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let schedule = Schedule::Once { at: past };
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(schedule.next_occurrence(now), None);
    }

    #[test]
    fn once_exhausted_error() {
        let past = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let schedule = Schedule::Once { at: past };
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        assert!(schedule.next_occurrence_or_err(now).is_err());
    }

    #[test]
    fn interval_before_start() {
        let start = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
        let schedule = Schedule::Interval {
            every: Duration::hours(1),
            start,
        };
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(schedule.next_occurrence(now), Some(start));
    }

    #[test]
    fn interval_after_start() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let schedule = Schedule::Interval {
            every: Duration::hours(1),
            start,
        };
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 2, 30, 0).unwrap();
        let next = schedule.next_occurrence(now).expect("should find next");
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 1, 3, 0, 0).unwrap());
    }

    #[test]
    fn interval_zero_duration_returns_none() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let schedule = Schedule::Interval {
            every: Duration::zero(),
            start,
        };
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 1, 0, 0).unwrap();
        assert_eq!(schedule.next_occurrence(now), None);
    }

    #[test]
    fn cron_schedule_next() {
        let expr = CronExpr::parse("0 12 * * *").expect("valid");
        let schedule = Schedule::Cron(expr);
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 10, 0, 0).unwrap();
        let next = schedule.next_occurrence(now).expect("should find next");
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap());
    }

    #[test]
    fn schedule_display_once() {
        let at = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        let schedule = Schedule::Once { at };
        let display = schedule.to_string();
        assert!(display.starts_with("once at"));
    }

    #[test]
    fn schedule_display_interval() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let schedule = Schedule::Interval {
            every: Duration::hours(1),
            start,
        };
        let display = schedule.to_string();
        assert!(display.starts_with("every"));
    }

    #[test]
    fn schedule_display_cron() {
        let expr = CronExpr::parse("0 12 * * *").expect("valid");
        let schedule = Schedule::Cron(expr);
        let display = schedule.to_string();
        assert!(display.starts_with("cron:"));
    }

    #[test]
    fn schedule_serde_round_trip_once() {
        let at = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        let schedule = Schedule::Once { at };
        let json = serde_json::to_string(&schedule).expect("serialize");
        let back: Schedule = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(schedule, back);
    }

    #[test]
    fn schedule_serde_round_trip_interval() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let schedule = Schedule::Interval {
            every: Duration::hours(1),
            start,
        };
        let json = serde_json::to_string(&schedule).expect("serialize");
        let back: Schedule = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(schedule, back);
    }

    #[test]
    fn schedule_serde_round_trip_cron() {
        let expr = CronExpr::parse("*/15 * * * *").expect("valid");
        let schedule = Schedule::Cron(expr);
        let json = serde_json::to_string(&schedule).expect("serialize");
        let back: Schedule = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(schedule, back);
    }
}
