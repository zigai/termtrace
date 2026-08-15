use std::{error::Error as StdError, fmt, io, path::PathBuf, result::Result as StdResult};

/// Failure while parsing, validating, executing, or reporting scenarios.
#[derive(Debug)]
pub enum Error {
    /// A filesystem operation failed.
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O failure.
        source: io::Error,
    },
    /// A strict scenario or snapshot JSON document was invalid.
    InvalidJson {
        /// Path of the invalid document.
        path: PathBuf,
        /// JSON path at which decoding failed.
        json_path: String,
        /// Underlying JSON decoding failure.
        source: serde_json::Error,
    },
    /// A scenario contract invariant was violated.
    InvalidScenario {
        /// Scenario contract violation.
        reason: String,
    },
    /// A lower-layer operation failed.
    Operation {
        /// Stable lower-layer operation name.
        operation: &'static str,
        /// Safe lower-layer failure detail.
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Filesystem { path, source } => {
                return write!(
                    f,
                    "filesystem operation failed for `{}`: {source}",
                    path.display()
                );
            }
            Self::InvalidJson {
                path,
                json_path,
                source,
            } => {
                return write!(
                    f,
                    "invalid JSON in `{}` at `{json_path}`: {source}",
                    path.display()
                );
            }
            Self::InvalidScenario { reason } => {
                return write!(f, "invalid scenario: {reason}");
            }
            Self::Operation { operation, message } => {
                return write!(f, "scenario {operation} failed: {message}");
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Filesystem { source, .. } => return Some(source),
            Self::InvalidJson { source, .. } => return Some(source),
            Self::InvalidScenario { .. } | Self::Operation { .. } => return None,
        }
    }
}

/// Result returned by scenario operations.
pub type Result<T, E = Error> = StdResult<T, E>;

pub(crate) fn invalid(reason: impl Into<String>) -> Error {
    return Error::InvalidScenario {
        reason: reason.into(),
    };
}

pub(crate) fn operation(operation: &'static str, error: impl fmt::Display) -> Error {
    return Error::Operation {
        operation,
        message: error.to_string(),
    };
}
