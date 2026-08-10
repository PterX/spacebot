//! Shared schedule trigger layer.
//!
//! A `ScheduleSpec` names when something recurs — a standard 5-field cron
//! expression, an interval, or both (cron wins). Cron jobs and schedule-trigger
//! wake definitions both resolve their next fire through this module so the
//! expression expansion and timezone conversion exist exactly once.

use crate::error::Result;

use std::str::FromStr as _;

/// When a scheduled trigger recurs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSpec {
    /// Standard 5-field cron expression. Takes precedence over
    /// `interval_secs` when both are set.
    pub cron_expr: Option<String>,
    pub interval_secs: Option<u64>,
}

impl ScheduleSpec {
    /// Next occurrence strictly after `after`, evaluated in `tz` and returned
    /// in UTC. Returns `None` for unparseable expressions (they were validated
    /// at load; a runtime surprise degrades to no occurrence) and for specs
    /// with neither a cron expression nor a positive interval.
    pub fn next_occurrence(
        &self,
        after: chrono::DateTime<chrono::Utc>,
        tz: chrono_tz::Tz,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        self.next_occurrence_in(after, &tz)
    }

    /// Generic over the timezone so callers that fall back to the system-local
    /// timezone (no configured cron timezone) share the same computation.
    pub(crate) fn next_occurrence_in<Z: chrono::TimeZone>(
        &self,
        after: chrono::DateTime<chrono::Utc>,
        tz: &Z,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        if let Some(expr) = self.cron_expr.as_deref() {
            let schedule = match parse_cron_schedule(expr) {
                Ok(schedule) => schedule,
                Err(error) => {
                    tracing::warn!(cron_expr = expr, %error, "invalid cron expression in schedule spec");
                    return None;
                }
            };
            return next_cron_occurrence(&schedule, after, tz);
        }

        // A zero interval would recur immediately forever; treat it as no
        // schedule rather than a hot loop. An interval too large to represent
        // as a duration or to add to the anchor likewise yields no occurrence.
        let interval = self.interval_secs.filter(|secs| *secs > 0)?;
        let duration = i64::try_from(interval)
            .ok()
            .and_then(chrono::Duration::try_seconds)?;
        after.checked_add_signed(duration)
    }
}

/// Expand a 5-field standard cron expression to the 7-field format required by
/// the `cron` crate: `sec min hour dom month dow year`. If the expression
/// already has 6+ fields, return it as-is.
fn expand_cron_expr(expr: &str) -> String {
    let field_count = expr.split_whitespace().count();
    if field_count == 5 {
        format!("0 {expr} *")
    } else {
        expr.to_string()
    }
}

/// Parse a cron expression, accepting the standard 5-field form.
pub(crate) fn parse_cron_schedule(
    expr: &str,
) -> std::result::Result<cron::Schedule, cron::error::Error> {
    cron::Schedule::from_str(&expand_cron_expr(expr))
}

/// Next fire of `schedule` strictly after `after`, evaluated in `tz` and
/// returned in UTC.
pub(crate) fn next_cron_occurrence<Z: chrono::TimeZone>(
    schedule: &cron::Schedule,
    after: chrono::DateTime<chrono::Utc>,
    tz: &Z,
) -> Option<chrono::DateTime<chrono::Utc>> {
    schedule
        .after(&after.with_timezone(tz))
        .next()
        .map(|next| next.with_timezone(&chrono::Utc))
}

/// Validate and normalize an optional 5-field cron expression, returning the
/// original 5-field form. Shared by cron job and wake config validation.
pub(crate) fn normalize_cron_expr(cron_expr: Option<String>) -> Result<Option<String>> {
    let Some(expr) = cron_expr else {
        return Ok(None);
    };

    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let field_count = trimmed.split_whitespace().count();
    if field_count != 5 {
        return Err(crate::error::Error::Other(anyhow::anyhow!(
            "cron expression must have exactly 5 fields (got {field_count}): '{trimmed}'"
        )));
    }

    parse_cron_schedule(trimmed).map_err(|error| {
        crate::error::Error::Other(anyhow::anyhow!(
            "invalid cron expression '{trimmed}': {error}"
        ))
    })?;

    // Store the original 5-field form — it's what users and the UI expect.
    Ok(Some(trimmed.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn interval_adds_seconds_to_anchor() {
        let spec = ScheduleSpec {
            cron_expr: None,
            interval_secs: Some(600),
        };
        let after = utc(2026, 8, 9, 10, 0, 0);
        assert_eq!(
            spec.next_occurrence(after, chrono_tz::UTC),
            Some(utc(2026, 8, 9, 10, 10, 0))
        );
    }

    #[test]
    fn zero_interval_yields_no_occurrence() {
        let spec = ScheduleSpec {
            cron_expr: None,
            interval_secs: Some(0),
        };
        assert_eq!(
            spec.next_occurrence(utc(2026, 8, 9, 10, 0, 0), chrono_tz::UTC),
            None
        );
    }

    #[test]
    fn oversized_interval_yields_no_occurrence() {
        for interval in [u64::MAX, i64::MAX as u64] {
            let spec = ScheduleSpec {
                cron_expr: None,
                interval_secs: Some(interval),
            };
            assert_eq!(
                spec.next_occurrence(utc(2026, 8, 9, 10, 0, 0), chrono_tz::UTC),
                None
            );
        }
    }

    #[test]
    fn empty_spec_yields_no_occurrence() {
        let spec = ScheduleSpec {
            cron_expr: None,
            interval_secs: None,
        };
        assert_eq!(
            spec.next_occurrence(utc(2026, 8, 9, 10, 0, 0), chrono_tz::UTC),
            None
        );
    }

    #[test]
    fn cron_expression_evaluates_in_timezone() {
        // Daily at 08:00 local. At 12:00 UTC on Jan 15 it is 07:00 in New
        // York (UTC-5), so the next fire is 08:00 EST the same day — 13:00
        // UTC — even though 08:00 UTC has already passed.
        let spec = ScheduleSpec {
            cron_expr: Some("0 8 * * *".to_string()),
            interval_secs: None,
        };
        let after = utc(2026, 1, 15, 12, 0, 0);
        assert_eq!(
            spec.next_occurrence(after, chrono_tz::America::New_York),
            Some(utc(2026, 1, 15, 13, 0, 0))
        );
    }

    #[test]
    fn cron_expression_wins_over_interval() {
        let spec = ScheduleSpec {
            cron_expr: Some("0 8 * * *".to_string()),
            interval_secs: Some(600),
        };
        let after = utc(2026, 1, 15, 12, 0, 0);
        // The cron path fires at 08:00 the next day in UTC, not after + 600s.
        assert_eq!(
            spec.next_occurrence(after, chrono_tz::UTC),
            Some(utc(2026, 1, 16, 8, 0, 0))
        );
    }

    #[test]
    fn invalid_expression_yields_no_occurrence() {
        let spec = ScheduleSpec {
            cron_expr: Some("not a cron expression".to_string()),
            interval_secs: Some(600),
        };
        assert_eq!(
            spec.next_occurrence(utc(2026, 8, 9, 10, 0, 0), chrono_tz::UTC),
            None
        );
    }

    #[test]
    fn normalize_accepts_five_field_form() {
        assert_eq!(
            normalize_cron_expr(Some(" 0 8 * * * ".to_string())).unwrap(),
            Some("0 8 * * *".to_string())
        );
        assert_eq!(normalize_cron_expr(None).unwrap(), None);
        assert_eq!(normalize_cron_expr(Some("  ".to_string())).unwrap(), None);
    }

    #[test]
    fn normalize_rejects_wrong_field_count_and_bad_fields() {
        assert!(normalize_cron_expr(Some("0 8 * *".to_string())).is_err());
        assert!(normalize_cron_expr(Some("0 8 * * * *".to_string())).is_err());
        assert!(normalize_cron_expr(Some("99 99 * * *".to_string())).is_err());
    }
}
