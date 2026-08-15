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
        return capture_native(&options).map_err(Error::from);
    }

    /// Play this canonical session through the native playback runtime.
    pub fn playback(&self, options: &PlaybackOptions) -> Result<()> {
        let mut options = options.clone();
        options.session_directory = self.session.path.clone();
        return playback(&options).map_err(Error::from);
    }

    /// Export an already normalized presentation plan.
    pub fn export(
        &self,
        plan: &PresentationPlan,
        destination: impl AsRef<Path>,
    ) -> Result<PresentationOutcome> {
        return export_presentation(plan, destination).map_err(Error::from);
    }

    /// Render an atomic presentation preview.
    pub fn preview(
        &self,
        plan: &PresentationPlan,
        destination: impl AsRef<Path>,
        options: PreviewOptions,
    ) -> Result<PresentationPreviewOutcome> {
        return preview_presentation(plan, destination, options).map_err(Error::from);
    }
}
