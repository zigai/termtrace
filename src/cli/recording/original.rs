use super::*;
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    flag, low_level,
};
use std::sync::{atomic::AtomicBool, Arc};
use termtrace_evidence::session::read_completion;
use termtrace_media::native_capture::{
    capture_original_with_backend, CaptureKind, CaptureOptions, OriginalCaptureBackend,
};
use termtrace_model::{execution::session::CaptureTarget, terminal::replay::StateSelection};

struct CancellationSignals(Vec<signal_hook::SigId>);

impl Drop for CancellationSignals {
    fn drop(&mut self) {
        for signal in self.0.drain(..) {
            low_level::unregister(signal);
        }
    }
}

pub(super) fn run(response: &mut ResponseWriter, mut arguments: RecordArgs) -> Result<ExitCode> {
    let backend = match arguments.original_backend {
        Some(super::args::OriginalBackendArg::WaylandPortal) => {
            if arguments.original_fps.is_some() {
                return Err(miette::miette!(
                    "Wayland portal capture uses compositor frame timing; omit --original-fps"
                ));
            }

            OriginalCaptureBackend::WaylandPortal
        }
        Some(super::args::OriginalBackendArg::PrivateX11) | None => {
            OriginalCaptureBackend::PrivateX11
        }
    };

    let cwd = env::current_dir().into_diagnostic()?;
    let out = arguments
        .out
        .unwrap_or_else(|| return cwd.join("original-native.termtrace"));

    let session = if out.is_absolute() { out } else { cwd.join(out) };

    if let Some(path) = arguments.recording.environment_spec.as_mut() {
        *path = path.canonicalize().into_diagnostic()?;
    }

    if let Some(path) = arguments.recording.cwd.as_mut() {
        *path = path.canonicalize().into_diagnostic()?;
    } else {
        arguments.recording.cwd = Some(cwd);
    }

    let overrides = GhosttyVisualOverrides {
        font_family: arguments.recording.font_family.take(),
        font_size: arguments.recording.font_size.take(),
        padding_x: arguments.recording.padding_x.take(),
        padding_y: arguments.recording.padding_y.take(),
        foreground: arguments.recording.foreground.take(),
        background: arguments.recording.background.take(),
    };
    let profile = resolve_ghostty_profile(arguments.recording.ghostty_config.as_deref(), overrides)
        .into_diagnostic()?;

    let target = arguments
        .recording
        .viewport
        .unwrap_or(CaptureTarget::new(1280, 720));

    arguments.recording.geometry = GeometryMode::Inherit;
    arguments.recording.viewport = None;

    let cancelled = Arc::new(AtomicBool::new(false));
    let mut signals = CancellationSignals(Vec::new());

    for signal in [SIGINT, SIGTERM] {
        signals
            .0
            .push(flag::register(signal, Arc::clone(&cancelled)).into_diagnostic()?);
    }

    let options = CaptureOptions {
        session_directory: session.clone(),
        width: target.width,
        height: target.height,
        padding: target.padding,
        fps: arguments.original_fps.unwrap_or(24),
        start_monotonic_ns: None,
        end_monotonic_ns: None,
        state_selection: StateSelection::All,
        geometry_solver: None,
        kind: CaptureKind::Video,
        destination: None,
        scale: None,
        dpi: None,
    };

    let provenance = serde_json::to_string(&profile.provenance).into_diagnostic()?;

    let captured = capture_original_with_backend(
        &env::current_exe().into_diagnostic()?,
        &options,
        &profile.replay_config,
        &cancelled,
        backend,
        |config| {
            arguments.recording.ghostty_config = Some(config.to_path_buf());
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

            return args;
        },
    )
    .into_diagnostic()?;

    write_json_line(
        response,
        &serde_json::json!({
            "kind": "original_native_recording", "session": session,
            "visual": captured.directory, "video": captured.video, "screenshot": captured.screenshot,
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
