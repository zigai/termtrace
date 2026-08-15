//! Terminal execution errors.

use std::{error::Error as StdError, fmt, result::Result as StdResult};

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
        }
    }
}

impl StdError for Error {}

/// Terminal operation result.
pub type Result<T, E = Error> = StdResult<T, E>;
