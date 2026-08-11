//! Task-oriented analysis and bounded bug-report handoffs.

use std::{collections::BTreeMap, path::Path};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    analysis::{DiagnosticCheck, DiagnosticFinding, FindingConfidence, diagnose},
    artifact::{
        ArtifactMember, ArtifactSensitivity, collect_artifact_members_bounded, commit_directory,
        create_private_directory, sync_directory, write_secure,
    },
    diff::{RowChange, ValueChange, diff_states_with_semantic_states, inspect_point},
    event::{ByteString, Event, EventRecord, OperationStatus},
    event_log::EventLog,
    ghostty_vt::{CursorState, ModeState, SemanticState, TerminalMode},
    observation::{StatePoint, observe_session},
    replay::{StateSelector, inspect, replay},
    request_journal::{MUTATION_JOURNAL_NAME, MutationJournalRecord, read_mutation_journal},
    semantic_image::render_png,
    session::{SessionGeometry, read_manifest, validate_completed_log},
    vt_framer::frame_event_log,
};

const MAX_ACTION_WARNINGS: usize = 16;
const MAX_ACTION_FINDINGS: usize = 32;
const MAX_FINDING_FACTS: usize = 4;
const MAX_FINDING_REFERENCES: usize = 64;
const MAX_BUG_REPORT_TREE_ENTRIES: usize = 16;

/// Canonical request and intent/result boundaries defining one input action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LastActionBoundary {
    /// Caller-owned request identifier when the action was durably journaled.
    pub request_id: Option<Uuid>,
    /// Ordered canonical operation identifiers for every input atom.
    pub operation_ids: Vec<Uuid>,
    /// Number of input atoms in the request.
    pub atom_count: u32,
    /// Inclusive canonical event boundary before request execution.
    pub request_event_start_seq: u64,
    /// Exclusive canonical event boundary after request execution and postcondition evaluation.
    pub request_event_end_seq: u64,
    /// First canonical input-intent event sequence.
    pub intent_event_seq: u64,
    /// Last canonical input-result event sequence.
    pub result_event_seq: u64,
    /// Intended input byte count without the input bytes themselves.
    pub intended_bytes: u64,
    /// Aggregate applied or failed result.
    pub status: OperationStatus,
    /// Bytes confirmed written across every atom.
    pub bytes_written: u64,
    /// First stable result error code, when any atom failed.
    pub error_code: Option<String>,
}

/// Aggregate child output attributable to the last action boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionOutputSummary {
    /// Canonical output-event count after the input intent.
    pub event_count: u64,
    /// Exact output byte count without the output bytes themselves.
    pub byte_count: u64,
}

/// One changed ANSI or DEC terminal mode.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionModeChange {
    /// Numeric terminal mode identifier.
    pub number: u16,
    /// `true` for ANSI and `false` for DEC-private mode.
    pub ansi: bool,
    /// Earlier set/reset value.
    pub before: bool,
    /// Later set/reset value.
    pub after: bool,
}

/// One bounded warning derived from canonical action evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionWarning {
    /// Stable warning code.
    pub code: String,
    /// Conservative confidence label.
    pub confidence: FindingConfidence,
    /// Bounded neutral evidence facts.
    pub facts: Vec<String>,
}

/// Compact semantic result of the most recent canonical input action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LastActionReport {
    /// Stable report format.
    pub format: String,
    /// Report schema version.
    pub version: u32,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Final canonical source hash.
    pub source_final_event_hash: String,
    /// Canonical input action boundary.
    pub action: LastActionBoundary,
    /// Earlier semantic state identifier.
    pub from_state_id: u64,
    /// Later semantic state identifier.
    pub to_state_id: u64,
    /// Sorted zero-based viewport rows with semantic cell changes.
    pub rows_changed: Vec<u16>,
    /// Total changed semantic cells.
    pub cells_changed: u64,
    /// Complete cursor movement when the cursor changed.
    pub cursor_movement: Option<ValueChange<CursorState>>,
    /// Sorted terminal mode changes.
    pub mode_changes: Vec<ActionModeChange>,
    /// Child output generated after the canonical input intent.
    pub output_generated: ActionOutputSummary,
    /// Whether the resulting semantic screen was presentation eligible.
    pub presentation_eligible: bool,
    /// Number of presentation-eligible semantic boundaries after the action.
    pub presentation_eligible_states: u64,
    /// Bounded conservative warnings.
    pub warnings: Vec<ActionWarning>,
    /// Whether additional warnings were omitted.
    pub warnings_truncated: bool,
}

/// Diagnostics restricted to the semantic and token range after the last input action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LastActionDiagnosticsReport {
    /// Stable report format.
    pub format: String,
    /// Report schema version.
    pub version: u32,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Final canonical source hash.
    pub source_final_event_hash: String,
    /// Canonical input action boundary.
    pub action: LastActionBoundary,
    /// Earlier semantic state identifier.
    pub from_state_id: u64,
    /// Later semantic state identifier.
    pub to_state_id: u64,
    /// Bounded findings supported inside the selected action range.
    pub findings: Vec<DiagnosticFinding>,
    /// Whether additional findings were omitted.
    pub findings_truncated: bool,
}

/// Both task-level reports derived in one canonical replay/diagnostic pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LastActionAnalysis {
    /// Compact action diff and warnings.
    pub inspection: LastActionReport,
    /// Range-restricted diagnostics.
    pub diagnostics: LastActionDiagnosticsReport,
}

/// Derive the latest input action from its canonical intent/result pair.
///
/// # Errors
///
/// Returns an error when no input intent exists, its result is absent, or canonical replay fails.
pub fn analyze_last_action(
    session: &Path,
    log: &EventLog,
    broad_change_percent: u8,
) -> Result<LastActionAnalysis> {
    if !(1..=100).contains(&broad_change_percent) {
        return Err(task_error(
            "broad-change percentage must be between 1 and 100",
        ));
    }
    let action = last_input_action(session, log)?;
    let final_inspection = inspect(log, StateSelector::Final)?;
    let (difference, before, after) = diff_states_with_semantic_states(
        log,
        StatePoint::EventSeq {
            event_seq: action.request_event_start_seq,
        },
        StatePoint::StateId {
            state_id: final_inspection.record.state_id,
        },
    )?;
    let output_generated = output_summary(log, action.intent_event_seq);
    let replayed = replay(log)?;
    let action_states = replayed
        .states
        .iter()
        .filter(|state| {
            return state.state_id > difference.from.state_id
                && state.state_id <= difference.to.state_id;
        })
        .collect::<Vec<_>>();
    let presentation_eligible_states = u64::try_from(
        action_states
            .iter()
            .filter(|state| return state.presented)
            .count(),
    )
    .unwrap_or(u64::MAX);
    let diagnostic_report = diagnose(session, log, DiagnosticCheck::all(), broad_change_percent)?;
    let action_token_ids = frame_event_log(log)?
        .into_iter()
        .filter(|token| {
            return token
                .source
                .iter()
                .any(|source| return source.event_seq > action.intent_event_seq);
        })
        .map(|token| return token.token_id)
        .collect::<Vec<_>>();
    let (findings, findings_truncated) = bounded_action_findings(
        diagnostic_report.findings,
        difference.from.state_id,
        difference.to.state_id,
        &action_token_ids,
    );
    let (warnings, warning_limit_reached) = action_warnings(
        &difference.rows,
        &before,
        &after,
        &output_generated,
        presentation_eligible_states,
        broad_change_percent,
        &findings,
    );
    let warnings_truncated = warning_limit_reached || findings_truncated;
    let rows_changed = difference.rows.iter().map(|row| return row.row).collect();
    let cells_changed = difference.rows.iter().fold(0_u64, |total, row| {
        return total.saturating_add(u64::try_from(row.cells.len()).unwrap_or(u64::MAX));
    });
    let mode_changes = changed_modes(difference.modes.as_ref());
    let source_final_event_hash = log
        .events
        .last()
        .ok_or_else(|| return task_error("canonical input action has no event record"))?
        .hash
        .clone();
    let inspection = LastActionReport {
        format: "termtrace-last-action".to_owned(),
        version: 1,
        session_id: log.header.session_id,
        source_final_event_hash: source_final_event_hash.clone(),
        action: action.clone(),
        from_state_id: difference.from.state_id,
        to_state_id: difference.to.state_id,
        rows_changed,
        cells_changed,
        cursor_movement: difference.cursor,
        mode_changes,
        output_generated,
        presentation_eligible: difference.to.presented,
        presentation_eligible_states,
        warnings,
        warnings_truncated,
    };
    let diagnostics = LastActionDiagnosticsReport {
        format: "termtrace-last-action-diagnostics".to_owned(),
        version: 1,
        session_id: log.header.session_id,
        source_final_event_hash,
        action,
        from_state_id: difference.from.state_id,
        to_state_id: difference.to.state_id,
        findings,
        findings_truncated,
    };
    return Ok(LastActionAnalysis {
        inspection,
        diagnostics,
    });
}

fn last_input_action(session: &Path, log: &EventLog) -> Result<LastActionBoundary> {
    let journal = read_mutation_journal(&session.join(MUTATION_JOURNAL_NAME))?;
    for request in journal.iter().rev() {
        if let Some(action) = journaled_input_action(log, request)? {
            return Ok(action);
        }
    }
    return unjournaled_input_action(log);
}

fn journaled_input_action(
    log: &EventLog,
    request: &MutationJournalRecord,
) -> Result<Option<LastActionBoundary>> {
    let intents = log
        .events
        .iter()
        .filter(|record| {
            return record.seq >= request.event_start_seq
                && record.seq < request.event_end_seq
                && matches!(record.event, Event::InputIntent { .. });
        })
        .collect::<Vec<_>>();
    if intents.is_empty() {
        return Ok(None);
    }
    return Ok(Some(action_boundary(
        log,
        Some(request.request_id),
        request.event_start_seq,
        request.event_end_seq,
        &intents,
    )?));
}

fn unjournaled_input_action(log: &EventLog) -> Result<LastActionBoundary> {
    let intent = log
        .events
        .iter()
        .rev()
        .find(|record| return matches!(record.event, Event::InputIntent { .. }))
        .ok_or_else(|| return task_error("session contains no canonical input action"))?;
    return action_boundary(
        log,
        None,
        intent.seq,
        intent.seq.saturating_add(2),
        &[intent],
    );
}

fn action_boundary(
    log: &EventLog,
    request_id: Option<Uuid>,
    request_event_start_seq: u64,
    request_event_end_seq: u64,
    intents: &[&EventRecord],
) -> Result<LastActionBoundary> {
    let mut operation_ids = Vec::with_capacity(intents.len());
    let mut intended_bytes = 0_u64;
    let mut bytes_written = 0_u64;
    let mut status = OperationStatus::Applied;
    let mut error_code = None;
    let mut first_intent_event_seq = None;
    let mut last_result_event_seq = None;
    for intent in intents {
        let Event::InputIntent {
            operation_id, data, ..
        } = &intent.event
        else {
            return Err(task_error("selected action intent changed event type"));
        };
        operation_ids.push(*operation_id);
        intended_bytes =
            intended_bytes.saturating_add(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX));
        first_intent_event_seq.get_or_insert(intent.seq);
        let result = log
            .events
            .iter()
            .find(|record| {
                return record.seq > intent.seq
                    && matches!(
                        record.event,
                        Event::InputResult {
                            operation_id: result_id,
                            ..
                        } if result_id == *operation_id
                    );
            })
            .ok_or_else(|| return task_error("input intent has no canonical input result"))?;
        let Event::InputResult {
            status: atom_status,
            bytes_written: atom_bytes_written,
            error_code: atom_error_code,
            ..
        } = &result.event
        else {
            return Err(task_error("matched input result changed event type"));
        };
        bytes_written = bytes_written.saturating_add(*atom_bytes_written);
        last_result_event_seq = Some(result.seq);
        if *atom_status == OperationStatus::Failed {
            status = OperationStatus::Failed;
            if error_code.is_none() {
                error_code.clone_from(atom_error_code);
            }
        }
    }
    let intent_event_seq = first_intent_event_seq
        .ok_or_else(|| return task_error("input action has no canonical intent"))?;
    let result_event_seq = last_result_event_seq
        .ok_or_else(|| return task_error("input action has no canonical result"))?;
    return Ok(LastActionBoundary {
        request_id,
        atom_count: u32::try_from(operation_ids.len()).unwrap_or(u32::MAX),
        operation_ids,
        request_event_start_seq,
        request_event_end_seq,
        intent_event_seq,
        result_event_seq,
        intended_bytes,
        status,
        bytes_written,
        error_code,
    });
}

fn output_summary(log: &EventLog, intent_event_seq: u64) -> ActionOutputSummary {
    let mut event_count = 0_u64;
    let mut byte_count = 0_u64;
    for record in &log.events {
        if record.seq <= intent_event_seq {
            continue;
        }
        if let Event::Output { data } = &record.event {
            event_count = event_count.saturating_add(1);
            byte_count =
                byte_count.saturating_add(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX));
        }
    }
    return ActionOutputSummary {
        event_count,
        byte_count,
    };
}

fn changed_modes(change: Option<&ValueChange<ModeState>>) -> Vec<ActionModeChange> {
    let Some(change) = change else {
        return Vec::new();
    };
    let before = terminal_mode_map(&change.before.values);
    let after = terminal_mode_map(&change.after.values);
    let mut keys = before
        .keys()
        .chain(after.keys())
        .copied()
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    return keys
        .into_iter()
        .filter_map(|(ansi, number)| {
            let before = before.get(&(ansi, number)).copied().unwrap_or(false);
            let after = after.get(&(ansi, number)).copied().unwrap_or(false);
            return (before != after).then_some(ActionModeChange {
                number,
                ansi,
                before,
                after,
            });
        })
        .collect();
}

fn terminal_mode_map(modes: &[TerminalMode]) -> BTreeMap<(bool, u16), bool> {
    return modes
        .iter()
        .map(|mode| return ((mode.ansi, mode.number), mode.enabled))
        .collect();
}

fn bounded_action_findings(
    findings: Vec<DiagnosticFinding>,
    from_state_id: u64,
    to_state_id: u64,
    action_token_ids: &[u64],
) -> (Vec<DiagnosticFinding>, bool) {
    let mut selected = findings
        .into_iter()
        .filter(|finding| {
            return finding
                .state_ids
                .iter()
                .any(|state_id| return *state_id > from_state_id && *state_id <= to_state_id)
                || finding
                    .token_ids
                    .iter()
                    .any(|token_id| return action_token_ids.binary_search(token_id).is_ok());
        })
        .collect::<Vec<_>>();
    let truncated = selected.len() > MAX_ACTION_FINDINGS;
    selected.truncate(MAX_ACTION_FINDINGS);
    for finding in &mut selected {
        finding.facts.truncate(MAX_FINDING_FACTS);
        finding.state_ids.truncate(MAX_FINDING_REFERENCES);
        finding.token_ids.truncate(MAX_FINDING_REFERENCES);
        finding.native_correlations.truncate(MAX_FINDING_REFERENCES);
    }
    return (selected, truncated);
}

fn action_warnings(
    rows: &[RowChange],
    before: &SemanticState,
    after: &SemanticState,
    output: &ActionOutputSummary,
    presentation_eligible_states: u64,
    broad_change_percent: u8,
    findings: &[DiagnosticFinding],
) -> (Vec<ActionWarning>, bool) {
    let changed_cells = rows.iter().fold(0_u64, |total, row| {
        return total.saturating_add(u64::try_from(row.cells.len()).unwrap_or(u64::MAX));
    });
    let total_cells = u64::from(after.cols.max(before.cols))
        .saturating_mul(u64::from(after.rows.max(before.rows)));
    let mut warnings = Vec::new();
    if total_cells > 0
        && changed_cells.saturating_mul(100)
            >= total_cells.saturating_mul(u64::from(broad_change_percent))
    {
        warnings.push(ActionWarning {
            code: "broad_repaint".to_owned(),
            confidence: FindingConfidence::High,
            facts: vec![format!(
                "{changed_cells} of {total_cells} viewport cells changed (threshold {broad_change_percent}%)"
            )],
        });
    }
    if output.byte_count > 0 && presentation_eligible_states == 0 {
        warnings.push(ActionWarning {
            code: "output_without_presentation".to_owned(),
            confidence: FindingConfidence::High,
            facts: vec![format!(
                "{} output bytes produced no presentation-eligible semantic boundary",
                output.byte_count
            )],
        });
    }
    if !after.cursor.in_viewport {
        warnings.push(ActionWarning {
            code: "cursor_outside_expected_region".to_owned(),
            confidence: FindingConfidence::High,
            facts: vec!["resulting cursor is outside the viewport".to_owned()],
        });
    }
    if after.modes.synchronized_output {
        warnings.push(ActionWarning {
            code: "synchronized_output_misuse".to_owned(),
            confidence: FindingConfidence::High,
            facts: vec![
                "synchronized-output mode remained enabled at the action endpoint".to_owned(),
            ],
        });
    }
    for finding in findings {
        let code = match finding.code.as_str() {
            "presented_transient" | "clear_then_repaint" => "flicker",
            "hidden_transient" => "synchronized_output_misuse",
            _ => finding.code.as_str(),
        };
        if warnings.iter().any(|warning| return warning.code == code) {
            continue;
        }
        warnings.push(ActionWarning {
            code: code.to_owned(),
            confidence: finding.confidence,
            facts: finding
                .facts
                .iter()
                .take(MAX_FINDING_FACTS)
                .cloned()
                .collect(),
        });
    }
    let truncated = warnings.len() > MAX_ACTION_WARNINGS;
    warnings.truncate(MAX_ACTION_WARNINGS);
    return (warnings, truncated);
}

/// Redacted session metadata included in a bug report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BugReportMetadata {
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Friendly creation alias, when supplied.
    pub creation_alias: Option<String>,
    /// Unix wall-clock recording origin in nanoseconds.
    pub started_unix_ns: u128,
    /// Initial PTY geometry.
    pub geometry: SessionGeometry,
    /// Termtrace package version used to record the session.
    pub termtrace_version: String,
    /// Recording host operating system.
    pub operating_system: String,
    /// Recording host architecture.
    pub architecture: String,
    /// Child exit code, when available.
    pub exit_code: Option<i32>,
    /// Child signal name, when available.
    pub signal_name: Option<String>,
    /// Total canonical output bytes without their contents.
    pub output_bytes: u64,
    /// Canonical event count.
    pub event_count: u64,
    /// Stable recorder failure code, when present.
    pub failure_code: Option<String>,
}

/// Exact canonical source reference without copied canonical byte streams.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BugReportSessionReference {
    /// Canonical source path bytes.
    pub canonical_session: ByteString,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Final canonical event hash.
    pub final_event_hash: String,
    /// Canonical event count.
    pub event_count: u64,
}

/// Whole-file manifest for a bounded bug-report directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BugReportManifest {
    /// Stable manifest format.
    pub format: String,
    /// Manifest schema version.
    pub version: u32,
    /// Canonical source identity.
    pub session_id: Uuid,
    /// Exact canonical source path bytes.
    pub canonical_session: ByteString,
    /// Final canonical source hash.
    pub source_final_event_hash: String,
    /// Raw input/output event byte streams are intentionally excluded.
    pub includes_raw_streams: bool,
    /// Whether a semantic screenshot was explicitly requested.
    pub includes_screenshot: bool,
    /// Whether the handoff includes last-action analysis and diagnostics.
    pub includes_last_action: bool,
    /// Whole-file member inventory.
    pub members: Vec<ArtifactMember>,
}

/// Atomically publish a bounded, manifested bug-report handoff.
///
/// The default payload deliberately omits canonical events and raw input/output byte streams.
///
/// # Errors
///
/// Returns an error when the source is incomplete or the destination cannot be published
/// without replacement.
pub fn create_bug_report(
    session: &Path,
    destination: &Path,
    include_screenshot: bool,
) -> Result<BugReportManifest> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    if destination.starts_with(&canonical) {
        return Err(task_error(
            "bug-report destination must be outside the canonical session",
        ));
    }
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let session_manifest = read_manifest(&canonical)?;
    let analysis = log
        .events
        .iter()
        .any(|record| return matches!(record.event, Event::InputIntent { .. }))
        .then(|| return analyze_last_action(&canonical, &log, 50))
        .transpose()?;
    let observation = observe_session(&canonical, StatePoint::LatestPresented)?;
    let parent = destination
        .parent()
        .ok_or_else(|| return task_error("bug-report path has no parent"))?;
    if !parent.is_dir() {
        return Err(task_error("bug-report parent directory does not exist"));
    }
    let name = destination
        .file_name()
        .ok_or_else(|| return task_error("bug-report path has no basename"))?;
    let staging = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        Uuid::now_v7()
    ));
    create_private_directory(&staging)?;
    let publish = (|| -> Result<BugReportManifest> {
        let metadata = BugReportMetadata {
            session_id: session_manifest.session_id,
            creation_alias: session_manifest.creation_alias.clone(),
            started_unix_ns: session_manifest.started_unix_ns,
            geometry: session_manifest.geometry.clone(),
            termtrace_version: session_manifest.termtrace_version.clone(),
            operating_system: session_manifest.operating_system.clone(),
            architecture: session_manifest.architecture.clone(),
            exit_code: completion.exit_code,
            signal_name: completion.signal_name.clone(),
            output_bytes: completion.output_bytes,
            event_count: completion.event_count,
            failure_code: completion
                .failure
                .as_ref()
                .map(|failure| return failure.code.clone()),
        };
        let reference = BugReportSessionReference {
            canonical_session: ByteString::from(canonical.as_os_str().as_encoded_bytes()),
            session_id: session_manifest.session_id,
            final_event_hash: completion.final_event_hash.clone(),
            event_count: completion.event_count,
        };
        write_json(&staging.join("metadata.json"), &metadata)?;
        write_secure(
            &staging.join("last-screen.txt"),
            observation.frame.viewport_text.as_bytes(),
        )?;
        if let Some(analysis) = &analysis {
            write_json(&staging.join("last-action.json"), &analysis.inspection)?;
            write_json(&staging.join("diagnostics.json"), &analysis.diagnostics)?;
        }
        write_json(&staging.join("session-reference.json"), &reference)?;
        if include_screenshot {
            let screenshot_state = inspect_point(&log, &StatePoint::LatestPresented)?.state;
            write_secure(
                &staging.join("screenshot.png"),
                &render_png(&screenshot_state)?,
            )?;
        }
        let mut members = collect_artifact_members_bounded(&staging, MAX_BUG_REPORT_TREE_ENTRIES)?;
        for member in &mut members {
            let (sensitivity, relationship, schema_type) =
                bug_report_member_semantics(&member.path).ok_or_else(|| {
                    return task_error(format!(
                        "created bug-report member `{}` is outside the bounded profile",
                        member.path
                    ));
                })?;
            member.sensitivity = sensitivity;
            member.relationship = relationship.to_owned();
            member.schema_type = schema_type.map(str::to_owned);
        }
        let manifest = BugReportManifest {
            format: "termtrace-bug-report".to_owned(),
            version: 2,
            session_id: session_manifest.session_id,
            canonical_session: ByteString::from(canonical.as_os_str().as_encoded_bytes()),
            source_final_event_hash: completion.final_event_hash.clone(),
            includes_raw_streams: false,
            includes_screenshot: include_screenshot,
            includes_last_action: analysis.is_some(),
            members,
        };
        write_json(&staging.join("manifest.json"), &manifest)?;
        sync_directory(&staging)?;
        return Ok(manifest);
    })();
    let manifest = match publish {
        Ok(value) => value,
        Err(error) => {
            let _cleanup = fs_err::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _cleanup = fs_err::remove_dir_all(&staging);
        return Err(error);
    }
    return Ok(manifest);
}

fn bug_report_member_semantics(
    path: &str,
) -> Option<(ArtifactSensitivity, &'static str, Option<&'static str>)> {
    let sensitive = ArtifactSensitivity::SensitiveTerminalData;
    let operational = ArtifactSensitivity::OperationalMetadata;
    return match path {
        "metadata.json" => Some((operational, "redacted_session_metadata", None)),
        "last-screen.txt" => Some((sensitive, "last_relevant_screen", None)),
        "last-action.json" => Some((operational, "last_action_diff", None)),
        "diagnostics.json" => Some((operational, "last_action_diagnostics", None)),
        "session-reference.json" => Some((operational, "canonical_session_reference", None)),
        "screenshot.png" => Some((sensitive, "optional_semantic_screenshot", None)),
        _ => None,
    };
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    bytes.push(b'\n');
    return write_secure(path, &bytes);
}

fn task_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
