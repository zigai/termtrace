//! Terminal execution errors.

use std::{error::Error as StdError, fmt, result::Result as StdResult, str::Utf8Error};

/// Failure while framing or interpreting terminal data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// A pinned libghostty-vt operation failed.
    GhosttyVt {
        /// Stable operation label.
        operation: &'static str,
        /// Native Ghostty result code.
        result: i32,
    },
    /// A terminal policy operation was rejected.
    TerminalOperation {
        /// Stable operation label.
        operation: &'static str,
        /// Actionable rejection reason.
        reason: String,
    },
    /// A VT stream could not be framed within configured limits.
    InvalidVtStream {
        /// Stable failure reason.
        reason: String,
    },
    /// Text returned by libghostty-vt was not valid UTF-8.
    InvalidUtf8 {
        /// Stable operation label.
        operation: &'static str,
        /// Underlying decoding failure.
        source: Utf8Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GhosttyVt { operation, result } => {
                return write!(f, "libghostty-vt {operation} failed with result {result}");
            }
            Self::TerminalOperation { operation, reason } => {
                return write!(f, "terminal operation {operation} failed: {reason}");
            }
            Self::InvalidVtStream { reason } => {
                return write!(f, "invalid VT stream: {reason}");
            }
            Self::InvalidUtf8 { operation, source } => {
                return write!(
                    f,
                    "libghostty-vt {operation} returned invalid UTF-8: {source}"
                );
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::InvalidUtf8 { source, .. } => return Some(source),
            Self::GhosttyVt { .. }
            | Self::TerminalOperation { .. }
            | Self::InvalidVtStream { .. } => {
                return None;
            }
        }
    }
}

/// Terminal operation result.
pub type Result<T, E = Error> = StdResult<T, E>;
