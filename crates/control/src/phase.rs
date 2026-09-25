//! The one observation of a session's phase from its evidence and recorder.

use crate::{
    transport::{accepted, connect_recorder, exchange, ExchangeTiming},
    Error, Result,
};
use serde::de::DeserializeOwned;
use std::{
    fs::File,
    io::{ErrorKind, Read as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use termtrace_model::{
    execution::{
        layout::SessionLayout,
        session::{SessionCompletion, SessionManifest, SessionPhase},
    },
    protocol::control::{
        ControlCommand, ControlData, ControlRequest, DEFAULT_CONTROL_RESPONSE_TIMEOUT,
        MAX_CONTROL_MESSAGE_BYTES,
    },
};
use termtrace_process::{poll_until, ProcessIdentity};

const COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A session's phase with the evidence that decided it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhaseObservation {
    /// Observed phase.
    pub phase: SessionPhase,
    /// Why the session is not simply live or completed, when known.
    pub diagnostic: Option<String>,
    /// Identity of the recorder that answered, when the session is live.
    pub recorder: Option<ProcessIdentity>,
}

/// A request's outcome against a session that may have completed.
#[derive(Debug)]
pub enum Reached {
    /// The live actor answered.
    Live(Box<ControlData>),
    /// The session had completed, before the request or while it was failing.
    Completed(Box<SessionCompletion>),
}

/// Read published completion evidence after checking it names the manifest's session.
///
/// Returns `None` while no completion is published. Does not contact the recorder.
///
/// # Errors
///
/// Returns an error when completion or manifest evidence is unreadable or invalid,
/// or when completion names a different session.
pub fn read_completed(session: &Path) -> Result<Option<SessionCompletion>> {
    let layout = SessionLayout::new(session);
    let completion: SessionCompletion = match read_json_file(&layout.completion()) {
        Ok(completion) => completion,
        Err(Error::Filesystem { source, .. }) if source.kind() == ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };

    if read_manifest(session)?.session_id != completion.session_id {
        return Err(Error::CompletionIdentity {
            session: session.to_path_buf(),
        });
    }
    return Ok(Some(completion));
}

/// Whether valid completion evidence is published; unreadable evidence is not completion.
pub fn is_completed(session: &Path) -> bool {
    return matches!(read_completed(session), Ok(Some(_)));
}

/// Poll [`read_completed`] until completion is published or `grace` elapses.
///
/// # Errors
///
/// Returns the first evidence error reported by [`read_completed`].
fn wait_completed(session: &Path, grace: Duration) -> Result<Option<SessionCompletion>> {
    return poll_until(Instant::now() + grace, COMPLETION_POLL_INTERVAL, || {
        return read_completed(session);
    });
}

/// Observe a session's phase from its evidence and the recorder answering its control socket.
pub fn observe_phase(session: &Path) -> PhaseObservation {
    if let Some(observation) = decided_by_evidence(session) {
        return observation;
    }

    if let Ok((_, credentials)) = connect_recorder(
        session,
        &ExchangeTiming::Timeout(DEFAULT_CONTROL_RESPONSE_TIMEOUT),
    ) {
        return PhaseObservation {
            phase: SessionPhase::Live,
            diagnostic: None,
            recorder: u32::try_from(credentials.pid())
                .ok()
                .and_then(|pid| return ProcessIdentity::of(pid).ok()),
        };
    }
    return observed(
        SessionPhase::Incomplete,
        Some("session has no completion evidence and no reachable recorder".to_owned()),
    );
}

/// Return the identity of the recorder answering a session's control socket.
pub fn recorder_identity(session: &Path) -> Option<ProcessIdentity> {
    return observe_phase(session).recorder;
}

/// Send a request within `timing` unless the session has completed; a connection failure
/// that coincides with completion published within `grace` also reports completion.
///
/// # Errors
///
/// Returns the request error when the session did not complete, or an evidence error.
pub fn send_or_completed(
    session: &Path,
    request: &ControlRequest,
    timing: ExchangeTiming,
    grace: Duration,
) -> Result<Reached> {
    if let Some(completion) = read_completed(session)? {
        return Ok(Reached::Completed(Box::new(completion)));
    }
    return match exchange(session, request, timing).and_then(accepted) {
        Ok(data) => Ok(Reached::Live(Box::new(data))),
        Err(error) if error.is_connection_failure() => {
            wait_completed(session, grace)?.map_or(Err(error), |completion| {
                return Ok(Reached::Completed(Box::new(completion)));
            })
        }
        Err(error) => Err(error),
    };
}

/// Send one command through [`send_or_completed`].
///
/// # Errors
///
/// Returns the same errors as [`send_or_completed`].
pub fn request_or_completed(
    session: &Path,
    command: ControlCommand,
    grace: Duration,
) -> Result<Reached> {
    let timing = ExchangeTiming::Timeout(command.client_response_timeout());

    return send_or_completed(session, &ControlRequest::new(command), timing, grace);
}

/// Phase decided by evidence alone; a published session without completion is `Incomplete`.
pub(crate) fn evidence_phase(session: &Path) -> PhaseObservation {
    return decided_by_evidence(session)
        .unwrap_or_else(|| return observed(SessionPhase::Incomplete, None));
}

/// Phase decided without the recorder, or `None` when only the recorder can decide it.
fn decided_by_evidence(session: &Path) -> Option<PhaseObservation> {
    if !session.is_dir() {
        return Some(observed(
            SessionPhase::MissingEvidence,
            Some("session directory is missing or unreadable".to_owned()),
        ));
    }

    match read_completed(session) {
        Ok(Some(_)) => return Some(observed(SessionPhase::Completed, None)),
        Ok(None) => {}
        Err(error @ Error::CompletionIdentity { .. }) => {
            return Some(observed(
                SessionPhase::IdentityMismatch,
                Some(error.to_string()),
            ));
        }
        Err(error) => {
            return Some(observed(
                SessionPhase::MissingEvidence,
                Some(error.to_string()),
            ));
        }
    }

    return match read_manifest(session) {
        Ok(_) => None,
        Err(Error::Filesystem { source, .. }) if source.kind() == ErrorKind::NotFound => {
            Some(observed(
                SessionPhase::Incomplete,
                Some("session manifest has not been published".to_owned()),
            ))
        }
        Err(error) => Some(observed(
            SessionPhase::MissingEvidence,
            Some(error.to_string()),
        )),
    };
}

const fn observed(phase: SessionPhase, diagnostic: Option<String>) -> PhaseObservation {
    return PhaseObservation {
        phase,
        diagnostic,
        recorder: None,
    };
}

pub(crate) fn read_manifest(session: &Path) -> Result<SessionManifest> {
    return read_json_file(&SessionLayout::new(session).manifest());
}

fn read_json_file<T>(path: &Path) -> Result<T>
where
    T: DeserializeOwned,
{
    let filesystem = |source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    };

    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            return file
                .take(MAX_CONTROL_MESSAGE_BYTES + 1)
                .read_to_end(&mut bytes);
        })
        .map_err(filesystem)?;

    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CONTROL_MESSAGE_BYTES {
        return Err(Error::InvalidProtocol {
            reason: format!("`{}` exceeds the client transport limit", path.display()),
        });
    }
    return serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from(path),
            json_path: "$".to_owned(),
            source,
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn missing_directory_is_missing_evidence() {
        let temporary = TempDir::new().expect("temporary directory");
        let observation = observe_phase(&temporary.path().join("absent"));
        assert_eq!(observation.phase, SessionPhase::MissingEvidence);
    }

    #[test]
    fn unpublished_manifest_is_incomplete_without_contacting_a_recorder() {
        let temporary = TempDir::new().expect("temporary directory");
        let observation = observe_phase(temporary.path());
        assert_eq!(observation.phase, SessionPhase::Incomplete);
        assert_eq!(observation.recorder, None);
        assert!(read_completed(temporary.path())
            .expect("no evidence")
            .is_none());
    }

    #[test]
    fn unreadable_completion_is_missing_evidence() {
        let temporary = TempDir::new().expect("temporary directory");
        fs::write(SessionLayout::new(temporary.path()).completion(), b"{").expect("write");
        assert_eq!(
            observe_phase(temporary.path()).phase,
            SessionPhase::MissingEvidence
        );
    }
}
