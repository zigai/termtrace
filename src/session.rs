//! Stable session provenance and completion manifests.

use std::{collections::BTreeMap, path::Path};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result, event::ByteString};

/// Session manifest format identifier.
pub const SESSION_MANIFEST_FORMAT: &str = "termtrace-session";
/// Session manifest schema version.
pub const SESSION_MANIFEST_VERSION: u32 = 1;

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
pub struct SessionCompletion {
    /// Session identifier.
    pub session_id: Uuid,
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
