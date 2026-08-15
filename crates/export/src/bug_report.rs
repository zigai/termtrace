//! Bounded, redacted bug-report evidence exports.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use termtrace_engine::{
    analysis::last_action::analyze_last_action, diff::inspect_point, observation::observe_session,
};
use termtrace_evidence::{
    artifact::{
        collect_artifact_members, commit_directory, create_private_directory, sync_directory,
        write_secure,
    },
    event_log::EventLog,
    session::{read_manifest, validate_completed_log},
};
use termtrace_model::{
    artifact::metadata::{ArtifactMember, ArtifactSensitivity},
    execution::event::Event,
    execution::session::SessionGeometry,
    terminal::observation::StatePoint,
    value::ByteString,
};
use termtrace_render::render_png;
use uuid::Uuid;

use crate::{Error, Result};

const MAX_BUG_REPORT_MEMBERS: usize = 16;

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
    /// Whether the export includes last-action analysis and diagnostics.
    pub includes_last_action: bool,
    /// Whole-file member inventory.
    pub members: Vec<ArtifactMember>,
}

/// Atomically publish a bounded, manifested bug-report evidence export.
///
/// The payload deliberately omits canonical events and raw input/output byte streams.
pub fn create(
    session: &Path,
    destination: &Path,
    include_screenshot: bool,
) -> Result<BugReportManifest> {
    let canonical = session
        .canonicalize()
        .map_err(|source| return Error::filesystem(session, source))?;
    if destination.starts_with(&canonical) {
        return Err(Error::invalid(
            "bug-report destination must be outside the canonical session",
        ));
    }
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let session_manifest = read_manifest(&canonical)?;
    let analysis = log
        .events
        .iter()
        .any(|record| matches!(&record.event, Event::InputIntent { .. }))
        .then(|| return analyze_last_action(&canonical, &log, 50))
        .transpose()?;
    let observation = observe_session(&canonical, StatePoint::LatestPresented)?;
    let parent = destination
        .parent()
        .ok_or_else(|| return Error::invalid("bug-report path has no parent"))?;
    if !parent.is_dir() {
        return Err(Error::invalid("bug-report parent directory does not exist"));
    }
    let name = destination
        .file_name()
        .ok_or_else(|| return Error::invalid("bug-report path has no basename"))?;
    let staging = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        Uuid::now_v7()
    ));
    create_private_directory(&staging)?;
    let published = (|| -> Result<BugReportManifest> {
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
        let mut members = collect_artifact_members(&staging)?;
        if members.len() > MAX_BUG_REPORT_MEMBERS {
            return Err(Error::invalid(format!(
                "bug report exceeds {MAX_BUG_REPORT_MEMBERS} members"
            )));
        }
        for member in &mut members {
            let (sensitivity, relationship, schema_type) = member_semantics(&member.path)
                .ok_or_else(|| {
                    return Error::invalid(format!(
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
    let manifest = match published {
        Ok(manifest) => manifest,
        Err(error) => {
            let _cleanup = fs_err::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _cleanup = fs_err::remove_dir_all(&staging);
        return Err(error.into());
    }
    return Ok(manifest);
}

fn member_semantics(
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
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|source| return Error::json(PathBuf::from(path), source))?;
    bytes.push(b'\n');
    write_secure(path, &bytes)?;
    return Ok(());
}
