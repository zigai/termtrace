//! Original-execution recordings: the command runs in a private Ghostty display whose pixels
//! are acquired while the canonical session records it.

use super::{
    await_ready, client_error, launch::validate_visual_profile, Readiness, Session, SessionTarget,
    StopRequest,
};
use crate::{
    defaults,
    registry::{spawn_detached, RegistryService},
    system::SystemService,
    Error, Result,
};
use nix::{
    libc::{O_CLOEXEC, O_NOFOLLOW},
    sys::signal::{kill, Signal},
    unistd::{geteuid, Pid},
};
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::Read as _,
    os::unix::{
        ffi::OsStrExt as _,
        fs::{MetadataExt as _, OpenOptionsExt as _},
    },
    path::{Path, PathBuf},
    process::Child,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use termtrace_control::{observe_phase, read_completed};
use termtrace_engine::recording::resolve_visual_profile;
use termtrace_evidence::session::read_manifest;
use termtrace_media::native_capture::verify_original_capture;
use termtrace_model::{
    artifact::{
        original::{
            original_capture_status_path, CellGrid, OriginalCaptureFacts, OriginalCaptureFailure,
            OriginalCaptureState, OriginalCaptureStatus, PixelCanvas, SurvivingKind,
            SurvivingPathRecord,
        },
        sdk::MediaArtifactResult,
    },
    execution::{
        environment::EnvironmentSpec,
        session::{SessionCompletion, SessionPhase},
        visual_profile::{VisualProfile, VisualProfileFailure},
    },
    protocol::capability::NATIVE_VIDEO_CAPABILITY,
    registry::DetachedSessionPaths,
    value::ByteString,
};
use termtrace_process::ProcessIdentity;
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long a cancelled or timed-out supervisor may take to release the display, encoder and
/// terminal before it is killed.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const KILL_REAP_TIMEOUT: Duration = Duration::from_secs(2);
const SUPERVISION_INTERVAL: Duration = Duration::from_millis(50);
/// How long canonical completion may take to appear after the supervisor is gone before its
/// application is killed.
const SUPERVISOR_LOSS_GRACE: Duration = Duration::from_secs(2);
const DEFAULT_VIEWPORT: (u16, u16) = (1280, 720);

/// Constant acquisition rate of an original capture.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OriginalFps {
    /// 24 frames per second.
    #[default]
    Fps24,
    /// 30 frames per second.
    Fps30,
    /// 60 frames per second.
    Fps60,
}

impl OriginalFps {
    /// Frames per second.
    #[must_use]
    pub const fn get(self) -> u16 {
        return match self {
            Self::Fps24 => 24,
            Self::Fps30 => 30,
            Self::Fps60 => 60,
        };
    }

    /// The rate for `fps`, which must be 24, 30 or 60.
    ///
    /// # Errors
    /// Returns an error for any other rate.
    pub fn new(fps: u16) -> Result<Self> {
        return match fps {
            24 => Ok(Self::Fps24),
            30 => Ok(Self::Fps30),
            60 => Ok(Self::Fps60),
            _ => Err(client_error("original capture fps must be 24, 30, or 60")),
        };
    }
}

/// How the captured terminal's initial grid is chosen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginalGeometry {
    /// Ghostty fills a window of this pixel size; the grid follows its font metrics.
    Viewport {
        /// Window width in pixels.
        width: u16,
        /// Window height in pixels.
        height: u16,
    },
    /// The terminal starts with exactly this grid, or the capture fails with `grid_mismatch`.
    Grid(CellGrid),
}

impl Default for OriginalGeometry {
    fn default() -> Self {
        return Self::Viewport {
            width: DEFAULT_VIEWPORT.0,
            height: DEFAULT_VIEWPORT.1,
        };
    }
}

/// What an original recording runs and how it is captured.
#[derive(Clone, Debug)]
pub struct OriginalLaunchSpec {
    /// Child command and arguments.
    pub command: Vec<OsString>,
    /// Child working directory, or the caller's when `None`.
    pub working_directory: Option<PathBuf>,
    /// How long acquisition and recorder startup may take before the capture is torn down.
    pub startup_timeout: Duration,
    /// Complete deterministic child-environment policy, passed without process arguments.
    pub environment: EnvironmentSpec,
    /// Terminate runaway work after this many output bytes without truncating evidence.
    pub output_byte_budget: Option<u64>,
    /// Terminate the child after this wall-clock duration, in whole milliseconds.
    pub max_duration: Option<Duration>,
    /// Terminate the child after this long without activity, in whole milliseconds.
    pub idle_timeout: Option<Duration>,
    /// Initial grid choice.
    pub geometry: OriginalGeometry,
    /// Constant display and video size; the terminal window when `None`. Live resizes must
    /// keep the window inside it.
    pub canvas: Option<PixelCanvas>,
    /// Acquisition rate.
    pub fps: OriginalFps,
    /// Ghostty configuration resolved into the recorded visual profile.
    pub ghostty_config: Option<PathBuf>,
    /// Visual profile with a native section; replaces `ghostty_config`.
    pub visual_profile: Option<VisualProfile>,
    /// Registry alias bound to the session once it is live.
    pub alias: Option<String>,
    /// Registry state root for `alias`; the default registry when `None`.
    pub state_root: Option<PathBuf>,
}

impl OriginalLaunchSpec {
    /// Run `command` with every other setting at its default.
    pub fn new(command: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        return Self {
            command: command.into_iter().map(Into::into).collect(),
            working_directory: None,
            startup_timeout: defaults::STARTUP_TIMEOUT,
            environment: EnvironmentSpec::default(),
            output_byte_budget: None,
            max_duration: None,
            idle_timeout: None,
            geometry: OriginalGeometry::default(),
            canvas: None,
            fps: OriginalFps::default(),
            ghostty_config: None,
            visual_profile: None,
            alias: None,
            state_root: None,
        };
    }

    fn validate(&self) -> Result<()> {
        if self.command.is_empty() {
            return Err(client_error("launch command is empty"));
        }

        match self.geometry {
            OriginalGeometry::Viewport { width, height } => {
                if width == 0 || height == 0 {
                    return Err(client_error("original viewport must be nonzero"));
                }

                if let Some(canvas) = self.canvas
                    && (width > canvas.width || height > canvas.height)
                {
                    return Err(client_error(
                        "original viewport does not fit the requested canvas",
                    ));
                }
            }
            OriginalGeometry::Grid(grid) => {
                if grid.cols == 0 || grid.rows == 0 {
                    return Err(client_error("original grid must be nonzero"));
                }
            }
        }

        if self
            .canvas
            .is_some_and(|canvas| return canvas.width == 0 || canvas.height == 0)
        {
            return Err(client_error("original canvas must be nonzero"));
        }

        if self.state_root.is_some() && self.alias.is_none() {
            return Err(client_error("a registry state root requires an alias"));
        }

        if let Some(profile) = &self.visual_profile {
            validate_visual_profile(Some(profile))?;

            if self.ghostty_config.is_some() {
                return Err(client_error(
                    "a visual profile replaces the Ghostty config; set only one",
                ));
            }

            if profile.native.is_none() {
                return Err(Error::Engine(termtrace_engine::Error::VisualProfile {
                    failure: VisualProfileFailure::InvalidVisualProfile,
                    reason: "original capture requires a native visual profile section".to_owned(),
                }));
            }
        }

        for (name, limit) in [
            ("max duration", self.max_duration),
            ("idle timeout", self.idle_timeout),
        ] {
            if limit.is_some_and(|limit| {
                return limit.is_zero() || limit.subsec_nanos() % 1_000_000 != 0;
            }) {
                return Err(client_error(format!(
                    "launch {name} must be a nonzero whole number of milliseconds"
                )));
            }
        }

        return self
            .environment
            .validate()
            .map_err(|source| return client_error(source.to_string()));
    }

    fn supervisor_arguments(&self, session: &Path) -> Vec<OsString> {
        let mut arguments = vec![
            OsString::from("record"),
            "--original-native".into(),
            "--original-fps".into(),
            self.fps.get().to_string().into(),
            "--out".into(),
            session.as_os_str().into(),
            "--environment-spec-stdin".into(),
        ];

        match self.geometry {
            OriginalGeometry::Viewport { width, height } => {
                arguments.extend(["--viewport".into(), format!("{width}x{height}").into()]);
            }
            OriginalGeometry::Grid(grid) => {
                arguments.extend([
                    "--original-grid".into(),
                    format!("{}x{}", grid.cols, grid.rows).into(),
                ]);
            }
        }

        if let Some(canvas) = self.canvas {
            arguments.extend([
                "--original-canvas".into(),
                format!("{}x{}", canvas.width, canvas.height).into(),
            ]);
        }

        if let Some(alias) = &self.alias {
            arguments.extend(["--session-alias".into(), alias.into()]);
        }

        if let Some(path) = &self.working_directory {
            arguments.extend(["--cwd".into(), path.as_os_str().into()]);
        }

        if let Some(path) = &self.ghostty_config {
            arguments.extend(["--ghostty-config".into(), path.as_os_str().into()]);
        }

        if let Some(profile) = &self.visual_profile {
            arguments.extend([
                "--visual-profile".into(),
                serde_json::to_string(profile).unwrap_or_default().into(),
            ]);
        }

        if let Some(limit) = self.output_byte_budget {
            arguments.extend(["--output-byte-budget".into(), limit.to_string().into()]);
        }

        if let Some(limit) = self.max_duration {
            arguments.extend([
                "--max-duration".into(),
                format!("{}ms", limit.as_millis()).into(),
            ]);
        }

        if let Some(limit) = self.idle_timeout {
            arguments.extend([
                "--idle-timeout".into(),
                format!("{}ms", limit.as_millis()).into(),
            ]);
        }

        arguments.push("--".into());
        arguments.extend(self.command.iter().cloned());

        return arguments;
    }
}

/// A finished original recording whose capture was published.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct OriginalRecording {
    /// Validated canonical completion of the captured application.
    pub completion: SessionCompletion,
    /// Whether the application exited 0 with no signal, resource limit or recorder failure.
    /// A published capture of a failing application is still a successful capture.
    pub canonical_success: bool,
    /// Published `visual-original` artifact.
    pub capture: MediaArtifactResult,
}

/// Supervisor status of an original capture, with `abandoned` derived from the supervisor's
/// process identity.
///
/// # Errors
/// Returns an error when the sidecar is missing or invalid.
pub fn original_capture_status(session: &Path) -> Result<OriginalCaptureStatus> {
    let path = original_capture_status_path(session);
    let filesystem = |source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    };
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_CLOEXEC)
        .open(&path)
        .map_err(filesystem)?;
    let metadata = file.metadata().map_err(filesystem)?;
    if !metadata.is_file() || metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
        return Err(client_error(format!(
            "{}: original-capture status must be an owner-only file of the current user",
            path.display()
        )));
    }

    let mut bytes = Vec::new();
    #[expect(
        clippy::verbose_file_reads,
        reason = "the status is read through the descriptor whose owner and mode were checked"
    )]
    file.read_to_end(&mut bytes).map_err(filesystem)?;
    let mut status: OriginalCaptureStatus = serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    status
        .validate()
        .map_err(|reason| return client_error(format!("{}: {reason}", path.display())))?;

    if let Some(expected) = status.session_id {
        let recorded = read_manifest(session)?.session_id;
        if recorded != expected {
            return Err(client_error(format!(
                "{}: original-capture status belongs to session {expected}, not {recorded}",
                path.display()
            )));
        }
    }

    if !status.state.is_terminal() && !supervisor_running(&status) {
        status.state = OriginalCaptureState::Abandoned;
    }

    return Ok(status);
}

// Every read for a selected session: a status that does not name it, including one copied from
// another session, authorizes nothing for it.
fn bound_status(session: &Session) -> Result<OriginalCaptureStatus> {
    let status = original_capture_status(session.path())?;
    if status.session_id != Some(session.session_id()) {
        return Err(client_error(format!(
            "{}: original-capture status is not bound to session {}",
            session.path().display(),
            session.session_id()
        )));
    }
    return Ok(status);
}

fn attached_original(session: Session) -> Result<OriginalSession> {
    bound_status(&session)?;

    return Ok(OriginalSession {
        session,
        supervision: None,
    });
}

fn supervisor_running(status: &OriginalCaptureStatus) -> bool {
    return u32::try_from(status.supervisor_pid.0)
        .ok()
        .and_then(|pid| return ProcessIdentity::of(pid).ok())
        .is_some_and(|identity| return identity.start_ticks == status.supervisor_start_ticks.0);
}

fn canonical_success(completion: &SessionCompletion) -> bool {
    return completion.exit_code == Some(0)
        && completion.signal_name.is_none()
        && completion.resource_limit.is_none()
        && completion.failure.is_none();
}

fn capture_facts(
    session: &Path,
    status: Option<&OriginalCaptureStatus>,
    code: Option<OriginalCaptureFailure>,
    reason: String,
    cleanup: Option<String>,
) -> OriginalCaptureFacts {
    let failure = status.and_then(|status| return status.failure.as_ref());
    let surviving = failure.map_or_else(
        || {
            return [
                (session.to_path_buf(), SurvivingKind::Session),
                (
                    original_capture_status_path(session),
                    SurvivingKind::Sidecar,
                ),
            ]
            .into_iter()
            .filter(|(path, _kind)| return path.symlink_metadata().is_ok())
            .map(|(path, kind)| {
                return SurvivingPathRecord {
                    path: ByteString::from(path.as_os_str().as_bytes()),
                    kind,
                };
            })
            .collect();
        },
        |failure| return failure.surviving.clone(),
    );

    return OriginalCaptureFacts {
        code: code.or_else(|| return failure.map(|failure| return failure.code)),
        state: status.map_or(OriginalCaptureState::Abandoned, |status| {
            return status.state;
        }),
        session: ByteString::from(session.as_os_str().as_bytes()),
        completion: read_completed(session).ok().flatten().map(Box::new),
        surviving,
        cleanup: cleanup
            .or_else(|| return failure.and_then(|failure| return failure.cleanup.clone())),
        reason: failure.map_or(reason, |failure| return failure.reason.clone()),
    };
}

fn capture_failed(
    session: &Path,
    code: Option<OriginalCaptureFailure>,
    reason: impl Into<String>,
    cleanup: Option<String>,
) -> Error {
    return capture_failed_with(
        session,
        original_capture_status(session).ok(),
        code,
        reason,
        cleanup,
    );
}

fn session_capture_failed(
    session: &Session,
    code: Option<OriginalCaptureFailure>,
    reason: impl Into<String>,
    cleanup: Option<String>,
) -> Error {
    return capture_failed_with(
        session.path(),
        bound_status(session).ok(),
        code,
        reason,
        cleanup,
    );
}

fn capture_failed_with(
    session: &Path,
    status: Option<OriginalCaptureStatus>,
    code: Option<OriginalCaptureFailure>,
    reason: impl Into<String>,
    cleanup: Option<String>,
) -> Error {
    return Error::OriginalCaptureFailed {
        facts: Box::new(capture_facts(
            session,
            status.as_ref(),
            code,
            reason.into(),
            cleanup,
        )),
    };
}

// The supervisor has exited, so its sidecar is final or abandoned. A published result counts
// only when it verifies against this session's own completion and capture bytes.
fn finished_capture(session: &Session, completion: SessionCompletion) -> Result<OriginalRecording> {
    let status = bound_status(session).map_err(|error| {
        // A status that exists but does not bind to this session publishes nothing for it.
        let code = if original_capture_status_path(session.path())
            .symlink_metadata()
            .is_ok()
        {
            OriginalCaptureFailure::PublicationFailed
        } else {
            OriginalCaptureFailure::SupervisorAbandoned
        };

        return session_capture_failed(
            session,
            Some(code),
            format!("original capture status is unavailable for this session: {error}"),
            None,
        );
    })?;

    return match (status.state, status.result) {
        (OriginalCaptureState::Published, Some(capture)) => {
            let verified = if completion.session_id == session.session_id() {
                verify_original_capture(session.path(), &completion, &capture)
                    .map_err(|error| return error.to_string())
            } else {
                Err(format!(
                    "canonical completion names session {}, not {}",
                    completion.session_id,
                    session.session_id()
                ))
            };

            match verified {
                Ok(()) => Ok(OriginalRecording {
                    canonical_success: canonical_success(&completion),
                    completion,
                    capture,
                }),
                Err(reason) => Err(session_capture_failed(
                    session,
                    Some(OriginalCaptureFailure::PublicationFailed),
                    format!("the published capture does not verify for this session: {reason}"),
                    None,
                )),
            }
        }
        (OriginalCaptureState::Abandoned, _) => Err(session_capture_failed(
            session,
            Some(OriginalCaptureFailure::SupervisorAbandoned),
            "the capture supervisor exited before publishing a final state",
            None,
        )),
        _ => Err(session_capture_failed(
            session,
            None,
            "original capture did not publish",
            None,
        )),
    };
}

fn signal_child(child: &Child, signal: Signal) {
    // The Child is unreaped, so its PID still names this process.
    if let Ok(pid) = i32::try_from(child.id()) {
        let _signalled = kill(Pid::from_raw(pid), signal);
    }
}

fn reap_within(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;

    loop {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return true;
        }

        if Instant::now() >= deadline {
            return false;
        }

        thread::sleep(POLL_INTERVAL);
    }
}

// Cancel the supervisor so it releases acquisition and publishes `cancelled`; kill the app's
// group and the supervisor when it does not exit in time. Returns a cleanup failure, if any.
fn tear_down(session: &Path, helper: &Path, mut child: Child) -> Option<String> {
    signal_child(&child, Signal::SIGTERM);
    if reap_within(&mut child, TEARDOWN_TIMEOUT) {
        return None;
    }

    if observe_phase(session).phase == SessionPhase::Live {
        return match Session::open(session.to_path_buf(), helper.to_path_buf(), Some(child)) {
            Ok(mut owned) => owned
                .force_close()
                .err()
                .map(|error| return error.to_string()),
            Err(error) => Some(format!("cannot own the late session: {error}")),
        };
    }

    let _killed = child.kill();
    if reap_within(&mut child, KILL_REAP_TIMEOUT) {
        return None;
    }

    thread::spawn(move || {
        let _status = child.wait();
    });

    return Some(
        "capture supervisor did not exit after SIGKILL; it is reaped in the background".to_owned(),
    );
}

// Cancellation wins over any failure seen while it was being observed, so a cancelled launch
// always reports cancellation.
fn prepare_original(spec: &OriginalLaunchSpec, cancelled: &AtomicBool) -> Result<()> {
    let prepared = SystemService::require(NATIVE_VIDEO_CAPABILITY, cancelled).and_then(|()| {
        if let Some(profile) = &spec.visual_profile {
            resolve_visual_profile(profile, cancelled)?;
        }

        return Ok(());
    });

    if !cancelled.load(Ordering::Acquire) {
        return prepared;
    }
    return Err(Error::OriginalCaptureFailed {
        facts: Box::new(OriginalCaptureFacts {
            code: Some(OriginalCaptureFailure::Cancelled),
            state: OriginalCaptureState::Cancelled,
            session: ByteString::default(),
            completion: None,
            surviving: Vec::new(),
            cleanup: None,
            reason: "cancelled before the capture started".to_owned(),
        }),
    });
}

impl super::Terminal {
    /// Start `spec` as an original recording and return it once the command runs inside the
    /// captured terminal.
    ///
    /// The private display and encoder are acquired before the first application byte.
    /// Setting `cancelled` tears the capture down within a bounded deadline, including a
    /// session that became live while cancellation was being observed.
    ///
    /// # Errors
    /// Returns the native capability failure, [`Error::OriginalCaptureFailed`] when acquisition
    /// fails or is cancelled, or [`Error::RecorderStartup`] when startup misses its deadline.
    pub fn start_original(
        &self,
        spec: &OriginalLaunchSpec,
        cancelled: &AtomicBool,
    ) -> Result<OriginalSession> {
        spec.validate()?;

        prepare_original(spec, cancelled)?;

        fs_err::create_dir_all(self.evidence_root()).map_err(|source| {
            return Error::Filesystem {
                path: self.evidence_root().to_path_buf(),
                source,
            };
        })?;

        let id = Uuid::now_v7();
        let paths = DetachedSessionPaths {
            session: self.evidence_root().join(format!("{}.tt", id.simple())),
            stdout_log: self
                .evidence_root()
                .join(format!(".{id}.original.stdout.log")),
            stderr_log: self
                .evidence_root()
                .join(format!(".{id}.original.stderr.log")),
        };

        let registry = spec
            .alias
            .as_ref()
            .map(|alias| -> Result<_> {
                let registry = spec
                    .state_root
                    .as_ref()
                    .map_or_else(RegistryService::open_default, RegistryService::open)?;

                let reservation = registry.reserve_session_alias(
                    alias,
                    &paths.session,
                    spec.startup_timeout.saturating_add(TEARDOWN_TIMEOUT),
                )?;

                return Ok((registry, reservation));
            })
            .transpose()?;

        let environment = serde_json::to_vec(&spec.environment).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<environment-spec-stdin>"),
                json_path: "$".to_owned(),
                source,
            };
        })?;

        let spawned = spawn_detached(
            self.binary(),
            spec.supervisor_arguments(&paths.session),
            &paths,
            Some(&environment),
        );

        let child = match spawned {
            Ok(child) => child,
            Err(error) => {
                if let Some((registry, reservation)) = &registry {
                    registry.release_session_alias(reservation);
                }

                return Err(error);
            }
        };

        let started = self.await_original(&paths, child, spec.startup_timeout, cancelled);
        let session = match started {
            Ok(session) => session,
            Err(error) => {
                if let Some((registry, reservation)) = &registry {
                    registry.release_session_alias(reservation);
                }

                return Err(error);
            }
        };

        if let Some((registry, reservation)) = &registry {
            registry.promote_session_alias(reservation)?;
        }

        return Ok(session);
    }

    fn await_original(
        &self,
        paths: &DetachedSessionPaths,
        mut child: Child,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<OriginalSession> {
        let session = &paths.session;
        let deadline = Instant::now() + timeout;
        let log_reason = || {
            return format!(
                "diagnostics: {}, {}",
                paths.stdout_log.display(),
                paths.stderr_log.display()
            );
        };

        loop {
            if cancelled.load(Ordering::Acquire) {
                let cleanup = tear_down(session, self.binary(), child);

                return Err(capture_failed(
                    session,
                    Some(OriginalCaptureFailure::Cancelled),
                    "original capture startup was cancelled",
                    cleanup,
                ));
            }

            let state = original_capture_status(session)
                .ok()
                .map(|status| return status.state);

            let exited = matches!(child.try_wait(), Ok(Some(_)));

            match state {
                Some(OriginalCaptureState::Live) => {
                    let readiness = await_ready(session, &mut child, deadline)?;
                    if matches!(readiness, Readiness::Live | Readiness::Completed(_)) {
                        break;
                    }
                }
                Some(
                    OriginalCaptureState::Failed
                    | OriginalCaptureState::Cancelled
                    | OriginalCaptureState::Abandoned,
                ) => {
                    let cleanup = (!reap_within(&mut child, TEARDOWN_TIMEOUT))
                        .then(|| return tear_down(session, self.binary(), child))
                        .flatten();

                    return Err(capture_failed(
                        session,
                        None,
                        format!("original capture failed during startup; {}", log_reason()),
                        cleanup,
                    ));
                }
                Some(OriginalCaptureState::Published | OriginalCaptureState::Finalizing) => break,
                Some(OriginalCaptureState::Acquiring) | None => {}
            }

            if exited {
                let code = state.map_or(OriginalCaptureFailure::StartupFailed, |_| {
                    return OriginalCaptureFailure::SupervisorAbandoned;
                });

                return Err(capture_failed(
                    session,
                    Some(code),
                    format!(
                        "the capture supervisor exited during startup; {}",
                        log_reason()
                    ),
                    None,
                ));
            }

            if Instant::now() >= deadline {
                let cleanup = tear_down(session, self.binary(), child);
                let state = original_capture_status(session).map_or(
                    OriginalCaptureState::Abandoned,
                    |status| {
                        return status.state;
                    },
                );

                return Err(Error::RecorderStartup {
                    reason: format!(
                        "original capture did not become live within {} ms and was torn down (capture {state:?}{}); {}",
                        timeout.as_millis(),
                        cleanup.map_or_else(String::new, |cleanup| return format!(", cleanup: {cleanup}")),
                        log_reason()
                    ),
                });
            }

            thread::sleep(POLL_INTERVAL);
        }

        let session = Session::open(session.clone(), self.binary().to_path_buf(), Some(child))?;
        let owned = OriginalSession {
            supervision: Some(Supervision::start(&session)),
            session,
        };

        // Cancellation that raced readiness still owns the late session.
        if cancelled.load(Ordering::Acquire) {
            return Err(owned.cancel());
        }
        return Ok(owned);
    }

    /// Attach to an original recording without owning its supervisor.
    ///
    /// # Errors
    /// Returns an error when the session or its capture status is missing or invalid.
    pub fn attach_original(&self, session: impl AsRef<Path>) -> Result<OriginalSession> {
        return attached_original(self.attach(session)?);
    }

    /// Attach to an original recording named by path or registry alias, without owning its
    /// supervisor. Aliases resolve in the registry under `state_root`, or the default registry.
    ///
    /// # Errors
    /// Returns an error when the target does not resolve, or the session or its capture status
    /// is missing, invalid, not owner-only, or published for a different session.
    pub fn attach_original_target(
        &self,
        target: &SessionTarget,
        state_root: Option<&Path>,
    ) -> Result<OriginalSession> {
        return attached_original(self.attach(target.path_in(state_root)?)?);
    }
}

/// An original recording: the canonical session plus its capture supervisor.
pub struct OriginalSession {
    session: Session,
    supervision: Option<Supervision>,
}

// While an owner holds the supervisor, a supervisor that dies without completing the session
// cannot stop its application any more; the owner kills the recorded group through the shared
// recorder authority instead, so an application ignoring SIGHUP and SIGTERM does not survive.
struct Supervision {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Supervision {
    fn start(session: &Session) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let view = session.lifecycle_observer();
        let thread = thread::spawn({
            let stop = Arc::clone(&stop);

            move || return supervise(view, &stop)
        });

        return Self {
            stop,
            thread: Some(thread),
        };
    }
}

impl Drop for Supervision {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);

        if let Some(thread) = self.thread.take() {
            let _joined = thread.join();
        }
    }
}

fn supervise(mut view: Session, stop: &AtomicBool) {
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }

        match view.reap_exited_recorder() {
            Ok(true) => break,
            Ok(false) => thread::sleep(SUPERVISION_INTERVAL),
            Err(error) => {
                tracing::warn!(%error, "cannot observe the original-capture supervisor");

                return;
            }
        }
    }

    let deadline = Instant::now() + SUPERVISOR_LOSS_GRACE;

    while observe_phase(view.path()).phase != SessionPhase::Completed {
        if stop.load(Ordering::Acquire) {
            return;
        }

        if Instant::now() >= deadline {
            if let Err(error) = view.force_close() {
                tracing::warn!(%error, "cannot stop the application of a lost capture supervisor");
            }

            return;
        }

        thread::sleep(SUPERVISION_INTERVAL);
    }
}

impl OriginalSession {
    /// Canonical session; live control, observation and evidence work as for any session.
    #[must_use]
    pub const fn session(&self) -> &Session {
        return &self.session;
    }

    /// Canonical session for lifecycle operations that need exclusive access.
    pub const fn session_mut(&mut self) -> &mut Session {
        return &mut self.session;
    }

    /// Current supervisor status, with `abandoned` derived.
    ///
    /// # Errors
    /// Returns an error when the status sidecar is missing or invalid.
    pub fn capture_status(&self) -> Result<OriginalCaptureStatus> {
        return bound_status(&self.session);
    }

    /// Stop the application and wait until the capture is published, all within `timeout`.
    ///
    /// # Errors
    /// Stop and completion failures keep their protocol classification.
    /// [`Error::OriginalCaptureFailed`] reports a capture that did not publish, with the
    /// canonical completion when it exists; [`Error::OriginalFinalizationTimeout`] reports a
    /// capture still finalizing at the deadline. Neither is retried automatically.
    pub fn finish(&mut self, request: StopRequest, timeout: Duration) -> Result<OriginalRecording> {
        let deadline = Instant::now() + timeout;
        let completed = self.session.stop_bounded(request, timeout);

        return self.finished(completed, deadline);
    }

    /// Wait without stopping until the application exits and the capture is published.
    ///
    /// # Errors
    /// Same as [`Self::finish`], without a stop request.
    pub fn wait_finished(&mut self, timeout: Duration) -> Result<OriginalRecording> {
        let deadline = Instant::now() + timeout;
        let completed = self.session.complete(timeout);

        return self.finished(completed, deadline);
    }

    fn finished(
        &self,
        completed: Result<SessionCompletion>,
        deadline: Instant,
    ) -> Result<OriginalRecording> {
        let path = self.session.path();

        return match completed {
            // An attached caller does not reap the supervisor, so it waits for its final state.
            Ok(completion) => loop {
                let status = bound_status(&self.session).ok();
                let settled = status.as_ref().is_none_or(|status| {
                    return status.state.is_terminal()
                        || status.state == OriginalCaptureState::Abandoned;
                });
                if settled {
                    break finished_capture(&self.session, completion);
                }

                if Instant::now() >= deadline {
                    break Err(Error::OriginalFinalizationTimeout {
                        facts: Box::new(capture_facts(
                            path,
                            status.as_ref(),
                            None,
                            "canonical completion is published, but the capture is still finalizing".to_owned(),
                            None,
                        )),
                    });
                }

                thread::sleep(POLL_INTERVAL);
            },
            Err(error) => {
                let status = bound_status(&self.session).ok();
                let completion = read_completed(path).ok().flatten();

                match (status, completion) {
                    // Canonical evidence is complete; only the supervisor is still running.
                    (Some(status), Some(_))
                        if !status.state.is_terminal()
                            && status.state != OriginalCaptureState::Abandoned =>
                    {
                        Err(Error::OriginalFinalizationTimeout {
                            facts: Box::new(capture_facts(
                                path,
                                Some(&status),
                                None,
                                format!("capture is still {:?}: {error}", status.state),
                                None,
                            )),
                        })
                    }
                    (Some(status), Some(_))
                        if matches!(
                            status.state,
                            OriginalCaptureState::Failed
                                | OriginalCaptureState::Cancelled
                                | OriginalCaptureState::Abandoned
                        ) =>
                    {
                        Err(Error::OriginalCaptureFailed {
                            facts: Box::new(capture_facts(
                                path,
                                Some(&status),
                                (status.state == OriginalCaptureState::Abandoned)
                                    .then_some(OriginalCaptureFailure::SupervisorAbandoned),
                                format!("original capture did not publish: {error}"),
                                None,
                            )),
                        })
                    }
                    // The supervisor is gone, so nothing can publish; canonical completion may be
                    // missing too when its recorder died with it.
                    (Some(status), None) if status.state == OriginalCaptureState::Abandoned => {
                        Err(Error::OriginalCaptureFailed {
                            facts: Box::new(capture_facts(
                                path,
                                Some(&status),
                                Some(OriginalCaptureFailure::SupervisorAbandoned),
                                format!("the capture supervisor is gone: {error}"),
                                None,
                            )),
                        })
                    }
                    _ => Err(error),
                }
            }
        };
    }

    /// Force-close a recording whose start was cancelled after it became live, and return the
    /// cancellation failure for the caller.
    #[must_use]
    pub fn cancel(mut self) -> Error {
        let cleanup = self
            .session
            .force_close()
            .err()
            .map(|error| return error.to_string());

        return session_capture_failed(
            &self.session,
            Some(OriginalCaptureFailure::Cancelled),
            "original capture startup was cancelled after the session became live",
            cleanup,
        );
    }

    /// Kill the application's process group and the capture supervisor. The capture is
    /// abandoned; canonical evidence keeps what was recorded.
    ///
    /// # Errors
    /// Returns the underlying force-close failure.
    pub fn force_close(&mut self) -> Result<()> {
        return self.session.force_close();
    }

    /// Stop managing the recording without stopping it; an owned supervisor is reaped when it
    /// exits.
    #[must_use]
    pub fn detach(self) -> PathBuf {
        drop(self.supervision);

        return self.session.detach();
    }
}
