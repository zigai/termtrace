//! Logical session handles, live control, completed evidence, and state resolution.

use crate::{
    actions::{ActionOptions, ActionResult, InputAtom},
    client::Session as ClientSession,
    error::{Error, Result},
    protocol::request,
    types::{ResolvedState, StateSelector},
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_engine::observation::{
    project_screen_frame, resolved_screen_state_for_session, state_text,
};
use termtrace_evidence::session::read_manifest;
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
    /// Returns an error if the directory does not exist or has no valid session ID.
    pub fn from_directory(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let session_id = if let Ok(manifest) = read_manifest(&path) {
            manifest.session_id.to_string()
        } else {
            path.file_name()
                .and_then(|name| return name.to_str())
                .unwrap_or("unknown")
                .to_owned()
        };

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
    /// Returns an error if the session manifest cannot be read or verified.
    pub fn from_directory(path: impl Into<PathBuf>) -> Result<Self> {
        let session = Session::from_directory(path)?;
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
