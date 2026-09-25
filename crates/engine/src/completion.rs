//! Whether a session has completed, as decided by `termtrace_control`, in engine errors.

use crate::{Error, Result};
use std::path::Path;
use termtrace_control::Error as ControlError;
use termtrace_model::execution::session::SessionCompletion;

/// Published, identity-checked completion of `session`; `None` while it has not completed.
///
/// # Errors
///
/// Returns an error when completion or manifest evidence is unreadable, invalid, or names a
/// different session.
pub(crate) fn published(session: &Path) -> Result<Option<SessionCompletion>> {
    return termtrace_control::read_completed(session).map_err(|error| {
        return match error {
            ControlError::Filesystem { path, source } => Error::Filesystem { path, source },
            ControlError::InvalidJson {
                path,
                json_path,
                source,
            } => Error::InvalidJson {
                path,
                json_path,
                source,
            },
            other => Error::InvalidEvidence {
                reason: other.to_string(),
            },
        };
    });
}
