//! Recording ownership, options, detached lifecycle, and process cleanup.

use crate::{
    context::OperationContext,
    error::Result,
    inspection::wait_for_completion,
    protocol::request,
    session::{CompletedSession, Session},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Child,
    thread,
    time::{Duration, Instant},
};
use termtrace_model::{
    execution::environment::EnvironmentSpec,
    execution::session::ResourceLimits,
    protocol::control::{ControlCommand, ProcessSignal},
};

const DEFAULT_FINISH_TIMEOUT: Duration = Duration::from_secs(10);

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
        options: StopOptions,
        context: &OperationContext,
    ) -> Result<StopReceipt> {
        context.check_deadline("request_stop")?;

        let command = options.signal.map_or(ControlCommand::Stop, |signal| {
            return ControlCommand::Signal { signal };
        });

        request(self.path(), command)?;

        if let Some(wait_timeout) = options.wait_timeout {
            wait_for_completion(self.path(), wait_timeout)?;
        }

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
        policy: FinishPolicy,
        context: &OperationContext,
    ) -> Result<CompletedSession> {
        context.check_deadline("finish")?;

        match policy {
            FinishPolicy::Graceful => {
                let remaining = context.remaining_duration().unwrap_or(DEFAULT_FINISH_TIMEOUT);
                let natural_budget = remaining / 2;
                let start = Instant::now();
                let mut exited_naturally = false;

                while start.elapsed() < natural_budget {
                    if self.path().join("completion.json").is_file() {
                        exited_naturally = true;
                        break;
                    }
                    if let Some(child) = &mut self.child
                        && matches!(child.try_wait(), Ok(Some(_)))
                    {
                        exited_naturally = true;
                        break;
                    }
                    thread::sleep(
                        Duration::from_millis(20)
                            .min(natural_budget.saturating_sub(start.elapsed())),
                    );
                }

                if !exited_naturally {
                    request(self.path(), ControlCommand::Stop)?;
                }

                let remaining_evidence =
                    context.remaining_duration().unwrap_or(DEFAULT_FINISH_TIMEOUT);
                wait_for_completion(self.path(), remaining_evidence)?;
            }
            FinishPolicy::Terminate => {
                request(
                    self.path(),
                    ControlCommand::Signal {
                        signal: ProcessSignal::Terminate,
                    },
                )?;
                let remaining = context.remaining_duration().unwrap_or(DEFAULT_FINISH_TIMEOUT);
                wait_for_completion(self.path(), remaining)?;
            }
            FinishPolicy::ForceKill => {
                if let Some(child) = &mut self.child {
                    let _ = child.kill();
                } else {
                    let _ = request(
                        self.path(),
                        ControlCommand::Signal {
                            signal: ProcessSignal::Terminate,
                        },
                    );
                }
                let remaining = context.remaining_duration().unwrap_or(DEFAULT_FINISH_TIMEOUT);
                let start = Instant::now();
                while start.elapsed() < remaining {
                    if self.path().join("completion.json").is_file() {
                        break;
                    }
                    if let Some(child) = &mut self.child
                        && matches!(child.try_wait(), Ok(Some(_)))
                    {
                        break;
                    }
                    thread::sleep(
                        Duration::from_millis(20).min(remaining.saturating_sub(start.elapsed())),
                    );
                }
                let remaining_evidence =
                    context.remaining_duration().unwrap_or(DEFAULT_FINISH_TIMEOUT);
                wait_for_completion(self.path(), remaining_evidence)?;
            }
        }

        // Only take and reap `self.child` after completion evidence is successfully obtained
        if let Some(mut child) = self.child.take() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    use std::process::Command;
    use tempfile::TempDir;
    use uuid::Uuid;
    use crate::session::tests::write_test_manifest;

    fn setup_test_session(dir: &Path) -> Session {
        let session_id = Uuid::now_v7();
        assert!(write_test_manifest(dir, session_id));
        return Session::from_directory(dir).expect("session from directory");
    }
    #[test]
    fn test_finish_policy_default() {
        assert_eq!(FinishPolicy::default(), FinishPolicy::Graceful);
        return;
    }

    #[test]
    fn test_request_stop_deadline_expired() {
        let temporary = TempDir::new().expect("tempdir");
        let session = setup_test_session(temporary.path());
        let recording = Recording::new(session, None, temporary.path().to_path_buf());

        let context = OperationContext::default().with_timeout(Duration::ZERO);
        thread::sleep(Duration::from_millis(5));

        let result = recording.request_stop(StopOptions::default(), &context);
        let error = result.expect_err("should fail deadline");
        assert_eq!(error.kind(), ErrorKind::ControlTimeout);
        return;
    }

    #[test]
    fn test_finish_deadline_expired() {
        let temporary = TempDir::new().expect("tempdir");
        let session = setup_test_session(temporary.path());
        let mut recording = Recording::new(session, None, temporary.path().to_path_buf());

        let context = OperationContext::default().with_timeout(Duration::ZERO);
        thread::sleep(Duration::from_millis(5));

        let result = recording.finish(FinishPolicy::Graceful, &context);
        let error = result.expect_err("should fail deadline");
        assert_eq!(error.kind(), ErrorKind::ControlTimeout);
        return;
    }

    #[test]
    fn test_request_stop_does_not_discard_request_error() {
        let temporary = TempDir::new().expect("tempdir");
        let session = setup_test_session(temporary.path());
        let recording = Recording::new(session, None, temporary.path().to_path_buf());

        let context = OperationContext::default();
        let result = recording.request_stop(StopOptions::default(), &context);
        assert!(result.is_err(), "request_stop must propagate error when control socket missing");
        return;
    }

    #[test]
    fn test_finish_retains_child_ownership_on_failure() {
        let temporary = TempDir::new().expect("tempdir");
        let session = setup_test_session(temporary.path());

        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();

        // Verify child is alive initially
        let raw_pid = i32::try_from(pid).expect("pid fits i32");
        assert!(kill(Pid::from_raw(raw_pid), None).is_ok());

        let mut recording = Recording::new(session, Some(child), temporary.path().to_path_buf());

        // Context with short timeout; finish should fail because control socket/completion.json is missing
        let context = OperationContext::default().with_timeout(Duration::from_millis(50));
        let result = recording.finish(FinishPolicy::Graceful, &context);
        assert!(result.is_err(), "finish should fail due to missing completion/socket");

        // Crucial verification: self.child was NOT taken! Child is still alive because recording still owns it.
        assert!(kill(Pid::from_raw(raw_pid), None).is_ok(), "child should still be alive because ownership was retained");
        assert!(recording.child.is_some(), "recording.child must remain Some on failure");

        // When recording is dropped now, child should be reaped
        drop(recording);
        thread::sleep(Duration::from_millis(50));
        assert!(kill(Pid::from_raw(raw_pid), None).is_err(), "child should be reaped upon drop of recording");
        return;
    }

    #[test]
    fn test_finish_force_kill_retains_child_ownership_on_failure() {
        let temporary = TempDir::new().expect("tempdir");
        let session = setup_test_session(temporary.path());

        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");

        let mut recording = Recording::new(session, Some(child), temporary.path().to_path_buf());

        let context = OperationContext::default().with_timeout(Duration::from_millis(50));
        let result = recording.finish(FinishPolicy::ForceKill, &context);
        assert!(result.is_err(), "finish should fail due to missing completion.json");

        // Ownership must be retained in recording.child
        assert!(recording.child.is_some(), "recording.child must remain Some on failure");
        return;
    }

    #[test]
    fn test_finish_terminate_retains_child_ownership_on_failure() {
        let temporary = TempDir::new().expect("tempdir");
        let session = setup_test_session(temporary.path());

        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");

        let mut recording = Recording::new(session, Some(child), temporary.path().to_path_buf());

        let context = OperationContext::default().with_timeout(Duration::from_millis(50));
        let result = recording.finish(FinishPolicy::Terminate, &context);
        assert!(result.is_err(), "finish should fail due to missing control socket");

        // Ownership must be retained in recording.child
        assert!(recording.child.is_some(), "recording.child must remain Some on failure");
        return;
    }

    #[test]
    fn test_request_stop_with_signal_and_timeout() {
        let temporary = TempDir::new().expect("tempdir");
        let session = setup_test_session(temporary.path());
        let recording = Recording::new(session, None, temporary.path().to_path_buf());

        let context = OperationContext::default();
        let options = StopOptions {
            signal: Some(ProcessSignal::Terminate),
            wait_timeout: Some(Duration::from_millis(50)),
        };
        let result = recording.request_stop(options, &context);
        assert!(result.is_err(), "request_stop must propagate error when control socket missing");
        return;
    }
}
