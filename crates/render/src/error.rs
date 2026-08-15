//! Deterministic rendering failures.

use std::{error::Error as StdError, fmt, result::Result as StdResult};

/// Failure while producing a deterministic semantic image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// A semantic image could not be rendered within the renderer contract.
    Render {
        /// Actionable rendering failure reason.
        reason: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Render { reason } => {
                return write!(f, "semantic image rendering failed: {reason}");
            }
        }
    }
}

impl StdError for Error {}

/// Semantic image rendering result.
pub type Result<T, E = Error> = StdResult<T, E>;

pub(super) fn render_error(reason: impl Into<String>) -> Error {
    return Error::Render {
        reason: reason.into(),
    };
}
