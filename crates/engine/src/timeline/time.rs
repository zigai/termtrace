use std::result::Result as StdResult;

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_MINUTE: u64 = 60 * NANOS_PER_SECOND;
const NANOS_PER_HOUR: u64 = 60 * NANOS_PER_MINUTE;

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
    fn rejects_invalid_offsets() {
        assert!(parse_time_offset_ns("-1s").is_err());
        assert!(parse_time_offset_ns("1:60").is_err());
        assert!(parse_time_offset_ns("0.1ns").is_err());
        assert!(parse_time_offset_ns("2m30s").is_err());
    }
}
