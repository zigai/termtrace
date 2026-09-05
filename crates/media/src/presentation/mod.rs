//! Marker-resolved presentation edit plans over native or semantic sources.

mod ffmpeg;
mod mapping;
mod pipeline;
mod planning;
mod validation;

#[cfg(test)]
mod tests;

pub use pipeline::{export, preview};
pub use planning::{
    VideoIdlePolicy, markers, plan, plan_with_source, video_clip_ranges, video_clips,
};
pub use termtrace_model::artifact::presentation::{
    AnnotationAnchor, AnnotationPosition, AnnotationScope, AnnotationStyle, CaptionLayout,
    ChangeMode, ChangeRegion, ChangeSource, ChangeVisualization, CursorMaskFrame,
    EDIT_PLAN_VERSION, EditClip, EditPlan, EditSelector, MarkerRecord, NormalizedAnnotation,
    NormalizedChangeFrame, NormalizedClip, NormalizedSubtitle, PRESENTATION_FORMAT_VERSION,
    PixelTransform, PresentationDelivery, PresentationLayout, PresentationManifest,
    PresentationMapEntry, PresentationOutcome, PresentationPlan, PresentationPreviewManifest,
    PresentationPreviewOutcome, PresentationSource, PresentationSourceRequest, PreviewOptions,
    RationalSpeed, SubtitleTrack, TextAnnotation, VideoRenderConfig,
};
pub use validation::read_edit_plan;

use std::path::Path;

use crate::{Error, Result};

const NANOS_PER_SECOND: u64 = 1_000_000_000;

fn plan_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
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
