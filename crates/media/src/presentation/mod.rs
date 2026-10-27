//! Marker-resolved presentation edit plans over native or semantic sources.

mod derivative;
mod ffmpeg;

pub use derivative::{
    export_derivatives, export_derivatives_with_cancel, verify_derivative_artifact,
    verify_source_selection,
};

mod mapping;
mod pipeline;
mod planning;
#[cfg(test)]
mod tests;
mod validation;

use crate::{Error, Result};
pub use pipeline::{export, preview};
pub use planning::{
    markers, plan, plan_with_capture_directory, plan_with_source, video_clip_ranges,
    video_clip_ranges_session, video_clips, VideoIdlePolicy,
};
use std::path::Path;
pub use termtrace_model::artifact::presentation::{
    AnnotationAnchor, AnnotationPosition, AnnotationScope, AnnotationStyle, CaptionLayout,
    ChangeMode, ChangeRegion, ChangeSource, ChangeVisualization, CursorMaskFrame, EditClip,
    EditPlan, EditSelector, MarkerRecord, NormalizedAnnotation, NormalizedChangeFrame,
    NormalizedClip, NormalizedSubtitle, PixelTransform, PresentationDelivery, PresentationLayout,
    PresentationManifest, PresentationMapEntry, PresentationOutcome, PresentationPlan,
    PresentationPreviewManifest, PresentationPreviewOutcome, PresentationSource,
    PresentationSourceRequest, PreviewOptions, RationalSpeed, SubtitleTrack, TextAnnotation,
    VideoRenderConfig, EDIT_PLAN_VERSION, PRESENTATION_FORMAT_VERSION,
};
pub use validation::read_edit_plan;

const NANOS_PER_SECOND: u64 = 1_000_000_000;

fn plan_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
        source: None,
    };
}

fn filter_path_value(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| return plan_error("FFmpeg filter resource path is not UTF-8"))?;

    return Ok(value
        .replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('\'', "\\'"));
}
