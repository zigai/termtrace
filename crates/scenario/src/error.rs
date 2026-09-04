use std::{fmt, io, path::PathBuf, result::Result as StdResult};

/// Failure while parsing, validating, executing, or reporting scenarios.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed.
    #[error("filesystem operation failed for `{}`: {source}", path.display())]
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: io::Error,
    },
    /// A strict scenario or snapshot JSON document was invalid.
    #[error("invalid JSON in `{}` at `{json_path}`: {source}", path.display())]
    InvalidJson {
        /// Path of the invalid document.
        path: PathBuf,
        /// JSON path at which decoding failed.
        json_path: String,
        /// Underlying JSON decoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// A scenario contract invariant was violated.
    #[error("invalid scenario: {reason}")]
    InvalidScenario {
        /// Scenario contract violation.
        reason: String,
    },
    /// A lower-layer operation failed.
    #[error("scenario {operation} failed: {message}")]
    Operation {
        /// Stable lower-layer operation name.
        operation: &'static str,
        /// Safe lower-layer failure detail.
        message: String,
    },
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
