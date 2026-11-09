use super::super::{
    capture_native, playback, CaptureOptions, CaptureOutcome, Duration, Error, Path,
    PlaybackOptions, Result, Session, StatePoint,
};
use crate::system::SystemService;
use std::sync::{atomic::AtomicBool, Arc};
use termtrace_engine::observation::replay_endpoint_for_session;
use termtrace_media::native_capture::{native_capture_target, NativeReplayWorker};
use termtrace_model::{
    execution::session::CaptureTarget, protocol::capability::NATIVE_SCREENSHOT_CAPABILITY,
};

/// Session-bound native capture, playback, and presentation operations.
pub struct Native<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Native<'_> {
    /// Start an owned terminal for forward screenshots and explicit live prefixes.
    ///
    /// Drop or close the returned worker to stop its terminal and display. The
    /// cancellation flag remains owned by the caller and is checked during replay.
    ///
    /// # Errors
    /// Returns an error when the capture viewport cannot be resolved or the worker's terminal and
    /// display cannot be started within `timeout`.
    pub fn replay_worker(
        &self,
        viewport: Option<CaptureTarget>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<NativeReplayWorker> {
        let (target, _) =
            native_capture_target(self.session.helper(), self.session.path(), viewport)?;

        return NativeReplayWorker::start(
            self.session.helper(),
            self.session.path(),
            target,
            cancelled,
            timeout,
        )
        .map_err(Error::from);
    }

    /// Capture one native screenshot of `point` in this completed session into the new
    /// `destination` directory, holding the PNG, its manifest, and the replay endpoint.
    ///
    /// Resolves `point`, requires the `native-screenshot` capability, then starts a replay
    /// worker, captures, and closes the worker. `cancelled` is checked during the probe and replay.
    ///
    /// # Errors
    /// Returns an error when the capability is blocked, the session is live, `point` does not
    /// resolve, `destination` exists, or the capture cannot be produced within `timeout`.
    pub fn screenshot(
        &self,
        point: &StatePoint,
        destination: &Path,
        viewport: Option<CaptureTarget>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<CaptureOutcome> {
        let endpoint = replay_endpoint_for_session(self.session.path(), point, &cancelled)?;
        SystemService::require(NATIVE_SCREENSHOT_CAPABILITY, &cancelled)?;
        let mut worker = self.replay_worker(viewport, cancelled, timeout)?;
        let capture = worker.capture_endpoint(&endpoint, destination)?;
        worker.close();

        return Ok(capture);
    }

    /// Capture native pixels for this canonical session.
    ///
    /// # Errors
    /// Returns an error when native capture tools are unavailable or the capture cannot be
    /// produced, verified, or published.
    pub fn capture(&self, options: &CaptureOptions) -> Result<CaptureOutcome> {
        let mut options = options.clone();
        options.session_directory = self.session.path().to_path_buf();

        return capture_native(self.session.helper(), &options).map_err(Error::from);
    }

    /// Play this canonical session through the native playback runtime.
    ///
    /// # Errors
    /// Returns an error when replay fails or its capture handshake files cannot be published.
    pub fn playback(&self, options: &PlaybackOptions) -> Result<()> {
        let mut options = options.clone();
        options.session_directory = self.session.path().to_path_buf();

        return playback(&options).map_err(Error::from);
    }
}
