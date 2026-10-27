//! Bounded wait durations read from binding arguments.

use std::time::Duration;
use termtrace_model::protocol::control::MAX_CONTROL_WAIT;

/// Convert a binding's seconds value into a wait duration of at most [`MAX_CONTROL_WAIT`].
///
/// # Errors
/// Returns a reason when `seconds` is negative, not finite, or above the bound.
pub fn wait_duration_from_secs(seconds: f64) -> Result<Duration, String> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!(
            "duration {seconds} seconds must be a non-negative finite number"
        ));
    }
    return Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|duration| return *duration <= MAX_CONTROL_WAIT)
        .ok_or_else(|| {
            return format!(
                "duration {seconds} seconds exceeds the maximum of {} seconds",
                MAX_CONTROL_WAIT.as_secs()
            );
        });
}

/// Convert a binding's whole-milliseconds value into a wait duration of at most
/// [`MAX_CONTROL_WAIT`].
///
/// # Errors
/// Returns a reason when `milliseconds` is negative, fractional, not finite, or above the bound.
pub fn wait_duration_from_millis(milliseconds: f64) -> Result<Duration, String> {
    let maximum = MAX_CONTROL_WAIT.as_millis();
    if !milliseconds.is_finite() || milliseconds < 0.0 || milliseconds.fract() != 0.0 {
        return Err(format!(
            "duration {milliseconds} ms must be a non-negative whole number of milliseconds"
        ));
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is a finite non-negative integer checked against a 3.6e6 bound"
    )]
    let whole = milliseconds as u128;
    if whole > maximum {
        return Err(format!(
            "duration {milliseconds} ms exceeds the maximum of {maximum} ms"
        ));
    }
    return Ok(Duration::from_millis(
        u64::try_from(whole).unwrap_or(u64::MAX),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_reject_negative_fractional_non_finite_and_oversized_values() {
        assert_eq!(
            wait_duration_from_millis(1250.0),
            Ok(Duration::from_millis(1250))
        );
        assert_eq!(
            wait_duration_from_secs(1.25),
            Ok(Duration::from_millis(1250))
        );
        assert_eq!(wait_duration_from_millis(3_600_000.0), Ok(MAX_CONTROL_WAIT));
        for milliseconds in [-1.0, 1.5, f64::NAN, f64::INFINITY, 3_600_001.0] {
            assert!(
                wait_duration_from_millis(milliseconds).is_err(),
                "{milliseconds}"
            );
        }

        for seconds in [-0.5, f64::NAN, f64::NEG_INFINITY, 3_600.5] {
            assert!(wait_duration_from_secs(seconds).is_err(), "{seconds}");
        }
    }
}
