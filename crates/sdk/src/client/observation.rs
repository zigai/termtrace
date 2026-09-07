use super::control::client_error;
use super::{
    ControlCommand, ControlData, ControlRequest, Duration, Error, Instant, Observation, Path,
    Result, StatePoint, completed_semantic_state_with_hash_for_session,
    observation_from_live_snapshot, observe_completed_session_with_options,
    semantic_state_from_live_snapshot, send_request, thread,
};
use termtrace_model::terminal::state::SemanticState;

const COMPLETION_TRANSITION_TIMEOUT: Duration = Duration::from_secs(2);
const COMPLETION_TRANSITION_POLL_INTERVAL: Duration = Duration::from_millis(10);

fn request_snapshot_across_completion(session: &Path) -> Result<Option<ControlData>> {
    match send_request(session, &ControlRequest::new(ControlCommand::Snapshot)) {
        Ok(snapshot) => return Ok(Some(snapshot)),
        Err(error)
            if matches!(
                &error,
                Error::ControlUnavailable { .. }
                    | Error::ControlTimeout { .. }
                    | Error::ControlTransport { .. }
            ) =>
        {
            let completion = session.join("completion.json");
            let deadline = Instant::now() + COMPLETION_TRANSITION_TIMEOUT;
            loop {
                if completion.is_file() {
                    return Ok(None);
                }
                let now = Instant::now();
                if now >= deadline {
                    return Err(error);
                }
                thread::sleep(
                    COMPLETION_TRANSITION_POLL_INTERVAL
                        .min(deadline.saturating_duration_since(now)),
                );
            }
        }
        Err(error) => return Err(error),
    }
}

/// Observe a live actor snapshot or a validated completed semantic state.
///
/// # Errors
///
/// Returns an error when the session is invalid, a live selector is unsupported,
/// actor observation fails, or completed replay cannot resolve the point.
pub fn observe_session(session: impl AsRef<Path>, state_point: StatePoint) -> Result<Observation> {
    return observe_session_with_options(session, state_point, false);
}

/// Observe a live actor snapshot or completed state with hidden-state permission.
///
/// # Errors
///
/// Returns the same errors as [`observe_session`] and rejects unsupported live
/// selectors.
pub fn observe_session_with_options(
    session: impl AsRef<Path>,
    state_point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let session = session.as_ref();
    if session.join("completion.json").is_file() {
        return Ok(observe_completed_session_with_options(
            session,
            state_point,
            include_hidden,
        )?);
    }
    if state_point != StatePoint::LatestPresented {
        return Err(client_error(
            "live sessions currently support only latest-presented selection",
        ));
    }
    let Some(snapshot) = request_snapshot_across_completion(session)? else {
        return Ok(observe_completed_session_with_options(
            session,
            state_point,
            include_hidden,
        )?);
    };
    return Ok(observation_from_live_snapshot(session, snapshot)?);
}

/// Resolve the complete semantic state behind a live or completed session.
///
/// # Errors
///
/// Returns an error when the session is invalid, the live selector is
/// unsupported, actor observation fails, or replay cannot resolve the point.
pub fn semantic_state_for_session(
    session: impl AsRef<Path>,
    point: &StatePoint,
    include_hidden: bool,
) -> Result<SemanticState> {
    let (state, _) = semantic_state_with_hash_for_session(session, point, include_hidden)?;
    return Ok(state);
}

/// Resolve a complete semantic state with the exact hash assigned by its source.
///
/// # Errors
///
/// Returns the same errors as [`semantic_state_for_session`].
pub fn semantic_state_with_hash_for_session(
    session: impl AsRef<Path>,
    point: &StatePoint,
    include_hidden: bool,
) -> Result<(SemanticState, String)> {
    let session = session.as_ref();
    if session.join("completion.json").is_file() {
        return Ok(completed_semantic_state_with_hash_for_session(
            session,
            point,
            include_hidden,
        )?);
    }
    if point != &StatePoint::LatestPresented {
        return Err(client_error(
            "live semantic states support only latest-presented selection",
        ));
    }
    let Some(snapshot) = request_snapshot_across_completion(session)? else {
        return Ok(completed_semantic_state_with_hash_for_session(
            session,
            point,
            include_hidden,
        )?);
    };
    return Ok(semantic_state_from_live_snapshot(snapshot)?);
}
