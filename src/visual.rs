//! Native Ghostty capture on an isolated Xvfb display.

mod capture;
mod geometry;
mod index;
mod media;
mod model;
mod process;
mod util;
mod validation;

pub use capture::capture;
pub use geometry::{native_capture_target, probe_ghostty_geometry, solve_ghostty_geometry};
pub use model::{
    CaptureKind, CaptureOptions, CaptureOutcome, FittedGeometry, GHOSTTY_EFFECTIVE_CONFIG_NAME,
    GeometryProbeAttempt, GeometrySolveReport, NativeFrameSample, VISUAL_DIRECTORY,
    VISUAL_MANIFEST_VERSION, VideoMapEntry, VisualManifest, visual_directory_name,
    visual_directory_name_for,
};
pub use validation::validate_visual_manifest;
pub(crate) use validation::validate_visual_manifest_binding;

#[cfg(test)]
mod tests;
