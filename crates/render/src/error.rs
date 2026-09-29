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
    /// A bundled font face could not be parsed.
    FontFace {
        /// Actionable rendering failure reason.
        reason: String,
        /// Underlying font parsing failure.
        source: ttf_parser::FaceParsingError,
    },
    /// Formatting rendered markup failed.
    Format(fmt::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Render { reason } | Self::FontFace { reason, .. } => {
                return write!(f, "semantic image rendering failed: {reason}");
            }
            Self::Format(source) => {
                return write!(f, "semantic image rendering failed: {source}");
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::FontFace { source, .. } => return Some(source),
            Self::Format(source) => return Some(source),
            Self::Render { .. } => return None,
        }
    }
}

impl From<fmt::Error> for Error {
    fn from(source: fmt::Error) -> Self {
        return Self::Format(source);
    }
}

/// Semantic image rendering result.
pub type Result<T, E = Error> = StdResult<T, E>;

pub(super) fn render_error(reason: impl Into<String>) -> Error {
    return Error::Render {
        reason: reason.into(),
    };
}
