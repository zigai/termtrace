use super::*;
use super::{
    evidence::{bounded_secondary, capture_failure_artifacts, write_evidence_manifest},
    snapshot::{bounded_text, snapshot_path},
};

/// Evidence retention requested for an owned terminal test.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EvidencePolicy {
    /// Discard evidence only after the body and finalization both succeed.
    #[default]
    OnFailure,
    /// Retain evidence for every outcome.
    Always,
    /// Leave retention and disposal to the caller while still finalizing the process.
    Manual,
}

/// Classification recorded when an owned test is finalized.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    /// The body returned successfully.
    Passed,
    /// A runner-neutral assertion failed.
    AssertionFailed,
    /// The body returned an error.
    Error,
    /// The body unwound.
    Panic,
    /// The runner declared a timeout.
    TimedOut,
}

/// One bounded failure that could not replace the body's primary failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecondaryFailure {
    /// Stable finalization phase.
    pub phase: &'static str,
    /// Bounded human-readable detail.
    pub message: String,
    /// Whether the original detail was truncated.
    pub truncated: bool,
}

/// Result of exactly-once test finalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalizationReport {
    /// Outcome supplied by the wrapper or advanced caller.
    pub outcome: TestOutcome,
    /// Canonical evidence retained by Termtrace.
    pub evidence_directory: Option<PathBuf>,
    /// Artifact paths verified through public Termtrace verifiers.
    pub verified_artifacts: Vec<PathBuf>,
    /// Non-primary bounded cleanup failures.
    pub secondary_failures: Vec<SecondaryFailure>,
    /// Whether retention was requested or forced by a failure.
    pub retained: bool,
}

/// Configuration for one fresh, attempt-owned terminal session.
#[derive(Clone, Debug)]
pub struct TerminalTestConfig {
    /// Termtrace executable and caller-owned evidence root.
    pub terminal: Terminal,
    /// Child command and arguments.
    pub command: Vec<OsString>,
    /// Immutable initial terminal width.
    pub columns: u16,
    /// Immutable initial terminal height.
    pub rows: u16,
    /// Actor launch deadline.
    pub startup_timeout: Duration,
    /// Graceful finalization deadline.
    pub finalization_timeout: Duration,
    /// Evidence retention policy.
    pub evidence_policy: EvidencePolicy,
    /// Caller-owned deterministic snapshot root.
    pub snapshot_root: PathBuf,
    /// Whether reviewed snapshots may be created or replaced.
    pub update_snapshots: bool,
}

impl TerminalTestConfig {
    /// Construct a conservative configuration around an explicit executable, root, and command.
    pub fn new(terminal: Terminal, command: impl IntoIterator<Item = OsString>) -> Self {
        let snapshot_root = terminal.evidence_root().join("snapshots");

        return Self {
            terminal,
            command: command.into_iter().collect(),
            columns: 160,
            rows: 45,
            startup_timeout: Duration::from_secs(10),
            finalization_timeout: Duration::from_secs(10),
            evidence_policy: EvidencePolicy::OnFailure,
            snapshot_root,
            update_snapshots: false,
        };
    }
}

enum LifecycleState {
    Open(Session),
    Finalizing,
    Finalized(FinalizationReport),
}

/// One fresh session owned by one ordinary Rust test attempt.
pub struct TerminalTest {
    state: LifecycleState,
    attempt_directory: PathBuf,
    snapshot_root: PathBuf,
    update_snapshots: bool,
    finalization_timeout: Duration,
    evidence_policy: EvidencePolicy,
}

impl TerminalTest {
    /// Launch the fresh session when the test body begins.
    pub fn launch(config: TerminalTestConfig) -> Result<Self> {
        let attempt_directory = config
            .terminal
            .evidence_root()
            .join(format!(".termtrace-test-{}", Uuid::now_v7()));

        create_private_directory(&attempt_directory)?;
        let session = config
            .terminal
            .with_evidence_root(&attempt_directory)
            .launch(config.command)
            .geometry(config.columns, config.rows)
            .startup_timeout(config.startup_timeout)
            .start()?;

        return Ok(Self {
            state: LifecycleState::Open(session),
            attempt_directory,
            snapshot_root: config.snapshot_root,
            update_snapshots: config.update_snapshots,
            finalization_timeout: config.finalization_timeout,
            evidence_policy: config.evidence_policy,
        });
    }

    /// Borrow the owned session.
    ///
    /// Returns `None` after explicit finalization, preventing accidental post-finalize I/O.
    #[must_use]
    pub fn session(&self) -> Option<&Session> {
        let LifecycleState::Open(session) = &self.state else {
            return None;
        };

        return Some(session);
    }

    /// Mutably borrow the owned session for explicit advanced lifecycle operations.
    #[must_use]
    pub fn session_mut(&mut self) -> Option<&mut Session> {
        let LifecycleState::Open(session) = &mut self.state else {
            return None;
        };

        return Some(session);
    }

    /// Attempt-owned evidence directory while retained artifacts remain available.
    #[must_use]
    pub fn evidence_directory(&self) -> Option<&Path> {
        if matches!(
            self.state,
            LifecycleState::Finalized(FinalizationReport {
                retained: false,
                ..
            })
        ) {
            return None;
        }
        return Some(&self.attempt_directory);
    }

    /// Presented-screen namespace for this attempt while it remains open.
    #[must_use]
    pub fn screen(&self) -> Option<Screen<'_>> {
        return self.session().map(Session::screen);
    }

    /// Bind a pure immutable locator to the owned session without performing I/O.
    #[must_use]
    pub fn locator(&self, spec: LocatorSpec) -> Option<Locator<'_>> {
        return self
            .session()
            .map(|session| return Locator::new(session, spec));
    }

    /// Capture one exact bounded frame and assert that its presented text contains a literal.
    pub fn assert_screen_text(
        &self,
        expected: &str,
    ) -> StdResult<ScreenFrame, TerminalAssertionError> {
        let screen = self
            .screen()
            .ok_or(TerminalAssertionError::SessionUnavailable)?;

        let frame = screen.frame(Default::default())?;
        assert_screen_text(&frame, expected)
            .map_err(|error| return AssertionError::Mismatch(error))?;

        return Ok(frame);
    }

    /// Capture one exact bounded frame and assert its complete cursor state.
    pub fn assert_cursor(
        &self,
        expected: &CursorState,
    ) -> StdResult<ScreenFrame, TerminalAssertionError> {
        let screen = self
            .screen()
            .ok_or(TerminalAssertionError::SessionUnavailable)?;

        let frame = screen.frame(Default::default())?;
        assert_cursor(&frame, expected).map_err(|error| return AssertionError::Mismatch(error))?;

        return Ok(frame);
    }

    /// Compare one already captured exact frame with a reviewed deterministic snapshot.
    pub fn assert_snapshot(
        &self,
        name: &str,
        frame: &ScreenFrame,
        profile: SnapshotProfile,
        options: ScreenSnapshotOptions,
    ) -> StdResult<(), TerminalAssertionError> {
        let path = snapshot_path(&self.snapshot_root, name)?;

        return assert_snapshot_file(frame, &path, profile, options, self.update_snapshots);
    }

    /// Finalize exactly once; repeated calls return the originally recorded report.
    pub fn finalize(&mut self, outcome: TestOutcome) -> FinalizationReport {
        if let LifecycleState::Finalized(report) = &self.state {
            return report.clone();
        }

        let state = replace(&mut self.state, LifecycleState::Finalizing);
        let LifecycleState::Open(mut session) = state else {
            let report = FinalizationReport {
                outcome,
                evidence_directory: Some(self.attempt_directory.clone()),
                verified_artifacts: Vec::new(),
                secondary_failures: vec![bounded_secondary(
                    "lifecycle",
                    "recursive finalization was rejected",
                )],
                retained: true,
            };
            self.state = LifecycleState::Finalized(report.clone());

            return report;
        };
        let canonical_session = session.path().to_path_buf();
        let session_id = session.session_id();
        let body_passed = outcome == TestOutcome::Passed;
        let mut secondary_failures: Vec<SecondaryFailure> = Vec::new();
        let mut verified_artifacts: Vec<PathBuf> = Vec::new();
        let finalization_started = Instant::now();
        let stop_failure = stop_session_with_timeout(
            &session,
            remaining_timeout(finalization_started, self.finalization_timeout),
        )
        .err();
        let completed = match session.complete(remaining_timeout(
            finalization_started,
            self.finalization_timeout,
        )) {
            Ok(_) => {
                if let Some(stop_error) = stop_failure {
                    secondary_failures.push(bounded_secondary("session_stop", &stop_error));
                }

                true
            }
            Err(error) => {
                if let Some(stop_error) = stop_failure {
                    secondary_failures.push(bounded_secondary("session_stop", &stop_error));
                }

                secondary_failures
                    .push(bounded_secondary("session_completion", &error.to_string()));

                if let Err(close_error) = session.force_close() {
                    secondary_failures.push(bounded_secondary(
                        "session_force_close",
                        &close_error.to_string(),
                    ));
                }

                false
            }
        };

        let mut failure_artifacts_captured = false;

        if completed && !body_passed {
            capture_failure_artifacts(
                &session,
                &self.attempt_directory,
                &mut verified_artifacts,
                &mut secondary_failures,
            );

            failure_artifacts_captured = true;
        }

        if completed {
            let verification = session.evidence().verify(VerificationLevel::Content);
            if verification.valid {
                verified_artifacts.push(canonical_session.clone());
            } else {
                let summary = verification
                    .findings
                    .iter()
                    .map(|finding| format!("{}: {}", finding.code, finding.message))
                    .collect::<Vec<String>>()
                    .join("; ");
                secondary_failures.push(bounded_secondary("session_verify", &summary));
            }
        }

        if completed && !failure_artifacts_captured && !secondary_failures.is_empty() {
            capture_failure_artifacts(
                &session,
                &self.attempt_directory,
                &mut verified_artifacts,
                &mut secondary_failures,
            );
        }

        let retained = match self.evidence_policy {
            EvidencePolicy::Always | EvidencePolicy::Manual => true,
            EvidencePolicy::OnFailure => !body_passed || !secondary_failures.is_empty(),
        };

        if !retained {
            let discard_failure = session
                .discard_completed()
                .and_then(|()| {
                    return fs_err::remove_dir_all(&self.attempt_directory).map_err(|source| {
                        return Error::Filesystem {
                            path: self.attempt_directory.clone(),
                            source,
                        };
                    });
                })
                .and_then(|()| {
                    let Some(parent) = self.attempt_directory.parent() else {
                        return Ok(());
                    };

                    return Ok(sync_directory(parent)?);
                })
                .err();

            verified_artifacts.clear();

            if let Some(error) = discard_failure {
                secondary_failures.push(bounded_secondary("evidence_discard", &error.to_string()));
            }
        }

        let mut report = FinalizationReport {
            outcome,
            evidence_directory: retained.then(|| return self.attempt_directory.clone()),
            verified_artifacts,
            secondary_failures,
            retained,
        };

        if retained {
            match write_evidence_manifest(
                &self.attempt_directory,
                session_id,
                &canonical_session,
                &report,
            ) {
                Ok(path) => report.verified_artifacts.push(path),
                Err(error) => report
                    .secondary_failures
                    .push(bounded_secondary("evidence_manifest", &error.to_string())),
            }
        }

        self.state = LifecycleState::Finalized(report.clone());

        return report;
    }
}

impl Drop for TerminalTest {
    fn drop(&mut self) {
        if matches!(self.state, LifecycleState::Open(_)) {
            let outcome = if panicking() { TestOutcome::Panic } else { TestOutcome::Error };
            let _report = self.finalize(outcome);
        }
    }
}

/// Failure returned by [`run_terminal_test`] without erasing the primary body error.
#[derive(Debug)]
pub enum TestFailure<E> {
    /// The attempt could not launch.
    Launch(Error),
    /// The body error remains primary; finalization detail is attached.
    Body {
        /// Original body error.
        error: E,
        /// Exactly-once finalization report.
        finalization: FinalizationReport,
    },
    /// A passing body was followed by a finalization failure.
    Finalization(FinalizationReport),
}

/// Run one ordinary Rust test body with fresh ownership and exactly-once finalization.
///
/// Panics are caught only around the body, finalized, reported with bounded secondary cleanup
/// diagnostics when needed, and resumed with their original payload.
pub fn run_terminal_test<T, E>(
    config: TerminalTestConfig,
    body: impl FnOnce(&mut TerminalTest) -> StdResult<T, E>,
) -> StdResult<T, TestFailure<E>> {
    let mut test = TerminalTest::launch(config).map_err(TestFailure::Launch)?;
    let body_result = catch_unwind(AssertUnwindSafe(|| return body(&mut test)));
    match body_result {
        Ok(Ok(value)) => {
            let report = test.finalize(TestOutcome::Passed);
            if report.secondary_failures.is_empty() {
                return Ok(value);
            }
            return Err(TestFailure::Finalization(report));
        }
        Ok(Err(error)) => {
            let finalization = test.finalize(TestOutcome::Error);

            return Err(TestFailure::Body {
                error,
                finalization,
            });
        }
        Err(payload) => {
            let report = test.finalize(TestOutcome::Panic);
            emit_panic_finalization_diagnostics(&report);

            resume_unwind(payload);
        }
    }
}

fn remaining_timeout(started: Instant, timeout: Duration) -> Duration {
    return timeout.saturating_sub(started.elapsed());
}

fn stop_session_with_timeout(session: &Session, timeout: Duration) -> StdResult<(), String> {
    if timeout.is_zero() {
        return Err("finalization deadline expired before the stop request".to_owned());
    }

    let request = ControlRequest::new(ControlCommand::Stop);
    let timing = ExchangeTiming::deadline(timeout).map_err(|error| return error.to_string())?;
    let response =
        exchange(session.path(), &request, timing).map_err(|error| return error.to_string())?;

    match response.result {
        ControlResult::Ok { .. } => return Ok(()),
        ControlResult::Error { code, message, .. } => {
            return Err(format!("stop request was rejected ({code:?}): {message}"));
        }
    }
}

fn emit_panic_finalization_diagnostics(report: &FinalizationReport) {
    if report.secondary_failures.is_empty() {
        return;
    }

    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    let _ = writeln!(
        stderr,
        "termtrace: secondary failures occurred while finalizing a panicked test"
    );

    for failure in &report.secondary_failures {
        let _ = writeln!(stderr, "termtrace: {}: {}", failure.phase, failure.message);
    }

    if let Some(path) = &report.evidence_directory {
        let _ = writeln!(
            stderr,
            "termtrace: retained panic evidence: {}",
            bounded_text(&path.display().to_string())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_diagnostics_are_unicode_bounded() {
        let failure = bounded_secondary("cleanup", &"界".repeat(MAX_SECONDARY_FAILURE_CHARS));
        assert!(failure.truncated);
        assert_eq!(
            failure.message,
            "界".repeat(MAX_SECONDARY_FAILURE_CHARS / "界".len())
        );
    }

    #[test]
    fn snapshot_names_cannot_escape_the_reviewed_root() {
        let error = snapshot_path(Path::new("snapshots"), "../escape")
            .expect_err("path traversal must fail");
        assert!(error.to_string().contains("terminal_snapshot_name"));
    }
}
