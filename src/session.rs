//! Stable session provenance and completion manifests.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    event::{ByteString, Event},
    event_log::EventLog,
};

/// Session manifest format identifier.
pub const SESSION_MANIFEST_FORMAT: &str = "termtrace-session";
/// Session manifest schema version.
pub const SESSION_MANIFEST_VERSION: u32 = 2;
/// Completion manifest format identifier.
pub const SESSION_COMPLETION_FORMAT: &str = "termtrace-completion";
/// Completion manifest schema version.
pub const SESSION_COMPLETION_VERSION: u32 = 2;
/// Validation contract used when the completion manifest was created.
pub const COMPLETION_VALIDATION_VERSION: u32 = 1;
/// Verification report format identifier.
pub const VERIFICATION_REPORT_FORMAT: &str = "termtrace-verification";
/// Verification report schema version.
pub const VERIFICATION_REPORT_VERSION: u32 = 1;

/// Requested depth for an offline evidence verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerificationLevel {
    /// Parse all canonical files and enforce their versioned structure.
    Structure,
    /// Verify the manifest binding, event chain, completion, and terminal state.
    Content,
    /// Verify content plus the chain-bound durability policy and recorded outcomes.
    DurabilityRecord,
    /// Verify content plus a detached signature and configured signer trust.
    Signature,
}

/// One stable verification diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationFinding {
    /// Stable machine-readable finding code.
    pub code: String,
    /// Human-readable context that is not intended for programmatic matching.
    pub message: String,
}

/// Machine-readable result from offline session verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationReport {
    /// Stable report format identifier.
    pub format: String,
    /// Report schema version.
    pub version: u32,
    /// Canonical session directory checked by the verifier.
    pub session: PathBuf,
    /// Requested verification depth.
    pub level: VerificationLevel,
    /// Whether every invariant at the requested level passed.
    pub valid: bool,
    /// Immutable session identifier, when readable.
    pub session_id: Option<Uuid>,
    /// Exact session-manifest hash, when readable.
    pub session_manifest_hash: Option<String>,
    /// Final canonical event hash, when readable.
    pub final_event_hash: Option<String>,
    /// Stable findings; empty when verification succeeds.
    pub findings: Vec<VerificationFinding>,
}

/// Initial PTY geometry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionGeometry {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Total pixel width.
    pub pixel_width: u16,
    /// Total pixel height.
    pub pixel_height: u16,
}

/// Origin of the initial terminal geometry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GeometrySource {
    /// Explicit columns, rows, and cell dimensions.
    #[default]
    Manual,
    /// Dimensions inherited from the controlling terminal.
    Inherited,
    /// Dimensions measured from Ghostty at a target viewport size.
    GhosttyFit,
}

/// Pixel viewport intended for native visual capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CaptureTarget {
    /// Viewport width in pixels.
    pub width: u16,
    /// Viewport height in pixels.
    pub height: u16,
}

/// Provenance for automatic terminal geometry selection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeometryProvenance {
    /// How the initial geometry was selected.
    pub source: GeometrySource,
    /// Intended native-capture viewport, when selected during recording.
    pub capture_target: Option<CaptureTarget>,
    /// Effective Ghostty configuration hash used for a fitted measurement.
    pub ghostty_config_hash: Option<String>,
}

/// Focused Termtrace overrides applied to a Ghostty visual profile.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GhosttyVisualOverrides {
    /// Regular terminal font family.
    pub font_family: Option<String>,
    /// Font size in points, represented exactly as a Ghostty config value.
    pub font_size: Option<String>,
    /// Symmetric horizontal window padding in pixels.
    pub padding_x: Option<u16>,
    /// Symmetric vertical window padding in pixels.
    pub padding_y: Option<u16>,
    /// Default foreground color in Ghostty syntax.
    pub foreground: Option<String>,
    /// Default background color in Ghostty syntax.
    pub background: Option<String>,
}

impl GhosttyVisualOverrides {
    /// Whether no convenience override was supplied.
    pub fn is_empty(&self) -> bool {
        return self.font_family.is_none()
            && self.font_size.is_none()
            && self.padding_x.is_none()
            && self.padding_y.is_none()
            && self.foreground.is_none()
            && self.background.is_none();
    }
}

/// Persisted provenance for a canonical per-recording Ghostty profile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GhosttyVisualProvenance {
    /// Original custom configuration path bytes, when supplied.
    pub source_config: Option<ByteString>,
    /// Focused overrides applied after resolving the source configuration.
    pub overrides: GhosttyVisualOverrides,
    /// Relative canonical effective-configuration artifact.
    pub effective_config: String,
    /// BLAKE3 of the canonical effective configuration.
    pub effective_config_hash: String,
}

/// Immutable provenance written before the child is spawned.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionManifest {
    /// Stable format identifier.
    pub format: String,
    /// Schema version.
    pub version: u32,
    /// Session identifier shared with the event log.
    pub session_id: Uuid,
    /// Unix wall-clock origin in nanoseconds.
    pub started_unix_ns: u128,
    /// Exact executable and argument bytes.
    pub command: Vec<ByteString>,
    /// Exact effective working-directory bytes; absent only in legacy sessions.
    pub working_directory: Option<ByteString>,
    /// Initial PTY geometry.
    pub geometry: SessionGeometry,
    /// Selection source and optional capture target for the initial geometry.
    #[serde(default)]
    pub geometry_provenance: GeometryProvenance,
    /// Canonical per-recording Ghostty profile, when explicitly resolved.
    #[serde(default)]
    pub ghostty_visual: Option<GhosttyVisualProvenance>,
    /// Allowlisted child environment overrides.
    pub child_environment: BTreeMap<String, String>,
    /// Termtrace package version.
    pub termtrace_version: String,
    /// Build target operating system.
    pub operating_system: String,
    /// Build target architecture.
    pub architecture: String,
    /// Canonical event-log relative path.
    pub event_log: String,
    /// Live control socket relative path.
    pub control_socket: String,
    /// Explicit sensitivity warning.
    pub contains_sensitive_terminal_data: bool,
}

/// Immutable completion facts written after the final event is durable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCompletion {
    /// Stable format identifier.
    pub format: String,
    /// Completion schema version.
    pub version: u32,
    /// Validation contract applied before publication.
    pub validation_version: u32,
    /// Session identifier.
    pub session_id: Uuid,
    /// BLAKE3 of the exact immutable session manifest bytes.
    pub session_manifest_hash: String,
    /// BLAKE3 of the exact event-log header bytes.
    pub event_header_hash: String,
    /// Normal exit code when available.
    pub exit_code: Option<i32>,
    /// Signal name when available.
    pub signal_name: Option<String>,
    /// Total raw output bytes.
    pub output_bytes: u64,
    /// Hash of the final canonical event.
    pub final_event_hash: String,
    /// Number of canonical events.
    pub event_count: u64,
    /// Sequence number of the required terminal `Exit` event.
    pub terminal_event_seq: u64,
}

/// Serialize a session manifest into its sole canonical on-disk form.
///
/// # Errors
///
/// Returns an error if the manifest cannot be serialized.
pub fn canonical_manifest_bytes(manifest: &SessionManifest) -> Result<Vec<u8>> {
    return serde_json::to_vec(manifest).map_err(|source| {
        return Error::InvalidJson {
            path: "session.json".into(),
            json_path: "$".to_owned(),
            source,
        };
    });
}

/// Return the BLAKE3 digest of exact session-manifest bytes.
pub fn manifest_hash(bytes: &[u8]) -> String {
    return blake3::hash(bytes).to_hex().to_string();
}

/// Verify one completed session without mutating its canonical evidence.
pub fn verify_session(
    session_directory: impl AsRef<Path>,
    level: VerificationLevel,
) -> VerificationReport {
    let session_directory = session_directory.as_ref();
    let mut report = VerificationReport {
        format: VERIFICATION_REPORT_FORMAT.to_owned(),
        version: VERIFICATION_REPORT_VERSION,
        session: session_directory.to_path_buf(),
        level,
        valid: false,
        session_id: None,
        session_manifest_hash: None,
        final_event_hash: None,
        findings: Vec::new(),
    };
    let manifest_path = session_directory.join("session.json");
    let manifest_bytes = match fs_err::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            report.findings.push(VerificationFinding {
                code: "session_manifest_unreadable".to_owned(),
                message: error.to_string(),
            });
            return report;
        }
    };
    report.session_manifest_hash = Some(manifest_hash(&manifest_bytes));
    let manifest = match read_manifest(session_directory) {
        Ok(manifest) => manifest,
        Err(error) => {
            report.findings.push(VerificationFinding {
                code: "session_manifest_invalid".to_owned(),
                message: error.to_string(),
            });
            return report;
        }
    };
    report.session_id = Some(manifest.session_id);
    if let Err(error) = validate_manifest(&manifest) {
        report.findings.push(VerificationFinding {
            code: "session_format_unsupported".to_owned(),
            message: error.to_string(),
        });
        return report;
    }
    let event_path = session_directory.join(&manifest.event_log);
    let log = match EventLog::read(event_path) {
        Ok(log) => log,
        Err(error) => {
            report.findings.push(VerificationFinding {
                code: "event_log_invalid".to_owned(),
                message: error.to_string(),
            });
            return report;
        }
    };
    report.final_event_hash = log.events.last().map(|record| return record.hash.clone());
    let completion = match read_completion(session_directory) {
        Ok(completion) => completion,
        Err(error) => {
            report.findings.push(VerificationFinding {
                code: "completion_invalid_or_missing".to_owned(),
                message: error.to_string(),
            });
            return report;
        }
    };
    if let Err(error) = validate_completion_format(&completion) {
        report.findings.push(VerificationFinding {
            code: "completion_format_unsupported".to_owned(),
            message: error.to_string(),
        });
        return report;
    }
    if level == VerificationLevel::Structure {
        report.valid = true;
        return report;
    }
    if let Err(error) = validate_completed_log(session_directory, &log) {
        report.findings.push(VerificationFinding {
            code: content_finding_code(&error).to_owned(),
            message: error.to_string(),
        });
        return report;
    }
    match level {
        VerificationLevel::Structure | VerificationLevel::Content => report.valid = true,
        VerificationLevel::DurabilityRecord => {
            report.findings.push(VerificationFinding {
                code: "durability_record_unavailable".to_owned(),
                message: "this format does not yet contain a verifiable durability record"
                    .to_owned(),
            });
        }
        VerificationLevel::Signature => {
            report.findings.push(VerificationFinding {
                code: "signature_receipt_missing".to_owned(),
                message: "no detached evidence receipt is available".to_owned(),
            });
        }
    }
    return report;
}

/// Read a session manifest from a session directory.
///
/// # Errors
///
/// Returns an error when the manifest cannot be read or decoded.
pub fn read_manifest(session_directory: &Path) -> Result<SessionManifest> {
    let path = session_directory.join("session.json");
    let bytes = fs_err::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    return serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let json_path = error.path().to_string();
        let source = error.into_inner();
        return Error::InvalidJson {
            path,
            json_path,
            source,
        };
    });
}

/// Read immutable completion facts from a completed session directory.
///
/// # Errors
///
/// Returns an error when the completion manifest cannot be read or decoded.
pub fn read_completion(session_directory: &Path) -> Result<SessionCompletion> {
    let path = session_directory.join("completion.json");
    let bytes = fs_err::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    return serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let json_path = error.path().to_string();
        let source = error.into_inner();
        return Error::InvalidJson {
            path,
            json_path,
            source,
        };
    });
}

/// Verify that an event log is the exact immutable log named by completion.
///
/// # Errors
///
/// Returns an error when completion is absent or disagrees with canonical data.
pub fn validate_completed_log(
    session_directory: &Path,
    log: &EventLog,
) -> Result<SessionCompletion> {
    if !session_directory.join("completion.json").is_file() {
        return Err(Error::SessionIncomplete {
            session: session_directory.to_path_buf(),
        });
    }
    let manifest_path = session_directory.join("session.json");
    let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
        return Error::Filesystem {
            path: manifest_path.clone(),
            source,
        };
    })?;
    let manifest = read_manifest(session_directory)?;
    validate_manifest(&manifest)?;
    if canonical_manifest_bytes(&manifest)? != manifest_bytes {
        return Err(Error::InvalidReplay {
            reason: "session manifest bytes are not in the canonical serialization".to_owned(),
        });
    }
    let completion = read_completion(session_directory)?;
    validate_completion_format(&completion)?;
    let final_event_hash = log.events.last().map(|record| return record.hash.as_str());
    let event_count = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
    let actual_manifest_hash = manifest_hash(&manifest_bytes);
    let header_bytes = serde_json::to_vec(&log.header).map_err(|source| {
        return Error::InvalidJson {
            path: session_directory.join("events.jsonl"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    let event_header_hash = blake3::hash(&header_bytes).to_hex().to_string();
    if manifest.session_id != log.header.session_id
        || manifest.started_unix_ns != log.header.started_unix_ns
        || manifest.geometry.cols != log.header.cols
        || manifest.geometry.rows != log.header.rows
        || manifest.geometry.pixel_width != log.header.pixel_width
        || manifest.geometry.pixel_height != log.header.pixel_height
    {
        return Err(Error::InvalidReplay {
            reason: "session manifest identity or geometry does not match the event-log header"
                .to_owned(),
        });
    }
    if log.header.session_manifest_hash != actual_manifest_hash {
        return Err(Error::InvalidReplay {
            reason: format!(
                "event-log session manifest hash `{}` does not match actual hash `{actual_manifest_hash}`",
                log.header.session_manifest_hash
            ),
        });
    }
    if completion.session_id != log.header.session_id {
        return Err(Error::InvalidReplay {
            reason: "completion session identifier does not match the event log".to_owned(),
        });
    }
    if completion.session_manifest_hash != actual_manifest_hash {
        return Err(Error::InvalidReplay {
            reason: "completion session manifest hash does not match session.json".to_owned(),
        });
    }
    if completion.event_header_hash != event_header_hash {
        return Err(Error::InvalidReplay {
            reason: "completion event header hash does not match events.jsonl".to_owned(),
        });
    }
    if final_event_hash != Some(completion.final_event_hash.as_str()) {
        return Err(Error::InvalidReplay {
            reason: format!(
                "completion final hash `{}` does not match event-log hash `{}`",
                completion.final_event_hash,
                final_event_hash.unwrap_or("<missing>")
            ),
        });
    }
    if completion.event_count != event_count {
        return Err(Error::InvalidReplay {
            reason: format!(
                "completion event count {} does not match event-log count {event_count}",
                completion.event_count
            ),
        });
    }
    let Some(terminal) = log.events.last() else {
        return Err(Error::InvalidReplay {
            reason: "completed event log has no terminal Exit event".to_owned(),
        });
    };
    let Event::Exit {
        code, signal_name, ..
    } = &terminal.event
    else {
        return Err(Error::InvalidReplay {
            reason: "completed event log does not end with an Exit event".to_owned(),
        });
    };
    if completion.terminal_event_seq != terminal.seq {
        return Err(Error::InvalidReplay {
            reason: "completion terminal event sequence does not match the final Exit event"
                .to_owned(),
        });
    }
    if completion.exit_code != *code || completion.signal_name != *signal_name {
        return Err(Error::InvalidReplay {
            reason: "completion exit outcome does not match the final Exit event".to_owned(),
        });
    }
    if !log.pending_operations.is_empty() {
        return Err(Error::InvalidReplay {
            reason: "completed event log contains pending mutation intents".to_owned(),
        });
    }
    let output_bytes = log.events.iter().try_fold(0_u64, |total, record| {
        let bytes = match &record.event {
            Event::Output { data } => u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX),
            _ => 0,
        };
        return total.checked_add(bytes).ok_or_else(|| {
            return Error::InvalidReplay {
                reason: "canonical output byte count overflow".to_owned(),
            };
        });
    })?;
    if completion.output_bytes != output_bytes {
        return Err(Error::InvalidReplay {
            reason: format!(
                "completion output byte count {} does not match event-log count {output_bytes}",
                completion.output_bytes
            ),
        });
    }
    return Ok(completion);
}

fn validate_manifest(manifest: &SessionManifest) -> Result<()> {
    if manifest.format != SESSION_MANIFEST_FORMAT {
        return Err(Error::InvalidReplay {
            reason: format!("unsupported session format `{}`", manifest.format),
        });
    }
    if manifest.version != SESSION_MANIFEST_VERSION {
        return Err(Error::InvalidReplay {
            reason: format!("unsupported session format version {}", manifest.version),
        });
    }
    return Ok(());
}

fn validate_completion_format(completion: &SessionCompletion) -> Result<()> {
    if completion.format != SESSION_COMPLETION_FORMAT {
        return Err(Error::InvalidReplay {
            reason: format!("unsupported completion format `{}`", completion.format),
        });
    }
    if completion.version != SESSION_COMPLETION_VERSION {
        return Err(Error::InvalidReplay {
            reason: format!(
                "unsupported completion format version {}",
                completion.version
            ),
        });
    }
    if completion.validation_version != COMPLETION_VALIDATION_VERSION {
        return Err(Error::InvalidReplay {
            reason: format!(
                "unsupported completion validation version {}",
                completion.validation_version
            ),
        });
    }
    return Ok(());
}

fn content_finding_code(error: &Error) -> &'static str {
    let Error::InvalidReplay { reason } = error else {
        return "canonical_content_invalid";
    };
    if reason.contains("session manifest hash") {
        return "session_manifest_hash_mismatch";
    }
    if reason.contains("event header hash") {
        return "event_header_hash_mismatch";
    }
    if reason.contains("does not end with an Exit") || reason.contains("no terminal Exit") {
        return "terminal_exit_missing";
    }
    if reason.contains("pending mutation") {
        return "pending_mutation_intents";
    }
    if reason.contains("event count") {
        return "event_count_mismatch";
    }
    if reason.contains("output byte count") {
        return "output_count_mismatch";
    }
    if reason.contains("final hash") {
        return "final_event_hash_mismatch";
    }
    if reason.contains("exit outcome") {
        return "exit_outcome_mismatch";
    }
    return "canonical_content_invalid";
}

#[cfg(test)]
mod tests {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt as _};

    use tempfile::TempDir;

    use super::*;
    use crate::{
        event::{EventLogHeader, InputKind, InputSource},
        event_log::{EventLogWriter, SessionClock},
    };

    fn completed_fixture(events: Vec<Event>) -> (TempDir, EventLog) {
        let temporary = TempDir::new().expect("temporary directory");
        let session = temporary.path().join("session");
        fs_err::create_dir(&session).expect("session directory");
        fs_err::set_permissions(&session, Permissions::from_mode(0o700))
            .expect("secure session directory");
        let clock = SessionClock::now().expect("session clock");
        let session_id = Uuid::now_v7();
        let manifest = SessionManifest {
            format: SESSION_MANIFEST_FORMAT.to_owned(),
            version: SESSION_MANIFEST_VERSION,
            session_id,
            started_unix_ns: clock.started_unix_ns(),
            command: vec![ByteString::from(&b"true"[..])],
            working_directory: Some(ByteString::from(&b"/tmp"[..])),
            geometry: SessionGeometry {
                cols: 80,
                rows: 24,
                pixel_width: 720,
                pixel_height: 432,
            },
            geometry_provenance: GeometryProvenance::default(),
            ghostty_visual: None,
            child_environment: BTreeMap::new(),
            termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
            operating_system: "linux".to_owned(),
            architecture: "test".to_owned(),
            event_log: "events.jsonl".to_owned(),
            control_socket: "control.sock".to_owned(),
            contains_sensitive_terminal_data: true,
        };
        let manifest_bytes = canonical_manifest_bytes(&manifest).expect("manifest JSON");
        fs_err::write(session.join("session.json"), &manifest_bytes).expect("write manifest");
        let manifest_hash = manifest_hash(&manifest_bytes);
        let header = EventLogHeader::new(
            session_id,
            &manifest_hash,
            clock.started_unix_ns(),
            80,
            24,
            720,
            432,
            "xterm-ghostty",
        );
        let header_bytes = serde_json::to_vec(&header).expect("header JSON");
        let mut writer = EventLogWriter::create(session.join("events.jsonl"), &header, clock)
            .expect("event log");
        for event in events {
            writer.append(event).expect("append event");
        }
        let event_count = writer.next_seq();
        let final_event_hash = writer.current_hash();
        drop(writer);
        let completion = SessionCompletion {
            format: SESSION_COMPLETION_FORMAT.to_owned(),
            version: SESSION_COMPLETION_VERSION,
            validation_version: COMPLETION_VALIDATION_VERSION,
            session_id,
            session_manifest_hash: manifest_hash,
            event_header_hash: blake3::hash(&header_bytes).to_hex().to_string(),
            exit_code: None,
            signal_name: None,
            output_bytes: 0,
            final_event_hash,
            event_count,
            terminal_event_seq: event_count.saturating_sub(1),
        };
        fs_err::write(
            session.join("completion.json"),
            serde_json::to_vec(&completion).expect("completion JSON"),
        )
        .expect("write completion");
        let log = EventLog::read(session.join("events.jsonl")).expect("read event log");
        return (temporary, log);
    }

    #[test]
    fn completed_validation_requires_terminal_exit() {
        let (temporary, log) = completed_fixture(vec![Event::Marker {
            label: "not-an-exit".to_owned(),
        }]);
        let error = validate_completed_log(&temporary.path().join("session"), &log)
            .expect_err("nonterminal completion must fail");
        assert!(matches!(
            error,
            Error::InvalidReplay { reason } if reason.contains("does not end with an Exit")
        ));
    }

    #[test]
    fn completed_validation_rejects_pending_mutation() {
        let operation_id = Uuid::now_v7();
        let (temporary, log) = completed_fixture(vec![
            Event::InputIntent {
                operation_id,
                source: InputSource::Agent,
                input_kind: InputKind::Text,
                data: ByteString::from(&b"x"[..]),
                intent: None,
            },
            Event::Exit {
                code: None,
                signal: None,
                signal_name: None,
            },
        ]);
        let error = validate_completed_log(&temporary.path().join("session"), &log)
            .expect_err("pending mutation must fail");
        assert!(matches!(
            error,
            Error::InvalidReplay { reason } if reason.contains("pending mutation")
        ));
    }
}
