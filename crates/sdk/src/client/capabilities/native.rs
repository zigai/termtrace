use super::super::*;
use std::sync::{Arc, atomic::AtomicBool};
use termtrace_media::native_capture::{NativeReplayWorker, native_capture_target};
use termtrace_model::execution::session::CaptureTarget;

/// Session-bound native capture, playback, and presentation operations.
pub struct Native<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Native<'_> {
    /// Start an owned terminal for forward screenshots and explicit live prefixes.
    ///
    /// Drop or close the returned worker to stop its terminal and display. The
    /// cancellation flag remains owned by the caller and is checked during replay.
    pub fn replay_worker(
        &self,
        viewport: Option<CaptureTarget>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<NativeReplayWorker> {
        let (target, _) =
            native_capture_target(&self.session.helper, &self.session.path, viewport)?;
        return NativeReplayWorker::start(
            &self.session.helper,
            &self.session.path,
            target,
            cancelled,
            timeout,
        )
        .map_err(Error::from);
    }

    /// Capture native pixels for this canonical session.
    pub fn capture(&self, options: &CaptureOptions) -> Result<CaptureOutcome> {
        let mut options = options.clone();
        options.session_directory = self.session.path.clone();
        return capture_native(&self.session.helper, &options).map_err(Error::from);
    }

    /// Play this canonical session through the native playback runtime.
    pub fn playback(&self, options: &PlaybackOptions) -> Result<()> {
        let mut options = options.clone();
        options.session_directory = self.session.path.clone();
        return playback(&options).map_err(Error::from);
    }
}
