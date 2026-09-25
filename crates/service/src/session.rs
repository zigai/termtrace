//! Logical session handles, live control, completed evidence, and state resolution.

use crate::{
    actions::{ActionOptions, ActionResult, InputAtom},
    client::Session as ClientSession,
    error::{Error, Result},
    types::{ResolvedState, StateSelector},
};
use std::{
    fs::metadata,
    io::{Error as IoError, ErrorKind as IoErrorKind},
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_control::request;
use termtrace_engine::observation::{
    project_screen_frame, resolved_screen_state_for_session, state_text,
};
use termtrace_evidence::session::{read_manifest, validate_completed_session};
use termtrace_model::terminal::observation::CaptureQuality;
use termtrace_model::{
    execution::session::SessionManifest,
    protocol::control::{ControlCommand, ProcessSignal},
    terminal::screen::ScreenProjectionRequest,
};

/// Options controlling how a state occurrence is resolved.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResolveOptions {
    /// Allow resolving synchronized-output hidden frames that were not presented.
    pub allow_hidden: bool,
    /// Whether to force a cache bypass and recompute from raw event logs.
    pub bypass_cache: bool,
}

/// Cheaply cloneable logical handle to a Termtrace session directory.
///
/// Dropping this handle does NOT terminate the recorded process or delete evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    path: PathBuf,
    session_id: String,
}

impl Session {
    /// Create a session handle from an existing session directory.
    ///
    /// # Errors
    /// Returns an error if the directory does not exist, is not a directory, or
    /// contains no valid session manifest.
    pub fn from_directory(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let metadata = metadata(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        if !metadata.is_dir() {
            return Err(Error::Filesystem {
                path,
                source: IoError::new(
                    IoErrorKind::NotADirectory,
                    "expected session path to be a directory",
                ),
            });
        }

        let manifest = read_manifest(&path)?;
        let session_id = manifest.session_id.to_string();

        return Ok(Self { path, session_id });
    }

    /// Access the canonical session directory path.
    #[must_use]
    pub fn path(&self) -> &Path {
        return &self.path;
    }

    /// Access the immutable session ID string.
    #[must_use]
    pub fn session_id(&self) -> &str {
        return &self.session_id;
    }

    /// Resolve a specific state point into an immutable, queryable `ResolvedState`.
    ///
    /// # Errors
    /// Returns an error if the state cannot be found or decoded.
    pub fn resolve_state(
        &self,
        selector: StateSelector,
        options: ResolveOptions,
    ) -> Result<ResolvedState> {
        let state_point = selector.to_state_point();
        let (state, source, presented) = resolved_screen_state_for_session(
            &self.path,
            state_point.clone(),
            options.allow_hidden,
        )?;

        let frame = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            source,
            presented,
            CaptureQuality::Completed,
        )
        .map_err(Error::ScreenFrame)?;

        let viewport_text = state_text(&state);

        return Ok(ResolvedState {
            session_id: self.session_id.clone(),
            state_id: 0,
            state_hash: frame.state_hash.clone(),
            viewport_text,
            frame,
            presented,
            provenance: state_point,
        });
    }

    /// Attempt to acquire a live-control handle for this session.
    ///
    /// # Errors
    /// Returns an error if the session is completed or its control endpoint is unreachable.
    pub fn live(&self) -> Result<LiveSession> {
        return LiveSession::connect(self.clone());
    }

    /// Attempt to validate and acquire a completed session handle.
    ///
    /// # Errors
    /// Returns an error if the session is still live or evidence is incomplete.
    pub fn completed(&self) -> Result<CompletedSession> {
        return CompletedSession::from_directory(&self.path);
    }
}

/// Live-control handle for executing mutations, inputs, and waits on an active session.
#[derive(Clone, Debug)]
pub struct LiveSession {
    session: Session,
}

impl LiveSession {
    /// Connect to a running session.
    ///
    /// Note that this constructor performs deferred connection: no immediate transport
    /// verification or socket handshake is performed during `connect`. Endpoint availability
    /// and control protocol readiness are verified lazily upon issuing commands (such as
    /// [`act`](Self::act), [`signal`](Self::signal), or [`stop`](Self::stop)).
    ///
    /// # Errors
    /// Returns `Ok` immediately; deferred connection errors are reported on command execution.
    pub fn connect(session: Session) -> Result<Self> {
        return Ok(Self { session });
    }

    /// Access the underlying logical session.
    #[must_use]
    pub fn session(&self) -> &Session {
        return &self.session;
    }

    /// Execute an atomic sequence of input atoms with state guards and postconditions.
    ///
    /// # Errors
    /// Returns an error if the input was rejected, state was stale, or transport failed.
    pub fn act(&self, atoms: &[InputAtom], options: ActionOptions) -> Result<ActionResult> {
        let handle = ClientSession::open(
            self.session.path().to_path_buf(),
            PathBuf::from("termtrace"),
            None,
        )?;

        return handle.act(atoms, options);
    }

    /// Send a process signal to the recorded process group.
    ///
    /// # Errors
    /// Returns an error if signal delivery fails.
    pub fn signal(&self, signal: ProcessSignal) -> Result<()> {
        let _ = request(self.session.path(), ControlCommand::Signal { signal })?;

        return Ok(());
    }

    /// Request termination of this running session.
    ///
    /// # Errors
    /// Returns an error if the stop command fails.
    pub fn stop(&self, _timeout: Duration) -> Result<()> {
        let _ = request(self.session.path(), ControlCommand::Stop)?;

        return Ok(());
    }
}

/// Validated completed evidence handle bound to a verified session manifest.
#[derive(Clone, Debug)]
pub struct CompletedSession {
    session: Session,
    manifest: SessionManifest,
}

impl CompletedSession {
    /// Open and validate a completed session directory.
    ///
    /// # Errors
    /// Returns an error if the session path is not a directory, the session manifest
    /// cannot be read or verified, or the session is not completed.
    pub fn from_directory(path: impl Into<PathBuf>) -> Result<Self> {
        let session = Session::from_directory(path)?;
        let _ = validate_completed_session(session.path())?;
        let manifest = read_manifest(session.path())?;

        return Ok(Self { session, manifest });
    }

    /// Access the logical session reference.
    #[must_use]
    pub fn session(&self) -> &Session {
        return &self.session;
    }

    /// Access the session manifest.
    #[must_use]
    pub fn manifest(&self) -> &SessionManifest {
        return &self.manifest;
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::{CompletedSession, LiveSession, Session};
    use crate::error::ErrorKind;
    use std::{collections::BTreeMap, ffi::OsString, fs::write, path::Path};
    use tempfile::TempDir;
    use termtrace_evidence::{
        provenance::{collect_execution_provenance, RecorderBuildIdentity},
        session::canonical_manifest_bytes,
    };
    use termtrace_model::{
        execution::{
            environment::ChildEnvironmentPolicy,
            event_log::DurabilityPolicy,
            layout::{self, SessionLayout},
            provenance::ProvenancePolicy,
            session::{
                GeometryProvenance, ResourceLimits, SessionGeometry, SessionManifest,
                SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION,
            },
        },
        value::ByteString,
    };
    use uuid::Uuid;

    #[test]
    fn test_session_from_directory_nonexistent() {
        let result = Session::from_directory("/path/that/does/not/exist/termtrace_nonexistent");
        assert!(result.is_err());
        let Err(error) = result else {
            return;
        };
        assert_eq!(error.kind(), ErrorKind::Filesystem);
        return;
    }

    #[test]
    fn test_session_from_directory_file_not_dir() {
        let temporary = TempDir::new();
        assert!(temporary.is_ok());
        let Ok(temporary) = temporary else {
            return;
        };
        let file_path = temporary.path().join("regular_file.txt");
        assert!(write(&file_path, "not a directory").is_ok());

        let result = Session::from_directory(&file_path);
        assert!(result.is_err());
        let Err(error) = result else {
            return;
        };
        assert_eq!(error.kind(), ErrorKind::Filesystem);
        return;
    }

    #[test]
    fn test_session_from_directory_missing_manifest() {
        let temporary = TempDir::new();
        assert!(temporary.is_ok());
        let Ok(temporary) = temporary else {
            return;
        };
        let result = Session::from_directory(temporary.path());
        assert!(result.is_err());
        let Err(error) = result else {
            return;
        };
        assert_eq!(error.kind(), ErrorKind::Evidence);
        return;
    }

    #[test]
    fn test_completed_session_rejects_missing_session() {
        let temporary = TempDir::new();
        assert!(temporary.is_ok());
        let Ok(temporary) = temporary else {
            return;
        };
        let result = CompletedSession::from_directory(temporary.path());
        assert!(result.is_err());
        let Err(error) = result else {
            return;
        };
        assert_eq!(error.kind(), ErrorKind::Evidence);
        return;
    }

    pub(crate) fn write_test_manifest(dir: &Path, session_id: Uuid) -> bool {
        let manifest = SessionManifest {
            failure_policy: Default::default(),
            format: SESSION_MANIFEST_FORMAT.to_owned(),
            version: SESSION_MANIFEST_VERSION,
            session_id,
            creation_alias: None,
            lineage: None,
            import_provenance: None,
            tmux_pane: None,
            started_unix_ns: 1_000_000,
            command: vec![ByteString::from(&b"true"[..])],
            working_directory: Some(ByteString::from(&b"/tmp"[..])),
            geometry: SessionGeometry {
                cols: 80,
                rows: 24,
                pixel_width: 720,
                pixel_height: 432,
            },
            geometry_provenance: GeometryProvenance::default(),
            ghostty_visual: None,
            child_environment: ChildEnvironmentPolicy::default(),
            execution_provenance: collect_execution_provenance(
                &ProvenancePolicy::default(),
                &BTreeMap::new(),
                &[OsString::from("/bin/true")],
                Path::new("/"),
                RecorderBuildIdentity {
                    version: env!("CARGO_PKG_VERSION"),
                    source_revision: "test",
                    semantic_engine_revision: "test",
                },
            ),
            termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
            operating_system: "linux".to_owned(),
            architecture: "test".to_owned(),
            event_log: layout::EVENT_LOG.to_owned(),
            control_socket: format!("termtrace-control-{session_id}"),
            request_journal: layout::MUTATION_JOURNAL.to_owned(),
            contains_sensitive_terminal_data: true,
            durability_policy: DurabilityPolicy::default(),
            resource_limits: ResourceLimits::default(),
        };

        let Ok(bytes) = canonical_manifest_bytes(&manifest) else {
            return false;
        };
        return write(SessionLayout::new(dir).manifest(), bytes).is_ok();
    }

    #[test]
    fn test_completed_session_rejects_incomplete_when_manifest_present() {
        let temporary = TempDir::new();
        assert!(temporary.is_ok());
        let Ok(temporary) = temporary else {
            return;
        };
        let session_id = Uuid::now_v7();
        assert!(write_test_manifest(temporary.path(), session_id));

        // Session::from_directory succeeds because session.json is valid
        let session_result = Session::from_directory(temporary.path());
        assert!(session_result.is_ok());
        let Ok(session) = session_result else {
            return;
        };
        assert_eq!(session.session_id(), session_id.to_string());

        // CompletedSession::from_directory MUST fail because completion.json is missing
        let result = CompletedSession::from_directory(temporary.path());
        assert!(result.is_err());
        let Err(error) = result else {
            return;
        };
        assert_eq!(error.kind(), ErrorKind::Evidence);
        return;
    }

    #[test]
    fn test_live_session_connect_deferred() {
        let temporary = TempDir::new();
        assert!(temporary.is_ok());
        let Ok(temporary) = temporary else {
            return;
        };
        let session_id = Uuid::now_v7();
        assert!(write_test_manifest(temporary.path(), session_id));

        let session_result = Session::from_directory(temporary.path());
        assert!(session_result.is_ok());
        let Ok(session) = session_result else {
            return;
        };
        let live_result = LiveSession::connect(session.clone());
        assert!(live_result.is_ok());
        let Ok(live) = live_result else {
            return;
        };
        assert_eq!(live.session(), &session);
        return;
    }
}
