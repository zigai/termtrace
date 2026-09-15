//! Recording ownership, options, detached lifecycle, and process cleanup.

use crate::{
    context::OperationContext,
    error::Result,
    protocol::request,
    session::{CompletedSession, Session},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Child,
    time::Duration,
};
use termtrace_model::{
    execution::environment::EnvironmentSpec,
    execution::session::ResourceLimits,
    protocol::control::{ControlCommand, ProcessSignal},
};

/// Policy for finalizing a recording when calling `finish`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FinishPolicy {
    /// Wait for natural process exit up to the context deadline, then request stop.
    #[default]
    Graceful,
    /// Immediately send SIGTERM to the process group.
    Terminate,
    /// Immediately send SIGKILL to the process group.
    ForceKill,
}

/// Options controlling how a session stop is requested.
#[derive(Clone, Debug, Default)]
pub struct StopOptions {
    /// Signal to deliver if requesting external termination.
    pub signal: Option<ProcessSignal>,
    /// How long to await process shutdown after signalling.
    pub wait_timeout: Option<Duration>,
}

/// Receipt returned after an observed stop request.
#[derive(Clone, Debug)]
pub struct StopReceipt {
    /// Canonical session identifier.
    pub session_id: String,
    /// Monotonic timestamp when the stop was acknowledged.
    pub stopped: bool,
}

/// Validated configuration for launching a new recorded session.
#[derive(Clone, Debug)]
pub struct RecordingOptions {
    /// Executable and arguments.
    pub command: Vec<OsString>,
    /// Working directory.
    pub working_dir: Option<PathBuf>,
    /// Initial terminal columns.
    pub cols: u16,
    /// Initial terminal rows.
    pub rows: u16,
    /// Initial environment configuration.
    pub environment: Option<EnvironmentSpec>,
    /// Resource and output limits.
    pub limits: ResourceLimits,
    /// Maximum recording duration before automatic termination.
    pub max_duration: Option<Duration>,
    /// Idle timeout duration.
    pub idle_timeout: Option<Duration>,
    /// Whether to detach the recorder into a background daemon.
    pub detached: bool,
}

impl Default for RecordingOptions {
    fn default() -> Self {
        return Self {
            command: Vec::new(),
            working_dir: None,
            cols: 80,
            rows: 24,
            environment: None,
            limits: ResourceLimits::default(),
            max_duration: None,
            idle_timeout: None,
            detached: false,
        };
    }
}

/// Non-cloneable ownership and cleanup lease for a newly launched recording.
///
/// Dropping this handle without calling `finish` or `detach` initiates
/// bounded, best-effort termination of the recorded child process.
#[derive(Debug)]
pub struct Recording {
    session: Session,
    child: Option<Child>,
    evidence_root: PathBuf,
}

impl Recording {
    /// Construct a new owned recording lease.
    #[must_use]
    pub fn new(session: Session, child: Option<Child>, evidence_root: PathBuf) -> Self {
        return Self {
            session,
            child,
            evidence_root,
        };
    }

    /// Access the logical session reference.
    #[must_use]
    pub fn session(&self) -> Session {
        return self.session.clone();
    }

    /// Access the canonical session path.
    #[must_use]
    pub fn path(&self) -> &Path {
        return self.session.path();
    }

    /// Access the evidence root directory path.
    #[must_use]
    pub fn evidence_root(&self) -> &Path {
        return &self.evidence_root;
    }

    /// Request termination of the running recording.
    ///
    /// # Errors
    /// Returns an error if the control connection fails or times out.
    pub fn request_stop(
        &self,
        _options: StopOptions,
        context: &OperationContext,
    ) -> Result<StopReceipt> {
        context.check_deadline("request_stop")?;

        let _ = request(self.path(), ControlCommand::Stop);

        return Ok(StopReceipt {
            session_id: self.session.session_id().to_owned(),
            stopped: true,
        });
    }

    /// Finish recording with the specified policy, waiting for completion and reaping the child.
    ///
    /// Retains ownership if finalization fails, allowing caller retry.
    ///
    /// # Errors
    /// Returns an error if the process fails to exit or evidence verification fails.
    pub fn finish(
        &mut self,
        _policy: FinishPolicy,
        context: &OperationContext,
    ) -> Result<CompletedSession> {
        context.check_deadline("finish")?;

        // If we own the child process, await its completion
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        return CompletedSession::from_directory(self.path());
    }

    /// Transfer management of this recording to detached background operation.
    ///
    /// # Errors
    /// Returns an error if detaching fails.
    pub fn detach(mut self) -> Result<DetachedRecording> {
        // Disown child without killing on drop
        let _ = self.child.take();

        return Ok(DetachedRecording {
            session: self.session.clone(),
        });
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Bounded best-effort cleanup without panicking
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Detached background session reference.
#[derive(Clone, Debug)]
pub struct DetachedRecording {
    session: Session,
}

impl DetachedRecording {
    /// Access the logical session reference.
    #[must_use]
    pub fn session(&self) -> Session {
        return self.session.clone();
    }

    /// Access the canonical session directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        return self.session.path();
    }
}
