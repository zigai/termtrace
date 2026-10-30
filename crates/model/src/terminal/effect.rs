//! Terminal side effects derived by replaying canonical output.

use crate::value::ByteString;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Hex characters kept from a payload's BLAKE3 hash.
pub const PAYLOAD_HASH_HEX_CHARS: usize = 16;
/// Most leading bytes of an unsupported sequence kept to identify it.
pub const UNKNOWN_SEQUENCE_IDENTIFIER_BYTES: usize = 16;

/// Length and short hash of a payload that may hold a secret, without the payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PayloadDigest {
    /// Payload length in bytes.
    pub byte_length: u64,
    /// Leading hex characters of the payload's BLAKE3 hash.
    pub blake3: String,
}

impl PayloadDigest {
    /// Digest a payload.
    pub fn of(bytes: &[u8]) -> Self {
        let mut blake3 = blake3::hash(bytes).to_hex().to_string();
        blake3.truncate(PAYLOAD_HASH_HEX_CHARS);

        return Self {
            byte_length: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            blake3,
        };
    }
}

/// Clipboard a program asked to write.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardTarget {
    /// The standard system clipboard.
    Standard,
    /// The selection clipboard.
    Selection,
    /// The primary selection clipboard.
    Primary,
    /// A target this build does not name.
    Other,
}

/// One MIME representation of a clipboard write.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipboardRepresentation {
    /// MIME type of the representation.
    pub mime: String,
    /// Digest of the decoded representation data.
    pub data: PayloadDigest,
}

/// State of an OSC 9;4 progress report.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProgressState {
    /// Remove any progress indication.
    Remove,
    /// Determinate progress.
    Set,
    /// Failed progress.
    Error,
    /// Indeterminate progress.
    Indeterminate,
    /// Paused progress.
    Pause,
    /// A state this build does not name.
    Other,
}

/// Which prompt an OSC 133 prompt start draws.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// The main prompt before each command.
    Primary,
    /// A prompt at the right edge of the line.
    Right,
    /// A prompt for an extra line of a multi-line command.
    Continuation,
    /// Another prompt for an extra input line, such as bash's PS2.
    Secondary,
    /// A prompt kind this build does not name.
    Other,
}

/// Family of a control string the semantic engine does not implement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnknownSequenceKind {
    /// Application Program Command.
    Apc,
    /// Operating System Command.
    Osc,
}

/// A side effect the program asked the terminal for, as libghostty-vt reported it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminalEffect {
    /// BEL outside any control string.
    Bell,
    /// OSC 0 or OSC 2 set the window title.
    TitleChanged {
        /// New title bytes.
        title: ByteString,
    },
    /// OSC 7, OSC 9, or OSC 1337 reported the working directory.
    WorkingDirectoryChanged {
        /// Reported location, usually a `file://` URI; empty clears it.
        location: ByteString,
    },
    /// OSC 52, OSC 1337, or OSC 5522 asked to write the clipboard.
    ClipboardWrite {
        /// Destination clipboard.
        target: ClipboardTarget,
        /// Representations of the written value; empty clears the clipboard.
        representations: Vec<ClipboardRepresentation>,
    },
    /// OSC 9 or OSC 777 asked for a desktop notification.
    DesktopNotification {
        /// Digest of the notification title.
        title: PayloadDigest,
        /// Digest of the notification body.
        body: PayloadDigest,
    },
    /// OSC 9;4 reported progress.
    ProgressReport {
        /// Reported state.
        state: ProgressState,
        /// Percentage from 0 through 100 when the program sent one.
        percent: Option<u8>,
    },
    /// RIS reset the terminal.
    FullReset,
    /// CSI 8 t asked to resize the window; the terminal itself is not resized.
    WindowResizeRequest {
        /// Requested rows, absent to keep the current height.
        rows: Option<u16>,
        /// Requested columns, absent to keep the current width.
        columns: Option<u16>,
    },
    /// OSC 133 started drawing a prompt.
    PromptStart {
        /// Which prompt started.
        prompt: PromptKind,
    },
    /// OSC 133 marked the end of the prompt and the start of command input.
    InputStart,
    /// OSC 133 marked the submitted command starting to run.
    CommandStart {
        /// Decoded command line when the shell sent one.
        command: Option<ByteString>,
    },
    /// OSC 133 marked the command finishing.
    CommandEnd {
        /// Exit code when the shell reported one.
        exit_code: Option<i32>,
        /// Error description when the shell sent one.
        error: Option<ByteString>,
    },
    /// The program sent an APC or OSC string that the semantic engine ignored.
    UnknownSequence {
        /// Control string family.
        kind: UnknownSequenceKind,
        /// Leading command name or number: at most 16 ASCII alphanumeric, `-`, `_`, or `.` bytes.
        identifier: ByteString,
    },
    /// libghostty-vt hit an error that may have prevented a semantic update.
    ProcessingError,
}

/// One derived terminal effect bound to the canonical byte that completed it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerminalEffectRecord {
    /// Canonical output event holding the completing byte.
    pub event_seq: u64,
    /// Offset of the completing byte within that event.
    pub event_byte_offset: u64,
    /// Session-monotonic event timestamp.
    pub monotonic_ns: u64,
    /// Reported effect.
    pub effect: TerminalEffect,
}
