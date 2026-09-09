//! Versioned events that make up the canonical terminal execution record.

use super::application::ApplicationEvent;
use crate::value::ByteString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current on-disk event-log format version.
pub const EVENT_LOG_VERSION: u32 = 6;
/// Stable event-log format identifier.
pub const EVENT_LOG_FORMAT: &str = "termtrace-events";

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
    /// Terminal-window focus transition.
    Focus,
    /// Cell-addressed mouse or wheel operation.
    Pointer,
}

/// Structured semantic provenance for exact input bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputDescriptor {
    /// Uninterpreted caller or host bytes.
    Raw,
    /// UTF-8 text intentionally entered as textual content.
    Text,
    /// One semantic key encoded against a live terminal-mode snapshot.
    Key {
        /// Stable symbolic key spelling.
        key: String,
        /// Shift modifier.
        shift: bool,
        /// Control modifier.
        control: bool,
        /// Alt modifier.
        alt: bool,
        /// Super modifier.
        super_key: bool,
        /// Stable encoder identity.
        encoder: String,
        /// BLAKE3 of canonical JSON for the terminal mode snapshot.
        terminal_modes_hash: String,
    },
    /// Paste content encoded against live bracketed-paste mode.
    Paste {
        /// Whether the actor applied bracketed-paste wrappers.
        bracketed: bool,
        /// BLAKE3 of canonical JSON for the terminal mode snapshot.
        terminal_modes_hash: String,
    },
    /// Focus transition encoded while DEC focus reporting was enabled.
    Focus {
        /// Whether the terminal gained (`true`) or lost (`false`) focus.
        focused: bool,
        /// Pinned encoder identity.
        encoder: String,
        /// BLAKE3 of canonical JSON for the terminal mode snapshot.
        terminal_modes_hash: String,
    },
    /// Pointer operation encoded against live mouse modes and geometry.
    Pointer {
        /// `click`, `multi_click`, `drag`, or `scroll`.
        operation: String,
        /// Button name for clicks, otherwise wheel direction.
        button: String,
        /// Signed requested wheel lines, or zero for clicks.
        lines: i16,
        /// Zero-based viewport column.
        column: u16,
        /// Zero-based viewport row.
        row: u16,
        /// Optional zero-based destination column for a drag.
        end_column: Option<u16>,
        /// Optional zero-based destination row for a drag.
        end_row: Option<u16>,
        /// Complete click count, or zero for non-click operations.
        click_count: u8,
        /// Terminal columns at encoding time.
        cols: u16,
        /// Terminal rows at encoding time.
        rows: u16,
        /// Total terminal surface width in pixels.
        pixel_width: u16,
        /// Total terminal surface height in pixels.
        pixel_height: u16,
        /// Pinned encoder identity.
        encoder: String,
        /// BLAKE3 of canonical JSON for the terminal mode snapshot.
        terminal_modes_hash: String,
    },
    /// Semantic visible-text resolution followed by a mode-aware click.
    Activate {
        /// Exact matched text.
        text: String,
        /// Zero-based first matched column.
        start_column: u16,
        /// Zero-based exclusive matched column.
        end_column: u16,
        /// Zero-based matched row.
        row: u16,
        /// Cell chosen for the click.
        target_column: u16,
        /// Presented-state hash used for resolution and encoding.
        presented_state_hash: String,
        /// Current grid columns.
        cols: u16,
        /// Current grid rows.
        rows: u16,
        /// Pinned encoder identity.
        encoder: String,
        /// BLAKE3 of canonical JSON for the terminal mode snapshot.
        terminal_modes_hash: String,
    },
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
        /// Structured semantic source of the exact emitted bytes.
        descriptor: InputDescriptor,
        /// Exact emitted bytes.
        data: ByteString,
        /// BLAKE3 of canonical terminal modes used to encode this atom, when mode-aware.
        terminal_modes_hash: Option<String>,
        /// Canonical symbolic input, such as `ctrl+u` or `paste`.
        semantic_input: Option<String>,
        /// Optional caller-supplied reason for applying the input.
        caller_intent: Option<String>,
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
    /// Structured application instrumentation received over the control socket.
    Application {
        /// Caller-owned request identity for durable deduplication.
        operation_id: Uuid,
        /// Caller-reported diagnostic payload.
        data: ApplicationEvent,
    },
    /// A terminal capability query and corresponding response.
    QueryResponse {
        /// Canonical output event containing the end of the query.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_event_seq: Option<u64>,
        /// Inclusive offset of the query's final byte in that output event.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_byte_offset: Option<u64>,
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

impl Event {
    /// Total decoded bytes owned by byte-bearing fields in this event.
    pub fn decoded_payload_bytes(&self) -> u64 {
        let length = match self {
            Self::Output { data } | Self::InputIntent { data, .. } => data.as_bytes().len(),
            Self::QueryResponse {
                query, response, ..
            } => query
                .as_bytes()
                .len()
                .saturating_add(response.as_bytes().len()),
            _ => 0,
        };

        return u64::try_from(length).unwrap_or(u64::MAX);
    }
}

/// Integrity-chained event persisted as one JSON line.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventRecord {
    /// Contiguous zero-based event sequence.
    pub seq: u64,
    /// Nanoseconds elapsed from the session's monotonic origin.
    pub monotonic_ns: u64,
    /// PTY read completion time, when observed separately from canonical append time.
    /// This is not the application's write time or a framebuffer presentation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_monotonic_ns: Option<u64>,
    /// BLAKE3 hash of the preceding header or event.
    pub prev_hash: String,
    /// BLAKE3 hash of this record's canonical body and `prev_hash`.
    pub hash: String,
    /// Event payload.
    pub event: Event,
}

/// Hash-and-length summary for one elided canonical byte field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BytePayloadSummary {
    /// Event field name (`data`, `query`, or `response`).
    pub field: String,
    /// Exact byte count.
    pub byte_count: u64,
    /// BLAKE3 of the exact bytes.
    pub blake3: String,
}

/// Stable JSONL projection of one canonical event record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventProjection {
    /// Canonical event sequence.
    pub seq: u64,
    /// Session-monotonic event timestamp.
    pub monotonic_ns: u64,
    /// Canonical event hash.
    pub hash: String,
    /// Stable event variant name.
    pub event_type: String,
    /// Whether exact byte fields were elided.
    pub metadata_only: bool,
    /// Exact typed event when byte payloads were retained.
    pub event: Option<Event>,
    /// Non-byte event fields when exact payloads were elided.
    pub metadata: Option<serde_json::Value>,
    /// Hash-and-length summaries when byte fields were elided.
    pub byte_payloads: Vec<BytePayloadSummary>,
}

impl EventProjection {
    /// Project one canonical record, optionally eliding exact byte payloads.
    #[must_use]
    pub fn from_record(record: &EventRecord, metadata_only: bool) -> Self {
        let byte_payloads = if metadata_only { byte_summaries(&record.event) } else { Vec::new() };

        return Self {
            seq: record.seq,
            monotonic_ns: record.monotonic_ns,
            hash: record.hash.clone(),
            event_type: event_type(&record.event).to_owned(),
            metadata_only,
            event: (!metadata_only).then(|| return record.event.clone()),
            metadata: metadata_only.then(|| return event_metadata(&record.event)),
            byte_payloads,
        };
    }
}

fn event_metadata(event: &Event) -> serde_json::Value {
    match event {
        Event::Application { operation_id, data } => {
            return serde_json::json!({
                "type": "application",
                "operation_id": operation_id,
                "process_instance_id": data.process_instance_id,
                "process_id": data.process_id,
                "kind": data.kind,
                "span_id": data.span_id,
                "parent_span_id": data.parent_span_id,
            });
        }
        Event::Output { .. } => {
            return serde_json::json!({
                "type": "output",
            });
        }
        Event::InputIntent {
            operation_id,
            source,
            input_kind,
            descriptor,
            terminal_modes_hash,
            semantic_input,
            caller_intent,
            ..
        } => {
            return serde_json::json!({
                "type": "input_intent",
                "operation_id": operation_id,
                "source": source,
                "input_kind": input_kind,
                "descriptor": descriptor,
                "terminal_modes_hash": terminal_modes_hash,
                "semantic_input": semantic_input,
                "caller_intent": caller_intent,
            });
        }
        Event::QueryResponse { responder, .. } => {
            return serde_json::json!({
                "type": "query_response",
                "responder": responder,
            });
        }
        other => return serde_json::to_value(other).unwrap_or(serde_json::Value::Null),
    }
}

/// Stable snake-case event variant name.
#[must_use]
pub fn event_type(event: &Event) -> &'static str {
    return match event {
        Event::Output { .. } => "output",
        Event::InputIntent { .. } => "input_intent",
        Event::InputResult { .. } => "input_result",
        Event::ResizeIntent { .. } => "resize_intent",
        Event::ResizeResult { .. } => "resize_result",
        Event::SignalIntent { .. } => "signal_intent",
        Event::SignalResult { .. } => "signal_result",
        Event::Marker { .. } => "marker",
        Event::Application { .. } => "application",
        Event::QueryResponse { .. } => "query_response",
        Event::Lifecycle { .. } => "lifecycle",
        Event::Exit { .. } => "exit",
        Event::HostError { .. } => "host_error",
    };
}

fn byte_summaries(event: &Event) -> Vec<BytePayloadSummary> {
    return match event {
        Event::Output { data } | Event::InputIntent { data, .. } => {
            vec![byte_summary("data", data)]
        }
        Event::QueryResponse {
            query, response, ..
        } => vec![
            byte_summary("query", query),
            byte_summary("response", response),
        ],
        _ => Vec::new(),
    };
}

fn byte_summary(field: &str, bytes: &ByteString) -> BytePayloadSummary {
    return BytePayloadSummary {
        field: field.to_owned(),
        byte_count: u64::try_from(bytes.as_bytes().len()).unwrap_or(u64::MAX),
        blake3: blake3::hash(bytes.as_bytes()).to_hex().to_string(),
    };
}

/// Canonical portion of an event covered by its integrity hash.
#[derive(Serialize)]
pub struct EventBody<'event> {
    /// Monotonic event sequence.
    pub seq: u64,
    /// Session-monotonic event time.
    pub monotonic_ns: u64,
    /// Original observation time, covered by the canonical integrity chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_monotonic_ns: Option<u64>,
    /// Event payload.
    pub event: &'event Event,
}
