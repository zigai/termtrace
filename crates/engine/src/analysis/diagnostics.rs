//! Deterministic evidence-backed explanations and conservative diagnostics.

mod stream;

use crate::{
    observation::state_selector_for_session,
    replay::{
        inspect_completed_session, replay, trust::SourceIdentity, visit_session_states_while,
        visit_states,
    },
    timeline::stream::TokenCursor,
    Error, Result,
};
use std::{
    collections::BTreeMap,
    iter,
    path::{Component, Path, PathBuf},
    sync::atomic::AtomicBool,
};
pub use stream::{diagnose_session, diagnose_snapshot};
use termtrace_evidence::{event_log::EventLog, session::validate_completed_session};
use termtrace_model::{
    artifact::visual::{VideoMapEntry, VisualManifest},
    execution::event::Event,
    terminal::analysis::{
        DiagnoseReport, DiagnosticCheck, DiagnosticFinding, ExplainFinding, ExplainReport,
        FindingConfidence, NativeCorrelation, NativeEvidenceLevel,
    },
    terminal::observation::StatePoint,
    terminal::replay::{ReplayEndpoint, StateRecord},
    terminal::state::{CursorState, DirtyState},
    terminal::timeline::TimelineItem,
    terminal::vt::VtToken,
};
use termtrace_terminal::frame_event_records;
use uuid::Uuid;

fn analysis_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

/// A native manifest whose source binding was validated by its owning media layer.
#[derive(Clone, Debug)]
pub struct ValidatedVisualManifest {
    manifest_path: PathBuf,
    manifest: VisualManifest,
    video_map: Vec<VideoMapEntry>,
    screenshot_state_id: Option<u64>,
}

impl ValidatedVisualManifest {
    /// Validate a model manifest using a directional validator owned by the caller.
    pub fn new(
        manifest_path: PathBuf,
        manifest: VisualManifest,
        video_map: Vec<VideoMapEntry>,
        validate: impl FnOnce(&VisualManifest) -> Result<()>,
    ) -> Result<Self> {
        if manifest_path.as_os_str().is_empty()
            || manifest_path.is_absolute()
            || manifest_path
                .components()
                .any(|component| return !matches!(component, Component::Normal(_)))
        {
            return Err(analysis_error(
                "native correlation manifest path must be a non-empty relative path",
            ));
        }

        validate(&manifest)?;

        return Ok(Self {
            manifest_path,
            screenshot_state_id: manifest.selected_state_id,
            manifest,
            video_map,
        });
    }

    /// Bind a screenshot without a capture-time dense ID to a verified canonical state.
    ///
    /// The media owner verifies the stored source prefix before supplying the resolved
    /// endpoint. This records a derived correlation without changing the source manifest.
    pub fn bind_screenshot_state(
        &mut self,
        state_id: u64,
        canonical: &ReplayEndpoint,
    ) -> Result<()> {
        if self.manifest.capture_kind != "screenshot"
            || self.manifest.session_id != canonical.session_id
            || self.manifest.selected_state_hash.as_deref() != Some(canonical.state_hash.as_str())
            || self.manifest.endpoint_event_seq != canonical.event_seq
            || self.manifest.endpoint_event_byte_offset != canonical.event_byte_offset
        {
            return Err(analysis_error(
                "native screenshot does not match the selected canonical endpoint",
            ));
        }

        self.screenshot_state_id = Some(state_id);
        return Ok(());
    }
}

/// Collect honest direct-frame and clock-scheduled correlations from validated model values.
#[must_use]
pub fn native_correlations(
    log: &EventLog,
    evidence: &[ValidatedVisualManifest],
) -> BTreeMap<u64, Vec<NativeCorrelation>> {
    let mut correlations = BTreeMap::<u64, Vec<NativeCorrelation>>::new();
    visit_native_correlations(log.header.session_id, None, evidence, |correlation| {
        correlations
            .entry(correlation.state_id)
            .or_default()
            .push(correlation);
    });
    return correlations;
}

/// Resolve only one selected semantic state's native correlations.
#[must_use]
pub fn native_correlations_for_state(
    session_id: Uuid,
    state_id: u64,
    evidence: &[ValidatedVisualManifest],
) -> Vec<NativeCorrelation> {
    let mut correlations = Vec::new();
    visit_native_correlations(session_id, Some(state_id), evidence, |correlation| {
        correlations.push(correlation);
    });
    return correlations;
}

pub(super) fn visit_native_correlations(
    session_id: Uuid,
    selected: Option<u64>,
    evidence: &[ValidatedVisualManifest],
    mut visit: impl FnMut(NativeCorrelation),
) {
    for evidence in evidence {
        if evidence.manifest.session_id != session_id {
            continue;
        }

        if let Some(state_id) = evidence.screenshot_state_id
            && selected.is_none_or(|selected| return selected == state_id)
        {
            let level = if evidence.manifest.presentation_evidence == "presentation_proven" {
                NativeEvidenceLevel::FrameObserved
            } else {
                NativeEvidenceLevel::PostEndpointStableCapture
            };

            visit(NativeCorrelation {
                level,
                manifest: evidence.manifest_path.clone(),
                state_id,
                frame: None,
                limitation: if level == NativeEvidenceLevel::FrameObserved {
                    "native presentation and captured frame were causally verified".to_owned()
                } else {
                    "stable native pixels were sampled after parser catch-up; endpoint-corresponding presentation was not proven".to_owned()
                },
            });
        }

        for mapped in &evidence.video_map {
            if selected.is_none_or(|selected| return selected == mapped.state_id)
                && let Some(frame) = mapped.frame
            {
                visit(NativeCorrelation {
                    level: NativeEvidenceLevel::ScheduledForFrame,
                    manifest: evidence.manifest_path.clone(),
                    state_id: mapped.state_id,
                    frame: Some(frame),
                    limitation:
                        "clock-map scheduling does not prove the state appeared in captured pixels"
                            .to_owned(),
                });
            }
        }
    }
}

/// Run the selected deterministic checks over canonical and validated visual evidence.
///
/// An empty `checks` collection enables every available diagnostic check.
///
/// # Errors
///
/// Returns an error when replay or terminal-event framing fails.
pub fn diagnose(
    log: &EventLog,
    checks: Vec<DiagnosticCheck>,
    broad_change_percent: u8,
    visual_evidence: &[ValidatedVisualManifest],
) -> Result<DiagnoseReport> {
    let mut states = Vec::new();
    visit_states(log, |record, _| {
        states.push(record.clone());
        return Ok(());
    })?;

    let tokens = frame_event_records(&log.events)?;

    return Ok(diagnose_states(
        log,
        checks,
        broad_change_percent,
        visual_evidence,
        &states,
        &tokens,
    ));
}

pub(super) fn diagnose_states(
    log: &EventLog,
    mut checks: Vec<DiagnosticCheck>,
    broad_change_percent: u8,
    visual_evidence: &[ValidatedVisualManifest],
    states: &[StateRecord],
    tokens: &[VtToken],
) -> DiagnoseReport {
    checks.sort_unstable();
    checks.dedup();

    if checks.is_empty() {
        checks = DiagnosticCheck::all();
    }

    let mut detected = Vec::new();

    if checks.contains(&DiagnosticCheck::FullRedraw) {
        detect_full_redraw(
            tokens,
            states,
            log.header.cols,
            log.header.rows,
            broad_change_percent,
            &mut detected,
        );
        detect_redundant_full_redraw(states, &mut detected);
    }

    if checks.contains(&DiagnosticCheck::Transient) {
        detect_transients(states, &mut detected);
    }

    if checks.contains(&DiagnosticCheck::Dirty) {
        detect_dirty_discrepancies(states, &mut detected);
    }

    if checks.contains(&DiagnosticCheck::Cursor) {
        detect_cursor_jumps(states, &mut detected);
    }

    detect_churn(&checks, states, &mut detected);

    if checks.contains(&DiagnosticCheck::QueryLoop) {
        detect_query_loops(log, &mut detected);
    }

    let native = native_correlations(log, visual_evidence);

    for finding in &mut detected {
        for state_id in &finding.state_ids {
            if let Some(correlations) = native.get(state_id) {
                finding.native_correlations.extend(correlations.clone());
            } else if states
                .iter()
                .any(|state| return state.state_id == *state_id && state.presented)
            {
                finding.native_correlations.push(NativeCorrelation {
                    level: NativeEvidenceLevel::PresentationEligible,
                    manifest: PathBuf::new(),
                    state_id: *state_id,
                    frame: None,
                    limitation: "semantic presentation eligibility is not native-frame observation"
                        .to_owned(),
                });
            }
        }
    }

    detected.sort_by(|left, right| {
        return left
            .state_ids
            .first()
            .cmp(&right.state_ids.first())
            .then_with(|| return left.code.cmp(&right.code));
    });

    return DiagnoseReport {
        format: "termtrace-diagnose".to_owned(),
        version: 1,
        session_id: log.header.session_id,
        source_final_event_hash: log.events.last().map_or_else(
            || return log.header.session_manifest_hash.clone(),
            |record| return record.hash.clone(),
        ),
        source_event_count: u64::try_from(log.events.len()).unwrap_or(u64::MAX),
        checks,
        broad_change_fraction: format!("{broad_change_percent}/100"),
        findings: detected,
        output_page: None,
    };
}

/// Explain a completed session while retaining only the requested event context.
///
/// # Errors
/// Rejects incomplete or changing sources, unresolved targets, and invalid replay.
pub fn explain_session(
    session: &Path,
    target: StatePoint,
    context_events: u64,
) -> Result<ExplainReport> {
    let source = SourceIdentity::capture(session)?;
    let completion = validate_completed_session(session)?;
    let selector = state_selector_for_session(session, &target, None)?;
    let target_state = inspect_completed_session(session, selector)?.record;
    let target_event = target_state.event_seq.unwrap_or(0);
    let start_event = target_event.saturating_sub(context_events);
    let end_event = target_event.saturating_add(context_events);

    let mut cursor = TokenCursor::new(session, None)?;
    let mut tokens = Vec::new();
    while let Some(entry) = cursor.next()? {
        if let TimelineItem::VtToken { token } = entry.item
            && token.source.iter().any(|segment| {
                return segment.event_seq >= start_event && segment.event_seq <= end_event;
            })
        {
            tokens.push(token);
        }
    }

    let mut states = Vec::new();
    visit_session_states_while(session, &AtomicBool::new(false), |record, _| {
        if record
            .event_seq
            .is_some_and(|sequence| return sequence > end_event)
        {
            return Ok(false);
        }

        if record
            .event_seq
            .is_none_or(|sequence| return sequence >= start_event)
        {
            states.push(record.clone());
        }

        return Ok(true);
    })?;

    source.check(session)?;

    let findings = findings(&tokens, &states);

    return Ok(ExplainReport {
        session_id: completion.session_id,
        source_final_event_hash: completion.final_event_hash,
        source_event_count: completion.event_count,
        target,
        target_state,
        context_events,
        tokens,
        states,
        findings,
        output_page: None,
    });
}

/// Explain exact VT frames and semantic transitions around one state point.
///
/// # Errors
///
/// Returns an error when the target cannot resolve or replay/framing fails.
pub fn explain(log: &EventLog, target: StatePoint, context_events: u64) -> Result<ExplainReport> {
    let replayed = replay(log)?;
    let target_state = resolve_target_record(log, &replayed.states, &target)?.clone();
    let target_event = target_state.event_seq.unwrap_or(0);
    let start_event = target_event.saturating_sub(context_events);
    let end_event = target_event.saturating_add(context_events);

    let tokens = frame_event_records(&log.events)?
        .into_iter()
        .filter(|token| {
            return token.source.iter().any(|segment| {
                return segment.event_seq >= start_event && segment.event_seq <= end_event;
            });
        })
        .collect::<Vec<_>>();

    let states = replayed
        .states
        .into_iter()
        .filter(|state| {
            return state
                .event_seq
                .is_none_or(|sequence| return sequence >= start_event && sequence <= end_event);
        })
        .collect::<Vec<_>>();

    let findings = findings(&tokens, &states);
    let source_event_count = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
    let source_final_event_hash = match log.events.last() {
        Some(record) => record.hash.clone(),
        None => {
            let header = serde_json::to_vec(&log.header).map_err(|source| {
                return Error::InvalidJson {
                    path: PathBuf::from("<event-log-header>"),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;

            blake3::hash(&header).to_hex().to_string()
        }
    };

    return Ok(ExplainReport {
        session_id: log.header.session_id,
        source_final_event_hash,
        source_event_count,
        target,
        target_state,
        context_events,
        tokens,
        states,
        findings,
        output_page: None,
    });
}

fn resolve_target_record<'states>(
    log: &EventLog,
    states: &'states [StateRecord],
    target: &StatePoint,
) -> Result<&'states StateRecord> {
    let marker_event = if let StatePoint::Marker { label } = target {
        let mut matches = log.events.iter().filter(|record| {
            return matches!(&record.event, Event::Marker { label: value, .. } if value == label);
        });
        let event_seq = matches
            .next()
            .map(|record| return record.seq)
            .ok_or_else(|| return analysis_error("marker does not exist"))?;

        if matches.next().is_some() {
            return Err(analysis_error("marker label is ambiguous"));
        }

        Some(event_seq)
    } else {
        None
    };

    return states
        .iter()
        .rev()
        .find(|state| match target {
            StatePoint::LatestPresented => return state.presented,
            StatePoint::StateId { state_id } => return state.state_id == *state_id,
            StatePoint::StateHash { state_hash } => return state.state_hash == *state_hash,
            StatePoint::EventSeq { event_seq } => {
                return state
                    .event_seq
                    .is_none_or(|value| return value <= *event_seq);
            }
            StatePoint::MonotonicNs { monotonic_ns } => {
                return state.monotonic_ns <= *monotonic_ns;
            }
            StatePoint::Marker { .. } => {
                return state
                    .event_seq
                    .is_none_or(|value| return value <= marker_event.unwrap_or(0));
            }
        })
        .ok_or_else(|| return analysis_error("no semantic state matches the target selector"));
}

fn detect_redundant_full_redraw(states: &[StateRecord], findings: &mut Vec<DiagnosticFinding>) {
    let presented = states
        .iter()
        .filter(|state| return state.presented)
        .collect::<Vec<_>>();

    for (index, state) in presented.iter().enumerate() {
        if state.ghostty_dirty != DirtyState::Full {
            continue;
        }

        let start = index.saturating_sub(8);
        let Some(prior_states) = presented.get(start..index) else {
            continue;
        };
        let prior = prior_states
            .iter()
            .rev()
            .find(|candidate| return candidate.state_hash == state.state_hash);

        if let Some(prior) = prior {
            findings.push(DiagnosticFinding {
                code: "redundant_full_redraw".to_owned(),
                confidence: FindingConfidence::High,
                state_ids: vec![prior.state_id, state.state_id],
                token_ids: Vec::new(),
                facts: vec![
                    "Ghostty reported a full redraw whose complete presented semantic state equals a recent presented state"
                        .to_owned(),
                    "semantic equality does not prove pixels were repainted or visibly flickered"
                        .to_owned(),
                ],
                native_correlations: Vec::new(),
            });
        }
    }
}

fn detect_full_redraw(
    tokens: &[VtToken],
    states: &[StateRecord],
    initial_cols: u16,
    initial_rows: u16,
    broad_change_percent: u8,
    findings: &mut Vec<DiagnosticFinding>,
) {
    let clears = tokens
        .iter()
        .filter(|token| return token.description == "erase display (all)")
        .collect::<Vec<_>>();

    for clear in clears {
        let event = clear.source.first().map(|segment| return segment.event_seq);
        let state = event.and_then(|event_seq| {
            return states.iter().find(|state| {
                return state.event_seq == Some(event_seq)
                    && state.event_byte_offset.is_some_and(|offset| {
                        return clear
                            .source
                            .iter()
                            .any(|segment| return offset >= segment.start_byte_offset);
                    });
            });
        });

        findings.push(DiagnosticFinding {
            code: "full_viewport_clear".to_owned(),
            confidence: FindingConfidence::High,
            state_ids: state.map_or_else(Vec::new, |state| return vec![state.state_id]),
            token_ids: vec![clear.token_id],
            facts: vec![
                "ED 2 occurred in canonical output".to_owned(),
                "semantic evidence does not by itself prove a visible pixel clear".to_owned(),
            ],
            native_correlations: Vec::new(),
        });

        let Some(clear_event) = event else {
            continue;
        };

        let mut cols = initial_cols;
        let mut rows = initial_rows;
        let mut changed = 0_u64;
        let mut repaint_states = Vec::new();

        for candidate in states.iter().filter(|candidate| {
            return candidate.event_seq.is_some_and(|sequence| {
                return sequence >= clear_event && sequence <= clear_event.saturating_add(2);
            });
        }) {
            if let Some((new_cols, new_rows)) = candidate.delta.geometry {
                cols = new_cols;
                rows = new_rows;
            }

            let candidate_changed = candidate.delta.rows.iter().fold(0_u64, |count, row| {
                return count.saturating_add(u64::try_from(row.cells.len()).unwrap_or(u64::MAX));
            });

            if candidate_changed > 0 {
                changed = changed.saturating_add(candidate_changed);
                repaint_states.push(candidate.state_id);
            }
        }

        let cells = u64::from(cols).saturating_mul(u64::from(rows)).max(1);

        if changed.saturating_mul(100) >= cells.saturating_mul(u64::from(broad_change_percent)) {
            findings.push(DiagnosticFinding {
                code: "clear_then_repaint".to_owned(),
                confidence: FindingConfidence::High,
                state_ids: repaint_states,
                token_ids: tokens
                    .iter()
                    .filter(|token| {
                        return token.token_id >= clear.token_id
                            && token.description == "printable bytes";
                    })
                    .take(32)
                    .map(|token| return token.token_id)
                    .chain(iter::once(clear.token_id))
                    .collect(),
                facts: vec![
                    format!("at least {changed} cell updates followed ED 2 within two events"),
                    format!("broad-change threshold was {broad_change_percent}% of {cells} cells"),
                    "ordered semantic repaint evidence is not a pixel-level flicker claim"
                        .to_owned(),
                ],
                native_correlations: Vec::new(),
            });
        }
    }

    let scrollback = tokens
        .iter()
        .filter(|token| return token.description == "erase saved lines")
        .map(|token| return token.token_id)
        .collect::<Vec<_>>();

    if !scrollback.is_empty() {
        findings.push(DiagnosticFinding {
            code: "clear_scrollback".to_owned(),
            confidence: FindingConfidence::High,
            state_ids: Vec::new(),
            token_ids: scrollback,
            facts: vec!["ED 3 occurred in canonical output".to_owned()],
            native_correlations: Vec::new(),
        });
    }
}

fn detect_transients(states: &[StateRecord], findings: &mut Vec<DiagnosticFinding>) {
    let hidden = states
        .iter()
        .filter(|state| return !state.presented && !state.delta.rows.is_empty())
        .map(|state| return state.state_id)
        .collect::<Vec<_>>();

    if !hidden.is_empty() {
        findings.push(DiagnosticFinding {
            code: "hidden_transient".to_owned(),
            confidence: FindingConfidence::High,
            state_ids: hidden.clone(),
            token_ids: Vec::new(),
            facts: vec![format!(
                "{} cell-changing states were not presentation eligible",
                hidden.len()
            )],
            native_correlations: Vec::new(),
        });
    }

    let presented = states
        .iter()
        .filter(|state| return state.presented)
        .collect::<Vec<_>>();

    for [before, transient, after] in presented.array_windows() {
        if before.state_hash == after.state_hash && transient.state_hash != before.state_hash {
            let duration_fact = if transient.monotonic_ns < after.monotonic_ns {
                format!(
                    "ordered transient duration is bounded above by {} ns",
                    after.monotonic_ns.saturating_sub(transient.monotonic_ns)
                )
            } else {
                "ordered_transient_duration_unknown: boundaries share an event timestamp".to_owned()
            };

            findings.push(DiagnosticFinding {
                code: "presented_transient".to_owned(),
                confidence: FindingConfidence::High,
                state_ids: vec![before.state_id, transient.state_id, after.state_id],
                token_ids: Vec::new(),
                facts: vec![
                    "a distinct presentation-eligible state occurred between equal semantic states"
                        .to_owned(),
                    duration_fact,
                ],
                native_correlations: Vec::new(),
            });
        }
    }
}

fn detect_dirty_discrepancies(states: &[StateRecord], findings: &mut Vec<DiagnosticFinding>) {
    for state in states {
        let cell_changes = !state.delta.rows.is_empty();

        if state.ghostty_dirty != DirtyState::Clean && !cell_changes {
            findings.push(DiagnosticFinding {
                code: "dirty_without_cell_change".to_owned(),
                confidence: FindingConfidence::High,
                state_ids: vec![state.state_id],
                token_ids: Vec::new(),
                facts: vec![format!(
                    "Ghostty reported {:?} dirtiness while complete semantic cells were unchanged",
                    state.ghostty_dirty
                )],
                native_correlations: Vec::new(),
            });
        }

        if state.ghostty_dirty == DirtyState::Clean && cell_changes {
            findings.push(DiagnosticFinding {
                code: "cell_change_without_expected_dirty".to_owned(),
                confidence: FindingConfidence::High,
                state_ids: vec![state.state_id],
                token_ids: Vec::new(),
                facts: vec![
                    "complete semantic cells changed while Ghostty reported clean dirtiness"
                        .to_owned(),
                ],
                native_correlations: Vec::new(),
            });
        }
    }
}

fn detect_cursor_jumps(states: &[StateRecord], findings: &mut Vec<DiagnosticFinding>) {
    let mut previous: Option<&CursorState> = None;

    for state in states {
        let Some(cursor) = state.delta.cursor.as_ref() else {
            continue;
        };

        if let (Some(old), Some(old_column), Some(old_row), Some(column), Some(row)) = (
            previous,
            previous.and_then(|value| return value.column),
            previous.and_then(|value| return value.row),
            cursor.column,
            cursor.row,
        ) {
            let distance = old_column
                .abs_diff(column)
                .saturating_add(old_row.abs_diff(row));

            if distance >= 10 {
                findings.push(DiagnosticFinding {
                    code: "cursor_jump".to_owned(),
                    confidence: FindingConfidence::High,
                    state_ids: vec![state.state_id],
                    token_ids: Vec::new(),
                    facts: vec![format!(
                        "cursor moved {distance} cells from ({old_column},{old_row}) to ({column},{row})"
                    )],
                    native_correlations: Vec::new(),
                });
            }

            let _ = old;
        }

        previous = Some(cursor);
    }
}

fn detect_churn(
    checks: &[DiagnosticCheck],
    states: &[StateRecord],
    findings: &mut Vec<DiagnosticFinding>,
) {
    for (enabled, code, ids) in [
        (
            checks.contains(&DiagnosticCheck::Modes),
            "mode_churn",
            states
                .iter()
                .filter(|state| return state.delta.modes.is_some())
                .map(|state| return state.state_id)
                .collect::<Vec<_>>(),
        ),
        (
            checks.contains(&DiagnosticCheck::Palette),
            "palette_churn",
            states
                .iter()
                .filter(|state| return state.delta.colors.is_some())
                .map(|state| return state.state_id)
                .collect::<Vec<_>>(),
        ),
    ] {
        if enabled && ids.len() >= 3 {
            findings.push(DiagnosticFinding {
                code: code.to_owned(),
                confidence: FindingConfidence::Medium,
                state_ids: ids.clone(),
                token_ids: Vec::new(),
                facts: vec![format!("{} related semantic mutations occurred", ids.len())],
                native_correlations: Vec::new(),
            });
        }
    }
}

fn detect_query_loops(log: &EventLog, findings: &mut Vec<DiagnosticFinding>) {
    let mut counts = BTreeMap::<(Vec<u8>, Vec<u8>), usize>::new();

    for record in &log.events {
        if let Event::QueryResponse {
            query, response, ..
        } = &record.event
        {
            *counts
                .entry((query.as_bytes().to_vec(), response.as_bytes().to_vec()))
                .or_default() += 1;
        }
    }

    for ((_query, _response), count) in counts {
        if count >= 3 {
            findings.push(DiagnosticFinding {
                code: "repeated_query_response_loop".to_owned(),
                confidence: FindingConfidence::Medium,
                state_ids: Vec::new(),
                token_ids: Vec::new(),
                facts: vec![format!(
                    "the same terminal query and host response occurred {count} times"
                )],
                native_correlations: Vec::new(),
            });
        }
    }
}

fn findings(tokens: &[VtToken], states: &[StateRecord]) -> Vec<ExplainFinding> {
    let mut findings = Vec::new();

    for (description, code, fact) in [
        (
            "erase display (all)",
            "full_viewport_clear",
            "ED 2 occurred in canonical output",
        ),
        (
            "erase saved lines",
            "clear_scrollback",
            "ED 3 occurred in canonical output",
        ),
    ] {
        let token_ids = tokens
            .iter()
            .filter(|token| return token.description == description)
            .map(|token| return token.token_id)
            .collect::<Vec<_>>();

        if !token_ids.is_empty() {
            findings.push(ExplainFinding {
                code: code.to_owned(),
                confidence: FindingConfidence::High,
                token_ids,
                facts: vec![fact.to_owned()],
            });
        }
    }

    let hidden = states
        .iter()
        .filter(|state| return !state.presented && !state.delta.rows.is_empty())
        .collect::<Vec<_>>();

    if !hidden.is_empty() {
        findings.push(ExplainFinding {
            code: "hidden_transient".to_owned(),
            confidence: FindingConfidence::High,
            token_ids: Vec::new(),
            facts: vec![format!(
                "{} cell-changing semantic states were not presentation eligible",
                hidden.len()
            )],
        });
    }

    let clear = findings
        .iter()
        .find(|finding| return finding.code == "full_viewport_clear");
    let printable_after_clear = clear.is_some_and(|finding| {
        let last_clear = finding.token_ids.iter().copied().max().unwrap_or(0);
        return tokens.iter().any(|token| {
            return token.token_id > last_clear
                && token.description == "printable bytes"
                && !token.raw.as_bytes().is_empty();
        });
    });

    if printable_after_clear {
        findings.push(ExplainFinding {
            code: "clear_then_repaint_candidate".to_owned(),
            confidence: FindingConfidence::Medium,
            token_ids: tokens
                .iter()
                .filter(|token| {
                    return token.description == "erase display (all)"
                        || token.description == "printable bytes";
                })
                .map(|token| return token.token_id)
                .collect(),
            facts: vec![
                "printable output followed a full semantic erase".to_owned(),
                "this does not establish that pixels flickered".to_owned(),
            ],
        });
    }

    return findings;
}
