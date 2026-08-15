//! Marker-resolved presentation edit plans over native or semantic sources.

mod ffmpeg;
mod mapping;
mod pipeline;
mod planning;
mod validation;

#[cfg(test)]
mod tests;

pub use pipeline::{export, preview};
pub use planning::{markers, plan, plan_with_source};
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

use crate::Error;

const NANOS_PER_SECOND: u64 = 1_000_000_000;

fn plan_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
