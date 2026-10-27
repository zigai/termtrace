use super::*;
use nix::{
    fcntl::{fcntl, FcntlArg, SealFlag},
    sys::{
        memfd::{memfd_create, MFdFlags},
        stat::{fchmod, Mode},
    },
};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    flag, low_level,
};
use std::{
    fs::{self, File},
    os::fd::{AsRawFd as _, OwnedFd},
    process,
    sync::{atomic::AtomicBool, Arc, Mutex},
};
use termtrace_evidence::session::read_completion;
use termtrace_media::{
    native_capture::{
        capture_original, solve_ghostty_geometry, CaptureKind, CaptureOptions,
        OriginalCaptureBackend, OriginalCaptureRequest, OriginalGrid, OriginalRecorderPaths,
        CAPTURE_ENVIRONMENT,
    },
    Error as MediaError,
};
use termtrace_model::{
    artifact::{
        original::{
            original_capture_status_path, CellGrid, OriginalCaptureFailure,
            OriginalCaptureFailureRecord, OriginalCaptureState, OriginalCaptureStatus, PixelCanvas,
            SurvivingKind, SurvivingPathRecord, ORIGINAL_CAPTURE_STATUS_FORMAT,
            ORIGINAL_CAPTURE_STATUS_VERSION,
        },
        sdk::{DecimalU64, MediaArtifactResult},
        visual::GeometrySolveReport,
    },
    execution::session::CaptureTarget,
    terminal::replay::StateSelection,
};
use termtrace_process::ProcessIdentity;

const DEFAULT_VIEWPORT: CaptureTarget = CaptureTarget::new(1280, 720);

struct CancellationSignals(Vec<signal_hook::SigId>);

impl Drop for CancellationSignals {
    fn drop(&mut self) {
        for signal in self.0.drain(..) {
            low_level::unregister(signal);
        }
    }
}

// Owner-only status beside the session, replaced atomically on every transition.
struct StatusSidecar {
    path: PathBuf,
    status: OriginalCaptureStatus,
}

impl StatusSidecar {
    fn create(
        session: &Path,
        grid: Option<CellGrid>,
        canvas: PixelCanvas,
        fps: u16,
    ) -> Result<Self> {
        let identity = ProcessIdentity::current().into_diagnostic()?;
        let sidecar = Self {
            path: original_capture_status_path(session),
            status: OriginalCaptureStatus {
                format: ORIGINAL_CAPTURE_STATUS_FORMAT.to_owned(),
                version: ORIGINAL_CAPTURE_STATUS_VERSION,
                backend: "private_x11".to_owned(),
                supervisor_pid: DecimalU64(u64::from(identity.pid)),
                supervisor_start_ticks: DecimalU64(identity.start_ticks),
                state: OriginalCaptureState::Acquiring,
                session_id: None,
                grid,
                canvas,
                fps,
                result: None,
                failure: None,
            },
        };
        let bytes = sidecar.encode()?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&sidecar.path)
            .into_diagnostic()?;
        file.write_all(&bytes)
            .and_then(|()| return file.sync_all())
            .into_diagnostic()?;

        return Ok(sidecar);
    }

    fn encode(&self) -> Result<Vec<u8>> {
        self.status
            .validate()
            .map_err(|reason| miette::miette!(reason))?;

        let mut bytes = serde_json::to_vec_pretty(&self.status).into_diagnostic()?;
        bytes.push(b'\n');

        return Ok(bytes);
    }

    fn publish(&mut self, state: OriginalCaptureState) -> Result<()> {
        self.status.state = state;
        let bytes = self.encode()?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| miette::miette!("status sidecar has no parent"))?;

        let temporary = parent.join(format!(".original-capture.{}.tmp", uuid::Uuid::now_v7()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .into_diagnostic()?;
        let written = file
            .write_all(&bytes)
            .and_then(|()| return file.sync_all())
            .and_then(|()| return fs::rename(&temporary, &self.path));
        if let Err(error) = written {
            let _removed = fs::remove_file(&temporary);

            return Err(error).into_diagnostic();
        }
        return OpenOptions::new()
            .read(true)
            .open(parent)
            .and_then(|directory| return directory.sync_all())
            .into_diagnostic();
    }

    fn published(&mut self, result: MediaArtifactResult) -> Result<()> {
        self.status.result = Some(result);

        return self.publish(OriginalCaptureState::Published);
    }

    fn failed(&mut self, failure: OriginalCaptureFailureRecord) -> Result<()> {
        let state = if failure.code == OriginalCaptureFailure::Cancelled {
            OriginalCaptureState::Cancelled
        } else {
            OriginalCaptureState::Failed
        };

        self.status.failure = Some(failure);

        return self.publish(state);
    }
}

// The environment specification lives only in a sealed owner-only memfd of this process. The
// recorder opens it through this process's descriptor, so nothing secret reaches the filesystem
// and the bytes disappear with the descriptor, including when this process is killed.
struct EnvironmentMemfd {
    descriptor: Mutex<Option<OwnedFd>>,
    path: PathBuf,
}

impl EnvironmentMemfd {
    fn create(bytes: &[u8]) -> Result<Self> {
        let descriptor = memfd_create(
            "termtrace-environment",
            MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING,
        )
        .into_diagnostic()?;

        fchmod(&descriptor, Mode::S_IRUSR | Mode::S_IWUSR).into_diagnostic()?;
        let mut file = File::from(descriptor);
        file.write_all(bytes).into_diagnostic()?;
        fcntl(
            &file,
            FcntlArg::F_ADD_SEALS(
                SealFlag::F_SEAL_SHRINK
                    | SealFlag::F_SEAL_GROW
                    | SealFlag::F_SEAL_WRITE
                    | SealFlag::F_SEAL_SEAL,
            ),
        )
        .into_diagnostic()?;

        let descriptor = OwnedFd::from(file);

        return Ok(Self {
            path: PathBuf::from(format!(
                "/proc/{}/fd/{}",
                process::id(),
                descriptor.as_raw_fd()
            )),
            descriptor: Mutex::new(Some(descriptor)),
        });
    }

    fn remove(&self) {
        let _closed = self
            .descriptor
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner())
            .take();
    }
}

// The application sees the caller's values of the variables that only configure the captured
// terminal, so a successor in another capture resolves the same environment.
fn application_environment(supplied: Option<EnvironmentSpec>) -> EnvironmentSpec {
    let mut spec = supplied.unwrap_or_default();
    for name in CAPTURE_ENVIRONMENT {
        let name = (*name).to_owned();
        if spec.set.contains_key(&name) || spec.unset.contains(&name) {
            continue;
        }

        let inherited = match spec.base {
            EnvironmentBase::Inherit => true,
            EnvironmentBase::Clear => spec.copy.remove(&name),
        };
        if !inherited {
            continue;
        }

        match env::var_os(&name) {
            Some(value) => {
                spec.set.insert(name, ByteString::from(value.as_bytes()));
            }
            None => {
                spec.unset.insert(name);
            }
        }
    }

    return spec;
}

struct Layout {
    grid: OriginalGrid,
    window: Option<CaptureTarget>,
    canvas: CaptureTarget,
    solver: Option<GeometrySolveReport>,
}

fn layout(
    helper: &Path,
    viewport: Option<CaptureTarget>,
    grid: Option<CellGrid>,
    canvas: Option<CaptureTarget>,
    profile: &ResolvedGhosttyProfile,
) -> Result<Layout> {
    let initial = viewport.unwrap_or(DEFAULT_VIEWPORT);
    let (window, solver) = match grid {
        Some(grid) => {
            let report = solve_ghostty_geometry(helper, grid.cols, grid.rows, initial, profile)
                .into_diagnostic()?;

            (report.selected, Some(report))
        }
        None => (initial, None),
    };
    let canvas = canvas.unwrap_or(window);
    if window.width > canvas.width || window.height > canvas.height {
        return Err(miette::miette!(
            "the {}x{} terminal window does not fit the {}x{} --original-canvas",
            window.width,
            window.height,
            canvas.width,
            canvas.height
        ));
    }
    return Ok(Layout {
        grid: grid.map_or(OriginalGrid::ViewportDerived, OriginalGrid::Exact),
        window: ((window.width, window.height) != (canvas.width, canvas.height)).then_some(window),
        canvas: CaptureTarget {
            padding: window.padding,
            ..canvas
        },
        solver,
    });
}

pub(super) fn run(response: &mut ResponseWriter, mut arguments: RecordArgs) -> Result<ExitCode> {
    let backend = match arguments.original_backend {
        Some(super::args::OriginalBackendArg::WaylandPortal) => {
            if arguments.original_fps.is_some() {
                return Err(miette::miette!(
                    "Wayland portal capture uses compositor frame timing; omit --original-fps"
                ));
            }

            if arguments.detach.is_some()
                || arguments.original_grid.is_some()
                || arguments.original_canvas.is_some()
                || arguments.restart_source.is_some()
                || arguments.recording.environment_spec_stdin
            {
                return Err(miette::miette!(
                    "--detach, --original-grid, --original-canvas and --environment-spec-stdin require the private-x11 original backend"
                ));
            }

            OriginalCaptureBackend::WaylandPortal
        }
        Some(super::args::OriginalBackendArg::PrivateX11) | None => {
            OriginalCaptureBackend::PrivateX11
        }
    };

    if arguments
        .original_fps
        .is_some_and(|fps| return ![24, 30, 60].contains(&fps))
    {
        return Err(miette::miette!("--original-fps must be 24, 30, or 60"));
    }

    for flag in &arguments.explicit_geometry {
        writeln!(
            io::stderr(),
            "warning: {flag} is ignored with --original-native; the grid follows the Ghostty window. Use --original-grid COLSxROWS for an exact grid."
        )
        .into_diagnostic()?;
    }

    if arguments.detach.is_some() {
        return run_detached(response, arguments);
    }

    let cwd = env::current_dir().into_diagnostic()?;
    let out = arguments
        .out
        .take()
        .unwrap_or_else(|| return cwd.join("original-native.termtrace"));

    let session = if out.is_absolute() { out } else { cwd.join(out) };
    let parent = session
        .parent()
        .ok_or_else(|| miette::miette!("original session has no parent directory"))?
        .to_path_buf();

    if let Some(path) = arguments.recording.environment_spec.as_mut() {
        *path = path.canonicalize().into_diagnostic()?;
    }

    if let Some(path) = arguments.recording.cwd.as_mut() {
        *path = path.canonicalize().into_diagnostic()?;
    } else {
        arguments.recording.cwd = Some(cwd);
    }

    let helper = env::current_exe().into_diagnostic()?;
    let restart = arguments.restart_source.take();
    let (profile, grid, canvas, fps) = match &restart {
        Some(predecessor) => predecessor_capture(predecessor)?,
        None => {
            let overrides = GhosttyVisualOverrides {
                font_family: arguments.recording.font_family.take(),
                font_size: arguments.recording.font_size.take(),
                padding_x: arguments.recording.padding_x.take(),
                padding_y: arguments.recording.padding_y.take(),
                foreground: arguments.recording.foreground.take(),
                background: arguments.recording.background.take(),
            };
            let profile =
                resolve_ghostty_profile(arguments.recording.ghostty_config.as_deref(), overrides)
                    .into_diagnostic()?;

            (
                profile,
                arguments.original_grid,
                arguments.original_canvas,
                arguments.original_fps.unwrap_or(24),
            )
        }
    };

    let layout = if backend == OriginalCaptureBackend::PrivateX11 {
        layout(
            &helper,
            arguments.recording.viewport,
            grid,
            canvas,
            &profile,
        )?
    } else {
        Layout {
            grid: OriginalGrid::ViewportDerived,
            window: None,
            canvas: arguments.recording.viewport.unwrap_or(DEFAULT_VIEWPORT),
            solver: None,
        }
    };

    arguments.recording.geometry = GeometryMode::Inherit;
    arguments.recording.viewport = None;

    let mut sidecar = if backend == OriginalCaptureBackend::PrivateX11 {
        Some(StatusSidecar::create(
            &session,
            match layout.grid {
                OriginalGrid::Exact(grid) => Some(grid),
                OriginalGrid::ViewportDerived => None,
            },
            PixelCanvas {
                width: layout.canvas.width,
                height: layout.canvas.height,
            },
            fps,
        )?)
    } else {
        None
    };

    let environment = if backend == OriginalCaptureBackend::PrivateX11 {
        let stdin = if arguments.recording.environment_spec_stdin {
            Some(super::record::read_environment_stdin()?)
        } else {
            None
        };

        let supplied = super::record::supplied_environment(&arguments.recording, stdin.as_deref())?;
        let bytes = serde_json::to_vec(&application_environment(supplied)).into_diagnostic()?;
        let file = EnvironmentMemfd::create(&bytes)?;
        let recording = &mut arguments.recording;
        recording.environment_spec_stdin = false;
        recording.environment_spec = Some(file.path.clone());
        recording.clear_env = false;
        recording.copy_env.clear();
        recording.unset_env.clear();
        recording.color_policy = None;

        Some(file)
    } else {
        None
    };

    let cancelled = Arc::new(AtomicBool::new(false));
    let mut signals = CancellationSignals(Vec::new());

    for signal in [SIGINT, SIGTERM] {
        signals
            .0
            .push(flag::register(signal, Arc::clone(&cancelled)).into_diagnostic()?);
    }

    let options = CaptureOptions {
        session_directory: session.clone(),
        width: layout.canvas.width,
        height: layout.canvas.height,
        padding: layout.canvas.padding,
        fps,
        start_monotonic_ns: None,
        end_monotonic_ns: None,
        state_selection: StateSelection::All,
        geometry_solver: layout.solver,
        kind: CaptureKind::Video,
        destination: None,
        scale: None,
        dpi: None,
    };

    let provenance = serde_json::to_string(&profile.provenance).into_diagnostic()?;
    let staging = parent.join(format!(".original-visual.{}.tmp", uuid::Uuid::now_v7()));
    let mut transition_failure = None;
    let mut observer = |state: OriginalCaptureState, grid: Option<CellGrid>| {
        if state == OriginalCaptureState::Live
            && let Some(environment) = environment.as_ref()
        {
            environment.remove();
        }

        if let Some(sidecar) = sidecar.as_mut() {
            // A short-lived command can skip `Live`, so bind on whichever transition comes first.
            if matches!(
                state,
                OriginalCaptureState::Live
                    | OriginalCaptureState::Finalizing
                    | OriginalCaptureState::Published
            ) && sidecar.status.session_id.is_none()
            {
                match read_manifest(&session) {
                    Ok(manifest) => sidecar.status.session_id = Some(manifest.session_id),
                    Err(error) => {
                        transition_failure.get_or_insert_with(|| {
                            return miette::miette!(
                                "cannot bind the capture status to its session: {error}"
                            );
                        });
                    }
                }
            }

            if grid.is_some() {
                sidecar.status.grid = grid;
            }

            if let Err(error) = sidecar.publish(state) {
                transition_failure.get_or_insert(error);
            }
        }
    };
    let captured = capture_original(
        &OriginalCaptureRequest {
            helper: &helper,
            options: &options,
            profile: &profile.replay_config,
            backend,
            window: layout.window,
            grid: layout.grid,
            staging: &staging,
        },
        &cancelled,
        &mut observer,
        |paths: &OriginalRecorderPaths<'_>| {
            let mut args = match &restart {
                Some(predecessor) => restart_recorder_arguments(
                    arguments.session_alias.as_deref(),
                    &session,
                    predecessor,
                    arguments.recording.environment_spec.as_deref(),
                ),
                None => {
                    arguments.recording.ghostty_config = Some(paths.ghostty_config.to_path_buf());
                    let mut args = super::start::record_arguments(
                        arguments.session_alias.as_deref(),
                        &session,
                        &arguments.recording,
                        true,
                    );
                    args.splice(
                        1..1,
                        [
                            OsString::from("--on-recording-error"),
                            OsString::from("stop"),
                            OsString::from("--resolved-ghostty-profile-provenance"),
                            OsString::from(&provenance),
                        ],
                    );

                    args
                }
            };

            if let Some(socket) = paths.resize_socket {
                args.splice(
                    1..1,
                    [
                        OsString::from("--outer-resize-socket"),
                        OsString::from(socket),
                    ],
                );
            }

            return args;
        },
    );

    drop(signals);
    drop(environment);

    let captured = match captured {
        Ok(captured) => captured,
        Err(error) => {
            if let Some(sidecar) = sidecar.as_mut() {
                let record = failure_record(&error, &session, &staging, &sidecar.path);
                sidecar.failed(record)?;
            }

            return Err(error).into_diagnostic();
        }
    };

    if let Some(error) = transition_failure {
        return Err(error);
    }

    if let Some(sidecar) = sidecar.as_mut() {
        sidecar.published(captured.result.clone())?;
    }

    write_json_line(
        response,
        &serde_json::json!({
            "kind": "original_native_recording", "session": session,
            "visual": captured.outcome.directory, "video": captured.outcome.video,
            "screenshot": captured.outcome.screenshot, "artifact": captured.result,
        }),
    )?;

    let completion = read_completion(&session).into_diagnostic()?;
    if completion.resource_limit.is_some() || completion.failure.is_some() {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(match (completion.exit_code, completion.signal_name) {
        (Some(code), None) => u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from),
        _ => ExitCode::FAILURE,
    });
}

// A faithful successor reuses the predecessor's resolved profile, exact initial grid, canvas
// and acquisition rate. Only a published predecessor capture can be restarted this way.
fn predecessor_capture(
    predecessor: &Path,
) -> Result<(
    ResolvedGhosttyProfile,
    Option<CellGrid>,
    Option<CaptureTarget>,
    u16,
)> {
    let bytes = fs::read(original_capture_status_path(predecessor)).into_diagnostic()?;
    let status: OriginalCaptureStatus = serde_json::from_slice(&bytes).into_diagnostic()?;
    status
        .validate()
        .map_err(|reason| miette::miette!(reason))?;

    if status.state != OriginalCaptureState::Published {
        return Err(miette::miette!(
            "the predecessor original capture is {:?}, not published; refusing a successor that would break capture continuity",
            status.state
        ));
    }

    let manifest = read_manifest(predecessor).into_diagnostic()?;
    let provenance = manifest
        .ghostty_visual
        .ok_or_else(|| miette::miette!("predecessor has no recorded Ghostty profile"))?;

    let profile = restore_ghostty_profile(
        fs::read(SessionLayout::new(predecessor).ghostty_replay_profile()).into_diagnostic()?,
        provenance,
    )
    .into_diagnostic()?;

    return Ok((
        profile,
        Some(CellGrid {
            cols: manifest.geometry.cols,
            rows: manifest.geometry.rows,
        }),
        Some(CaptureTarget::new(
            status.canvas.width,
            status.canvas.height,
        )),
        status.fps,
    ));
}

fn restart_recorder_arguments(
    alias: Option<&str>,
    session: &Path,
    predecessor: &Path,
    environment_spec: Option<&Path>,
) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("record"),
        OsString::from("--out"),
        session.as_os_str().to_os_string(),
        OsString::from("--restart-source"),
        predecessor.as_os_str().to_os_string(),
    ];

    if let Some(alias) = alias {
        arguments.extend([OsString::from("--session-alias"), OsString::from(alias)]);
    }

    if let Some(path) = environment_spec {
        arguments.extend([
            OsString::from("--environment-spec"),
            path.as_os_str().to_os_string(),
        ]);
    }

    arguments.extend([OsString::from("--"), OsString::from("/bin/true")]);

    return arguments;
}

fn failure_record(
    error: &MediaError,
    session: &Path,
    staging: &Path,
    sidecar: &Path,
) -> OriginalCaptureFailureRecord {
    let mut code = OriginalCaptureFailure::StartupFailed;
    let mut failed_visual = None;
    let mut cleanup = None;
    let mut primary = error;

    loop {
        match primary {
            MediaError::CaptureFailed { evidence, source } => {
                failed_visual = Some(evidence.clone());
                primary = source;
            }
            MediaError::CleanupFailed {
                primary: inner,
                cleanup: failure,
            } => {
                cleanup.get_or_insert_with(|| return failure.to_string());
                primary = inner;
            }
            MediaError::OriginalCapture {
                code: classified, ..
            } => {
                code = *classified;
                break;
            }
            _ => break,
        }
    }

    let surviving = [
        (session.to_path_buf(), SurvivingKind::Session),
        (sidecar.to_path_buf(), SurvivingKind::Sidecar),
        (staging.to_path_buf(), SurvivingKind::Staging),
    ]
    .into_iter()
    .chain(failed_visual.map(|path| return (path, SurvivingKind::FailedVisual)))
    .filter(|(path, _kind)| return path.symlink_metadata().is_ok())
    .map(|(path, kind)| {
        return SurvivingPathRecord {
            path: ByteString::from(path.as_os_str().as_bytes()),
            kind,
        };
    })
    .collect();

    return OriginalCaptureFailureRecord {
        code,
        reason: primary.to_string(),
        surviving,
        cleanup,
    };
}

fn run_detached(response: &mut ResponseWriter, mut arguments: RecordArgs) -> Result<ExitCode> {
    let detach = arguments.detach.take().unwrap_or_default();
    let name = if let Some(alias) = arguments.session_alias.take() {
        alias
    } else if !detach.is_empty() {
        detach
    } else {
        arguments
            .recording
            .command
            .iter()
            .find(|arg| return *arg != "--")
            .map(|arg| return arg.to_string_lossy().into_owned())
            .ok_or_else(|| {
                return miette::miette!("detached recording requires a session alias and command");
            })?
    };

    let cwd = env::current_dir().into_diagnostic()?;

    if let Some(path) = arguments.recording.environment_spec.as_mut() {
        *path = path.canonicalize().into_diagnostic()?;
    }

    if let Some(path) = arguments.recording.cwd.as_mut() {
        *path = path.canonicalize().into_diagnostic()?;
    } else {
        arguments.recording.cwd = Some(cwd.clone());
    }

    let stdin = if arguments.recording.environment_spec_stdin {
        let mut bytes = Vec::new();
        io::stdin()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .into_diagnostic()?;

        Some(bytes)
    } else {
        None
    };

    let startup_timeout = millis_or(arguments.startup_timeout_ms, defaults::STARTUP_TIMEOUT);
    let registry = RegistryService::open_default().into_diagnostic()?;
    let executable = env::current_exe().into_diagnostic()?;
    let out = arguments.out.take().map(|out| {
        return if out.is_absolute() { out } else { cwd.join(out) };
    });

    let started = registry
        .start_detached(
            &executable,
            DetachedStart {
                alias: name.clone(),
                session: out,
                startup_timeout,
                stdin,
            },
            |session| return supervisor_arguments(&name, session, &arguments),
        )
        .into_diagnostic()?;

    return super::start::report_start_outcome(
        response,
        &started.entry,
        &started.paths,
        started.completion.as_deref(),
        &super::start::StartSynchronization {
            until_idle: arguments.until_idle,
            until_screen: arguments.until_screen.clone(),
            until_screen_regex: arguments.until_screen_regex.clone(),
            timeout: arguments.timeout.unwrap_or(defaults::WAIT_TIMEOUT),
        },
        arguments.json,
    );
}

fn supervisor_arguments(alias: &str, session: &Path, arguments: &RecordArgs) -> Vec<OsString> {
    let mut supervisor =
        super::start::record_arguments(Some(alias), session, &arguments.recording, true);

    // The recording builder always names geometry; the supervisor would report it as ignored.
    let mut index = 0;
    while let Some(argument) = supervisor.get(index) {
        if argument == "--" {
            break;
        }

        if [
            "--geometry",
            "--cols",
            "--rows",
            "--cell-width",
            "--cell-height",
        ]
        .iter()
        .any(|flag| return argument == flag)
        {
            supervisor.drain(index..(index + 2).min(supervisor.len()));
        } else {
            index += 1;
        }
    }

    let mut original = vec![OsString::from("--original-native")];
    if let Some(fps) = arguments.original_fps {
        original.extend([
            OsString::from("--original-fps"),
            OsString::from(fps.to_string()),
        ]);
    }

    if let Some(grid) = arguments.original_grid {
        original.extend([
            OsString::from("--original-grid"),
            OsString::from(format!("{}x{}", grid.cols, grid.rows)),
        ]);
    }

    if let Some(canvas) = arguments.original_canvas {
        original.extend([
            OsString::from("--original-canvas"),
            OsString::from(format!("{}x{}", canvas.width, canvas.height)),
        ]);
    }

    supervisor.splice(1..1, original);

    return supervisor;
}
