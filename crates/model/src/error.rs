//! Validation errors for caller-authored model values.

use std::{any::Any, error::Error as StdError, fmt};

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

/// A joined thread panicked, retaining the panic message when the payload carried one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadPanic {
    message: Option<String>,
}

impl ThreadPanic {
    /// Capture the panic message from the payload returned by `JoinHandle::join`.
    #[must_use]
    pub fn from_payload(payload: Box<dyn Any + Send>) -> Self {
        let message = match payload.downcast::<String>() {
            Ok(message) => Some(*message),
            Err(payload) => payload
                .downcast_ref::<&str>()
                .map(|message| return (*message).to_owned()),
        };

        return Self { message };
    }

    /// Panic message, when the payload was a string.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        return self.message.as_deref();
    }
}

impl fmt::Display for ThreadPanic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.message {
            Some(message) => return write!(f, "thread panicked: {message}"),
            None => return f.write_str("thread panicked with a non-string payload"),
        }
    }
}

impl StdError for ThreadPanic {}
