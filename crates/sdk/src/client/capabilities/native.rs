use super::super::*;

/// Session-bound native capture, playback, and presentation operations.
pub struct Native<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Native<'_> {
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
