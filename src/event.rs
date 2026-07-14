//! Versioned events that make up the canonical terminal execution record.

use std::{borrow::Cow, fmt, result::Result as StdResult};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use uuid::Uuid;

/// Current on-disk event-log format version.
pub const EVENT_LOG_VERSION: u32 = 3;

/// Stable event-log format identifier.
pub const EVENT_LOG_FORMAT: &str = "termtrace-events";

/// Raw bytes serialized as standard padded Base64.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ByteString(Vec<u8>);

impl ByteString {
    /// Wrap an owned byte buffer.
    pub fn new(bytes: Vec<u8>) -> Self {
        return Self(bytes);
    }

    /// Borrow the original bytes.
    pub fn as_bytes(&self) -> &[u8] {
        return &self.0;
    }

    /// Consume this value and return the original bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        return self.0;
    }
}

impl fmt::Debug for ByteString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return f
            .debug_struct("ByteString")
            .field("len", &self.0.len())
            .finish_non_exhaustive();
    }
}

impl From<Vec<u8>> for ByteString {
    fn from(bytes: Vec<u8>) -> Self {
        return Self::new(bytes);
    }
}

impl From<&[u8]> for ByteString {
    fn from(bytes: &[u8]) -> Self {
        return Self::new(bytes.to_vec());
    }
}

impl Serialize for ByteString {
    fn serialize<S>(&self, serializer: S) -> StdResult<S::Ok, S::Error>
    where
        S: Serializer,
    {
        return serializer.serialize_str(&STANDARD.encode(&self.0));
    }
}

impl<'de> Deserialize<'de> for ByteString {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        return STANDARD
            .decode(encoded)
            .map(Self)
            .map_err(de::Error::custom);
    }
}

impl schemars::JsonSchema for ByteString {
    fn schema_name() -> Cow<'static, str> {
        return Cow::Borrowed("Base64Bytes");
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        return schemars::json_schema!({
            "type": "string",
            "contentEncoding": "base64",
            "pattern": "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$"
        });
    }
}

/// Metadata written before the first event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventLogHeader {
    /// Constant format identifier.
    pub format: String,
    /// Schema version.
    pub version: u32,
    /// Globally unique session identifier.
    pub session_id: Uuid,
    /// BLAKE3 of the exact immutable `session.json` bytes.
    pub session_manifest_hash: String,
    /// Wall-clock Unix timestamp corresponding to monotonic time zero.
    pub started_unix_ns: u128,
    /// Initial terminal columns.
    pub cols: u16,
    /// Initial terminal rows.
    pub rows: u16,
    /// Initial terminal pixel width.
    pub pixel_width: u16,
    /// Initial terminal pixel height.
    pub pixel_height: u16,
    /// Terminal identity exposed to the child process.
    pub term: String,
    /// Default colors presented by the emulated terminal host.
    #[serde(default)]
    pub colors: TerminalColors,
}

/// Default terminal colors captured independently from application SGR output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerminalColors {
    /// Provenance of the color values.
    pub source: String,
    /// Default foreground as red, green, and blue bytes.
    pub foreground: [u8; 3],
    /// Default background as red, green, and blue bytes.
    pub background: [u8; 3],
    /// Default cursor color when explicitly configured.
    pub cursor: Option<[u8; 3]>,
    /// Complete 256-color palette, or empty to use libghostty's built-in palette.
    pub palette: Vec<[u8; 3]>,
}

impl Default for TerminalColors {
    fn default() -> Self {
        return Self {
            source: "libghostty_builtin".to_owned(),
            foreground: [0xff, 0xff, 0xff],
            background: [0, 0, 0],
            cursor: None,
            palette: Vec::new(),
        };
    }
}

impl EventLogHeader {
    /// Construct a header for a new recording.
    pub fn new(
        session_id: Uuid,
        session_manifest_hash: impl Into<String>,
        started_unix_ns: u128,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
        term: impl Into<String>,
    ) -> Self {
        return Self {
            format: EVENT_LOG_FORMAT.to_owned(),
            version: EVENT_LOG_VERSION,
            session_id,
            session_manifest_hash: session_manifest_hash.into(),
            started_unix_ns,
            cols,
            rows,
            pixel_width,
            pixel_height,
            term: term.into(),
            colors: TerminalColors::default(),
        };
    }

    /// Override the terminal color profile embedded in a new header.
    #[must_use]
    pub fn with_colors(mut self, colors: TerminalColors) -> Self {
        self.colors = colors;
        return self;
    }
}

/// Origin of bytes sent to the child process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleKind {
    /// The PTY and child process were created.
    Spawned,
    /// The child output stream reached end-of-file.
    OutputClosed,
    /// The session host began orderly shutdown.
    Stopping,
}

/// Result of a state-changing operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    /// The complete operation was applied once.
    Applied,
    /// The operation failed without a complete application.
    Failed,
}

/// One ordered session event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    /// Bytes read from the child PTY.
    Output {
        /// Exact bytes, including control sequences and invalid UTF-8.
        data: ByteString,
    },
    /// Durable intent to send bytes to the child PTY.
    InputIntent {
        /// Identifier paired with the corresponding result.
        operation_id: Uuid,
        /// Input origin.
        source: InputSource,
        /// High-level input category.
        input_kind: InputKind,
        /// Exact emitted bytes.
        data: ByteString,
        /// Optional symbolic intent such as `ctrl-c` or `paste`.
        intent: Option<String>,
    },
    /// Result of one input intent.
    InputResult {
        /// Identifier from the corresponding intent.
        operation_id: Uuid,
        /// Whether the entire byte sequence was applied.
        status: OperationStatus,
        /// Bytes written before success or failure.
        bytes_written: u64,
        /// Stable diagnostic code on failure.
        error_code: Option<String>,
    },
    /// Durable intent to change terminal geometry.
    ResizeIntent {
        /// Identifier paired with the corresponding result.
        operation_id: Uuid,
        /// Columns after the resize.
        cols: u16,
        /// Rows after the resize.
        rows: u16,
        /// Total pixel width reported to the child.
        pixel_width: u16,
        /// Total pixel height reported to the child.
        pixel_height: u16,
    },
    /// Result of one resize intent.
    ResizeResult {
        /// Identifier from the corresponding intent.
        operation_id: Uuid,
        /// Whether the new geometry was applied.
        status: OperationStatus,
        /// Stable diagnostic code on failure.
        error_code: Option<String>,
    },
    /// Durable intent to signal the child process group.
    SignalIntent {
        /// Identifier paired with the corresponding result.
        operation_id: Uuid,
        /// Platform signal number.
        number: i32,
        /// Stable display name such as `SIGINT`.
        name: String,
    },
    /// Result of one signal intent.
    SignalResult {
        /// Identifier from the corresponding intent.
        operation_id: Uuid,
        /// Whether the signal syscall succeeded.
        status: OperationStatus,
        /// Stable diagnostic code on failure.
        error_code: Option<String>,
    },
    /// A named point in the session timeline.
    Marker {
        /// Caller-owned operation identifier used for durable deduplication.
        operation_id: Uuid,
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
        /// Platform signal description when available.
        signal_name: Option<String>,
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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
pub(crate) struct EventBody<'event> {
    pub(crate) seq: u64,
    pub(crate) monotonic_ns: u64,
    pub(crate) event: &'event Event,
}
