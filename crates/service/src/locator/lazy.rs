//! Lazy session-bound locators over pure rich-frame queries.

use super::query::{
    evaluate, LocatorSpec, QueryDescription, QueryError, QueryResult, ScreenMatch, StyleFilter,
};
use crate::{client::Session, screen::StableCaptureOptions};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    error::Error as StdError,
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, PoisonError,
    },
    thread,
    time::{Duration, Instant},
};
use termtrace_model::protocol::control::MAX_CONTROL_WAIT;
use termtrace_model::terminal::screen::{
    FrameDetail, ScreenFrame, ScreenProjectionRequest, ScreenRegion,
};

impl Session {
    /// Bind an immutable specification to the latest presented screen without performing I/O.
    #[must_use]
    pub fn locator(&self, spec: LocatorSpec) -> Locator<'_> {
        return Locator::new(self, spec);
    }

    /// Bind a nonempty literal text query without performing I/O.
    ///
    /// # Errors
    /// Returns `QueryError::InvalidQuery` when `text` is empty.
    pub fn get_by_text(&self, text: impl Into<String>) -> Result<Locator<'_>, QueryError> {
        return Ok(self.locator(LocatorSpec::text(text)?));
    }

    /// Bind an already compiled host-native regular expression without performing I/O.
    ///
    /// # Errors
    /// Returns `QueryError::InvalidQuery` when `regex` can produce a zero-width match.
    pub fn get_by_regex(&self, regex: Regex) -> Result<Locator<'_>, QueryError> {
        return Ok(self.locator(LocatorSpec::regex(regex)?));
    }

    /// Bind one absolute row scope without performing I/O.
    #[must_use]
    pub fn row(&self, row: u16) -> Locator<'_> {
        return self.locator(LocatorSpec::default().in_row(row));
    }

    /// Bind one exact absolute region without performing I/O.
    ///
    /// # Errors
    /// Returns `QueryError::InvalidQuery` when `region` is empty.
    pub fn region(&self, region: ScreenRegion) -> Result<Locator<'_>, QueryError> {
        return Ok(self.locator(LocatorSpec::default().in_region(region)?));
    }

    /// Bind a complete appearance filter without performing I/O.
    #[must_use]
    pub fn filter(&self, style: StyleFilter) -> Locator<'_> {
        return self.locator(LocatorSpec::default().with_style(style));
    }
}

/// Lazy session-bound immutable locator.
#[derive(Clone)]
pub struct Locator<'session> {
    session: &'session Session,
    spec: LocatorSpec,
}

impl<'session> Locator<'session> {
    /// Bind an immutable specification without performing I/O.
    #[must_use]
    pub fn new(session: &'session Session, spec: LocatorSpec) -> Self {
        return Self { session, spec };
    }

    /// Compose another pure specification without performing I/O.
    ///
    /// # Errors
    /// Returns `QueryError::InvalidQuery` when both specifications have text matchers, their row
    /// scopes conflict, or their regions do not intersect.
    pub fn and(&self, other: LocatorSpec) -> Result<Self, QueryError> {
        return Ok(Self {
            session: self.session,
            spec: self.spec.clone().and(other)?,
        });
    }

    /// Capture exactly one latest-presented frame and evaluate it once.
    ///
    /// # Errors
    /// Returns `LocatorError::Capture` when the frame cannot be captured and `LocatorError::Query`
    /// when the specification does not fit the captured frame.
    pub fn evaluate(&self) -> Result<QueryResult, LocatorError> {
        let frame = self
            .session
            .frame(ScreenProjectionRequest {
                detail: FrameDetail::Spans,
                region: self.spec.region(),
            })
            .map_err(LocatorError::Capture)?;

        return evaluate(Arc::new(frame), self.spec.clone()).map_err(LocatorError::Query);
    }

    /// Evaluate all matches after exactly one capture.
    ///
    /// # Errors
    /// Returns the errors of [`Self::evaluate`].
    pub fn all(&self) -> Result<Arc<[ScreenMatch]>, LocatorError> {
        return Ok(self.evaluate()?.matches);
    }

    /// Count matches after exactly one capture.
    ///
    /// # Errors
    /// Returns the errors of [`Self::evaluate`].
    pub fn count(&self) -> Result<usize, LocatorError> {
        return Ok(self.evaluate()?.count());
    }

    /// Strictly require one match after exactly one capture.
    ///
    /// # Errors
    /// Returns the errors of [`Self::evaluate`], and `LocatorError::Query` when there is not
    /// exactly one match.
    pub fn single(&self) -> Result<ScreenMatch, LocatorError> {
        return self
            .evaluate()?
            .single()
            .cloned()
            .map_err(LocatorError::Query);
    }

    /// Explicitly select the first match after exactly one capture.
    ///
    /// # Errors
    /// Returns the errors of [`Self::evaluate`], and `LocatorError::Query` when there is no match.
    pub fn first(&self) -> Result<ScreenMatch, LocatorError> {
        return self
            .evaluate()?
            .first()
            .cloned()
            .map_err(LocatorError::Query);
    }

    /// Explicitly select one zero-based match after exactly one capture.
    ///
    /// # Errors
    /// Returns the errors of [`Self::evaluate`], and `LocatorError::Query` when `index` is out of
    /// bounds.
    pub fn nth(&self, index: usize) -> Result<ScreenMatch, LocatorError> {
        return self
            .evaluate()?
            .nth(index)
            .cloned()
            .map_err(LocatorError::Query);
    }

    /// Poll exact one-frame evaluations and return the satisfying frame without recapture.
    ///
    /// # Errors
    /// Returns the errors of [`Self::wait_stable`].
    pub fn wait(
        &self,
        timeout: Duration,
        interval: Duration,
    ) -> Result<LocatorWaitResult, LocatorError> {
        return self.wait_stable(timeout, interval, Duration::ZERO);
    }

    /// Poll until a query remains satisfied in one unchanged semantic state.
    ///
    /// # Errors
    /// Returns the errors of [`Self::wait_for`] for [`LocatorCondition::Exists`] without
    /// cancellation.
    pub fn wait_stable(
        &self,
        timeout: Duration,
        interval: Duration,
        stable_for: Duration,
    ) -> Result<LocatorWaitResult, LocatorError> {
        return self.wait_for(LocatorWaitOptions {
            condition: LocatorCondition::Exists,
            timeout,
            interval,
            stable_for,
            cancel: None,
        });
    }

    /// Poll exact one-frame evaluations until `options.condition` holds in one semantic state for
    /// `options.stable_for`, returning the satisfying frame without recapture.
    ///
    /// Each poll asks the actor for a capture bounded by [`CAPTURE_QUANTUM`]. Cancellation is
    /// observed before every capture, after every evaluation, and during the sleep between polls.
    /// A capture already sent cannot be interrupted: against a responsive actor a cancelled wait
    /// settles within about one quantum plus a round trip, but an actor queued behind other work
    /// answers only after the quantum plus its two-second wait slack. The client gives up after
    /// the quantum plus three seconds, then waits up to [`crate::defaults::COMPLETION_GRACE`] in
    /// case the session completed: about 5.25 s worst case. A cancellation seen when the exchange
    /// returns wins over its outcome, so the wait still reports `Cancelled`.
    ///
    /// # Errors
    /// Returns `LocatorError::Query` when `timeout` or `interval` is zero, `stable_for` exceeds
    /// `timeout`, or the deadline overflows, `LocatorError::Timeout` with the last frame when the
    /// condition is not satisfied in time, `LocatorError::Cancelled` with the last frame after
    /// cancellation, and capture or query errors as they occur.
    pub fn wait_for(&self, options: LocatorWaitOptions) -> Result<LocatorWaitResult, LocatorError> {
        let LocatorWaitOptions {
            condition,
            timeout,
            interval,
            stable_for,
            cancel,
        } = options;

        if timeout.is_zero() || interval.is_zero() {
            return Err(LocatorError::Query(QueryError::InvalidQuery {
                reason: "locator wait timeout and interval must be greater than zero".to_owned(),
            }));
        }

        if stable_for > timeout {
            return Err(LocatorError::Query(QueryError::InvalidQuery {
                reason: "locator stable interval must not exceed its timeout".to_owned(),
            }));
        }

        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            return LocatorError::Query(QueryError::InvalidQuery {
                reason: "locator wait timeout exceeds the monotonic clock range".to_owned(),
            });
        })?;

        let mut matching_state: Option<(String, Instant)> = None;
        let mut last_frame: Option<Arc<ScreenFrame>> = None;
        let cancelled = |frame: Option<Arc<ScreenFrame>>| {
            return LocatorError::Cancelled {
                frame,
                query: self.spec.description(),
            };
        };

        let timed_out = |frame: Option<Arc<ScreenFrame>>| {
            return LocatorError::Timeout {
                timeout,
                frame,
                query: self.spec.description(),
            };
        };

        loop {
            if cancel.as_ref().is_some_and(WaitCancellation::is_cancelled) {
                return Err(cancelled(last_frame));
            }

            let before_capture = Instant::now();
            let Some(remaining) = deadline.checked_duration_since(before_capture) else {
                return Err(timed_out(last_frame));
            };
            let capture_timeout = remaining.min(CAPTURE_QUANTUM).min(MAX_CONTROL_WAIT);
            let capture = self.session.capture_stable_by(
                StableCaptureOptions {
                    stable_for: Duration::from_nanos(1).min(capture_timeout),
                    timeout: capture_timeout,
                    projection: ScreenProjectionRequest {
                        detail: FrameDetail::Spans,
                        region: self.spec.region(),
                    },
                    allow_incomplete: true,
                    reaction: None,
                },
                Some(deadline),
            );

            let observed_at = Instant::now();
            // A cancel that arrived while the exchange was in flight wins over its outcome.
            if cancel.as_ref().is_some_and(WaitCancellation::is_cancelled) {
                let newest = capture.ok().map(|capture| return Arc::new(capture.frame));

                return Err(cancelled(newest.or(last_frame)));
            }

            let capture = match capture {
                Ok(capture) => capture,
                Err(error)
                    if observed_at >= deadline
                        || matches!(
                            &error,
                            crate::Error::Control(termtrace_control::Error::Timeout { .. })
                        ) =>
                {
                    return Err(timed_out(last_frame));
                }
                Err(error) => return Err(LocatorError::Capture(error)),
            };

            let frame = Arc::new(capture.frame);
            last_frame = Some(Arc::clone(&frame));

            if observed_at >= deadline {
                return Err(timed_out(last_frame));
            }

            let result = evaluate(Arc::clone(&frame), self.spec.clone());
            let evaluated_at = Instant::now();

            if cancel.as_ref().is_some_and(WaitCancellation::is_cancelled) {
                return Err(cancelled(Some(frame)));
            }

            let result = result.map_err(LocatorError::Query)?;

            if evaluated_at >= deadline {
                return Err(timed_out(Some(result.frame)));
            }

            if condition.is_satisfied_by(result.matches.len()) {
                if stable_for.is_zero() {
                    return Ok(LocatorWaitResult {
                        frame: result.frame,
                        matches: result.matches,
                    });
                }

                match &matching_state {
                    Some((state_hash, since)) if state_hash == &result.frame.state_hash => {
                        if evaluated_at.saturating_duration_since(*since) >= stable_for {
                            return Ok(LocatorWaitResult {
                                frame: result.frame,
                                matches: result.matches,
                            });
                        }
                    }
                    _ => matching_state = Some((result.frame.state_hash.clone(), evaluated_at)),
                }
            } else {
                matching_state = None;
            }

            let pause = interval.min(deadline.saturating_duration_since(evaluated_at));

            match &cancel {
                Some(cancel) => {
                    if cancel.sleep(pause) {
                        return Err(cancelled(last_frame));
                    }
                }
                None => thread::sleep(pause),
            }
        }
    }
}

const FORWARD_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Upper bound on how long one locator poll asks the actor to wait for a capture.
///
/// It bounds how long a cancelled wait can be blocked behind an exchange already sent.
pub const CAPTURE_QUANTUM: Duration = Duration::from_millis(250);

/// Match-count condition a locator wait requires.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LocatorCondition {
    /// At least one match.
    #[default]
    Exists,
    /// No match.
    Absent,
    /// Exactly this many matches.
    Count(usize),
}

impl LocatorCondition {
    fn is_satisfied_by(self, count: usize) -> bool {
        return match self {
            Self::Exists => count > 0,
            Self::Absent => count == 0,
            Self::Count(expected) => count == expected,
        };
    }
}

/// Bounds and condition for [`Locator::wait_for`].
#[derive(Clone, Debug)]
pub struct LocatorWaitOptions {
    /// Required match-count condition.
    pub condition: LocatorCondition,
    /// Overall monotonic bound; must be nonzero.
    pub timeout: Duration,
    /// Pause between polls; must be nonzero.
    pub interval: Duration,
    /// How long the condition must hold in one unchanged semantic state; at most `timeout`.
    pub stable_for: Duration,
    /// Cooperative cancellation shared with other threads.
    pub cancel: Option<WaitCancellation>,
}

/// Cloneable cancellation flag that wakes a sleeping locator wait from any thread.
///
/// Cancelling never interrupts a capture exchange already sent; see [`Locator::wait_for`].
#[derive(Clone, Debug, Default)]
pub struct WaitCancellation {
    state: Arc<(Mutex<bool>, Condvar)>,
}

impl WaitCancellation {
    /// Create an uncancelled flag.
    #[must_use]
    pub fn new() -> Self {
        return Self::default();
    }

    /// Request cancellation and wake every waiter; idempotent.
    pub fn cancel(&self) {
        let (flag, wake) = &*self.state;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        let (flag, _) = &*self.state;

        return *flag.lock().unwrap_or_else(PoisonError::into_inner);
    }

    /// Run `operation` with a flag that is set once `token` is cancelled while it runs.
    ///
    /// Without a token the flag is never set.
    pub fn forward<T>(token: Option<&Self>, operation: impl FnOnce(&AtomicBool) -> T) -> T {
        let cancelled = AtomicBool::new(false);
        let Some(token) = token else {
            return operation(&cancelled);
        };

        let done = AtomicBool::new(false);

        return thread::scope(|scope| {
            scope.spawn(|| {
                while !done.load(Ordering::Acquire) {
                    if token.sleep(FORWARD_POLL_INTERVAL) {
                        cancelled.store(true, Ordering::Release);

                        return;
                    }
                }
            });

            let result = operation(&cancelled);
            done.store(true, Ordering::Release);

            return result;
        });
    }

    fn sleep(&self, duration: Duration) -> bool {
        let (flag, wake) = &*self.state;
        let guard = flag.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = wake
            .wait_timeout_while(guard, duration, |cancelled| return !*cancelled)
            .unwrap_or_else(PoisonError::into_inner);

        return *guard;
    }
}

/// Successful locator wait evidence from the same satisfying frame.
#[derive(Clone, Debug)]
pub struct LocatorWaitResult {
    /// Exact satisfying frame.
    pub frame: Arc<ScreenFrame>,
    /// Matches evaluated from that same frame.
    pub matches: Arc<[ScreenMatch]>,
}

/// Session-bound locator infrastructure and pure-query failures.
#[derive(Debug)]
pub enum LocatorError {
    /// Exact rich frame capture failed.
    Capture(crate::Error),
    /// Pure evaluation failed.
    Query(QueryError),
    /// The monotonic locator deadline elapsed.
    Timeout {
        /// Requested bound.
        timeout: Duration,
        /// Last exact frame captured before the deadline, when one was available.
        frame: Option<Arc<ScreenFrame>>,
        /// Bounded normalized query.
        query: QueryDescription,
    },
    /// The wait observed its cancellation; no input was sent.
    Cancelled {
        /// Last exact frame captured before cancellation, when one was available.
        frame: Option<Arc<ScreenFrame>>,
        /// Bounded normalized query.
        query: QueryDescription,
    },
}

/// Typed details of a locator query, timeout, or cancellation failure.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum LocatorFailure {
    /// The query itself failed.
    Query(QueryError),
    /// A wait ended without its condition holding.
    Wait(LocatorWaitFailure),
}

/// A locator wait that timed out or was cancelled, with the identity of its last frame.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocatorWaitFailure {
    /// The wait timed out.
    LocatorTimeout {
        /// Bounded normalized query.
        query: QueryDescription,
        /// Semantic state identity of the last frame, when one was captured.
        state_hash: Option<String>,
        /// Projection identity of the last frame, when one was captured.
        frame_hash: Option<String>,
    },
    /// The wait was cancelled.
    Cancelled {
        /// Bounded normalized query.
        query: QueryDescription,
        /// Semantic state identity of the last frame, when one was captured.
        state_hash: Option<String>,
        /// Projection identity of the last frame, when one was captured.
        frame_hash: Option<String>,
    },
}

impl LocatorError {
    /// Typed failure details; `None` for capture failures, which carry an SDK error instead.
    #[must_use]
    pub fn failure(&self) -> Option<LocatorFailure> {
        let identity = |frame: &Option<Arc<ScreenFrame>>| {
            return (
                frame.as_ref().map(|frame| return frame.state_hash.clone()),
                frame.as_ref().map(|frame| return frame.frame_hash.clone()),
            );
        };

        return match self {
            Self::Capture(_) => None,
            Self::Query(error) => Some(LocatorFailure::Query(error.clone())),
            Self::Timeout { frame, query, .. } => {
                let (state_hash, frame_hash) = identity(frame);

                Some(LocatorFailure::Wait(LocatorWaitFailure::LocatorTimeout {
                    query: query.clone(),
                    state_hash,
                    frame_hash,
                }))
            }
            Self::Cancelled { frame, query } => {
                let (state_hash, frame_hash) = identity(frame);

                Some(LocatorFailure::Wait(LocatorWaitFailure::Cancelled {
                    query: query.clone(),
                    state_hash,
                    frame_hash,
                }))
            }
        };
    }

    /// Stable machine-readable failure kind shared by every SDK; capture failures report
    /// `capture` and keep the underlying error's own kind in [`crate::Error::kind`].
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        return match self {
            Self::Capture(_) => "capture",
            Self::Query(error) => error.kind(),
            Self::Timeout { .. } => "locator_timeout",
            Self::Cancelled { .. } => "cancelled",
        };
    }
}

impl fmt::Display for LocatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capture(error) => return write!(f, "capture locator frame: {error}"),
            Self::Query(error) => return error.fmt(f),
            Self::Timeout { timeout, query, .. } => {
                return write!(f, "locator {} timed out after {timeout:?}", query.text);
            }
            Self::Cancelled { query, .. } => {
                return write!(f, "locator {} wait was cancelled", query.text);
            }
        }
    }
}

impl StdError for LocatorError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cancellation_wakes_a_sleeping_wait_from_another_thread() {
        let token = WaitCancellation::new();
        let sleeper = token.clone();
        let started = Instant::now();
        let handle = thread::spawn(move || return sleeper.sleep(Duration::from_secs(30)));

        thread::sleep(Duration::from_millis(50));

        token.cancel();

        assert!(handle.join().expect("join sleeper"));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancellation_before_sleep_returns_immediately_and_uncancelled_sleep_times_out() {
        let idle = WaitCancellation::new();
        assert!(!idle.sleep(Duration::from_millis(10)));

        idle.cancel();
        let started = Instant::now();

        assert!(idle.sleep(Duration::from_secs(30)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn conditions_accept_only_documented_shapes() {
        assert_eq!(
            serde_json::from_value::<LocatorCondition>(json!("absent")).expect("absent"),
            LocatorCondition::Absent
        );
        assert_eq!(
            serde_json::from_value::<LocatorCondition>(json!({ "count": 2 })).expect("count"),
            LocatorCondition::Count(2)
        );

        for value in [
            json!("present"),
            json!({ "count": -1 }),
            json!({ "count": 1.5 }),
            json!({ "count": 1, "extra": true }),
            json!(2),
        ] {
            assert!(
                serde_json::from_value::<LocatorCondition>(value.clone()).is_err(),
                "{value}"
            );
        }
    }
}
