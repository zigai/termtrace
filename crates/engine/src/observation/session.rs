use std::{
    io::Read as _,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
};

use uuid::Uuid;
use zstd::stream::read::Decoder;

use termtrace_evidence::{
    event_log::{CanonicalBoundary, EventLog},
    session::{read_completion, read_manifest, validate_completed_log},
};
use termtrace_model::{
    execution::event::Event,
    execution::session::SessionCompletion,
    protocol::control::ControlData,
    terminal::observation::{
        CaptureQuality, Observation, ObservationFrame, ObservationIntegrity, ObservationResolution,
        ObservationSourceKind, StatePoint, TruthLayer,
    },
    terminal::replay::StateSelector,
    terminal::screen::ScreenFrameSource,
    terminal::state::SemanticState,
    value::ByteString,
};
use termtrace_terminal::Format;

use super::{
    CachedSemanticState, frame_from_state, frame_hash, load_checkpoint, observation_error,
    publish_checkpoint, publish_endpoint_checkpoint,
};
use crate::replay::{
    EndpointInspection, RenderedStateFormats, inspect, inspect_latest_presented_fast,
    inspect_with_formats,
};
use crate::{Error, Result};

const MAX_LIVE_SEMANTIC_STATE_BYTES: u64 = 32 * 1024 * 1024;

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
    let manifest = read_manifest(&canonical)?;
    if !canonical.join("completion.json").is_file() {
        return Err(observation_error(
            "session is live; engine observation requires completed evidence or explicit validated-prefix observation",
        ));
    }
    return observe_completed(&canonical, manifest.session_id, state_point, include_hidden);
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
    let manifest = read_manifest(&canonical)?;
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let selector = state_selector(&log, &state_point)?;
    let (inspection, rendered) = inspect_with_formats(&log, selector, formats)?;
    let state = CachedSemanticState {
        state_id: Some(inspection.record.state_id),
        event_seq: inspection.record.event_seq,
        event_byte_offset: inspection.record.event_byte_offset,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        state_hash: inspection.record.state_hash,
        presented: inspection.record.presented,
        state: inspection.state,
    };
    let observation = completed_observation_from_cached(
        &canonical,
        manifest.session_id,
        state_point,
        &completion,
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
    snapshot: ControlData,
) -> Result<Observation> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    let manifest = read_manifest(&canonical)?;
    let ControlData::Snapshot {
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
    if canonical.join("completion.json").exists() {
        return Err(observation_error(
            "validated-prefix observation requires incomplete evidence",
        ));
    }
    let manifest = read_manifest(&canonical)?;
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let inspection = inspect(&log, state_selector(&log, &point)?)?;
    if !inspection.record.presented && !include_hidden {
        return Err(observation_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }
    let frame = frame_from_state(&inspection.state);
    let payload_hash = frame_hash(&frame)?;
    let output_bytes = log.events.iter().fold(0_u64, |total, record| {
        return total.saturating_add(match &record.event {
            Event::Output { data } => u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX),
            _ => 0,
        });
    });
    let event_hash = match log.events.last() {
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
            next_event_seq: u64::try_from(log.events.len()).unwrap_or(u64::MAX),
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
    session_id: Uuid,
    point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let completion = read_completion(session)?;
    let (state, resolution) = resolve_completed_state(session, &point, &completion)?;
    return completed_observation_from_cached(
        session,
        session_id,
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
    let manifest = read_manifest(&canonical)?;
    let completion = read_completion(&canonical)?;
    let (state, resolution) = resolve_completed_state(&canonical, &point, &completion)?;
    if !state.presented && !include_hidden {
        return Err(observation_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }
    let source = ScreenFrameSource::Resolved {
        source_kind: ObservationSourceKind::CompletedSession,
        session_id: Some(manifest.session_id),
        source_completed: true,
        state_point: point,
        state_id: state.state_id,
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

fn resolve_completed_state(
    session: &Path,
    point: &StatePoint,
    completion: &SessionCompletion,
) -> Result<(CachedSemanticState, ObservationResolution)> {
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(session, &log)?;
    if let Some(state) = load_checkpoint(session, point, completion)? {
        return Ok((state, ObservationResolution::ValidatedCheckpoint));
    }
    if matches!(point, StatePoint::StateHash { .. })
        && let Some(state) = load_checkpoint(session, &StatePoint::LatestPresented, completion)?
        && matches!(
            point,
            StatePoint::StateHash { state_hash } if state.state_hash == *state_hash
        )
    {
        return Ok((state, ObservationResolution::ValidatedCheckpoint));
    }
    if matches!(
        point,
        StatePoint::LatestPresented | StatePoint::StateHash { .. }
    ) {
        let endpoint = inspect_latest_presented_fast(&log)?;
        if let Err(error) = publish_endpoint_checkpoint(
            session,
            &StatePoint::LatestPresented,
            completion,
            &endpoint,
        ) {
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
            ));
        }
    }
    let selector = state_selector(&log, point)?;
    let inspection = inspect(&log, selector)?;
    if let Err(error) = publish_checkpoint(session, point, completion, &inspection) {
        tracing::warn!(%error, "could not publish disposable replay checkpoint");
    }
    return Ok((
        CachedSemanticState {
            state_id: Some(inspection.record.state_id),
            event_seq: inspection.record.event_seq,
            event_byte_offset: inspection.record.event_byte_offset,
            monotonic_ns: Some(inspection.record.monotonic_ns),
            state_hash: inspection.record.state_hash,
            presented: inspection.record.presented,
            state: inspection.state,
        },
        ObservationResolution::CanonicalReplay,
    ));
}

fn cached_state_from_endpoint(state: EndpointInspection) -> CachedSemanticState {
    return CachedSemanticState {
        state_id: None,
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
    session_id: Uuid,
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
        session_id: Some(session_id),
        canonical_session: Some(ByteString::from(session.as_os_str().as_bytes())),
        source_completed: true,
        state_point: point,
        state_id: state.state_id,
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

fn state_selector(log: &EventLog, point: &StatePoint) -> Result<StateSelector> {
    let selector = match point {
        StatePoint::LatestPresented => StateSelector::LatestPresented,
        StatePoint::StateId { state_id } => StateSelector::StateId(*state_id),
        StatePoint::StateHash { state_hash } => StateSelector::StateHash(state_hash.clone()),
        StatePoint::EventSeq { event_seq } => StateSelector::EventSeq(*event_seq),
        StatePoint::MonotonicNs { monotonic_ns } => StateSelector::MonotonicNs(*monotonic_ns),
        StatePoint::Marker { label } => {
            let mut matches = log.events.iter().filter(|record| {
                return matches!(&record.event, Event::Marker { label: value, .. } if value == label);
            });
            let event_seq = matches
                .next()
                .map(|record| return record.seq)
                .ok_or_else(|| return observation_error("marker does not exist"))?;
            if matches.next().is_some() {
                return Err(observation_error("marker label is ambiguous"));
            }
            StateSelector::EventSeq(event_seq)
        }
    };
    return Ok(selector);
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
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    if !canonical.join("completion.json").is_file() {
        return Err(observation_error(
            "session is live; semantic state resolution requires completed evidence",
        ));
    }
    let completion = read_completion(&canonical)?;
    let (state, _) = resolve_completed_state(&canonical, point, &completion)?;
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
pub fn semantic_state_from_live_snapshot(snapshot: ControlData) -> Result<(SemanticState, String)> {
    let ControlData::Snapshot {
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

/// Observe a completed canonical session created for a one-off command.
///
/// # Errors
///
/// Returns an error when the session is incomplete, invalid, or the requested
/// state cannot be resolved.
pub fn observe_one_off_session(
    session: &Path,
    state_point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let mut observation = observe_session_with_options(session, state_point, include_hidden)?;
    if !observation.source_completed {
        return Err(observation_error(
            "one-off command did not produce completed canonical evidence",
        ));
    }
    observation.source_kind = ObservationSourceKind::OneOffCommand;
    return Ok(observation);
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, repeat};

    use termtrace_model::{
        execution::event_log::CanonicalBoundary, terminal::observation::ActorBoundaries,
        terminal::state::ModeState, value::ByteString,
    };
    use zstd::stream::encode_all;

    use super::*;

    #[test]
    fn live_snapshot_decompression_stops_at_decoded_limit() {
        let oversized = repeat(b' ').take(MAX_LIVE_SEMANTIC_STATE_BYTES.saturating_add(1));
        let compressed = encode_all(oversized, 1).expect("compress oversized state");
        let boundary = CanonicalBoundary {
            next_event_seq: 0,
            event_hash: "0".repeat(64),
            output_bytes: 0,
        };
        let snapshot = ControlData::Snapshot {
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
        assert!(
            error
                .to_string()
                .contains("exceeds the 32 MiB decoded limit")
        );
    }
}
