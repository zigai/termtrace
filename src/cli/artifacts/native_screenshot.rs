use crate::cli::{
    output::{write_json_line, ResponseWriter},
    tooling::require_cli_capability,
};
use miette::{IntoDiagnostic as _, Result};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    flag, low_level,
};
use std::{
    env,
    path::Path,
    process::ExitCode,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
use termtrace_media::native_capture::{native_capture_target, NativeReplayWorker};
use termtrace_model::{
    execution::session::CaptureTarget,
    protocol::{
        capability::NATIVE_SCREENSHOT_CAPABILITY,
        control::{ControlCommand, ControlData},
    },
    terminal::observation::StatePoint,
};

struct CancellationSignals(Vec<signal_hook::SigId>);

impl Drop for CancellationSignals {
    fn drop(&mut self) {
        for signal in self.0.drain(..) {
            low_level::unregister(signal);
        }
    }
}

pub(super) fn run(
    response: &mut ResponseWriter,
    session: &Path,
    points: &[StatePoint],
    destination: &Path,
    viewport: Option<CaptureTarget>,
) -> Result<ExitCode> {
    require_cli_capability(NATIVE_SCREENSHOT_CAPABILITY)?;

    let live_boundary = if session.join("completion.json").is_file() {
        None
    } else {
        if points != [StatePoint::LatestPresented] {
            return Err(miette::miette!(
                "live native capture currently requires the latest presented state"
            ));
        }

        match termtrace_sdk::request(session, ControlCommand::Status) {
            Ok(ControlData::Status { boundaries, .. }) => Some(boundaries.durable),
            Err(_) if session.join("completion.json").is_file() => None,
            Err(error) => return Err(error).into_diagnostic(),
            _ => {
                return Err(miette::miette!(
                    "live status returned an unexpected response"
                ));
            }
        }
    };

    let cancelled = Arc::new(AtomicBool::new(false));
    let mut signals = CancellationSignals(Vec::new());

    for signal in [SIGINT, SIGTERM] {
        signals
            .0
            .push(flag::register(signal, Arc::clone(&cancelled)).into_diagnostic()?);
    }

    let helper = env::current_exe().into_diagnostic()?;
    let (target, _) = native_capture_target(&helper, session, viewport).into_diagnostic()?;
    let mut worker =
        NativeReplayWorker::start(&helper, session, target, cancelled, Duration::from_secs(30))
            .into_diagnostic()?;

    if points.len() > 1 {
        let captures = worker
            .capture_batch(points, destination)
            .into_diagnostic()?;
        worker.close();

        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "native_screenshot_batch", "truth_layer": "native", "directory": destination,
                "manifest": destination.join("batch-manifest.json"),
                "images": captures.iter().map(|capture| return &capture.screenshot).collect::<Vec<_>>(),
                "presentation_evidence": "post_endpoint_stable_capture",
            }),
        );
    }

    let capture = if let Some(boundary) = live_boundary {
        worker.capture_latest_prefix(&boundary, destination)
    } else {
        worker.capture(
            points
                .first()
                .ok_or_else(|| miette::miette!("missing screenshot selection"))?,
            destination,
        )
    }
    .into_diagnostic()?;
    worker.close();

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "native_screenshot", "truth_layer": "native", "directory": capture.directory,
            "image": capture.screenshot, "manifest": destination.join("manifest.json"),
            "endpoint": destination.join("replay-endpoint.json"),
            "presentation_evidence": "post_endpoint_stable_capture",
        }),
    );
}
