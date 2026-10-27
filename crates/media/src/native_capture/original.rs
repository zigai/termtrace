use super::{
    capture::{preserve_failed, require_native_tools, validate_options},
    capture_io::{
        check_cancelled, command_output, command_output_in_profile, external_error, outcome,
        secure_permissions,
    },
    index::{validate_acquisition_frames, write_original_acquisition_map},
    media::{
        capture_screenshot, probe_screenshot, probe_variable_video, probe_video, spawn_video_ffmpeg,
    },
    portal,
    process::{
        ghostty_overrides, owned_command, probe_output, spawn_xvfb, stop_ffmpeg, wait_for_x_socket,
        DisplayLease, ManagedChild,
    },
    profile::materialize,
    resize_server::{read_winsize, resize_decision, wait_for_grid, ResizeServer},
    resources::snapshot_mapped,
    validate_visual_manifest, validate_visual_manifest_binding,
    window::TerminalWindow,
    Error, Result,
};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Read as _,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use termtrace_evidence::{
    artifact::{
        append_artifact_root, collect_artifact_members, commit_directory, create_private_directory,
        secure_file, sync_directory, verify_artifact_members, write_secure,
    },
    event_log::{EventLogCursor, EventLogSummary},
    session::{read_completion, read_manifest, validate_completion_summary_candidate},
    Error as EvidenceError,
};
use termtrace_model::execution::layout::SessionLayout;
use termtrace_model::{
    artifact::{
        original::{
            CellGrid, OriginalCaptureFailure, OriginalCaptureState, WindowResizeRecord,
            WINDOW_RESIZES_NAME,
        },
        sdk::{
            ArtifactManifestRef, ArtifactProductKind, ArtifactProvenance, ArtifactSourceRef,
            DecimalU64, MediaArtifactResult, SdkArtifactMember, SdkCanonicalBoundary,
        },
        visual::{
            CaptureKind, CaptureOptions, CaptureOutcome, VisualManifest,
            GHOSTTY_EFFECTIVE_CONFIG_NAME, VISUAL_MANIFEST_VERSION,
        },
    },
    execution::event::Event,
    execution::session::{recorder_exit_status, CaptureTarget, SessionCompletion},
    value::{geometry::TerminalGeometryReport, ByteString},
};
use termtrace_process::poll_until;

/// Variables the private display, its D-Bus session and the Ghostty profile set for the
/// terminal; the recorded application should see the caller's values instead.
pub const CAPTURE_ENVIRONMENT: &[&str] = &[
    "DISPLAY",
    "GDK_BACKEND",
    "LIBGL_ALWAYS_SOFTWARE",
    "XDG_CONFIG_HOME",
    "DBUS_SESSION_BUS_ADDRESS",
];
const ACQUISITION_POLL_INTERVAL: Duration = Duration::from_millis(20);
const TERMINAL_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const LAYOUT_TIMEOUT: Duration = Duration::from_secs(5);

/// Pixel acquisition backend for a managed original execution.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OriginalCaptureBackend {
    /// An isolated X11 display with a configured acquisition rate.
    #[default]
    PrivateX11,
    /// A user-selected Wayland window, with the compositor's variable frame timing.
    WaylandPortal,
}

/// Terminal grid policy of a private X11 original capture.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OriginalGrid {
    /// Ghostty derives the grid from its window size.
    #[default]
    ViewportDerived,
    /// Capture fails with `grid_mismatch` unless Ghostty shows exactly this grid.
    Exact(CellGrid),
}

/// What to capture and how to lay out the terminal inside the private display.
pub struct OriginalCaptureRequest<'request> {
    /// Termtrace executable providing the owned-process and recorder subcommands.
    pub helper: &'request Path,
    /// New session, canvas size (`width`/`height`), frame rate and optional solver report.
    pub options: &'request CaptureOptions,
    /// Materialized Ghostty profile bytes.
    pub profile: &'request [u8],
    /// Pixel source.
    pub backend: OriginalCaptureBackend,
    /// Ghostty window size inside the canvas; `None` fills the canvas. Private X11 only.
    pub window: Option<CaptureTarget>,
    /// Grid policy. Private X11 only.
    pub grid: OriginalGrid,
    /// New private capture staging directory beside the session. It is renamed to
    /// `visual-failed-*` on failure; it keeps this name only when that preservation fails.
    pub staging: &'request Path,
}

/// Paths handed to the recorder command inside the captured terminal.
pub struct OriginalRecorderPaths<'paths> {
    /// Isolated Ghostty configuration file.
    pub ghostty_config: &'paths Path,
    /// Abstract socket name of the supervisor that owns outer-window resizes; `None` for the
    /// Wayland backend.
    pub resize_socket: Option<&'paths str>,
}

/// A published original capture.
#[derive(Clone, Debug)]
pub struct OriginalCapture {
    /// Published artifact paths.
    pub outcome: CaptureOutcome,
    /// Shared SDK envelope for the published artifact.
    pub result: MediaArtifactResult,
    /// Validated canonical completion of the captured run.
    pub completion: SessionCompletion,
}

/// Record a managed command inside Ghostty while acquiring its original pixels.
///
/// The argument builder receives the isolated Ghostty configuration file and, for private
/// X11, the resize socket. It must return arguments for an attached
/// `record --on-recording-error stop` invocation of `helper` writing the new session selected
/// by `options`. `observer` receives each state transition up to `finalizing`; the caller
/// reports the terminal state. The caller owns cancellation signalling.
///
/// The Wayland backend opens a visible Ghostty window, waits for portal window selection and
/// the first acquired frame, then starts the command. The selected window identity is user
/// supplied and is not independently verified. Its frame rate is compositor controlled;
/// `options.fps`, `window` and `grid` apply only to private X11. No replay or semantic
/// presentation claim is made by this capture.
///
/// # Errors
/// Returns an error on cancellation, invalid configuration, missing dependencies, failed
/// recording or acquisition, a grid mismatch, or publication failure. Every failure after
/// staging is classified with [`Error::OriginalCapture`]; failed acquisition evidence is
/// preserved when possible.
pub fn capture_original(
    request: &OriginalCaptureRequest<'_>,
    cancelled: &AtomicBool,
    observer: &mut dyn FnMut(OriginalCaptureState, Option<CellGrid>),
    arguments: impl FnOnce(&OriginalRecorderPaths<'_>) -> Vec<OsString>,
) -> Result<OriginalCapture> {
    let options = request.options;
    validate_options(options)?;
    if !matches!(options.kind, CaptureKind::Video)
        || options.start_monotonic_ns.is_some()
        || options.end_monotonic_ns.is_some()
        || !options.session_directory.is_absolute()
        || !request.helper.is_absolute()
        || options.session_directory.exists()
    {
        return Err(external_error(
            "original capture",
            "requires a new session and full execution video",
        ));
    }

    if request.backend == OriginalCaptureBackend::WaylandPortal
        && (request.window.is_some() || request.grid != OriginalGrid::ViewportDerived)
    {
        return Err(external_error(
            "original capture",
            "window layout and exact grids require the private X11 backend",
        ));
    }

    if request.window.is_some_and(|window| {
        return window.width > options.width || window.height > options.height;
    }) {
        return Err(external_error(
            "original capture",
            "the terminal window must fit inside the capture canvas",
        ));
    }

    let parent = options
        .session_directory
        .parent()
        .filter(|path| return path.is_dir())
        .ok_or_else(|| return external_error("original capture", "session parent must exist"))?;

    let destination = options
        .destination
        .clone()
        .unwrap_or_else(|| return options.session_directory.join("visual-original"));

    if destination.exists() {
        return Err(Error::ArtifactExists { path: destination });
    }

    let destination_parent = destination
        .parent()
        .filter(|parent| return parent.is_dir() || *parent == options.session_directory)
        .ok_or_else(|| {
            return external_error(
                "original capture",
                "destination parent must exist or be the new session",
            );
        })?;

    match request.backend {
        OriginalCaptureBackend::PrivateX11 => require_native_tools(&CaptureKind::Video)?,
        OriginalCaptureBackend::WaylandPortal => portal::require_tools()?,
    }

    observer(
        OriginalCaptureState::Acquiring,
        match request.grid {
            OriginalGrid::Exact(grid) => Some(grid),
            OriginalGrid::ViewportDerived => None,
        },
    );

    if request.staging.parent() != Some(parent) || request.staging.exists() {
        return Err(external_error(
            "original capture",
            "staging must be a new directory beside the session",
        ));
    }

    let staging = request.staging.to_path_buf();
    let control = parent.join(format!(".original-control.{}", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;

    if let Err(error) = create_private_directory(&control) {
        return Err(preserve_failed(
            parent,
            &staging,
            classify(
                OriginalCaptureFailure::StartupFailed,
                cancelled,
                error.into(),
            ),
        ));
    }

    let result = capture_staged(request, cancelled, observer, arguments, &staging, &control);
    let control_removed = fs::remove_dir_all(&control).map_err(|source| {
        return Error::Filesystem {
            path: control.clone(),
            source,
        };
    });

    let (manifest, manifest_bytes, completion) = match (result, control_removed) {
        (Ok(staged), Ok(())) => staged,
        (Ok(_), Err(cleanup)) => {
            return Err(preserve_failed(
                parent,
                &staging,
                classify(
                    OriginalCaptureFailure::PublicationFailed,
                    cancelled,
                    cleanup,
                ),
            ));
        }
        (Err(error), control_removed) => {
            // The attached recorder also receives terminal hangup when Ghostty is dropped.
            // Request a durable stop if its control socket is still reachable; an unreachable
            // socket is expected here, and the capture failure below is the reported outcome.
            drop(probe_output(
                "stop original recording",
                Command::new(request.helper)
                    .arg("stop")
                    .arg(&options.session_directory)
                    .args(["--wait", "--timeout-ms", "3000"]),
                Duration::from_secs(4),
            ));

            let error = match control_removed {
                Ok(()) => error,
                Err(cleanup) => Error::CleanupFailed {
                    primary: Box::new(error),
                    cleanup: Box::new(cleanup),
                },
            };

            return Err(preserve_failed(parent, &staging, error));
        }
    };

    let published = publish(
        &options.session_directory,
        parent,
        &staging,
        &destination,
        destination_parent,
        &manifest,
        &manifest_bytes,
        &completion,
    )
    .map_err(|error| {
        return classify(OriginalCaptureFailure::PublicationFailed, cancelled, error);
    })?;

    return Ok(OriginalCapture {
        outcome: outcome(destination, &CaptureKind::Video),
        result: published,
        completion,
    });
}

#[allow(
    clippy::too_many_arguments,
    reason = "publication names the session, staging and destination explicitly"
)]
fn publish(
    session_directory: &Path,
    parent: &Path,
    staging: &Path,
    destination: &Path,
    destination_parent: &Path,
    manifest: &VisualManifest,
    manifest_bytes: &[u8],
    completion: &SessionCompletion,
) -> Result<MediaArtifactResult> {
    // Recording creates the session itself. Only after completion can the native
    // staging directory become its direct child for the common artifact commit protocol.
    let publication_staging =
        destination_parent.join(format!(".visual.{}.tmp", uuid::Uuid::now_v7()));

    if let Err(source) = fs::rename(staging, &publication_staging) {
        return Err(preserve_failed(
            parent,
            staging,
            Error::Filesystem {
                path: publication_staging,
                source,
            },
        ));
    }

    if let Err(error) =
        commit_directory(destination_parent, &publication_staging, destination, false)
    {
        return Err(preserve_failed(
            destination_parent,
            &publication_staging,
            Error::from(error),
        ));
    }

    sync_directory(destination_parent)?;

    let session = fs::canonicalize(session_directory).map_err(|source| {
        return Error::Filesystem {
            path: session_directory.to_path_buf(),
            source,
        };
    })?;

    let canonical_destination = fs::canonicalize(destination).map_err(|source| {
        return Error::Filesystem {
            path: destination.to_path_buf(),
            source,
        };
    })?;

    if canonical_destination.starts_with(&session) {
        append_artifact_root(
            &session,
            &canonical_destination,
            "native_original_capture",
            manifest_bytes,
            &completion.session_manifest_hash,
            Some(completion.final_event_hash.as_str()),
            None,
        )?;
    }

    let result = MediaArtifactResult {
        version: 1,
        directory_bytes: ByteString::from(canonical_destination.as_os_str().as_bytes()),
        manifest: ArtifactManifestRef {
            path: "manifest.json".to_owned(),
            blake3: blake3::hash(manifest_bytes).to_hex().to_string(),
        },
        source: ArtifactSourceRef {
            session_id: completion.session_id,
            session_manifest_hash: completion.session_manifest_hash.clone(),
            boundary: SdkCanonicalBoundary {
                next_event_seq: DecimalU64(completion.event_count),
                event_hash: completion.final_event_hash.clone(),
                output_bytes: DecimalU64(completion.output_bytes),
            },
        },
        provenance: ArtifactProvenance::OriginalExecution,
        product_kind: ArtifactProductKind::Video,
        members: manifest
            .members
            .iter()
            .cloned()
            .map(SdkArtifactMember::from)
            .collect(),
    };

    result
        .validate()
        .map_err(|reason| return external_error("original capture result", reason))?;

    return Ok(result);
}

fn classify(code: OriginalCaptureFailure, cancelled: &AtomicBool, error: Error) -> Error {
    if matches!(error, Error::OriginalCapture { .. }) {
        return error;
    }

    let code = if cancelled.load(Ordering::Relaxed) {
        OriginalCaptureFailure::Cancelled
    } else {
        code
    };

    return Error::OriginalCapture {
        code,
        source: Box::new(error),
    };
}

fn stage<E: Into<Error>>(
    code: OriginalCaptureFailure,
    cancelled: &AtomicBool,
) -> impl Fn(E) -> Error + '_ {
    return move |error: E| return classify(code, cancelled, error.into());
}

fn grid_mismatch(expected: CellGrid, observed: CellGrid, stage: &str) -> Error {
    return Error::OriginalCapture {
        code: OriginalCaptureFailure::GridMismatch,
        source: Box::new(external_error(
            "original capture",
            format!(
                "{stage}: Ghostty shows {}x{} cells instead of the requested {}x{}",
                observed.cols, observed.rows, expected.cols, expected.rows
            ),
        )),
    };
}

struct Prepared {
    tty: PathBuf,
    window: TerminalWindow,
}

// Size the window and verify the grid before the program may write its first byte.
fn prepare_terminal(
    request: &OriginalCaptureRequest<'_>,
    display: &str,
    tty_file: &Path,
    ghostty: &mut ManagedChild,
    cancelled: &AtomicBool,
) -> Result<Prepared> {
    let tty = poll_until(
        Instant::now() + TERMINAL_STARTUP_TIMEOUT,
        ACQUISITION_POLL_INTERVAL,
        || {
            check_cancelled(cancelled, "original capture")?;

            ghostty.check_running()?;

            let line = fs::read(tty_file).unwrap_or_default();
            let Some(path) = line.strip_suffix(b"\n") else {
                return Ok::<_, Error>(None);
            };

            return Ok(Some(PathBuf::from(OsStr::from_bytes(path))));
        },
    )?
    .ok_or_else(|| {
        return external_error(
            "original capture",
            "timed out waiting for the captured terminal device",
        );
    })?;

    let window = TerminalWindow::connect(Some(display), TERMINAL_STARTUP_TIMEOUT)?;
    let canvas = (request.options.width, request.options.height);

    if let Some(target) = request.window {
        window.configure(u32::from(target.width), u32::from(target.height))?;

        let deadline = Instant::now() + LAYOUT_TIMEOUT;

        while window.size()? != (target.width, target.height) {
            check_cancelled(cancelled, "original capture")?;

            if Instant::now() >= deadline {
                return Err(external_error(
                    "original capture",
                    "the terminal window did not take the requested size",
                ));
            }

            thread::sleep(ACQUISITION_POLL_INTERVAL);
        }

        settle(&tty, cancelled)?;
    }

    if let OriginalGrid::Exact(grid) = request.grid {
        let current = read_winsize(&tty)?;
        if current.grid() != grid {
            let size = resize_decision(grid, current, window.size()?, canvas).map_err(
                |(code, message)| {
                    return Error::OriginalCapture {
                        code: OriginalCaptureFailure::GridMismatch,
                        source: Box::new(external_error(code, message)),
                    };
                },
            )?;
            window.configure(size.0, size.1)?;
        }

        if let Err(observed) = wait_for_grid(&tty, grid, LAYOUT_TIMEOUT)? {
            return Err(grid_mismatch(grid, observed.grid(), "before execution"));
        }
    }

    return Ok(Prepared { tty, window });
}

// Ghostty may report intermediate sizes while it applies a configure request.
fn settle(tty: &Path, cancelled: &AtomicBool) -> Result<()> {
    let deadline = Instant::now() + LAYOUT_TIMEOUT;
    let mut previous = read_winsize(tty)?;
    let mut stable_since = Instant::now();

    while stable_since.elapsed() < Duration::from_millis(200) && Instant::now() < deadline {
        check_cancelled(cancelled, "original capture")?;

        thread::sleep(ACQUISITION_POLL_INTERVAL);

        let current = read_winsize(tty)?;

        if current != previous {
            previous = current;
            stable_since = Instant::now();
        }
    }

    return Ok(());
}

fn correlate_resizes(
    session: &Path,
    records: &mut [WindowResizeRecord],
    cancelled: &AtomicBool,
) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }

    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;
    while let Some(record) = cursor.next_record()? {
        check_cancelled(cancelled, "original capture")?;

        if let Event::ResizeIntent {
            operation_id,
            cols,
            rows,
            ..
        } = record.event
            && let Some(entry) = records
                .iter_mut()
                .find(|entry| return entry.request_id == operation_id && entry.event_seq.is_none())
        {
            entry.event_seq = Some(DecimalU64(record.seq));
            entry.observed = Some(CellGrid { cols, rows });
        }
    }

    return Ok(());
}

fn write_window_resizes(staging: &Path, records: &[WindowResizeRecord]) -> Result<()> {
    let path = staging.join(WINDOW_RESIZES_NAME);
    let mut bytes = Vec::new();

    for record in records {
        serde_json::to_writer(&mut bytes, record).map_err(|source| {
            return Error::InvalidJson {
                path: path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        bytes.push(b'\n');
    }

    write_secure(&path, &bytes)?;

    return Ok(());
}

fn capture_staged(
    request: &OriginalCaptureRequest<'_>,
    cancelled: &AtomicBool,
    observer: &mut dyn FnMut(OriginalCaptureState, Option<CellGrid>),
    arguments: impl FnOnce(&OriginalRecorderPaths<'_>) -> Vec<OsString>,
    staging: &Path,
    control: &Path,
) -> Result<(VisualManifest, Vec<u8>, SessionCompletion)> {
    use OriginalCaptureFailure::{
        AcquisitionFailed, PublicationFailed, RecordingFailed, StartupFailed,
    };

    let helper = request.helper;
    let options = request.options;
    let backend = request.backend;

    let root = materialize(&staging.join("ghostty-profile"), request.profile)
        .map_err(stage(StartupFailed, cancelled))?;
    let effective = command_output_in_profile("ghostty", &["+show-config"], Some(&root))
        .map_err(stage(StartupFailed, cancelled))?;
    let effective_hash = blake3::hash(&effective).to_hex().to_string();

    write_secure(&staging.join(GHOSTTY_EFFECTIVE_CONFIG_NAME), &effective)
        .map_err(stage(StartupFailed, cancelled))?;

    let ghostty_version = String::from_utf8_lossy(
        &command_output("ghostty", &["+version"]).map_err(stage(StartupFailed, cancelled))?,
    )
    .into_owned();

    let ffmpeg_version = String::from_utf8_lossy(
        &command_output("ffmpeg", &["-version"]).map_err(stage(StartupFailed, cancelled))?,
    )
    .lines()
    .next()
    .unwrap_or_default()
    .to_owned();

    if let Some(report) = &options.geometry_solver {
        let path = staging.join("geometry-solver.json");
        let bytes = serde_json::to_vec_pretty(report).map_err(|source| {
            return Error::InvalidJson {
                path: path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;

        write_secure(&path, &bytes).map_err(stage(StartupFailed, cancelled))?;
    }

    let lease = if backend == OriginalCaptureBackend::PrivateX11 {
        Some(DisplayLease::acquire().map_err(stage(StartupFailed, cancelled))?)
    } else {
        None
    };

    let display = lease
        .as_ref()
        .map(|lease| return format!(":{}", lease.number));

    let mut xvfb = if let Some(lease) = lease.as_ref() {
        let mut server = spawn_xvfb(
            Some(helper),
            lease,
            options.width,
            options.height,
            staging,
            options.dpi,
        )
        .map_err(stage(AcquisitionFailed, cancelled))?;
        wait_for_x_socket(lease, &mut server, Duration::from_secs(5))
            .map_err(stage(AcquisitionFailed, cancelled))?;

        Some(server)
    } else {
        None
    };

    let video = staging.join("session.mkv");
    let mut ffmpeg = if let Some(display) = display.as_ref() {
        let mut capture = spawn_video_ffmpeg(Some(helper), options, staging, display, &video)
            .map_err(stage(AcquisitionFailed, cancelled))?;
        wait_for_acquisition(&mut capture, staging, cancelled)
            .map_err(stage(AcquisitionFailed, cancelled))?;

        Some(capture)
    } else {
        None
    };

    let finished = staging.join("recording-exit-status");
    let stderr = secure_file(&staging.join("ghostty.stderr.log"))
        .map_err(stage(StartupFailed, cancelled))?;
    let stdout = secure_file(&staging.join("ghostty.stdout.log"))
        .map_err(stage(StartupFailed, cancelled))?;

    let mut overrides = ghostty_overrides(options.padding);

    if backend == OriginalCaptureBackend::WaylandPortal {
        overrides.retain(|arg| {
            return !arg.starts_with("--window-width=")
                && !arg.starts_with("--window-height=")
                && !arg.starts_with("--window-position-");
        });
        overrides.extend([
            "--window-width=100".to_owned(),
            "--window-height=30".to_owned(),
            "--title=Termtrace original capture".to_owned(),
        ]);
    }

    let gate = staging.join("execution-ready");
    let tty_file = control.join("tty");
    let resize_socket = format!("termtrace-outer-resize-{}", uuid::Uuid::now_v7().simple());

    let mut command = if backend == OriginalCaptureBackend::PrivateX11 {
        let mut command =
            owned_command(helper, "dbus-run-session").map_err(stage(StartupFailed, cancelled))?;
        command.arg("--").arg("ghostty");
        command
    } else {
        owned_command(helper, "ghostty").map_err(stage(StartupFailed, cancelled))?
    };

    if let Some(display) = display.as_ref() {
        command
            .env("DISPLAY", display)
            .env("GDK_BACKEND", "x11")
            .env("LIBGL_ALWAYS_SOFTWARE", "1");
    } else {
        command.env("GDK_BACKEND", "wayland");
    }

    let recorder_arguments = arguments(&OriginalRecorderPaths {
        ghostty_config: &root.join("ghostty/config.ghostty"),
        resize_socket: (backend == OriginalCaptureBackend::PrivateX11)
            .then_some(resize_socket.as_str()),
    });

    // All user paths and arguments are positional shell parameters, never shell source.
    let mut ghostty = ManagedChild::spawn_logged("ghostty", command
        .args(&overrides).arg("-e").arg("/bin/sh")
        .args(["-c", "finished=$1; log=$2; gate=$3; device=$4; shift 4; tty >\"$device\"; while ! test -f \"$gate\"; do sleep 0.02; done; \"$@\" 2>\"$log\"; status=$?; printf '%s\\n' \"$status\" >\"$finished\"; while :; do sleep 1; done", "termtrace-original"])
        .arg(&finished).arg(staging.join("recorder.stderr.log")).arg(&gate).arg(&tty_file).arg(helper)
        .args(recorder_arguments)
        .env("XDG_CONFIG_HOME", &root)
        .stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr))).map_err(stage(AcquisitionFailed, cancelled))?;

    let mut prepared = None;
    if let Some(display) = display.as_ref() {
        let terminal = prepare_terminal(request, display, &tty_file, &mut ghostty, cancelled)
            .map_err(stage(AcquisitionFailed, cancelled))?;
        let server = ResizeServer::start(
            &resize_socket,
            terminal.window,
            terminal.tty.clone(),
            (options.width, options.height),
        )
        .map_err(stage(AcquisitionFailed, cancelled))?;

        write_secure(&gate, b"ready\n").map_err(stage(AcquisitionFailed, cancelled))?;
        prepared = Some((terminal.tty, server));
    }

    if backend == OriginalCaptureBackend::WaylandPortal {
        let mut capture =
            portal::spawn(helper, options, staging).map_err(stage(AcquisitionFailed, cancelled))?;
        portal::wait_for_acquisition(&mut capture, &mut ghostty, staging, cancelled)
            .map_err(stage(AcquisitionFailed, cancelled))?;

        write_secure(&gate, b"ready\n").map_err(stage(AcquisitionFailed, cancelled))?;

        ffmpeg = Some(capture);
    }

    let mut ffmpeg = ffmpeg.ok_or_else(|| {
        return classify(
            AcquisitionFailed,
            cancelled,
            external_error("original capture", "capture process was not started"),
        );
    })?;

    let manifest_path = SessionLayout::new(&options.session_directory).manifest();

    let mut live = false;
    while !finished.is_file() {
        let healthy = (|| {
            check_cancelled(cancelled, "original capture")?;

            ghostty.check_running()?;
            ffmpeg.check_running()?;

            if let Some(xvfb) = xvfb.as_mut() {
                xvfb.check_running()?;
            }

            return Ok::<(), Error>(());
        })();

        healthy.map_err(stage(AcquisitionFailed, cancelled))?;

        if !live && manifest_path.is_file() {
            live = true;
            let grid = match request.grid {
                OriginalGrid::Exact(grid) => Some(grid),
                OriginalGrid::ViewportDerived => prepared
                    .as_ref()
                    .and_then(|(tty, _server)| return read_winsize(tty).ok())
                    .map(|size| return size.grid()),
            };

            observer(OriginalCaptureState::Live, grid);
        }

        thread::sleep(ACQUISITION_POLL_INTERVAL);
    }

    if let Some((_tty, server)) = prepared.as_ref() {
        server.finalize();
    }

    observer(OriginalCaptureState::Finalizing, None);

    let completion = (|| {
        let completion = read_completion(&options.session_directory)?;
        let summary = EventLogSummary::read_with_check(
            SessionLayout::new(&options.session_directory).event_log(),
            || {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(EvidenceError::InvalidEvidence {
                        reason: "original capture cancelled".to_owned(),
                        source: None,
                    });
                }
                return Ok(());
            },
        )?;

        validate_completion_summary_candidate(&options.session_directory, &summary, &completion)?;
        if completion.failure.is_some() {
            return Err(external_error(
                "original capture",
                "recorder reported an evidence failure",
            ));
        }

        let status = fs::read_to_string(&finished).map_err(|source| {
            return Error::Filesystem {
                path: finished.clone(),
                source,
            };
        })?;

        let recorder_status = status
            .trim()
            .parse::<u8>()
            .map_err(|error| return external_error("original capture", error))?;

        let expected_status = recorder_exit_status(
            completion.exit_code,
            completion.signal_name.as_deref(),
            completion.resource_limit.is_some(),
        );

        if Some(recorder_status) != expected_status {
            return Err(external_error(
                "original capture",
                "recorder exit status does not match canonical completion",
            ));
        }
        return Ok(completion);
    })()
    .map_err(stage(RecordingFailed, cancelled))?;
    let session =
        read_manifest(&options.session_directory).map_err(stage(RecordingFailed, cancelled))?;

    if session
        .ghostty_visual
        .as_ref()
        .map(|profile| return profile.effective_config_hash.as_str())
        != Some(effective_hash.as_str())
    {
        return Err(classify(
            RecordingFailed,
            cancelled,
            external_error(
                "original capture",
                "recorded configuration differs from original terminal configuration",
            ),
        ));
    }

    if let OriginalGrid::Exact(grid) = request.grid {
        let recorded = CellGrid {
            cols: session.geometry.cols,
            rows: session.geometry.rows,
        };

        if recorded != grid {
            return Err(grid_mismatch(grid, recorded, "canonical recording"));
        }
    }

    // Retain the terminal after command completion; this frame is an acquisition,
    // not an assertion that a particular semantic state has been presented.
    let screenshot = staging.join("final.png");

    let acquired = (|| {
        if let Some(display) = display.as_ref() {
            capture_screenshot(options, staging, display, &screenshot)?;
        }

        snapshot_mapped(&ghostty, staging, cancelled, "after_execution")?;

        stop_ffmpeg(&mut ffmpeg, Duration::from_secs(10), staging)?;

        return Ok::<(), Error>(());
    })();

    acquired.map_err(stage(AcquisitionFailed, cancelled))?;

    if let Some((_tty, server)) = prepared.take() {
        let (mut records, errors) = server.finish();
        correlate_resizes(&options.session_directory, &mut records, cancelled)
            .map_err(stage(RecordingFailed, cancelled))?;
        write_window_resizes(staging, &records).map_err(stage(PublicationFailed, cancelled))?;

        if !errors.is_empty() {
            write_secure(
                &staging.join("resize-server.log"),
                errors.join("\n").as_bytes(),
            )
            .map_err(stage(PublicationFailed, cancelled))?;
        }
    }

    ghostty.terminate();

    if let Some(xvfb) = xvfb.as_mut() {
        xvfb.terminate();
    }

    let verified = (|| {
        secure_permissions(&video)?;
        secure_permissions(&screenshot)?;

        let video_probe = match backend {
            OriginalCaptureBackend::PrivateX11 => {
                probe_video(&video, options.fps, options.width, options.height)?
            }
            OriginalCaptureBackend::WaylandPortal => {
                probe_variable_video(&video, options.width, options.height)?
            }
        };

        let packets = video_probe
            .pointer("/streams/0/nb_read_packets")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| return value.parse().ok())
            .ok_or_else(|| {
                return external_error("original capture", "missing encoded frame count");
            })?;

        let capture_started = match backend {
            OriginalCaptureBackend::PrivateX11 => {
                validate_acquisition_frames(&staging.join("ffmpeg-video.log"), packets)?
            }
            OriginalCaptureBackend::WaylandPortal => portal::validate_completion(staging, packets)?,
        };

        if capture_started > session.started_unix_ns {
            return Err(external_error(
                "original capture",
                "first frame was acquired after execution began",
            ));
        }

        if backend == OriginalCaptureBackend::PrivateX11 {
            write_original_acquisition_map(
                &staging.join("acquisition-map.jsonl"),
                &staging.join("ffmpeg-video.log"),
                session.started_unix_ns,
            )?;
        }

        return Ok((video_probe, capture_started));
    })();
    let (video_probe, capture_started) = verified.map_err(stage(AcquisitionFailed, cancelled))?;

    let correlation = match backend {
        OriginalCaptureBackend::PrivateX11 => "acquisition_clock_only_no_presentation_claim",
        OriginalCaptureBackend::WaylandPortal => {
            "pipewire_pts_and_collector_receipt_no_presentation_claim"
        }
    };

    let published = (|| {
        write_secure(
            &staging.join("execution-timing.json"),
            format!(
                "{{\"execution_origin_unix_ns\":{},\"correlation\":\"{correlation}\"}}\n",
                session.started_unix_ns
            )
            .as_bytes(),
        )?;

        let geometry = &session.geometry;
        let manifest = VisualManifest {
            version: VISUAL_MANIFEST_VERSION,
            session_id: session.session_id,
            source_event_hash: Some(completion.final_event_hash.clone()),
            source_range: None,
            state_selection: options.state_selection,
            geometry_report: TerminalGeometryReport {
                cols: geometry.cols,
                rows: geometry.rows,
                cell_width: geometry.pixel_width.checked_div(geometry.cols).unwrap_or(0),
                cell_height: geometry
                    .pixel_height
                    .checked_div(geometry.rows)
                    .unwrap_or(0),
                text_width: geometry.pixel_width,
                text_height: geometry.pixel_height,
            },
            fps: if backend == OriginalCaptureBackend::WaylandPortal { 0 } else { options.fps },
            width: options.width,
            height: options.height,
            capture_started_unix_ns: capture_started,
            replay_origin_unix_ns: 0,
            effective_config_hash: effective_hash,
            ghostty_version,
            ffmpeg_version: Some(ffmpeg_version),
            ghostty_overrides: overrides,
            video_probe: Some(video_probe),
            screenshot_probe: probe_screenshot(&screenshot, options.width, options.height)?,
            members: collect_artifact_members(staging)?,
            capture_kind: match backend {
                OriginalCaptureBackend::PrivateX11 => "original_execution",
                OriginalCaptureBackend::WaylandPortal => "original_execution_wayland_portal",
            }.to_owned(),
            selected_state_id: None,
            selected_state_hash: None,
            endpoint_event_seq: None,
            endpoint_event_byte_offset: None,
            presentation_evidence: match backend {
                OriginalCaptureBackend::PrivateX11 => "original_execution_pixels_no_semantic_presentation_claim",
                OriginalCaptureBackend::WaylandPortal => "user_selected_window_pixels_no_verified_window_identity_or_semantic_presentation_claim",
            }.to_owned(),
            frame_samples: Vec::new(),
            guarantee_level: "native_capture_environment".to_owned(),
        };

        validate_visual_manifest(&manifest)?;
        let bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
            return Error::InvalidJson {
                path: staging.join("manifest.json"),
                json_path: "$".to_owned(),
                source,
            };
        })?;

        write_secure(&staging.join("manifest.json"), &bytes)?;

        return Ok::<_, Error>((manifest, bytes));
    })();
    let (manifest, bytes) = published.map_err(stage(PublicationFailed, cancelled))?;

    return Ok((manifest, bytes, completion));
}

fn wait_for_acquisition(
    ffmpeg: &mut ManagedChild,
    staging: &Path,
    cancelled: &AtomicBool,
) -> Result<()> {
    let path = staging.join("ffmpeg-video.log");

    return poll_until(
        Instant::now() + Duration::from_secs(10),
        ACQUISITION_POLL_INTERVAL,
        || {
            check_cancelled(cancelled, "original capture")?;

            ffmpeg.check_running()?;

            let mut bytes = Vec::new();
            fs::File::open(&path)
                .and_then(|file| return file.take(65536).read_to_end(&mut bytes))
                .map_err(|source| {
                    return Error::Filesystem {
                        path: path.clone(),
                        source,
                    };
                })?;

            return Ok::<_, Error>(
                String::from_utf8_lossy(&bytes)
                    .contains(" pts_time:")
                    .then_some(()),
            );
        },
    )?
    .ok_or_else(|| {
        return external_error(
            "original capture",
            "timed out waiting for first acquired video frame",
        );
    });
}

/// Verify that a published original capture belongs to this exact completed session: the
/// canonical source and boundary, the `visual-original` directory, the exact manifest bytes,
/// every member's bytes, the effective terminal configuration and the provenance.
///
/// # Errors
/// Returns an error describing the first mismatch, or the read that failed.
pub fn verify_original_capture(
    session: &Path,
    completion: &SessionCompletion,
    result: &MediaArtifactResult,
) -> Result<()> {
    let mismatch = |reason: &str| return external_error("original capture verification", reason);
    result
        .validate()
        .map_err(|reason| return mismatch(&reason))?;

    let session_manifest = read_manifest(session)?;
    let boundary = &result.source.boundary;

    if session_manifest.session_id != completion.session_id
        || result.source.session_id != completion.session_id
        || result.source.session_manifest_hash != completion.session_manifest_hash
        || boundary.next_event_seq.0 != completion.event_count
        || boundary.event_hash != completion.final_event_hash
        || boundary.output_bytes.0 != completion.output_bytes
    {
        return Err(mismatch(
            "the capture's canonical source differs from this session's completion",
        ));
    }

    if result.provenance != ArtifactProvenance::OriginalExecution
        || result.product_kind != ArtifactProductKind::Video
    {
        return Err(mismatch("the capture is not an original-execution video"));
    }

    let canonical = fs::canonicalize(session).map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    let directory = canonical.join("visual-original");
    if result.directory_bytes.as_bytes() != directory.as_os_str().as_bytes()
        || result.manifest.path != "manifest.json"
    {
        return Err(mismatch(
            "the capture is not this session's visual-original artifact",
        ));
    }

    let manifest_path = directory.join(&result.manifest.path);
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| {
        return Error::Filesystem {
            path: manifest_path.clone(),
            source,
        };
    })?;

    if blake3::hash(&manifest_bytes).to_hex().as_str() != result.manifest.blake3 {
        return Err(mismatch(
            "the capture manifest bytes differ from the result",
        ));
    }

    let manifest: VisualManifest = serde_json::from_slice(&manifest_bytes).map_err(|source| {
        return Error::InvalidJson {
            path: manifest_path,
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let colors = EventLogSummary::read(session.join(&session_manifest.event_log))?
        .header
        .colors
        .source;

    validate_visual_manifest_binding(
        &manifest,
        session_manifest
            .ghostty_visual
            .as_ref()
            .map(|profile| return profile.effective_config_hash.as_str()),
        &colors,
    )?;

    let members: Vec<SdkArtifactMember> = manifest
        .members
        .iter()
        .cloned()
        .map(SdkArtifactMember::from)
        .collect();

    if manifest.session_id != completion.session_id
        || manifest.capture_kind != "original_execution"
        || members != result.members
    {
        return Err(mismatch("the capture manifest contradicts the result"));
    }

    verify_artifact_members(&directory, &manifest.members)?;

    return Ok(());
}
