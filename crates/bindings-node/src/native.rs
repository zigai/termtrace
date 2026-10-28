//! Native Ghostty pixels replayed from one canonical session.

use crate::host::{Arg, Model};
use crate::{blocking_failure, diagnostics, to_napi_err};
use napi_derive::napi;
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc};
use termtrace_model::artifact::visual::CaptureOutcome;
use termtrace_model::terminal::observation::StatePoint;
use termtrace_model::value::ExactPath;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::defaults::NATIVE_REPLAY_TIMEOUT;

/// Options of one native replay screenshot.
#[napi(object, object_to_js = false, js_name = "NativeScreenshotOptions")]
pub struct JsNativeScreenshotOptions {
    /// State point; the latest presented state when omitted.
    pub point: Option<Arg<StatePoint>>,
    /// Bound on the replay terminal's startup in milliseconds; 30 seconds when omitted.
    pub timeout_ms: Option<f64>,
}

/// Native replay operations for one canonical session.
#[napi(js_name = "Native")]
pub struct JsNative {
    pub(crate) terminal: Terminal,
    pub(crate) path: PathBuf,
}

#[napi]
impl JsNative {
    /// Replay this completed session in Ghostty and capture `options.point` as a PNG in the
    /// new `destination` directory, with its manifest and replay endpoint.
    ///
    /// # Errors
    ///
    /// Rejects when the `native-screenshot` capability is blocked, the session is live, the
    /// point does not resolve, `destination` exists, or capture fails.
    #[napi]
    pub async fn screenshot(
        &self,
        destination: Arg<ExactPath>,
        options: Option<JsNativeScreenshotOptions>,
    ) -> napi::Result<Model<CaptureOutcome>> {
        let (point, timeout_ms) = options.map_or((None, None), |options| {
            return (options.point, options.timeout_ms);
        });
        let timeout = diagnostics::timeout(timeout_ms, NATIVE_REPLAY_TIMEOUT)?;
        let point = point.map_or(StatePoint::LatestPresented, |point| return point.0);
        let terminal = self.terminal.clone();
        let path = self.path.clone();
        let destination = destination.0 .0;

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(&path)?;

            return session.native().screenshot(
                &point,
                &destination,
                None,
                Arc::new(AtomicBool::new(false)),
                timeout,
            );
        })
        .await
        .map_err(blocking_failure)?
        .map(Model)
        .map_err(to_napi_err);
    }
}
