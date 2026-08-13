//! Runner-neutral helpers for ordinary Rust tests.

use std::{
    error::Error as StdError,
    ffi::OsString,
    fmt,
    io::{self, BufRead as _, BufReader, Read as _, Write as _},
    mem::replace,
    net::Shutdown,
    os::unix::net::UnixStream,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    path::{Path, PathBuf},
    result::Result as StdResult,
    thread::panicking,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        create_private_directory, open_regular_file_nofollow, sync_directory, write_secure,
    },
    assertions::{
        AssertionError, AssertionFrameIdentity, ExpectationMismatch, SnapshotProfile,
        assert_cursor, assert_screen_text, normalized_snapshot,
    },
    bundle::BundleProfile,
    client::{Screen, Session, Terminal},
    control::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlRequest, ControlResponse, ControlResult,
        MAX_CONTROL_MESSAGE_BYTES, connect_session_control,
    },
    ghostty_vt::CursorState,
    locator::{Locator, LocatorSpec},
    observation::StatePoint,
    screen_frame::{ScreenFrame, ScreenSnapshotOptions},
    session::VerificationLevel,
};

const MAX_SECONDARY_FAILURE_CHARS: usize = 512;

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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

/// Typed operational or expectation failure from the Rust test helper.
#[derive(Debug)]
pub enum TerminalAssertionError {
    /// The attempt was already finalized.
    SessionUnavailable,
    /// Capturing or persisting evidence failed.
    Operation(Error),
    /// A valid runner-neutral expectation did not match.
    Assertion(AssertionError),
}

impl fmt::Display for TerminalAssertionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionUnavailable => {
                return f.write_str("terminal test session is already finalized");
            }
            Self::Operation(error) => return error.fmt(f),
            Self::Assertion(error) => return error.fmt(f),
        }
    }
}

impl StdError for TerminalAssertionError {}

impl From<Error> for TerminalAssertionError {
    fn from(error: Error) -> Self {
        return Self::Operation(error);
    }
}

impl From<AssertionError> for TerminalAssertionError {
    fn from(error: AssertionError) -> Self {
        return Self::Assertion(error);
    }
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
                    return sync_directory(parent);
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
            let outcome = if panicking() {
                TestOutcome::Panic
            } else {
                TestOutcome::Error
            };
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
    let stream =
        connect_session_control(session.path()).map_err(|error| return error.to_string())?;
    return send_stop_request(stream, &request, timeout);
}

fn send_stop_request(
    mut stream: UnixStream,
    request: &ControlRequest,
    timeout: Duration,
) -> StdResult<(), String> {
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| return stream.set_write_timeout(Some(timeout)))
        .map_err(|error| format!("failed to configure stop request deadline: {error}"))?;
    let mut encoded = serde_json::to_vec(request)
        .map_err(|error| format!("failed to encode stop request: {error}"))?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .map_err(|error| format!("failed to write stop request: {error}"))?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|error| format!("failed to finish stop request: {error}"))?;

    let mut response_bytes: Vec<u8> = Vec::new();
    BufReader::new(stream)
        .take(MAX_CONTROL_MESSAGE_BYTES + 1)
        .read_until(b'\n', &mut response_bytes)
        .map_err(|error| format!("failed to read stop response: {error}"))?;
    if response_bytes.last() != Some(&b'\n')
        || u64::try_from(response_bytes.len()).unwrap_or(u64::MAX) > MAX_CONTROL_MESSAGE_BYTES
    {
        return Err(
            "stop response must be one newline-terminated JSON object no larger than 1 MiB"
                .to_owned(),
        );
    }
    let response: ControlResponse = serde_json::from_slice(&response_bytes)
        .map_err(|error| format!("failed to decode stop response: {error}"))?;
    if response.version != CONTROL_PROTOCOL_VERSION || response.request_id != request.request_id {
        return Err("stop response version or request identifier does not match".to_owned());
    }
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

fn capture_failure_artifacts(
    session: &Session,
    attempt_directory: &Path,
    verified_artifacts: &mut Vec<PathBuf>,
    secondary_failures: &mut Vec<SecondaryFailure>,
) {
    let screenshot = attempt_directory.join("semantic-screenshot");
    match session
        .evidence()
        .semantic_screenshot(&screenshot, StatePoint::LatestPresented)
    {
        Ok(outcome) => verified_artifacts.push(outcome.directory),
        Err(error) => {
            secondary_failures.push(bounded_secondary("semantic_screenshot", &error.to_string()))
        }
    }

    let bundle = attempt_directory.join("debug-bundle");
    match session.evidence().bundle(&bundle, BundleProfile::Debug) {
        Ok(_) => verified_artifacts.push(bundle),
        Err(error) => {
            secondary_failures.push(bounded_secondary("debug_bundle", &error.to_string()))
        }
    }
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestEvidenceManifest {
    format: String,
    version: u32,
    session_id: Uuid,
    canonical_session: String,
    outcome: String,
    artifacts: Vec<String>,
}

#[allow(
    clippy::verbose_file_reads,
    reason = "the manifest must be opened with no-follow semantics before it is reread"
)]
fn write_evidence_manifest(
    attempt_directory: &Path,
    session_id: Uuid,
    canonical_session: &Path,
    report: &FinalizationReport,
) -> Result<PathBuf> {
    let relative_session = relative_evidence_path(attempt_directory, canonical_session)?;
    let artifacts = report
        .verified_artifacts
        .iter()
        .map(|path| return relative_evidence_path(attempt_directory, path))
        .collect::<Result<Vec<String>>>()?;
    let manifest = TestEvidenceManifest {
        format: "termtrace-rust-test-evidence-v1".to_owned(),
        version: 1,
        session_id,
        canonical_session: relative_session,
        outcome: format!("{:?}", report.outcome).to_lowercase(),
        artifacts,
    };
    let path = attempt_directory.join("termtrace-test-evidence.json");
    let mut bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    bytes.push(b'\n');
    write_secure(&path, &bytes)?;

    let (mut file, byte_count) = open_regular_file_nofollow(&path)?;
    if byte_count != u64::try_from(bytes.len()).unwrap_or(u64::MAX) {
        return Err(testing_error("test evidence manifest byte count changed"));
    }
    let mut observed = Vec::new();
    file.read_to_end(&mut observed).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let decoded: TestEvidenceManifest = serde_json::from_slice(&observed).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    if observed != bytes || decoded != manifest {
        return Err(testing_error(
            "test evidence manifest failed post-write verification",
        ));
    }
    return Ok(path);
}

fn relative_evidence_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        return testing_error("test artifact escaped its attempt evidence directory");
    })?;
    if relative.as_os_str().is_empty() {
        return Err(testing_error(
            "test artifact path resolved to the attempt root",
        ));
    }
    return relative
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| return testing_error("test artifact path is not valid UTF-8"));
}

fn snapshot_path(snapshot_root: &Path, name: &str) -> StdResult<PathBuf, TerminalAssertionError> {
    let valid = !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| return byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        return Ok(snapshot_root.join(format!("{name}.json")));
    }
    return Err(TerminalAssertionError::Assertion(AssertionError::Mismatch(
        ExpectationMismatch {
            assertion: "terminal_snapshot_name",
            expected: "1-128 ASCII letters, digits, '.', '_', or '-'".to_owned(),
            actual: bounded_text(name),
            excerpt: String::new(),
            frame: None,
            truncated: name.len() > MAX_SECONDARY_FAILURE_CHARS,
        },
    )));
}

/// Compare or explicitly update one deterministic frame snapshot file.
#[allow(
    clippy::verbose_file_reads,
    reason = "reviewed snapshots must be opened with no-follow semantics"
)]
pub fn assert_snapshot_file(
    frame: &ScreenFrame,
    path: &Path,
    profile: SnapshotProfile,
    options: ScreenSnapshotOptions,
    update: bool,
) -> StdResult<(), TerminalAssertionError> {
    const MAX_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;

    let mut actual = normalized_snapshot(frame, profile, options)?;
    actual.push('\n');
    let parent = path
        .parent()
        .ok_or_else(|| return testing_error("snapshot path has no parent"))?;
    if update {
        if !parent.exists() {
            create_private_directory(parent)?;
        }
        let temporary = parent.join(format!(".snapshot-{}.tmp", Uuid::now_v7()));
        write_secure(&temporary, actual.as_bytes())?;
        fs_err::rename(&temporary, path).map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        sync_directory(parent)?;
        return Ok(());
    }
    if !path.exists() {
        return Err(snapshot_mismatch(
            frame,
            format!("reviewed snapshot {}", path.display()),
            "snapshot file is missing".to_owned(),
            String::new(),
        ));
    }
    let (mut file, byte_count) = open_regular_file_nofollow(path)?;
    if byte_count > MAX_SNAPSHOT_BYTES {
        return Err(TerminalAssertionError::Operation(testing_error(
            "reviewed snapshot exceeds 4 MiB",
        )));
    }
    let mut expected = String::new();
    file.read_to_string(&mut expected).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    if expected == actual {
        return Ok(());
    }
    let excerpt = format!(
        "expected:\n{}\nactual:\n{}",
        bounded_text(&expected),
        bounded_text(&actual)
    );
    return Err(snapshot_mismatch(
        frame,
        format!("reviewed snapshot {}", path.display()),
        "normalized frame differs".to_owned(),
        excerpt,
    ));
}

fn snapshot_mismatch(
    frame: &ScreenFrame,
    expected: String,
    actual: String,
    excerpt: String,
) -> TerminalAssertionError {
    return TerminalAssertionError::Assertion(AssertionError::Mismatch(ExpectationMismatch {
        assertion: "terminal_snapshot",
        expected: bounded_text(&expected),

        actual: bounded_text(&actual),
        excerpt: bounded_text(&excerpt),
        frame: Some(Box::new(AssertionFrameIdentity {
            state_hash: frame.state_hash.clone(),
            projection_hash: frame.frame_hash.clone(),
            presented: frame.presented,
        })),
        truncated: expected.len() > MAX_SECONDARY_FAILURE_CHARS
            || actual.len() > MAX_SECONDARY_FAILURE_CHARS
            || excerpt.len() > MAX_SECONDARY_FAILURE_CHARS,
    }));
}

fn bounded_text(value: &str) -> String {
    let mut end = value.len().min(MAX_SECONDARY_FAILURE_CHARS);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    return value.get(..end).unwrap_or_default().to_owned();
}

fn testing_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

fn bounded_secondary(phase: &'static str, message: &str) -> SecondaryFailure {
    let mut end = message.len().min(MAX_SECONDARY_FAILURE_CHARS);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    return SecondaryFailure {
        phase,
        message: message.get(..end).unwrap_or_default().to_owned(),
        truncated: end != message.len(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn repeated_finalization_returns_the_recorded_report() {
        let recorded = FinalizationReport {
            outcome: TestOutcome::Passed,
            evidence_directory: Some(PathBuf::from("attempt.tt")),
            verified_artifacts: vec![PathBuf::from("attempt.tt")],
            secondary_failures: Vec::new(),
            retained: true,
        };
        let mut test = TerminalTest {
            state: LifecycleState::Finalized(recorded.clone()),
            attempt_directory: PathBuf::from("attempt.tt"),
            snapshot_root: PathBuf::from("snapshots"),
            update_snapshots: false,
            finalization_timeout: Duration::from_secs(1),
            evidence_policy: EvidencePolicy::OnFailure,
        };
        assert_eq!(test.finalize(TestOutcome::Error), recorded);
        assert_eq!(test.finalize(TestOutcome::Panic), recorded);
    }

    #[test]
    fn secondary_diagnostics_are_unicode_bounded() {
        let failure = bounded_secondary("cleanup", &"界".repeat(MAX_SECONDARY_FAILURE_CHARS));
        assert!(failure.truncated);
        assert!(failure.message.len() <= MAX_SECONDARY_FAILURE_CHARS);
        assert!(failure.message.is_char_boundary(failure.message.len()));
    }

    #[test]
    fn snapshot_names_cannot_escape_the_reviewed_root() {
        let error = snapshot_path(Path::new("snapshots"), "../escape")
            .expect_err("path traversal must fail");
        assert!(error.to_string().contains("terminal_snapshot_name"));
    }

    #[test]
    fn stop_request_obeys_the_supplied_deadline() {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let server_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            drop(server);
        });
        let request = ControlRequest::new(ControlCommand::Stop);
        let started = Instant::now();
        let error = send_stop_request(client, &request, Duration::from_millis(20))
            .expect_err("withheld stop response must time out");
        let elapsed = started.elapsed();

        assert!(error.contains("failed to read stop response"), "{error}");
        assert!(
            elapsed < Duration::from_millis(150),
            "stop request ignored its deadline: {elapsed:?}"
        );
        server_thread.join().expect("server thread");
    }
}
