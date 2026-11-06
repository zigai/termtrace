use super::{
    frame_from_state, frame_hash, load_checkpoint, observation_error, publish_checkpoint,
    publish_endpoint_checkpoint, CachedSemanticState,
};
use crate::completion;
use crate::replay::{
    inspect_latest_presented_prefix, inspect_latest_presented_stream, inspect_prefix_stream,
    inspect_session_stream, inspect_session_stream_with_formats,
    trust::{check_cancelled, SourceIdentity, ValidatedSession},
    EndpointInspection, RenderedStateFormats,
};
use crate::{Error, Result};
use std::{
    io::Read as _,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use termtrace_evidence::{
    event_log::{CanonicalBoundary, EventLogCursor, EventLogSummary},
    session::{canonical_manifest_bytes, manifest_hash, read_completion, read_manifest},
};
use termtrace_model::{
    execution::event::Event,
    execution::layout::SessionLayout,
    execution::session::SessionCompletion,
    protocol::control::ControlPayload,
    terminal::observation::{
        CaptureQuality, Observation, ObservationFrame, ObservationIntegrity, ObservationResolution,
        ObservationSourceKind, StatePoint, TruthLayer,
    },
    terminal::replay::{ReplayEndpoint, StateSelector},
    terminal::screen::ScreenFrameSource,
    terminal::state::SemanticState,
    value::ByteString,
};
use termtrace_terminal::Format;
use zstd::stream::read::Decoder;

const MAX_LIVE_SEMANTIC_STATE_BYTES: u64 = 32 * 1024 * 1024;

/// Resolve the latest presented state within an explicitly captured canonical prefix.
///
/// The caller may obtain a durable boundary from the live actor. The complete prefix is
/// verified against that boundary, including its output-byte count and manifest identity.
/// Later output and incomplete trailing writes are outside this snapshot.
///
/// # Errors
/// Returns validation, replay, missing-prefix, or cancellation failures.
pub fn replay_endpoint_for_prefix(
    session: &Path,
    boundary: &CanonicalBoundary,
    cancelled: &AtomicBool,
) -> Result<ReplayEndpoint> {
    let (header, state) = inspect_latest_presented_prefix(
        &SessionLayout::new(session).event_log(),
        boundary,
        cancelled,
    )?;

    let manifest = read_manifest(session)?;
    if header.session_id != manifest.session_id
        || header.session_manifest_hash != manifest_hash(&canonical_manifest_bytes(&manifest)?)
    {
        return Err(observation_error(
            "prefix manifest does not match its canonical header",
        ));
    }
    return Ok(ReplayEndpoint {
        session_id: header.session_id,
        session_manifest_hash: header.session_manifest_hash,
        source_complete: false,
        boundary: boundary.clone(),
        state_id: state.state_id,
        state_hash: state.state_hash,
        cols: state.state.cols,
        rows: state.state.rows,
        event_seq: state.event_seq,
        event_byte_offset: state.event_byte_offset,
        monotonic_ns: state.monotonic_ns,
    });
}

/// Resolve the exact byte endpoint for a native replay of a completed state.
///
/// This retains one semantic state and uses the same authenticated caches as observation.
/// Hidden synchronized-output states are rejected.
///
/// # Errors
/// Returns source validation, selector, replay, or cancellation failures.
pub fn replay_endpoint_for_session(
    session: &Path,
    point: &StatePoint,
    cancelled: &AtomicBool,
) -> Result<ReplayEndpoint> {
    return presented_replay_endpoint_for_session(session, point, cancelled)?.ok_or_else(|| {
        return observation_error("native replay requires a presentation eligible endpoint");
    });
}

/// Resolve a completed replay endpoint, returning absence for a valid hidden state.
///
/// # Errors
/// Returns source validation, selector, replay, or cancellation failures.
pub fn presented_replay_endpoint_for_session(
    session: &Path,
    point: &StatePoint,
    cancelled: &AtomicBool,
) -> Result<Option<ReplayEndpoint>> {
    let (state, _, completion) = resolve_completed_state(session, point, Some(cancelled))?;
    if !state.presented {
        return Ok(None);
    }
    return Ok(Some(ReplayEndpoint {
        session_id: completion.session_id,
        session_manifest_hash: completion.session_manifest_hash,
        source_complete: true,
        boundary: CanonicalBoundary {
            next_event_seq: completion.event_count,
            event_hash: completion.final_event_hash,
            output_bytes: completion.output_bytes,
        },
        state_id: state.state_id,
        state_hash: state.state_hash,
        cols: state.state.cols,
        rows: state.state.rows,
        event_seq: state.event_seq,
        event_byte_offset: state.event_byte_offset,
        monotonic_ns: state.monotonic_ns,
    }));
}

/// Observe a validated completed session.
///
/// # Errors
///
/// Returns an error when canonical identity/validation fails, completion is
/// absent, or replay cannot resolve the point.
pub fn observe_session(session: &Path, state_point: StatePoint) -> Result<Observation> {
    return observe_session_with_options(session, state_point, false);
}

/// Observe a session with explicit permission to select hidden semantic states.
///
/// # Errors
///
/// Returns the same errors as [`observe_session`] and rejects a hidden selected
/// state unless `include_hidden` is true.
pub fn observe_session_with_options(
    session: &Path,
    state_point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    if completion::published(&canonical)?.is_none() {
        return Err(observation_error(
            "session is live; engine observation requires completed evidence or explicit validated-prefix observation",
        ));
    }
    return observe_completed(&canonical, state_point, include_hidden);
}

/// Observe a completed session and format the selected state in the same replay.
///
/// # Errors
///
/// Returns the same errors as [`observe_session_with_options`] and fails when
/// any requested formatter cannot produce output.
pub fn observe_completed_with_formats(
    session: &Path,
    state_point: StatePoint,
    include_hidden: bool,
    formats: &[Format],
) -> Result<(Observation, RenderedStateFormats)> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    let validated = ValidatedSession::open(&canonical)?;
    let selector = state_selector_for_session(&canonical, &state_point, None)?;
    let (inspection, rendered) =
        inspect_session_stream_with_formats(&canonical, selector, formats)?;

    validated.check()?;

    let state = CachedSemanticState {
        state_id: inspection.record.state_id,
        event_seq: inspection.record.event_seq,
        event_byte_offset: inspection.record.event_byte_offset,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        state_hash: inspection.record.state_hash,
        presented: inspection.record.presented,
        state: inspection.state,
    };
    let observation = completed_observation_from_cached(
        &canonical,
        state_point,
        validated.completion(),
        &state,
        include_hidden,
        ObservationResolution::CanonicalReplay,
    )?;

    return Ok((observation, rendered));
}

/// Build a truth-labeled observation from one actor-owned live snapshot.
///
/// # Errors
///
/// Returns an error when the session identity is invalid or the control payload
/// is not a live snapshot.
pub fn observation_from_live_snapshot(
    session: &Path,
    snapshot: ControlPayload,
) -> Result<Observation> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    let manifest = read_manifest(&canonical)?;
    let ControlPayload::Snapshot {
        semantic_state_zstd: _,
        viewport_text,
        state_hash,
        presented,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        modes,
        observed_monotonic_ns,
        boundaries,
    } = snapshot
    else {
        return Err(observation_error(
            "actor returned an unexpected observation",
        ));
    };
    let frame = ObservationFrame {
        viewport_text,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        modes,
    };
    let payload_hash = frame_hash(&frame)?;

    return Ok(Observation {
        source_kind: ObservationSourceKind::LiveSession,
        session_id: Some(manifest.session_id),
        canonical_session: Some(ByteString::from(canonical.as_os_str().as_bytes())),
        source_completed: false,
        state_point: StatePoint::LatestPresented,
        state_id: None,
        state_hash,
        event_seq: boundaries.observed.next_event_seq.checked_sub(1),
        monotonic_ns: Some(observed_monotonic_ns),
        presented,
        boundary: boundaries.observed,
        capture_quality: CaptureQuality::Live,
        truth_layer: TruthLayer::Semantic,
        integrity: ObservationIntegrity::CanonicalChained,
        resolution: ObservationResolution::ActorSnapshot,
        frame,
        payload_hash,
    });
}

/// Observe an explicitly requested validated incomplete canonical prefix.
///
/// # Errors
///
/// Returns an error when completion exists, the event prefix is invalid, the
/// selector cannot resolve, or a hidden state was not explicitly permitted.
pub fn observe_validated_prefix(
    session: &Path,
    point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    let layout = SessionLayout::new(&canonical);

    if completion::published(&canonical)?.is_some() {
        return Err(observation_error(
            "validated-prefix observation requires incomplete evidence",
        ));
    }

    let manifest = read_manifest(&canonical)?;
    let source = SourceIdentity::capture_candidate(&canonical)?;
    let log = EventLogSummary::snapshot(layout.event_log())?;
    let inspection = inspect_prefix_stream(
        &canonical,
        state_selector_for_session(&canonical, &point, None)?,
    )?;

    if source != SourceIdentity::capture_candidate(&canonical)?
        || completion::published(&canonical)?.is_some()
    {
        return Err(observation_error(
            "canonical prefix changed during observation",
        ));
    }

    if !inspection.record.presented && !include_hidden {
        return Err(observation_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }

    let frame = frame_from_state(&inspection.state);
    let payload_hash = frame_hash(&frame)?;
    let output_bytes = log.output_bytes;
    let event_hash = match &log.terminal {
        Some(record) => record.hash.clone(),
        None => {
            let bytes = serde_json::to_vec(&log.header).map_err(|source| {
                return Error::InvalidJson {
                    path: PathBuf::from("<event-header>"),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;

            blake3::hash(&bytes).to_hex().to_string()
        }
    };

    return Ok(Observation {
        source_kind: ObservationSourceKind::InterruptedSession,
        session_id: Some(manifest.session_id),
        canonical_session: Some(ByteString::from(canonical.as_os_str().as_bytes())),
        source_completed: false,
        state_point: point,
        state_id: Some(inspection.record.state_id),
        state_hash: inspection.record.state_hash,
        event_seq: inspection.record.event_seq,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        presented: inspection.record.presented,
        boundary: CanonicalBoundary {
            next_event_seq: log.event_count,
            event_hash,
            output_bytes,
        },
        capture_quality: CaptureQuality::Incomplete,
        truth_layer: TruthLayer::Semantic,
        integrity: ObservationIntegrity::CanonicalChained,
        resolution: ObservationResolution::CanonicalReplay,
        frame,
        payload_hash,
    });
}

fn observe_completed(
    session: &Path,
    point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let (state, resolution, completion) = resolve_completed_state(session, &point, None)?;

    return completed_observation_from_cached(
        session,
        point,
        &completion,
        &state,
        include_hidden,
        resolution,
    );
}

/// Resolve one completed semantic state together with its canonical rich-frame source.
///
/// # Errors
///
/// Returns an error when the session is incomplete, replay fails, or the selected
/// state is hidden without explicit permission.
pub fn resolved_screen_state_for_session(
    session: &Path,
    point: StatePoint,
    include_hidden: bool,
) -> Result<(SemanticState, ScreenFrameSource, bool)> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    let (state, resolution, completion) = resolve_completed_state(&canonical, &point, None)?;
    if !state.presented && !include_hidden {
        return Err(observation_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }

    let source = ScreenFrameSource::Resolved {
        source_kind: ObservationSourceKind::CompletedSession,
        session_id: Some(completion.session_id),
        source_completed: true,
        state_point: point,
        state_id: Some(state.state_id),
        event_seq: state.event_seq,
        monotonic_ns: state.monotonic_ns,
        boundary: CanonicalBoundary {
            next_event_seq: completion.event_count,
            event_hash: completion.final_event_hash,
            output_bytes: completion.output_bytes,
        },
        integrity: ObservationIntegrity::CanonicalChained,
        resolution,
    };

    return Ok((state.state, source, state.presented));
}

/// Resolve a completed state under the one trust rule for derived state.
///
/// A replay checkpoint short-circuits validation only when its local trust record was minted
/// from a [`ValidatedSession`] for the session's current source identity. Otherwise the session
/// is validated canonically and replayed, and the result is published as a new checkpoint.
fn resolve_completed_state(
    session: &Path,
    point: &StatePoint,
    cancelled: Option<&AtomicBool>,
) -> Result<(
    CachedSemanticState,
    ObservationResolution,
    SessionCompletion,
)> {
    check_cancelled(cancelled)?;

    let completion = read_completion(session)?;
    if let Some(state) = load_checkpoint(session, point, &completion)? {
        return Ok((
            state,
            ObservationResolution::ValidatedCheckpoint,
            completion,
        ));
    }

    if matches!(point, StatePoint::StateHash { .. })
        && let Some(state) = load_checkpoint(session, &StatePoint::LatestPresented, &completion)?
        && matches!(
            point,
            StatePoint::StateHash { state_hash } if state.state_hash == *state_hash
        )
    {
        return Ok((
            state,
            ObservationResolution::ValidatedCheckpoint,
            completion,
        ));
    }

    let validated = ValidatedSession::open_cancellable(session, cancelled)?;

    if matches!(
        point,
        StatePoint::LatestPresented | StatePoint::StateHash { .. }
    ) {
        let endpoint =
            inspect_latest_presented_stream(&SessionLayout::new(session).event_log(), cancelled)?;

        validated.check()?;

        if let Err(error) =
            publish_endpoint_checkpoint(&validated, &StatePoint::LatestPresented, &endpoint)
        {
            tracing::warn!(%error, "could not publish endpoint replay checkpoint");
        }

        if matches!(point, StatePoint::LatestPresented)
            || matches!(
                point,
                StatePoint::StateHash { state_hash } if endpoint.state_hash == *state_hash
            )
        {
            return Ok((
                cached_state_from_endpoint(endpoint),
                ObservationResolution::CanonicalReplay,
                validated.completion().clone(),
            ));
        }
    }

    let selector = state_selector_for_session(session, point, cancelled)?;
    let inspection = inspect_session_stream(session, selector, cancelled)?;
    validated.check()?;

    if let Err(error) = publish_checkpoint(&validated, point, &inspection) {
        tracing::warn!(%error, "could not publish disposable replay checkpoint");
    }

    return Ok((
        CachedSemanticState {
            state_id: inspection.record.state_id,
            event_seq: inspection.record.event_seq,
            event_byte_offset: inspection.record.event_byte_offset,
            monotonic_ns: Some(inspection.record.monotonic_ns),
            state_hash: inspection.record.state_hash,
            presented: inspection.record.presented,
            state: inspection.state,
        },
        ObservationResolution::CanonicalReplay,
        validated.completion().clone(),
    ));
}

fn cached_state_from_endpoint(state: EndpointInspection) -> CachedSemanticState {
    return CachedSemanticState {
        state_id: state.state_id,
        event_seq: state.event_seq,
        event_byte_offset: state.event_byte_offset,
        monotonic_ns: state.monotonic_ns,
        state_hash: state.state_hash,
        presented: state.presented,
        state: state.state,
    };
}

fn completed_observation_from_cached(
    session: &Path,
    point: StatePoint,
    completion: &SessionCompletion,
    state: &CachedSemanticState,
    include_hidden: bool,
    resolution: ObservationResolution,
) -> Result<Observation> {
    if !state.presented && !include_hidden {
        return Err(observation_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }

    let frame = frame_from_state(&state.state);
    let payload_hash = frame_hash(&frame)?;

    return Ok(Observation {
        source_kind: ObservationSourceKind::CompletedSession,
        session_id: Some(completion.session_id),
        canonical_session: Some(ByteString::from(session.as_os_str().as_bytes())),
        source_completed: true,
        state_point: point,
        state_id: Some(state.state_id),
        state_hash: state.state_hash.clone(),
        event_seq: state.event_seq,
        monotonic_ns: state.monotonic_ns,
        presented: state.presented,
        boundary: CanonicalBoundary {
            next_event_seq: completion.event_count,
            event_hash: completion.final_event_hash.clone(),
            output_bytes: completion.output_bytes,
        },
        capture_quality: CaptureQuality::Completed,
        truth_layer: TruthLayer::Semantic,
        integrity: ObservationIntegrity::CanonicalChained,
        resolution,
        frame,
        payload_hash,
    });
}

/// Resolve a state point without retaining canonical history.
///
/// Numeric selectors are translated directly; marker selection validates the log
/// and requires a unique label. The caller establishes the session completion policy.
///
/// # Errors
/// Rejects missing/ambiguous markers, changing logs, and cancellation.
pub fn state_selector_for_session(
    session: &Path,
    point: &StatePoint,
    cancelled: Option<&AtomicBool>,
) -> Result<StateSelector> {
    return point.selector(|label| return unique_marker_event_seq(session, label, cancelled));
}

fn unique_marker_event_seq(
    session: &Path,
    label: &str,
    cancelled: Option<&AtomicBool>,
) -> Result<u64> {
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;
    let mut found = None;

    while let Some(record) = cursor.next_record()? {
        check_cancelled(cancelled)?;

        if matches!(&record.event, Event::Marker { label: value, .. } if value == label)
            && found.replace(record.seq).is_some()
        {
            return Err(observation_error("marker label is ambiguous"));
        }
    }

    let _pending = cursor.finish()?;

    return found.ok_or_else(|| return observation_error("marker does not exist"));
}

/// Resolve the complete semantic state behind a completed session observation.
///
/// # Errors
///
/// Returns an error when the source is invalid or incomplete, or hidden-state
/// permission is absent.
pub fn semantic_state_for_session(
    session: &Path,
    point: &StatePoint,
    include_hidden: bool,
) -> Result<SemanticState> {
    let (state, _) = semantic_state_with_hash_for_session(session, point, include_hidden)?;

    return Ok(state);
}

/// Resolve a complete semantic state with its exact canonical replay hash.
///
/// # Errors
///
/// Returns an error when the source is invalid or incomplete, or hidden-state
/// permission is absent.
pub fn semantic_state_with_hash_for_session(
    session: &Path,
    point: &StatePoint,
    include_hidden: bool,
) -> Result<(SemanticState, String)> {
    return semantic_state_with_hash_for_session_inner(session, point, include_hidden, None);
}

/// Resolve semantic evidence with cooperative cancellation during validation and replay.
///
/// # Errors
///
/// Returns source/selection errors or cancellation without publishing partial cache entries.
pub fn semantic_state_with_hash_for_session_cancellable(
    session: &Path,
    point: &StatePoint,
    include_hidden: bool,
    cancelled: &AtomicBool,
) -> Result<(SemanticState, String)> {
    return semantic_state_with_hash_for_session_inner(
        session,
        point,
        include_hidden,
        Some(cancelled),
    );
}

fn semantic_state_with_hash_for_session_inner(
    session: &Path,
    point: &StatePoint,
    include_hidden: bool,
    cancelled: Option<&AtomicBool>,
) -> Result<(SemanticState, String)> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    if completion::published(&canonical)?.is_none() {
        return Err(observation_error(
            "session is live; semantic state resolution requires completed evidence",
        ));
    }

    let (state, _, _) = resolve_completed_state(&canonical, point, cancelled)?;
    if !state.presented && !include_hidden {
        return Err(observation_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }
    return Ok((state.state, state.state_hash));
}

/// Decode the complete semantic state carried by one actor-owned live snapshot.
///
/// # Errors
///
/// Returns an error when the control payload is not a snapshot, decompression
/// fails, the decoded state exceeds 32 MiB, or its JSON is invalid.
pub fn semantic_state_from_live_snapshot(
    snapshot: ControlPayload,
) -> Result<(SemanticState, String)> {
    let ControlPayload::Snapshot {
        semantic_state_zstd,
        state_hash,
        ..
    } = snapshot
    else {
        return Err(observation_error(
            "actor returned an unexpected semantic snapshot",
        ));
    };
    let decoder = Decoder::new(semantic_state_zstd.as_bytes()).map_err(|error| {
        return observation_error(format!("decompress live semantic snapshot: {error}"));
    })?;

    let mut decoded: Vec<u8> = Vec::new();
    decoder
        .take(MAX_LIVE_SEMANTIC_STATE_BYTES.saturating_add(1))
        .read_to_end(&mut decoded)
        .map_err(|error| {
            return observation_error(format!("decompress live semantic snapshot: {error}"));
        })?;

    if u64::try_from(decoded.len()).unwrap_or(u64::MAX) > MAX_LIVE_SEMANTIC_STATE_BYTES {
        return Err(observation_error(
            "live semantic snapshot exceeds the 32 MiB decoded limit",
        ));
    }

    let state = serde_json::from_slice(&decoded).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<live-semantic-snapshot>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    return Ok((state, state_hash));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::repeat;
    use termtrace_model::{
        execution::event_log::CanonicalBoundary, terminal::observation::ActorBoundaries,
        terminal::state::ModeState, value::ByteString,
    };
    use zstd::stream::encode_all;

    #[test]
    fn live_snapshot_decompression_stops_at_decoded_limit() {
        let oversized = repeat(b' ').take(MAX_LIVE_SEMANTIC_STATE_BYTES.saturating_add(1));
        let compressed = encode_all(oversized, 1).expect("compress oversized state");
        let boundary = CanonicalBoundary {
            next_event_seq: 0,
            event_hash: "0".repeat(64),
            output_bytes: 0,
        };
        let snapshot = ControlPayload::Snapshot {
            semantic_state_zstd: ByteString::from(compressed),
            viewport_text: String::new(),
            state_hash: "0".repeat(64),
            presented: true,
            cols: 1,
            rows: 1,
            cursor_column: None,
            cursor_row: None,
            cursor_visible: false,
            modes: ModeState {
                synchronized_output: false,
                alternate_screen: false,
                wraparound: true,
                bracketed_paste: false,
                values: Vec::new(),
            },
            observed_monotonic_ns: 0,
            boundaries: ActorBoundaries {
                durable: boundary.clone(),
                observed: boundary,
            },
        };
        let error = semantic_state_from_live_snapshot(snapshot)
            .expect_err("oversized live semantic state must be rejected");
        assert!(error
            .to_string()
            .contains("exceeds the 32 MiB decoded limit"));
    }
}
