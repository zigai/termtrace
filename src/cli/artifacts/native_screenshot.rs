use crate::cli::{
    output::{path_value, write_json_line, ResponseWriter},
    session_helpers::attach_session,
    tooling::{capability_report, require_cli_capability},
};
use miette::{IntoDiagnostic as _, Result};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    flag, low_level,
};
use std::{
    path::Path,
    process::ExitCode,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use termtrace_model::{
    artifact::visual::CaptureOutcome, execution::session::CaptureTarget,
    protocol::capability::NATIVE_SCREENSHOT_CAPABILITY, terminal::observation::StatePoint,
};
use termtrace_sdk::{defaults::NATIVE_REPLAY_TIMEOUT, session::SessionStatus};

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
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut signals = CancellationSignals(Vec::new());
    for signal in [SIGINT, SIGTERM] {
        signals
            .0
            .push(flag::register(signal, Arc::clone(&cancelled)).into_diagnostic()?);
    }

    let attached = attach_session(session)?;
    let timeout = NATIVE_REPLAY_TIMEOUT;
    let live_boundary = match attached.status().into_diagnostic()? {
        SessionStatus::Completed { .. } => None,
        SessionStatus::Live { boundaries, .. } => {
            if points != [StatePoint::LatestPresented] {
                return Err(miette::miette!(
                    "live native capture currently requires the latest presented state"
                ));
            }

            Some(boundaries.durable)
        }
    };

    if let (None, [point]) = (&live_boundary, points) {
        let capture = attached
            .native()
            .screenshot(
                point,
                destination,
                viewport,
                Arc::clone(&cancelled),
                timeout,
            )
            .map_err(|error| {
                if cancelled.load(Ordering::SeqCst) {
                    return miette::miette!(
                        "the `{NATIVE_SCREENSHOT_CAPABILITY}` capture was cancelled"
                    );
                }
                return capability_report(error);
            })?;

        return write_single(response, &capture, destination);
    }

    require_cli_capability(NATIVE_SCREENSHOT_CAPABILITY, &cancelled)?;
    let mut worker = attached
        .native()
        .replay_worker(viewport, cancelled, timeout)
        .into_diagnostic()?;

    if points.len() > 1 {
        let captures = worker
            .capture_batch(points, destination)
            .into_diagnostic()?;
        worker.close();

        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "native_screenshot_batch", "truth_layer": "native", "directory": path_value(destination),
                "manifest": path_value(&destination.join("batch-manifest.json")),
                "images": captures.iter().map(|capture| return path_value(&capture.screenshot)).collect::<Vec<_>>(),
                "presentation_evidence": "post_endpoint_stable_capture",
            }),
        );
    }

    let boundary = live_boundary.ok_or_else(|| miette::miette!("missing screenshot selection"))?;
    let capture = worker
        .capture_latest_prefix(&boundary, destination)
        .into_diagnostic()?;
    worker.close();

    return write_single(response, &capture, destination);
}

fn write_single(
    response: &mut ResponseWriter,
    capture: &CaptureOutcome,
    destination: &Path,
) -> Result<ExitCode> {
    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "native_screenshot", "truth_layer": "native", "directory": path_value(&capture.directory),
            "image": path_value(&capture.screenshot), "manifest": path_value(&destination.join("manifest.json")),
            "endpoint": path_value(&destination.join("replay-endpoint.json")),
            "presentation_evidence": "post_endpoint_stable_capture",
        }),
    );
}
