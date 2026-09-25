use std::{
    thread,
    time::{Duration, Instant},
};

/// Run `attempt` every `interval` until it yields a value, fails, or `deadline` passes.
///
/// `attempt` runs at least once, and once more at the deadline when an interval would
/// overshoot it. Returns `Ok(None)` when the deadline passes without a value.
///
/// # Errors
///
/// Returns the first error reported by `attempt`.
pub fn poll_until<T, E>(
    deadline: Instant,
    interval: Duration,
    mut attempt: impl FnMut() -> Result<Option<T>, E>,
) -> Result<Option<T>, E> {
    loop {
        if let Some(value) = attempt()? {
            return Ok(Some(value));
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }

        thread::sleep(interval.min(remaining));
    }
}

/// Run `attempt` every `interval` until it yields a value or fails.
pub(crate) fn poll<T, E>(
    interval: Duration,
    mut attempt: impl FnMut() -> Result<Option<T>, E>,
) -> Result<T, E> {
    loop {
        if let Some(value) = attempt()? {
            return Ok(value);
        }

        thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    #[test]
    fn expired_deadline_still_attempts_once() {
        let mut attempts = 0;
        let result = poll_until(Instant::now(), Duration::from_secs(1), || {
            attempts += 1;

            return Ok::<_, Infallible>(None::<()>);
        });
        assert_eq!(result, Ok(None));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn returns_the_first_value_and_stops_on_errors() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut attempts = 0;
        let value = poll_until(deadline, Duration::ZERO, || {
            attempts += 1;

            return Ok::<_, Infallible>((attempts == 3).then_some(attempts));
        });
        assert_eq!(value, Ok(Some(3)));
        let failed = poll_until(deadline, Duration::ZERO, || {
            return Err::<Option<()>, _>("stop");
        });
        assert_eq!(failed, Err("stop"));
    }
}
