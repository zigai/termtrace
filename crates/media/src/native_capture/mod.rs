//! Private-display native Ghostty capture and validation.

mod capability;
mod capture;
mod geometry;
mod index;
mod media;
mod original;
mod portal;
pub(crate) mod process;
mod profile;
mod resources;
mod util;
mod validation;
mod worker;

#[cfg(test)]
mod tests;
pub use capability::native_capabilities;

pub use crate::{Error, Result};
pub use capture::capture;
pub use geometry::{native_capture_target, probe_ghostty_geometry, solve_ghostty_geometry};
pub use original::{OriginalCaptureBackend, capture_original, capture_original_with_backend};
pub use termtrace_model::artifact::visual::{
    CaptureKind, CaptureOptions, CaptureOutcome, GHOSTTY_EFFECTIVE_CONFIG_NAME,
    GeometryProbeAttempt, GeometrySolveReport, NativeFrameSample, VISUAL_DIRECTORY,
    VISUAL_MANIFEST_VERSION, VideoMapEntry, VisualManifest, visual_directory_name_for,
};
pub(crate) use validation::validate_visual_manifest_binding;
pub use validation::{
    validate_replay_screenshot, validate_visual_manifest, validated_visual_manifests,
    validated_visual_manifests_for_state,
};
pub use worker::NativeReplayWorker;
