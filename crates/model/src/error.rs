//! Validation errors for caller-authored model values.

use std::{error::Error as StdError, fmt};

/// A precise failure to validate a caller-authored model value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelValidationError {
    /// An environment policy contains more entries than the model permits.
    TooManyEnvironmentEntries,
    /// An environment variable name is not portable to a child process.
    InvalidEnvironmentVariableName {
        /// Rejected variable name.
        name: String,
    },
    /// An environment specification attempts to override a recorder-owned variable.
    ControlledEnvironmentVariable {
        /// Rejected variable name.
        name: String,
    },
    /// A retained environment policy contains a recorder-owned variable.
    ReservedEnvironmentVariable {
        /// Rejected variable name.
        name: String,
    },
    /// An explicit environment value contains a NUL byte.
    EnvironmentValueContainsNul {
        /// Variable whose value was rejected.
        name: String,
    },
    /// An explicit environment value exceeds the model limit.
    EnvironmentValueTooLarge {
        /// Variable whose value was rejected.
        name: String,
    },
    /// One environment variable has multiple mutually exclusive operations.
    ConflictingEnvironmentOperations {
        /// Conflicting variable name.
        name: String,
    },
    /// A retained effective-environment digest is not lowercase BLAKE3 hex.
    InvalidEnvironmentDigest,
    /// Durability thresholds disagree with the selected durability mode.
    InvalidDurabilityPolicy,
    /// Key-shaped text does not name a valid key chord.
    InvalidKeyChord {
        /// Rejected chord text.
        chord: String,
        /// Why the chord was rejected.
        reason: &'static str,
    },
}

impl fmt::Display for ModelValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyEnvironmentEntries => {
                return write!(f, "environment policy exceeds 4096 entries");
            }
            Self::InvalidEnvironmentVariableName { name } => {
                return write!(f, "invalid environment variable name `{name}`");
            }
            Self::ControlledEnvironmentVariable { name } => {
                return write!(f, "`{name}` is controlled by --term or --color");
            }
            Self::ReservedEnvironmentVariable { name } => {
                return write!(f, "`{name}` is a reserved terminal or color variable");
            }
            Self::EnvironmentValueContainsNul { name } => {
                return write!(f, "value for `{name}` contains NUL");
            }
            Self::EnvironmentValueTooLarge { name } => {
                return write!(f, "value for `{name}` exceeds 1 MiB");
            }
            Self::ConflictingEnvironmentOperations { name } => {
                return write!(
                    f,
                    "environment variable `{name}` has conflicting operations"
                );
            }
            Self::InvalidEnvironmentDigest => {
                return write!(
                    f,
                    "effective environment digest must be lowercase BLAKE3 hex"
                );
            }
            Self::InvalidDurabilityPolicy => {
                return write!(
                    f,
                    "durability policy thresholds are invalid for the selected mode"
                );
            }
            Self::InvalidKeyChord { chord, reason } => {
                return write!(f, "invalid key chord `{chord}`: {reason}");
            }
        }
    }
}

impl StdError for ModelValidationError {}
