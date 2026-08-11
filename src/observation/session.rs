use super::*;

/// Observe a responsive live session or validated completed session.
///
/// # Errors
///
/// Returns an error when canonical identity/validation fails, a live selector
/// is not supported, actor observation fails, or replay cannot resolve the point.
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
    if canonical.join("completion.json").is_file() {
        return observe_completed(&canonical, manifest.session_id, state_point, include_hidden);
    }
    if state_point != StatePoint::LatestPresented {
        return Err(observation_error(
            "live sessions currently support only latest-presented selection",
        ));
    }
    let snapshot = request(&canonical, ControlCommand::Snapshot)?;
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
        state_point,
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

fn resolve_completed_state(
    session: &Path,
    point: &StatePoint,
    completion: &SessionCompletion,
) -> Result<(CachedSemanticState, ObservationResolution)> {
    if let Some(state) = load_checkpoint(session, point, completion)? {
        return Ok((state, ObservationResolution::ValidatedCheckpoint));
    }
    if matches!(point, StatePoint::StateHash { .. })
        && let Some(state) = load_checkpoint(session, &StatePoint::LatestPresented, completion)?
        && state_hash_matches_point(&state, point)
    {
        return Ok((state, ObservationResolution::ValidatedCheckpoint));
    }
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(session, &log)?;
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
            || endpoint_hash_matches_point(&endpoint, point)
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

fn state_hash_matches_point(state: &CachedSemanticState, point: &StatePoint) -> bool {
    return matches!(point, StatePoint::StateHash { state_hash } if state.state_hash == *state_hash);
}

fn endpoint_hash_matches_point(state: &EndpointInspection, point: &StatePoint) -> bool {
    return matches!(point, StatePoint::StateHash { state_hash } if state.state_hash == *state_hash);
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

pub(crate) fn completed_observation_from_inspection(
    session: &Path,
    session_id: Uuid,
    point: StatePoint,
    completion: &SessionCompletion,
    inspection: &StateInspection,
    resolution: ObservationResolution,
) -> Result<Observation> {
    let frame = frame_from_state(&inspection.state);
    let payload_hash = frame_hash(&frame)?;
    return Ok(Observation {
        source_kind: ObservationSourceKind::CompletedSession,
        session_id: Some(session_id),
        canonical_session: Some(ByteString::from(session.as_os_str().as_bytes())),
        source_completed: true,
        state_point: point,
        state_id: Some(inspection.record.state_id),
        state_hash: inspection.record.state_hash.clone(),
        event_seq: inspection.record.event_seq,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        presented: inspection.record.presented,
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

pub(crate) fn state_selector(log: &EventLog, point: &StatePoint) -> Result<StateSelector> {
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

/// Resolve the complete semantic state behind a session observation.
///
/// # Errors
///
/// Returns an error when the source is invalid, the selector is unsupported for
/// a live actor, or hidden-state permission is absent.
pub fn semantic_state_for_session(
    session: &Path,
    point: &StatePoint,
    include_hidden: bool,
) -> Result<SemanticState> {
    let (state, _) = semantic_state_with_hash_for_session(session, point, include_hidden)?;
    return Ok(state);
}

/// Resolve a complete semantic state with the exact hash assigned by its source.
///
/// The paired hash preserves the source's hash contract: completed evidence uses
/// its canonical replay hash, while a live actor returns its snapshot hash.
///
/// # Errors
///
/// Returns an error when the source is invalid, the selector is unsupported for
/// a live actor, or hidden-state permission is absent.
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
    if canonical.join("completion.json").is_file() {
        let completion = read_completion(&canonical)?;
        let (state, _) = resolve_completed_state(&canonical, point, &completion)?;
        if !state.presented && !include_hidden {
            return Err(observation_error(
                "selected semantic state is hidden; pass explicit hidden-state permission",
            ));
        }
        return Ok((state.state, state.state_hash));
    }
    if point != &StatePoint::LatestPresented || include_hidden {
        return Err(observation_error(
            "live semantic images support only latest-presented selection",
        ));
    }
    let ControlData::Snapshot {
        semantic_state_zstd,
        state_hash,
        ..
    } = request(&canonical, ControlCommand::Snapshot)?
    else {
        return Err(observation_error(
            "actor returned an unexpected semantic snapshot",
        ));
    };
    let decoded = decode_all(semantic_state_zstd.as_bytes()).map_err(|error| {
        return observation_error(format!("decompress live semantic snapshot: {error}"));
    })?;
    if decoded.len() > 32 * 1024 * 1024 {
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
