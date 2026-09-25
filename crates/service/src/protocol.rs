//! Screen waits that span a live session's transition to completion.

use crate::{Error, Result};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use termtrace_control::read_completed;
use termtrace_engine::{
    inspection::wait_for_completion,
    observation::{
        project_screen_frame, resolved_screen_state_for_session, state_text, CaptureQuality,
        StatePoint,
    },
    recording::control::{compile_screen_predicate, evaluate_screen_predicate},
};
use termtrace_model::{
    protocol::control::{ScreenPredicate, ScreenWaitQuality},
    terminal::{
        inspection::ScreenWaitOutcome,
        screen::{ScreenFrameSource, ScreenProjectionRequest},
    },
};

/// Wait for an exact predicate on the latest presentation-eligible viewport.
///
/// When the live actor becomes unreachable because the session completed, the
/// predicate is evaluated once against the completed final screen.
///
/// # Errors
///
/// Returns an error when the actor rejects the wait, or when it is unreachable
/// and the session does not complete within the remaining timeout.
pub fn wait_for_screen(
    session_directory: impl AsRef<Path>,
    predicate: ScreenPredicate,
    stable_for: Duration,
    timeout: Duration,
) -> Result<ScreenWaitOutcome> {
    let session_directory = session_directory.as_ref();
    let started_at = Instant::now();

    return match termtrace_control::wait_for_screen(
        session_directory,
        predicate.clone(),
        stable_for,
        timeout,
    ) {
        Ok(outcome) => Ok(outcome),
        Err(error) if error.is_connection_failure() => {
            let remaining = timeout
                .saturating_sub(started_at.elapsed())
                .max(Duration::from_secs(5));

            if let Err(completion_error) = wait_for_completion(session_directory, remaining) {
                if !matches!(read_completed(session_directory), Ok(None)) {
                    return Err(completion_error.into());
                }
                return Err(error.into());
            }

            let stable_for_ns = u64::try_from(stable_for.as_nanos()).unwrap_or(u64::MAX);

            completed_screen_wait(session_directory, predicate, stable_for_ns)
        }
        Err(error) => Err(error.into()),
    };
}

fn completed_screen_wait(
    session: &Path,
    predicate: ScreenPredicate,
    stable_for_ns: u64,
) -> Result<ScreenWaitOutcome> {
    let compiled = compile_screen_predicate(&predicate)
        .map_err(|reason| return Error::InvalidOperation { reason })?;

    let (state, source, presented) =
        resolved_screen_state_for_session(session, StatePoint::LatestPresented, false)?;

    let frame = project_screen_frame(
        &state,
        &ScreenProjectionRequest::default(),
        source,
        presented,
        CaptureQuality::Completed,
    )
    .map_err(Error::ScreenFrame)?;

    let viewport_text = state_text(&state);
    let matched = evaluate_screen_predicate(&compiled, &state, &viewport_text, &frame.state_hash);
    let observed_monotonic_ns = match &frame.source {
        ScreenFrameSource::Actor {
            observed_monotonic_ns,
            ..
        } => *observed_monotonic_ns,
        ScreenFrameSource::Resolved { monotonic_ns, .. } => monotonic_ns.unwrap_or_default(),
    };

    return Ok(ScreenWaitOutcome {
        predicate,
        matched,
        first_match_monotonic_ns: None,
        observed_monotonic_ns,
        stable_for_ns,
        quality: ScreenWaitQuality::ChildCompleted,
        frame,
    });
}
