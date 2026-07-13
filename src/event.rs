//! Versioned events that make up the canonical terminal execution record.

use std::fmt;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use uuid::Uuid;

/// Current on-disk event-log format version.
pub const EVENT_LOG_VERSION: u32 = 1;

/// Stable event-log format identifier.
pub const EVENT_LOG_FORMAT: &str = "termtrace-events";

/// Raw bytes serialized as standard padded Base64.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ByteString(Vec<u8>);

impl ByteString {
    /// Wrap an owned byte buffer.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Borrow the original bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consume this value and return the original bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for ByteString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ByteString")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl From<Vec<u8>> for ByteString {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

impl From<&[u8]> for ByteString {
    fn from(bytes: &[u8]) -> Self {
        Self::new(bytes.to_vec())
    }
}

impl Serialize for ByteString {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for ByteString {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        STANDARD
            .decode(encoded)
            .map(Self)
            .map_err(de::Error::custom)
    }
}

/// Metadata written before the first event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventLogHeader {
    /// Constant format identifier.
    pub format: String,
    /// Schema version.
    pub version: u32,
    /// Globally unique session identifier.
    pub session_id: Uuid,
    /// Wall-clock Unix timestamp corresponding to monotonic time zero.
    pub started_unix_ns: u128,
    /// Initial terminal columns.
    pub cols: u16,
    /// Initial terminal rows.
    pub rows: u16,
    /// Terminal identity exposed to the child process.
    pub term: String,
}

impl EventLogHeader {
    /// Construct a header for a new recording.
    pub fn new(
        session_id: Uuid,
        started_unix_ns: u128,
        cols: u16,
        rows: u16,
        term: impl Into<String>,
    ) -> Self {
        Self {
            format: EVENT_LOG_FORMAT.to_owned(),
            version: EVENT_LOG_VERSION,
            session_id,
            started_unix_ns,
            cols,
            rows,
            term: term.into(),
        }
    }
}

/// Origin of bytes sent to the child process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    /// Input received from the attached native terminal.
    Human,
    /// Input requested through the agent control protocol.
    Agent,
    /// Automatic terminal-host response or internal control input.
    Host,
}

/// User-facing interpretation of an input byte sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    /// Uninterpreted bytes.
    Raw,
    /// Typed textual content.
    Text,
    /// One or more symbolic keyboard keys.
    Keys,
    /// Pasted content.
    Paste,
}

/// Why a process lifecycle transition occurred.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleKind {
    /// The PTY and child process were created.
    Spawned,
    /// The child output stream reached end-of-file.
    OutputClosed,
    /// The session host began orderly shutdown.
    Stopping,
}

/// One ordered session event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    /// Bytes read from the child PTY.
    Output {
        /// Exact bytes, including control sequences and invalid UTF-8.
        data: ByteString,
    },
    /// Bytes sent to the child PTY.
    Input {
        /// Input origin.
        source: InputSource,
        /// High-level input category.
        input_kind: InputKind,
        /// Exact emitted bytes.
        data: ByteString,
        /// Optional symbolic intent such as `ctrl-c` or `paste`.
        intent: Option<String>,
    },
    /// Terminal geometry changed.
    Resize {
        /// Columns after the resize.
        cols: u16,
        /// Rows after the resize.
        rows: u16,
        /// Cell width reported to the child, in pixels.
        cell_width_px: u16,
        /// Cell height reported to the child, in pixels.
        cell_height_px: u16,
    },
    /// A signal was sent to the child process or process group.
    Signal {
        /// Platform signal number.
        number: i32,
        /// Stable display name such as `SIGINT`.
        name: String,
    },
    /// A named point in the session timeline.
    Marker {
        /// User-provided marker label.
        label: String,
    },
    /// A terminal capability query and corresponding response.
    QueryResponse {
        /// Query bytes emitted by the child.
        query: ByteString,
        /// Response bytes sent back to the child.
        response: ByteString,
        /// Component that generated the response.
        responder: String,
    },
    /// A process or host lifecycle transition.
    Lifecycle {
        /// Transition kind.
        lifecycle_kind: LifecycleKind,
        /// Child process ID when available.
        pid: Option<u32>,
    },
    /// Child process termination.
    Exit {
        /// Exit status when the process exited normally.
        code: Option<i32>,
        /// Signal number when signal termination occurred.
        signal: Option<i32>,
    },
    /// A host failure retained in the canonical timeline.
    HostError {
        /// Stable diagnostic code.
        code: String,
        /// Redacted human-readable message.
        message: String,
    },
}

/// Integrity-chained event persisted as one JSON line.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventRecord {
    /// Contiguous zero-based event sequence.
    pub seq: u64,
    /// Nanoseconds elapsed from the session's monotonic origin.
    pub monotonic_ns: u64,
    /// BLAKE3 hash of the preceding header or event.
    pub prev_hash: String,
    /// BLAKE3 hash of this record's canonical body and `prev_hash`.
    pub hash: String,
    /// Event payload.
    pub event: Event,
}

/// Canonical portion of an event covered by its integrity hash.
#[derive(Serialize)]
pub(crate) struct EventBody<'a> {
    pub(crate) seq: u64,
    pub(crate) monotonic_ns: u64,
    pub(crate) event: &'a Event,
}
