use super::control::client_error;
use super::{
    completed_semantic_state_with_hash_for_session, observation_from_live_snapshot,
    observe_completed_session_with_options, semantic_state_from_live_snapshot, ControlCommand,
    ControlData, Duration, Observation, Path, Result, StatePoint,
};
use termtrace_control::{read_completed, request_or_completed, Reached};
use termtrace_model::terminal::state::SemanticState;

const COMPLETION_TRANSITION_TIMEOUT: Duration = Duration::from_secs(2);

/// Take a live snapshot, or `None` once the session has completed.
fn live_snapshot(session: &Path) -> Result<Option<ControlData>> {
    return match request_or_completed(
        session,
        ControlCommand::Snapshot,
        COMPLETION_TRANSITION_TIMEOUT,
    )? {
        Reached::Live(snapshot) => Ok(Some(*snapshot)),
        Reached::Completed(_) => Ok(None),
    };
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
    if read_completed(session)?.is_some() {
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

    let Some(snapshot) = live_snapshot(session)? else {
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
    if read_completed(session)?.is_some() {
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

    let Some(snapshot) = live_snapshot(session)? else {
        return Ok(completed_semantic_state_with_hash_for_session(
            session,
            point,
            include_hidden,
        )?);
    };

    return Ok(semantic_state_from_live_snapshot(snapshot)?);
}
