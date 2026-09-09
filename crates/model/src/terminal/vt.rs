//! Stable lossless VT token contracts.

use crate::value::ByteString;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Stable VT token family. This is framing, not terminal interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VtTokenKind {
    /// Printable bytes, including valid UTF-8 continuation bytes.
    Printable,
    /// One C0 control byte.
    C0,
    /// One 8-bit C1 control byte.
    C1,
    /// ESC sequence.
    Esc,
    /// Control Sequence Introducer sequence.
    Csi,
    /// Operating System Command string.
    Osc,
    /// Device Control String.
    Dcs,
    /// Application Program Command string.
    Apc,
    /// Syntactically malformed or unrecognized sequence.
    Unknown,
    /// Sequence still open at end of canonical output.
    Incomplete,
}

/// Half-open byte range within one canonical output event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VtSourceSegment {
    /// Canonical output event sequence.
    pub event_seq: u64,
    /// Inclusive byte offset within the event payload.
    pub start_byte_offset: u64,
    /// Exclusive byte offset within the event payload.
    pub end_byte_offset: u64,
    /// Session-monotonic event timestamp.
    pub monotonic_ns: u64,
}

/// One lossless VT frame with exact canonical source ranges.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VtToken {
    /// Dense token identifier.
    pub token_id: u64,
    /// Framed sequence family.
    pub kind: VtTokenKind,
    /// Exact bytes, including introducer and terminator.
    pub raw: ByteString,
    /// Ordered exact source segments; concatenating them reproduces `raw`.
    pub source: Vec<VtSourceSegment>,
    /// Stable neutral description for common controls.
    pub description: String,
    /// Timing resolution; current recordings timestamp complete PTY reads.
    pub timing_resolution: String,
}
