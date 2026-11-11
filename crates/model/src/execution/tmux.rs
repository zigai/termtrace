//! Typed SDK input to the existing aggregate tmux recorder, not a second state machine.

use crate::value::ByteString;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Aggregate start settings. Exact bytes identify command arguments, targets, and paths.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TmuxRecordConfiguration {
    /// Unique aggregate name, validated by the existing tmux store.
    pub name: String,
    /// Start a new isolated server; false enables an existing selected server.
    pub new_server: bool,
    /// Full recorded tmux client command; its executable must match the selected namespace client.
    pub command: Vec<ByteString>,
    /// Existing integration's Ghostty capture policy.
    pub ghostty_profile: TmuxGhosttyProfile,
    /// Explicit Ghostty configuration path, if needed.
    pub ghostty_config: Option<ByteString>,
    /// Existing integration's uncovered-pane policy.
    pub coverage: TmuxCoverage,
    /// Use the existing patched pane-launch hook.
    pub launch_hook: bool,
    /// Record only explicitly launched panes.
    pub explicit_only: bool,
    /// Initial session scope targets, resolved by the existing tmux service.
    pub sessions: Vec<ByteString>,
    /// Initial window scope targets.
    pub windows: Vec<ByteString>,
    /// Initial stable pane scope targets.
    pub panes: Vec<ByteString>,
    /// Fallback cell width, matching CLI defaults.
    pub cell_width: u16,
    /// Fallback cell height, matching CLI defaults.
    pub cell_height: u16,
    /// Optional terminal identity override.
    pub term: Option<String>,
    /// Opt-in completed-pane retention age; absent means no age-based deletion.
    pub retain_days: Option<u64>,
    /// Opt-in completed-pane retention budget; absent means no budget-based deletion.
    pub retain_bytes: Option<u64>,
}

impl Default for TmuxRecordConfiguration {
    fn default() -> Self {
        return Self {
            name: String::new(),
            new_server: false,
            command: Vec::new(),
            ghostty_profile: TmuxGhosttyProfile::Auto,
            ghostty_config: None,
            coverage: TmuxCoverage::Warn,
            launch_hook: false,
            explicit_only: false,
            sessions: Vec::new(),
            windows: Vec::new(),
            panes: Vec::new(),
            cell_width: 12,
            cell_height: 24,
            term: None,
            retain_days: None,
            retain_bytes: None,
        };
    }
}

/// Existing Ghostty pane capture policies.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxGhosttyProfile {
    /// Capture when available.
    Auto,
    /// Fail unless capture is available.
    Required,
    /// Semantic evidence only.
    Off,
}

/// Existing uncovered-pane policies.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxCoverage {
    /// Record gaps without terminating uncovered panes.
    Warn,
    /// Permit the existing integration to terminate uncovered panes after its grace period.
    Strict,
}
