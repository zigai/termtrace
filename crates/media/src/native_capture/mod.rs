//! Private-display native Ghostty capture and validation.

mod capability;
mod capture;
mod capture_io;
mod geometry;
mod index;
mod media;
mod original;
mod portal;
pub(crate) mod process;
mod profile;
mod resize_server;
mod resources;
#[cfg(test)]
mod tests;
mod validation;
pub(crate) mod window;
mod worker;

pub use crate::{Error, Result};
pub use capability::native_capabilities;
pub use capture::{capture, resolve_display_scaling, DisplayScaling};
pub use geometry::{
    native_capture_target, preflight_visual_profile, probe_ghostty_geometry, solve_ghostty_geometry,
};
pub use original::{
    capture_original, verify_original_capture, OriginalCapture, OriginalCaptureBackend,
    OriginalCaptureRequest, OriginalGrid, OriginalRecorderPaths, CAPTURE_ENVIRONMENT,
};
pub use termtrace_model::artifact::visual::{
    visual_directory_name_for, CaptureKind, CaptureOptions, CaptureOutcome, GeometryProbeAttempt,
    GeometrySolveReport, NativeFrameSample, VideoMapEntry, VisualManifest,
    GHOSTTY_EFFECTIVE_CONFIG_NAME, VISUAL_DIRECTORY, VISUAL_MANIFEST_VERSION,
};
pub(crate) use validation::validate_visual_manifest_binding;
pub use validation::{
    validate_replay_screenshot, validate_visual_manifest, validated_visual_manifests,
    validated_visual_manifests_for_state,
};
pub use worker::NativeReplayWorker;

const MIN_VIEWPORT_WIDTH: u16 = 64;
const MIN_VIEWPORT_HEIGHT: u16 = 32;
const MAX_WINDOW_PADDING_POINTS: u16 = 2000;
