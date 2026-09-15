//! Core domain models, state selectors, resolved states, and artifact references.

use crate::error::{Error, Result};
use std::{
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_model::{
    terminal::observation::StatePoint,
    terminal::screen::{ScreenFrame, ScreenProjectionRequest},
};

/// Validated immutable session identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(String);

impl SessionId {
    /// Create a new session ID, validating it is non-empty and contains no path separators.
    ///
    /// # Errors
    /// Returns an error if the identifier contains path traversal characters.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(Error::InvalidOperation {
                reason: "session ID must not be empty".to_owned(),
            });
        }

        if id.contains('/') || id.contains('\\') || id.contains("..") {
            return Err(Error::InvalidOperation {
                reason: format!("session ID `{id}` contains illegal path characters"),
            });
        }
        return Ok(Self(id));
    }

    /// Access the underlying string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        return &self.0;
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return write!(f, "{}", self.0);
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        return &self.0;
    }
}

/// Selector identifying a specific state occurrence within a session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateSelector {
    /// The latest presentation-eligible viewport.
    LatestPresented,
    /// Exact sequential state identifier.
    StateId(u64),
    /// Cryptographic state hash.
    StateHash(String),
    /// Friendly milestone marker name.
    Marker(String),
    /// State closest to the specified session duration offset.
    Offset(Duration),
}

impl StateSelector {
    /// Convert to the underlying model `StatePoint` when applicable.
    #[must_use]
    pub fn to_state_point(&self) -> StatePoint {
        return match self {
            Self::LatestPresented => StatePoint::LatestPresented,
            Self::StateId(id) => StatePoint::StateId { state_id: *id },
            Self::StateHash(hash) => StatePoint::StateHash {
                state_hash: hash.clone(),
            },
            Self::Marker(name) => StatePoint::Marker {
                label: name.clone(),
            },
            Self::Offset(duration) => StatePoint::MonotonicNs {
                monotonic_ns: u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX),
            },
        };
    }
}

/// Immutable, already-resolved state occurrence with its complete provenance.
#[derive(Clone, Debug)]
pub struct ResolvedState {
    /// Canonical session identifier.
    pub session_id: String,
    /// Monotonic state identifier.
    pub state_id: u64,
    /// Blake3 state content hash.
    pub state_hash: String,
    /// Plain text viewport representation for fast inspection.
    pub viewport_text: String,
    /// Full rich screen frame for this state.
    pub frame: ScreenFrame,
    /// Whether this state was presented to an active display surface.
    pub presented: bool,
    /// Originating selector and evidence resolution provenance.
    pub provenance: StatePoint,
}

impl ResolvedState {
    /// Read the plain text viewport rows from the resolved frame.
    #[must_use]
    pub fn viewport_text(&self) -> &str {
        return &self.viewport_text;
    }

    /// Check whether this state occurrence was presentation-eligible.
    #[must_use]
    pub fn is_presented(&self) -> bool {
        return self.presented;
    }

    /// Access the state hash string.
    #[must_use]
    pub fn state_hash(&self) -> &str {
        return &self.state_hash;
    }

    /// Project the screen frame with specific detail and region options.
    #[must_use]
    pub fn project(&self, _request: &ScreenProjectionRequest) -> ScreenFrame {
        return self.frame.clone();
    }
}

/// Reference to a published artifact bound to canonical evidence and manifests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactRef {
    /// Canonical identifier for the artifact.
    pub artifact_id: String,
    /// Path to the published primary artifact file or directory.
    pub path: PathBuf,
    /// Path to the associated manifest file if generated.
    pub manifest_path: Option<PathBuf>,
    /// Originating session identifier.
    pub source_session_id: String,
    /// Exact source state hash bound to this artifact if state-derived.
    pub source_state_hash: Option<String>,
    /// Artifact kind label (e.g. `screenshot`, `video`, `bundle`, `save`).
    pub kind: &'static str,
}

impl ArtifactRef {
    /// Access the primary file or directory path.
    #[must_use]
    pub fn path(&self) -> &Path {
        return &self.path;
    }

    /// Access the manifest path if present.
    #[must_use]
    pub fn manifest_path(&self) -> Option<&Path> {
        return self.manifest_path.as_deref();
    }
}

/// Status of a long-running background operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationStatus {
    /// Operation is queued or actively processing.
    Running {
        /// Optional progress percentage (0..=100).
        percent: Option<u8>,
        /// Status message.
        message: String,
    },
    /// Operation finished successfully.
    Completed,
    /// Operation was cancelled by caller request.
    Cancelled,
    /// Operation failed.
    Failed {
        /// Failure message.
        reason: String,
    },
}

/// Handle to an in-flight or completed long-running job.
#[derive(Debug)]
pub struct Operation<T> {
    id: String,
    status: OperationStatus,
    result: Option<T>,
}

impl<T> Operation<T> {
    /// Construct a new completed operation handle.
    #[must_use]
    pub fn completed(id: impl Into<String>, value: T) -> Self {
        return Self {
            id: id.into(),
            status: OperationStatus::Completed,
            result: Some(value),
        };
    }

    /// Construct a running operation handle.
    #[must_use]
    pub fn running(id: impl Into<String>, message: impl Into<String>) -> Self {
        return Self {
            id: id.into(),
            status: OperationStatus::Running {
                percent: None,
                message: message.into(),
            },
            result: None,
        };
    }

    /// Access the operation identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        return &self.id;
    }

    /// Query current status.
    #[must_use]
    pub fn status(&self) -> &OperationStatus {
        return &self.status;
    }

    /// Consume the handle and return the result if completed.
    #[must_use]
    pub fn into_result(self) -> Option<T> {
        return self.result;
    }
}
