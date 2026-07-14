//! Validated source-time ranges shared by semantic and native replay.

use std::result::Result as StdResult;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_MINUTE: u64 = 60 * NANOS_PER_SECOND;
const NANOS_PER_HOUR: u64 = 60 * NANOS_PER_MINUTE;

/// A non-empty half-open interval on the canonical monotonic timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceRange {
    /// Inclusive source timestamp in nanoseconds.
    pub start_monotonic_ns: u64,
    /// Exclusive source timestamp in nanoseconds.
    pub end_monotonic_ns: u64,
}

impl SourceRange {
    /// Duration of the selected source interval.
    pub fn duration_ns(self) -> u64 {
        return self.end_monotonic_ns - self.start_monotonic_ns;
    }

    /// Whether a timestamp belongs to this half-open interval.
    pub fn contains(self, monotonic_ns: u64) -> bool {
        return monotonic_ns >= self.start_monotonic_ns && monotonic_ns < self.end_monotonic_ns;
    }

    /// Stable suffix used by range-specific no-replace artifact directories.
    pub fn artifact_suffix(self) -> String {
        return format!(
            "range-{}-{}",
            self.start_monotonic_ns, self.end_monotonic_ns
        );
    }
}

/// Resolve optional CLI bounds against a canonical event-log endpoint.
///
/// `start` is inclusive and defaults to zero. `end` is exclusive and defaults
/// to one nanosecond after the final event so that the final event is included.
/// An entirely absent request preserves legacy full-session behavior.
///
/// # Errors
///
/// Returns an error for reversed, empty, overflowing, or out-of-bounds ranges.
pub fn resolve_source_range(
    start: Option<u64>,
    end: Option<u64>,
    final_monotonic_ns: u64,
) -> Result<Option<SourceRange>> {
    if start.is_none() && end.is_none() {
        return Ok(None);
    }
    let source_end = final_monotonic_ns.checked_add(1).ok_or_else(|| {
        return range_error("source timeline ends at u64::MAX and cannot form an exclusive bound");
    })?;
    let start = start.unwrap_or(0);
    let end = end.unwrap_or(source_end);
    if start >= end {
        return Err(range_error(format!(
            "source range must be non-empty with FROM < TO, got {start}..{end}"
        )));
    }
    if start > final_monotonic_ns {
        return Err(range_error(format!(
            "source range starts at {start} ns after the final event at {final_monotonic_ns} ns"
        )));
    }
    if end > source_end {
        return Err(range_error(format!(
            "source range ends at {end} ns after the exclusive source endpoint {source_end} ns"
        )));
    }
    return Ok(Some(SourceRange {
        start_monotonic_ns: start,
        end_monotonic_ns: end,
    }));
}

/// Parse a human-readable non-negative source offset into nanoseconds.
///
/// Accepted forms are a decimal number with an optional `ns`, `us`, `ms`,
/// `s`, `m`, or `h` suffix (bare numbers are seconds), plus `MM:SS.fraction`
/// and `HH:MM:SS.fraction` clock notation.
pub fn parse_time_offset_ns(value: &str) -> StdResult<u64, String> {
    if value.is_empty() {
        return Err("time offset cannot be empty".to_owned());
    }
    if value.contains(':') {
        return parse_clock_offset(value);
    }
    let (number, scale) = [
        ("ns", 1),
        ("us", 1_000),
        ("ms", 1_000_000),
        ("s", NANOS_PER_SECOND),
        ("m", NANOS_PER_MINUTE),
        ("h", NANOS_PER_HOUR),
    ]
    .into_iter()
    .find_map(|(suffix, scale)| {
        return value
            .strip_suffix(suffix)
            .map(|number| return (number, scale));
    })
    .unwrap_or((value, NANOS_PER_SECOND));
    return parse_scaled_decimal(number, scale)
        .map_err(|reason| return format!("invalid time offset `{value}`: {reason}"));
}

fn parse_clock_offset(value: &str) -> StdResult<u64, String> {
    let parts = value.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0, parse_integer(minutes, "minutes")?, *seconds),
        [hours, minutes, seconds] => (
            parse_integer(hours, "hours")?,
            parse_integer(minutes, "minutes")?,
            *seconds,
        ),
        _ => {
            return Err(format!(
                "invalid time offset `{value}`: use MM:SS or HH:MM:SS"
            ));
        }
    };
    if parts.len() == 3 && minutes >= 60 {
        return Err(format!(
            "invalid time offset `{value}`: minutes must be below 60 in HH:MM:SS"
        ));
    }
    let seconds_ns = parse_scaled_decimal(seconds, NANOS_PER_SECOND)
        .map_err(|reason| return format!("invalid time offset `{value}`: {reason}"))?;
    if seconds_ns >= NANOS_PER_MINUTE {
        return Err(format!(
            "invalid time offset `{value}`: seconds must be below 60"
        ));
    }
    let hours_ns = hours
        .checked_mul(NANOS_PER_HOUR)
        .ok_or_else(|| return format!("invalid time offset `{value}`: value overflows"))?;
    let minutes_ns = minutes
        .checked_mul(NANOS_PER_MINUTE)
        .ok_or_else(|| return format!("invalid time offset `{value}`: value overflows"))?;
    return hours_ns
        .checked_add(minutes_ns)
        .and_then(|total| return total.checked_add(seconds_ns))
        .ok_or_else(|| return format!("invalid time offset `{value}`: value overflows"));
}

fn parse_scaled_decimal(number: &str, scale: u64) -> StdResult<u64, String> {
    let (whole, fraction) = number
        .split_once('.')
        .map_or((number, None), |(whole, fraction)| {
            return (whole, Some(fraction));
        });
    if whole.is_empty() || number.matches('.').count() > 1 {
        return Err("expected a non-negative decimal number".to_owned());
    }
    let whole = parse_integer(whole, "value")?;
    let whole_ns = whole
        .checked_mul(scale)
        .ok_or_else(|| return "value overflows nanoseconds".to_owned())?;
    let Some(fraction) = fraction else {
        return Ok(whole_ns);
    };
    if fraction.is_empty() || fraction.len() > 18 {
        return Err("fraction must contain between 1 and 18 digits".to_owned());
    }
    let numerator = parse_integer(fraction, "fraction")?;
    let exponent =
        u32::try_from(fraction.len()).map_err(|_| return "fraction is too precise".to_owned())?;
    let denominator = 10_u64
        .checked_pow(exponent)
        .ok_or_else(|| return "fraction is too precise".to_owned())?;
    let scaled = u128::from(numerator) * u128::from(scale);
    if scaled % u128::from(denominator) != 0 {
        return Err("fraction is more precise than one nanosecond".to_owned());
    }
    let fraction_ns = u64::try_from(scaled / u128::from(denominator))
        .map_err(|_| return "fraction overflows nanoseconds".to_owned())?;
    return whole_ns
        .checked_add(fraction_ns)
        .ok_or_else(|| return "value overflows nanoseconds".to_owned());
}

fn parse_integer(value: &str, component: &str) -> StdResult<u64, String> {
    return value
        .parse::<u64>()
        .map_err(|_| return format!("{component} must be a non-negative integer"));
}

fn range_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_half_open_bounds() {
        assert_eq!(
            resolve_source_range(None, None, 100).expect("full range"),
            None
        );
        assert_eq!(
            resolve_source_range(Some(20), None, 100).expect("open end"),
            Some(SourceRange {
                start_monotonic_ns: 20,
                end_monotonic_ns: 101,
            })
        );
        assert_eq!(
            resolve_source_range(None, Some(80), 100).expect("open start"),
            Some(SourceRange {
                start_monotonic_ns: 0,
                end_monotonic_ns: 80,
            })
        );
    }

    #[test]
    fn rejects_empty_and_out_of_bounds_ranges() {
        assert!(resolve_source_range(Some(10), Some(10), 100).is_err());
        assert!(resolve_source_range(Some(101), None, 100).is_err());
        assert!(resolve_source_range(None, Some(102), 100).is_err());
    }

    #[test]
    fn parses_units_and_clock_offsets_exactly() {
        assert_eq!(parse_time_offset_ns("1.5s"), Ok(1_500_000_000));
        assert_eq!(parse_time_offset_ns("250ms"), Ok(250_000_000));
        assert_eq!(parse_time_offset_ns("2m"), Ok(120_000_000_000));
        assert_eq!(parse_time_offset_ns("01:02.25"), Ok(62_250_000_000));
        assert_eq!(parse_time_offset_ns("01:01:02.25"), Ok(3_662_250_000_000));
        assert_eq!(parse_time_offset_ns("2"), Ok(2_000_000_000));
    }

    #[test]
    fn rejects_invalid_offsets() {
        assert!(parse_time_offset_ns("-1s").is_err());
        assert!(parse_time_offset_ns("1:60").is_err());
        assert!(parse_time_offset_ns("0.1ns").is_err());
        assert!(parse_time_offset_ns("2m30s").is_err());
    }
}
