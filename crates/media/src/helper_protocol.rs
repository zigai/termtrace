//! The hidden Termtrace subcommands native capture runs inside Ghostty.
//!
//! Media builds these helper invocations; the CLI's argument parser names the same
//! subcommands and long flags with the constants here, so both sides share one contract.
//! Flags are long-option names without the leading `--`.

/// Timed canonical-event playback run as Ghostty's child process.
pub mod playback {
    /// Subcommand of the Termtrace executable.
    pub const SUBCOMMAND: &str = "playback";
    /// Source session directory.
    pub const SESSION: &str = "session";
    /// Playback clock handshake path.
    pub const TIMING_FILE: &str = "timing-file";
    /// Structured failure handshake path.
    pub const ERROR_FILE: &str = "error-file";
    /// Playback-terminal geometry handshake path.
    pub const GEOMETRY_FILE: &str = "geometry-file";
    /// Final-frame readiness path.
    pub const READY_FILE: &str = "ready-file";
    /// Delay before event time zero, in milliseconds.
    pub const START_DELAY_MS: &str = "start-delay-ms";
    /// Final-frame hold duration, in milliseconds.
    pub const FINAL_HOLD_MS: &str = "final-hold-ms";
    /// Inclusive source offset of a ranged capture.
    pub const FROM_NS: &str = "from-ns";
    /// Exclusive source offset of a ranged capture.
    pub const TO_NS: &str = "to-ns";
    /// Exact endpoint event of a single-state capture.
    pub const ENDPOINT_EVENT_SEQ: &str = "endpoint-event-seq";
    /// Inclusive byte offset within the endpoint output event.
    pub const ENDPOINT_EVENT_BYTE_OFFSET: &str = "endpoint-event-byte-offset";
    /// Disable source-time sleeps while replaying an exact endpoint.
    pub const INSTANT: &str = "instant";
}

/// Persistent endpoint replay run as Ghostty's child process.
pub mod playback_worker {
    /// Subcommand of the Termtrace executable.
    pub const SUBCOMMAND: &str = "playback-worker";
    /// Canonical source session.
    pub const SESSION: &str = "session";
    /// Private parent protocol socket.
    pub const SOCKET: &str = "socket";
    /// Initial measured terminal geometry.
    pub const GEOMETRY_FILE: &str = "geometry-file";
}

/// Live terminal geometry probe run as Ghostty's child process.
pub mod geometry_probe {
    /// Subcommand of the Termtrace executable.
    pub const SUBCOMMAND: &str = "geometry-probe";
    /// New file receiving the measured terminal geometry.
    pub const OUT: &str = "out";
}

/// `--flag` spelling of a long-option name.
pub(crate) fn flag(name: &str) -> String {
    return format!("--{name}");
}
