use std::{
    error::Error as StdError,
    fmt,
    num::{ParseIntError, TryFromIntError},
    result::Result as StdResult,
};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_MINUTE: u64 = 60 * NANOS_PER_SECOND;
const NANOS_PER_HOUR: u64 = 60 * NANOS_PER_MINUTE;

/// A rejected human-readable time offset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeOffsetError {
    reason: String,
    source: Option<IntegerError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum IntegerError {
    Parse(ParseIntError),
    Range(TryFromIntError),
}

impl TimeOffsetError {
    fn new(reason: impl Into<String>) -> Self {
        return Self {
            reason: reason.into(),
            source: None,
        };
    }

    fn with_source(reason: impl Into<String>, source: IntegerError) -> Self {
        return Self {
            reason: reason.into(),
            source: Some(source),
        };
    }

    fn in_value(self, value: &str) -> Self {
        return Self {
            reason: format!("invalid time offset `{value}`: {}", self.reason),
            source: self.source,
        };
    }
}

impl fmt::Display for TimeOffsetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return f.write_str(&self.reason);
    }
}

impl StdError for TimeOffsetError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match &self.source {
            Some(IntegerError::Parse(source)) => return Some(source),
            Some(IntegerError::Range(source)) => return Some(source),
            None => return None,
        }
    }
}

/// Parse a human-readable non-negative source offset into nanoseconds.
///
/// Accepted forms are a decimal number with an optional `ns`, `us`, `ms`,
/// `s`, `m`, or `h` suffix (bare numbers are seconds), plus `MM:SS.fraction`
/// and `HH:MM:SS.fraction` clock notation.
///
/// # Errors
///
/// Returns [`TimeOffsetError`] when the value is empty, malformed, uses an unknown suffix,
/// is more precise than one nanosecond, has out-of-range clock components, or overflows `u64`
/// nanoseconds.
pub fn parse_time_offset_ns(value: &str) -> StdResult<u64, TimeOffsetError> {
    if value.is_empty() {
        return Err(TimeOffsetError::new("time offset cannot be empty"));
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

    return parse_scaled_decimal(number, scale).map_err(|error| return error.in_value(value));
}

fn parse_clock_offset(value: &str) -> StdResult<u64, TimeOffsetError> {
    let parts = value.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0, parse_integer(minutes, "minutes")?, *seconds),
        [hours, minutes, seconds] => (
            parse_integer(hours, "hours")?,
            parse_integer(minutes, "minutes")?,
            *seconds,
        ),
        _ => {
            return Err(TimeOffsetError::new(format!(
                "invalid time offset `{value}`: use MM:SS or HH:MM:SS"
            )));
        }
    };

    if parts.len() == 3 && minutes >= 60 {
        return Err(TimeOffsetError::new(format!(
            "invalid time offset `{value}`: minutes must be below 60 in HH:MM:SS"
        )));
    }

    let seconds_ns = parse_scaled_decimal(seconds, NANOS_PER_SECOND)
        .map_err(|error| return error.in_value(value))?;

    if seconds_ns >= NANOS_PER_MINUTE {
        return Err(TimeOffsetError::new(format!(
            "invalid time offset `{value}`: seconds must be below 60"
        )));
    }

    let overflow =
        || return TimeOffsetError::new(format!("invalid time offset `{value}`: value overflows"));
    let hours_ns = hours.checked_mul(NANOS_PER_HOUR).ok_or_else(overflow)?;
    let minutes_ns = minutes.checked_mul(NANOS_PER_MINUTE).ok_or_else(overflow)?;

    return hours_ns
        .checked_add(minutes_ns)
        .and_then(|total| return total.checked_add(seconds_ns))
        .ok_or_else(overflow);
}

fn parse_scaled_decimal(number: &str, scale: u64) -> StdResult<u64, TimeOffsetError> {
    let (whole, fraction) = number
        .split_once('.')
        .map_or((number, None), |(whole, fraction)| {
            return (whole, Some(fraction));
        });

    if whole.is_empty() || number.matches('.').count() > 1 {
        return Err(TimeOffsetError::new(
            "expected a non-negative decimal number",
        ));
    }

    let whole = parse_integer(whole, "value")?;
    let whole_ns = whole
        .checked_mul(scale)
        .ok_or_else(|| return TimeOffsetError::new("value overflows nanoseconds"))?;

    let Some(fraction) = fraction else {
        return Ok(whole_ns);
    };
    if fraction.is_empty()
        || fraction.len() > 18
        || !fraction.bytes().all(|byte| return byte.is_ascii_digit())
    {
        return Err(TimeOffsetError::new(
            "fraction must contain between 1 and 18 digits",
        ));
    }

    let numerator = parse_integer(fraction, "fraction")?;
    let exponent = u32::try_from(fraction.len()).map_err(|source| {
        return TimeOffsetError::with_source(
            "fraction is too precise",
            IntegerError::Range(source),
        );
    })?;

    let denominator = 10_u64
        .checked_pow(exponent)
        .ok_or_else(|| return TimeOffsetError::new("fraction is too precise"))?;

    let scaled = u128::from(numerator) * u128::from(scale);
    if scaled % u128::from(denominator) != 0 {
        return Err(TimeOffsetError::new(
            "fraction is more precise than one nanosecond",
        ));
    }

    let fraction_ns = u64::try_from(scaled / u128::from(denominator)).map_err(|source| {
        return TimeOffsetError::with_source(
            "fraction overflows nanoseconds",
            IntegerError::Range(source),
        );
    })?;

    return whole_ns
        .checked_add(fraction_ns)
        .ok_or_else(|| return TimeOffsetError::new("value overflows nanoseconds"));
}

fn parse_integer(value: &str, component: &str) -> StdResult<u64, TimeOffsetError> {
    return value.parse::<u64>().map_err(|source| {
        return TimeOffsetError::with_source(
            format!("{component} must be a non-negative integer"),
            IntegerError::Parse(source),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rejects_a_sign_inside_a_decimal_fraction() {
        assert!(parse_time_offset_ns("0.+1s").is_err());
    }

    #[test]
    fn rejects_invalid_offsets() {
        assert!(parse_time_offset_ns("-1s").is_err());
        assert!(parse_time_offset_ns("1:60").is_err());
        assert!(parse_time_offset_ns("0.1ns").is_err());
        assert!(parse_time_offset_ns("2m30s").is_err());
    }
}
