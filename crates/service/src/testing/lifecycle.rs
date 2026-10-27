use super::*;
use super::{
    evidence::{bounded_secondary, capture_failure_artifacts, write_evidence_manifest},
    snapshot::{bounded_text, path_for},
};
use crate::locator::WaitCancellation;

/// Evidence retention requested for an owned terminal test.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct SecondaryFailure {
    /// Stable finalization phase.
    pub phase: &'static str,
    /// Bounded human-readable detail.
    pub message: String,
    /// Whether the original detail was truncated.
    pub truncated: bool,
}

/// Result of exactly-once test finalization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct FinalizationReport {
    /// Outcome supplied by the wrapper or advanced caller.
    pub outcome: TestOutcome,
    /// Canonical evidence retained by Termtrace.
    pub evidence_directory: Option<PathBuf>,
    /// Artifact paths verified through public Termtrace verifiers.
    pub verified_artifacts: Vec<PathBuf>,
    /// Non-primary bounded cleanup failures.
    pub secondary_failures: Vec<SecondaryFailure>,
    /// Whether evidence actually survives (policy, failure, or unowned canonical data).
    pub retained: bool,
}

/// Cache metadata for layered native adapters, independent of the runner's exception objects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedFinalization {
    /// Exact caller-owned attempt root.
    pub root: PathBuf,
    /// Policy committed by this finalization operation.
    pub policy: EvidencePolicy,
    /// Original immutable report.
    pub report: FinalizationReport,
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
            columns: defaults::COLS,
            rows: defaults::ROWS,
            startup_timeout: defaults::STARTUP_TIMEOUT,
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
    artifact_directory: PathBuf,
    snapshot_root: PathBuf,
    update_snapshots: bool,
    finalization_timeout: Duration,
    evidence_policy: EvidencePolicy,
}

impl TerminalTest {
    /// Launch the fresh session when the test body begins.
    ///
    /// # Errors
    /// Returns an error when the private attempt directory cannot be created or the session cannot
    /// be started.
    pub fn launch(config: TerminalTestConfig) -> Result<Self> {
        let attempt_directory = config
            .terminal
            .evidence_root()
            .join(format!(".termtrace-test-{}", Uuid::now_v7()));

        create_private_directory(&attempt_directory)?;
        let session = config
            .terminal
            .with_evidence_root(&attempt_directory)
            .start(
                LaunchSpec::new(config.command)
                    .geometry(config.columns, config.rows)
                    .startup_timeout(config.startup_timeout),
            )?;

        return Ok(Self {
            state: LifecycleState::Open(session),
            artifact_directory: attempt_directory.clone(),
            attempt_directory,
            snapshot_root: config.snapshot_root,
            update_snapshots: config.update_snapshots,
            finalization_timeout: config.finalization_timeout,
            evidence_policy: config.evidence_policy,
        });
    }

    /// Adopt one already launched owned session, without launching a second process.
    ///
    /// # Errors
    /// Rejects an evidence root that does not contain the session. The caller must own the root.
    pub fn from_owned_session(
        session: Session,
        attempt_directory: PathBuf,
        evidence_policy: EvidencePolicy,
        finalization_timeout: Duration,
    ) -> Result<Self> {
        if !session.path().starts_with(&attempt_directory) || session.path() == attempt_directory {
            // Never drop a consumed launched Session on argument validation: Session is an
            // explicit owner, not a kill-on-drop guard. Reuse this same engine conservatively.
            let actual_root = session
                .path()
                .parent()
                .unwrap_or_else(|| return Path::new("."))
                .to_path_buf();
            let artifact_directory =
                actual_root.join(format!(".test-artifacts-{}", session.session_id()));

            let mut fallback = Self {
                state: LifecycleState::Open(session),
                snapshot_root: actual_root.join("snapshots"),
                attempt_directory: actual_root.clone(),
                artifact_directory,
                update_snapshots: false,
                finalization_timeout,
                evidence_policy: EvidencePolicy::Always,
            };
            let report = fallback.finalize(TestOutcome::Error);

            return Err(super::snapshot::testing_error(format!(
                "owned session is outside the test root; finalized at {}; secondary failures: {:?}",
                actual_root.display(),
                report.secondary_failures
            )));
        }

        let artifact_directory =
            attempt_directory.join(format!(".test-artifacts-{}", session.session_id()));

        // Artifact preparation must not prevent stop/completion/force-close of this owner.
        return Ok(Self {
            state: LifecycleState::Open(session),
            artifact_directory,
            snapshot_root: attempt_directory.join("snapshots"),
            attempt_directory,
            update_snapshots: false,
            finalization_timeout,
            evidence_policy,
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

    /// Bind a pure immutable locator to the owned session without performing I/O.
    #[must_use]
    pub fn locator(&self, spec: LocatorSpec) -> Option<Locator<'_>> {
        return self
            .session()
            .map(|session| return Locator::new(session, spec));
    }

    /// Capture one exact bounded frame and assert that its presented text contains a literal.
    ///
    /// # Errors
    /// Returns `TerminalAssertionError::SessionUnavailable` when the session is closed, a capture
    /// error when the frame cannot be captured, and a mismatch when `expected` is not on screen.
    pub fn assert_screen_text(
        &self,
        expected: &str,
    ) -> StdResult<ScreenFrame, TerminalAssertionError> {
        let session = self
            .session()
            .ok_or(TerminalAssertionError::SessionUnavailable)?;

        let frame = session.frame(Default::default())?;
        assert_screen_text(&frame, expected)?;

        return Ok(frame);
    }

    /// Capture one exact bounded frame and assert its complete cursor state.
    ///
    /// # Errors
    /// Returns `TerminalAssertionError::SessionUnavailable` when the session is closed, a capture
    /// error when the frame cannot be captured, and a mismatch when the cursor differs from
    /// `expected`.
    pub fn assert_cursor(
        &self,
        expected: &CursorState,
    ) -> StdResult<ScreenFrame, TerminalAssertionError> {
        let session = self
            .session()
            .ok_or(TerminalAssertionError::SessionUnavailable)?;

        let frame = session.frame(Default::default())?;
        assert_cursor(&frame, expected).map_err(|error| return AssertionError::Mismatch(error))?;

        return Ok(frame);
    }

    /// Compare one already captured exact frame with a reviewed deterministic snapshot.
    ///
    /// # Errors
    /// Returns a validation error when `name` is not 1-128 ASCII letters, digits, `.`, `_`, or `-`, and the
    /// errors of [`assert_snapshot_file`].
    pub fn assert_snapshot(
        &self,
        name: &str,
        frame: &ScreenFrame,
        profile: SnapshotProfile,
        options: ScreenSnapshotOptions,
    ) -> StdResult<(), TerminalAssertionError> {
        let path = path_for(&self.snapshot_root, name)?;

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
        let completion = match session.stop_bounded(
            StopRequest::default(),
            remaining_timeout(finalization_started, self.finalization_timeout),
        ) {
            Ok(completion) => Some(completion),
            Err(stop_error) => {
                secondary_failures.push(bounded_secondary("session_stop", &stop_error.to_string()));

                match session.complete(remaining_timeout(
                    finalization_started,
                    self.finalization_timeout,
                )) {
                    Ok(completion) => Some(completion),
                    Err(error) => {
                        secondary_failures
                            .push(bounded_secondary("session_completion", &error.to_string()));

                        if let Err(close_error) = session.force_close_for_finalization() {
                            secondary_failures.push(bounded_secondary(
                                "session_force_close",
                                &close_error.to_string(),
                            ));
                        }

                        None
                    }
                }
            }
        };
        session.seal_lifecycle_observers();

        if let Err(error) = session.drain_lifecycle_observers(Session::OBSERVER_DRAIN_TIMEOUT) {
            secondary_failures.push(bounded_secondary("lifecycle_observers", &error.to_string()));
        }

        let completed = completion.is_some();

        if let Some(failure) = completion
            .as_ref()
            .and_then(|completion| return completion.failure.as_ref())
        {
            secondary_failures.push(bounded_secondary(
                "session_failure",
                &format!("{}: {}", failure.code, failure.message),
            ));
        }

        let artifact_directory_ready = if self.artifact_directory == self.attempt_directory {
            true
        } else {
            match create_private_directory(&self.artifact_directory) {
                Ok(()) => true,
                Err(error) => {
                    secondary_failures
                        .push(bounded_secondary("artifact_directory", &error.to_string()));
                    false
                }
            }
        };

        let mut failure_artifacts_captured = false;

        if completed && !body_passed && artifact_directory_ready {
            capture_failure_artifacts(
                &session,
                &self.artifact_directory,
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

        if completed
            && artifact_directory_ready
            && !failure_artifacts_captured
            && !secondary_failures.is_empty()
        {
            capture_failure_artifacts(
                &session,
                &self.artifact_directory,
                &mut verified_artifacts,
                &mut secondary_failures,
            );
        }

        let mut retained = match self.evidence_policy {
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
                retained = self.attempt_directory.exists();
            }
        }

        let mut report = FinalizationReport {
            outcome,
            evidence_directory: retained.then(|| return self.attempt_directory.clone()),
            verified_artifacts,
            secondary_failures,
            retained,
        };
        report
            .verified_artifacts
            .retain(|path| return path.exists());

        if retained && canonical_session.exists() && artifact_directory_ready {
            match write_evidence_manifest(
                &self.attempt_directory,
                &self.artifact_directory,
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

        report
            .verified_artifacts
            .retain(|path| return path.exists());

        report.retained = self.attempt_directory.exists();
        report.evidence_directory = report
            .retained
            .then(|| return self.attempt_directory.clone());
        self.state = LifecycleState::Finalized(report.clone());

        return report;
    }
}

/// An owned session or its already-recorded finalization, for layered runner adapters.
pub enum OwnedSessionFinalization {
    /// Transfer a fresh session, or retain an attachment error while continuing siblings.
    Open(Result<Session>),
    /// Reuse an earlier exactly-once report without repeating stop, capture or verification.
    Finalized {
        /// The original report.
        report: FinalizationReport,
        /// Canonical session path for eventual successful disposal.
        path: PathBuf,
    },
}

/// Finalize a collection of already launched, exclusively owned sessions through the same
/// lifecycle engine as [`TerminalTest`]. Every sibling is attempted, even after a failure.
///
/// The caller owns `root` and must prevent further launches before calling. Session attachment
/// failures are passed as errors so that they retain evidence and cannot hide successful siblings.
#[must_use]
pub fn finalize_owned_sessions(
    sessions: Vec<OwnedSessionFinalization>,
    root: &Path,
    outcome: TestOutcome,
    policy: EvidencePolicy,
    timeout: Duration,
    retain_request: Option<&WaitCancellation>,
) -> FinalizationReport {
    let mut report = FinalizationReport {
        outcome,
        evidence_directory: None,
        verified_artifacts: Vec::new(),
        secondary_failures: Vec::new(),
        retained: false,
    };

    let mut completed_paths = Vec::new();
    for session in sessions {
        let session = match session {
            OwnedSessionFinalization::Open(session) => session,
            OwnedSessionFinalization::Finalized {
                report: previous,
                path,
            } => {
                completed_paths.push((PathBuf::new(), path));
                report
                    .verified_artifacts
                    .extend(previous.verified_artifacts);

                report
                    .secondary_failures
                    .extend(previous.secondary_failures);
                continue;
            }
        };

        let session = session.and_then(|session| {
            return TerminalTest::from_owned_session(
                session,
                root.to_path_buf(),
                EvidencePolicy::Always,
                timeout,
            );
        });
        match session {
            Ok(mut test) => {
                if let Some(session) = test.session() {
                    completed_paths
                        .push((session.helper().to_path_buf(), session.path().to_path_buf()));
                }

                let finalized = test.finalize(outcome);
                report
                    .verified_artifacts
                    .extend(finalized.verified_artifacts);

                report
                    .secondary_failures
                    .extend(finalized.secondary_failures);
            }
            Err(error) => report
                .secondary_failures
                .push(bounded_secondary("session_adopt", &error.to_string())),
        }
    }

    if retain_request.is_some_and(WaitCancellation::is_cancelled) {
        report.secondary_failures.push(bounded_secondary(
            "cancellation",
            "retention requested during finalization; input is not rolled back",
        ));
    }

    // A client exposed alongside the owned launch callable may also create caller-owned
    // recordings in this root. Never recursively discard those canonical directories.
    let foreign_session = match fs_err::read_dir(root) {
        Ok(entries) => entries
            .map(|entry| {
                return entry.map_err(|source| {
                    return Error::Filesystem {
                        path: root.to_path_buf(),
                        source,
                    };
                });
            })
            .try_fold(false, |found, entry| {
                let entry = entry?;
                let path = entry.path();
                let known = completed_paths
                    .iter()
                    .any(|(_, owned)| return *owned == path);

                let canonical_name = path
                    .extension()
                    .is_some_and(|extension| return extension == "tt");

                return Ok::<bool, Error>(
                    found || (!known && (canonical_name || path.join("session.json").is_file())),
                );
            }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::Filesystem {
            path: root.to_path_buf(),
            source,
        }),
    };

    let foreign_session = match foreign_session {
        Ok(found) => found,
        Err(error) => {
            report
                .secondary_failures
                .push(bounded_secondary("evidence_ownership", &error.to_string()));
            true
        }
    };

    let retain = foreign_session
        || policy != EvidencePolicy::OnFailure
        || outcome != TestOutcome::Passed
        || !report.secondary_failures.is_empty();

    if !retain {
        for (helper, path) in completed_paths {
            if let Err(error) = Session::attach(path, helper)
                .and_then(|mut session| return session.discard_completed())
            {
                report
                    .secondary_failures
                    .push(bounded_secondary("evidence_discard", &error.to_string()));
            }
        }

        if report.secondary_failures.is_empty()
            && let Err(error) = fs_err::remove_dir_all(root)
                .map_err(|source| {
                    return Error::Filesystem {
                        path: root.to_path_buf(),
                        source,
                    };
                })
                .and_then(|()| {
                    if let Some(parent) = root.parent() {
                        sync_directory(parent)?;
                    }

                    return Ok(());
                })
        {
            report
                .secondary_failures
                .push(bounded_secondary("evidence_discard", &error.to_string()));
        }
    }

    report
        .verified_artifacts
        .retain(|path| return path.exists());

    report.retained = root.exists();
    report.evidence_directory = report.retained.then(|| return root.to_path_buf());

    return report;
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
///
/// # Errors
/// Returns `TestFailure::Launch` when the session cannot start, `TestFailure::Body` with the
/// finalization report when the body returns an error, and `TestFailure::Finalization` when a
/// passing body leaves secondary finalization failures. A panicking body is finalized and its panic
/// resumed.
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

fn emit_panic_finalization_diagnostics(report: &FinalizationReport) {
    if report.secondary_failures.is_empty() {
        return;
    }

    // Standard error is the last-resort channel while a test panics; a failed write has nowhere
    // else to be reported.
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    drop(writeln!(
        stderr,
        "termtrace: secondary failures occurred while finalizing a panicked test"
    ));

    for failure in &report.secondary_failures {
        drop(writeln!(
            stderr,
            "termtrace: {}: {}",
            failure.phase, failure.message
        ));
    }

    if let Some(path) = &report.evidence_directory {
        drop(writeln!(
            stderr,
            "termtrace: retained panic evidence: {}",
            bounded_text(&path.display().to_string())
        ));
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
        let error =
            path_for(Path::new("snapshots"), "../escape").expect_err("path traversal must fail");
        assert!(error.to_string().contains("terminal_snapshot_name"));
        assert!(matches!(error, TerminalAssertionError::Validation(_)));
    }
}
