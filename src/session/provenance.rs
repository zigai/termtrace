use super::*;

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
#[serde(deny_unknown_fields)]
pub struct GeometryProvenance {
    /// How the initial geometry was selected.
    pub source: GeometrySource,
    /// Intended native-capture viewport, when selected during recording.
    pub capture_target: Option<CaptureTarget>,
}

/// Focused Termtrace overrides applied to a Ghostty visual profile.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct GhosttyVisualProvenance {
    /// Original custom configuration path bytes, when supplied.
    pub source_config: Option<ByteString>,
    /// Focused overrides applied after resolving the source configuration.
    pub overrides: GhosttyVisualOverrides,
    /// Relative canonical replay-profile artifact.
    pub replay_profile: String,
    /// BLAKE3 of the exact replay-profile input bytes.
    pub replay_profile_hash: String,
    /// BLAKE3 of the exact effective configuration bytes freshly reported by Ghostty.
    pub effective_config_hash: String,
}

/// Immutable evidence binding from a successor session to its predecessor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionLineage {
    /// Canonical predecessor session identifier.
    pub predecessor_session_id: Uuid,
    /// BLAKE3 of the exact predecessor `session.json` bytes.
    pub predecessor_manifest_hash: String,
    /// Chain root of the predecessor's final canonical event.
    pub predecessor_final_event_hash: String,
    /// Stable reason for creating the successor.
    pub reason: String,
}

/// Chain-bound provenance for bytes imported from a noncanonical ANSI stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportProvenance {
    /// Stable source class, currently `ansi_stream`.
    pub source_kind: String,
    /// Exact imported byte count.
    pub byte_count: u64,
    /// BLAKE3 of the exact imported bytes.
    pub blake3: String,
    /// Caller-facing path bytes or `-` for standard input.
    pub source: ByteString,
}

/// How the native tmux wrapper selected the command running in its child PTY.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxPaneLaunchKind {
    /// tmux created an ordinary pane and Termtrace restored its configured login shell.
    DefaultShell,
    /// tmux had a nonempty default command which Termtrace preserved through its configured shell.
    DefaultCommand,
    /// The caller supplied an exact executable and argument vector.
    ExplicitArgv,
    /// The caller supplied one command interpreted by `/bin/sh -c`.
    ShellCommand,
}

/// Immutable tmux identity captured before a Termtrace-owned pane child is spawned.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TmuxPaneProvenance {
    /// Version of this tmux pane provenance contract.
    pub version: u32,
    /// tmux version reported by the server.
    pub tmux_version: String,
    /// Exact tmux server socket path bytes.
    pub socket_path: ByteString,
    /// tmux server process identifier.
    pub server_pid: u32,
    /// Stable tmux session identifier at pane launch.
    pub session_id: String,
    /// Session label at pane launch; labels may change later.
    pub session_name: ByteString,
    /// Unix timestamp in seconds at which tmux created the session.
    pub session_created_unix_seconds: u64,
    /// Stable tmux window identifier at pane launch.
    pub window_id: String,
    /// Window index at pane launch.
    pub window_index: u32,
    /// Window label at pane launch; labels may change later.
    pub window_name: ByteString,
    /// Stable tmux pane identifier.
    pub pane_id: String,
    /// Pane index within the window at launch.
    pub pane_index: u32,
    /// Outer pane process identifier assigned to the Termtrace wrapper.
    pub pane_pid: u32,
    /// tmux pane width inherited by the child PTY.
    pub pane_width: u16,
    /// tmux pane height inherited by the child PTY.
    pub pane_height: u16,
    /// Exact tmux start-command display captured for diagnostics.
    pub pane_start_command: ByteString,
    /// How the wrapper selected the recorded child command.
    pub launch_kind: TmuxPaneLaunchKind,
}
