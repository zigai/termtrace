//! Presentation videos of completed sessions: plan, preview, or render.

use super::ArtifactService;
use crate::{error::Error, system::SystemService, Result};
use std::{
    fmt,
    io::ErrorKind as IoErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_engine::{
    replay::index_session_for_video,
    timeline::{resolve_session_source_selection, SourceRange},
    Error as EngineError,
};
use termtrace_evidence::{event_log::EventLogSummary, session::validate_completed_session};
use termtrace_media::{
    native_capture::{
        capture, native_capture_target, CaptureKind, CaptureOptions, VISUAL_DIRECTORY,
    },
    presentation::{
        export as export_presentation, plan_with_source as plan_presentation,
        preview as preview_presentation, read_edit_plan, video_clip_ranges_session, video_clips,
        EditPlan, PixelTransform, PresentationDelivery, PresentationPlan,
        PresentationPreviewOutcome, PresentationSource, PresentationSourceRequest, PreviewOptions,
        VideoIdlePolicy, VideoRenderConfig, EDIT_PLAN_VERSION,
    },
};
use termtrace_model::{
    artifact::save::SemanticRasterProfile,
    execution::layout::SessionLayout,
    execution::session::CaptureTarget,
    protocol::capability::{
        NATIVE_VIDEO_CAPABILITY, PRESENTATION_CAPABILITY, SEMANTIC_VIDEO_CAPABILITY,
    },
    terminal::replay::StateSelection,
};

const VIDEO_IDLE_THRESHOLD_NS: u64 = 2_000_000_000;
const VIDEO_IDLE_CONTEXT_NS: u64 = 500_000_000;
const VISUAL_MANIFEST: &str = "manifest.json";
/// Suffix of the manifested render directory kept next to a published video.
const SIDECAR_SUFFIX: &str = ".termtrace";

/// Delivery quality and default timing policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VideoPreset {
    /// Crisp, compact H.264 suitable for documentation.
    #[default]
    Docs,
    /// High-motion H.264 suitable for social publishing.
    Social,
    /// Lossless H.264 encoding with full chroma.
    Lossless,
}

/// Where a video's pixels come from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VideoRenderer {
    /// Verified pixels captured from the real Ghostty renderer; never falls back.
    #[default]
    Native,
    /// Deterministic pixels generated from canonical semantic replay.
    Semantic,
}

impl VideoRenderer {
    const fn as_str(self) -> &'static str {
        return match self {
            Self::Native => "native",
            Self::Semantic => "semantic",
        };
    }
}

/// Whether the recorded cursor stays visible.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CursorStyle {
    /// Preserve the cursor captured in the source.
    #[default]
    Recorded,
    /// Mask the cursor using canonical cursor geometry.
    Hidden,
}

/// What a video request produces.
#[derive(Clone, Debug)]
pub enum VideoMode {
    /// Resolve the presentation plan without rendering.
    Plan,
    /// Render a contact sheet and low-resolution review video into a new directory.
    Preview {
        /// New preview directory.
        directory: PathBuf,
        /// Storyboard and review-video settings.
        options: PreviewOptions,
    },
    /// Render an MP4 and keep its manifested `.termtrace` sidecar next to it.
    Render {
        /// MP4 output path.
        out: PathBuf,
    },
}

/// A presentation video of one completed session.
#[derive(Clone, Debug)]
pub struct VideoRequest {
    /// Completed canonical session.
    pub session: PathBuf,
    /// Plan, preview, or render.
    pub mode: VideoMode,
    /// Strict versioned edit plan; the facade options below are ignored when set.
    pub edit: Option<PathBuf>,
    /// Delivery quality and default timing policy.
    pub preset: VideoPreset,
    /// Exact output frame rate; a native capture must match.
    pub fps: Option<u16>,
    /// Pixel source.
    pub renderer: VideoRenderer,
    /// Semantic rasterization quality; invalid for the native renderer.
    pub semantic_raster_profile: Option<SemanticRasterProfile>,
    /// Exact even output dimensions.
    pub resolution: Option<CaptureTarget>,
    /// Integer output scale.
    pub scale: Option<u16>,
    /// Begin at this unique marker.
    pub start_marker: Option<String>,
    /// End at this unique marker.
    pub end_marker: Option<String>,
    /// Remove child-output gaps longer than two seconds.
    pub trim_idle: bool,
    /// First-frame hold; preset-specific when `None`.
    pub hold_start: Option<Duration>,
    /// Final-frame hold; preset-specific when `None`.
    pub hold_end: Option<Duration>,
    /// Whether the recorded cursor stays visible.
    pub cursor_style: CursorStyle,
}

impl VideoRequest {
    /// Request `mode` for `session` with every facade option at its default.
    pub fn new(session: impl Into<PathBuf>, mode: VideoMode) -> Self {
        return Self {
            session: session.into(),
            mode,
            edit: None,
            preset: VideoPreset::default(),
            fps: None,
            renderer: VideoRenderer::default(),
            semantic_raster_profile: None,
            resolution: None,
            scale: None,
            start_marker: None,
            end_marker: None,
            trim_idle: false,
            hold_start: None,
            hold_end: None,
            cursor_style: CursorStyle::default(),
        };
    }
}

/// What a video request produced.
#[derive(Debug)]
pub enum VideoOutcome {
    /// The resolved presentation plan.
    Planned(Box<PresentationPlan>),
    /// A rendered preview of the plan.
    Previewed {
        /// Rendered plan.
        plan: Box<PresentationPlan>,
        /// Preview directory contents.
        preview: PresentationPreviewOutcome,
    },
    /// A published MP4 and its manifested sidecar.
    Rendered {
        /// Rendered plan.
        plan: Box<PresentationPlan>,
        /// Published MP4.
        output: PathBuf,
        /// Manifested render directory kept next to the MP4.
        sidecar: PathBuf,
    },
}

impl ArtifactService {
    /// Plan, preview, or render a presentation video of a completed session.
    ///
    /// `helper` is the Termtrace executable that runs native capture helpers; `progress`
    /// receives human-readable stage announcements.
    ///
    /// # Errors
    /// Returns invalid requests, blocked capabilities, incomplete or invalid evidence, and
    /// capture, planning, or render failures.
    pub fn video(
        request: &VideoRequest,
        helper: &Path,
        progress: &mut dyn FnMut(fmt::Arguments<'_>),
    ) -> Result<VideoOutcome> {
        let sidecar = match &request.mode {
            VideoMode::Render { out } => {
                if out
                    .extension()
                    .is_none_or(|extension| return extension != "mp4")
                {
                    return Err(invalid("video output must use the `.mp4` extension"));
                }

                Some(video_sidecar_path(out))
            }
            VideoMode::Plan | VideoMode::Preview { .. } => None,
        };

        if request.renderer == VideoRenderer::Native && request.semantic_raster_profile.is_some() {
            return Err(invalid(
                "--semantic-raster-profile configures semantic video only",
            ));
        }

        let session = request.session.as_path();
        let preset = PresetConfig::from_preset(request.preset);
        let provisional_fps = request.fps.unwrap_or(preset.fps);

        if request.renderer == VideoRenderer::Native {
            ensure_native_capture(session, provisional_fps, helper, request, progress)?;
        }

        let source = match request.renderer {
            VideoRenderer::Native => PresentationSourceRequest::Native,
            VideoRenderer::Semantic => PresentationSourceRequest::Semantic {
                fps: provisional_fps,
                raster_profile: request
                    .semantic_raster_profile
                    .unwrap_or(preset.default_raster_profile),
            },
        };

        progress(format_args!("Planning video from {}...", session.display()));

        let plan = plan_video(request, session, &preset, provisional_fps, source)?;
        if let Some(requested) = request.fps
            && requested != plan.fps
        {
            return Err(invalid(format!(
                "requested {requested} FPS but the verified native capture is {} FPS; recapture at the requested rate",
                plan.fps
            )));
        }

        let (out, sidecar) = match (&request.mode, sidecar) {
            (VideoMode::Plan, _) => return Ok(VideoOutcome::Planned(Box::new(plan))),
            (VideoMode::Preview { directory, options }, _) => {
                require_render_capabilities(source)?;

                let preview = preview_presentation(&plan, directory, *options)?;

                return Ok(VideoOutcome::Previewed {
                    plan: Box::new(plan),
                    preview,
                });
            }
            (VideoMode::Render { out }, Some(sidecar)) => (out, sidecar),
            (VideoMode::Render { .. }, None) => {
                return Err(invalid("video export sidecar is missing"));
            }
        };

        require_render_capabilities(source)?;
        progress(format_args!(
            "Rendering {} frames at {} FPS with the {} renderer...",
            plan.output_frame_count,
            plan.fps,
            source_name(&plan.source),
        ));

        let outcome = export_presentation(&plan, &sidecar)?;
        publish(&outcome.video, out)?;
        progress(format_args!(
            "Published video and manifested render configuration."
        ));

        return Ok(VideoOutcome::Rendered {
            plan: Box::new(plan),
            output: out.clone(),
            sidecar,
        });
    }
}

/// Stable name of a plan's pixel source.
#[must_use]
pub const fn source_name(source: &PresentationSource) -> &'static str {
    return match source {
        PresentationSource::Native { .. } => VideoRenderer::Native.as_str(),
        PresentationSource::Semantic { .. } => VideoRenderer::Semantic.as_str(),
    };
}

/// Capture verified native frames unless the session already holds a complete capture.
fn ensure_native_capture(
    session: &Path,
    fps: u16,
    helper: &Path,
    request: &VideoRequest,
    progress: &mut dyn FnMut(fmt::Arguments<'_>),
) -> Result<()> {
    let visual = session.join(VISUAL_DIRECTORY);
    if visual.join(VISUAL_MANIFEST).is_file() {
        return Ok(());
    }

    SystemService::require(NATIVE_VIDEO_CAPABILITY)?;

    match index_session_for_video(session, u32::from(fps)) {
        Ok(_) | Err(EngineError::ArtifactExists { .. }) => {}
        Err(error) => return Err(error.into()),
    }

    if visual.exists() && !visual.join(VISUAL_MANIFEST).is_file() {
        let _ = fs_err::remove_dir_all(&visual);
    }

    progress(format_args!(
        "Capturing native video for {}...",
        session.display()
    ));

    let (target, geometry_solver) = native_capture_target(helper, session, request.resolution)?;

    capture(
        helper,
        &CaptureOptions {
            session_directory: session.to_path_buf(),
            width: target.width,
            height: target.height,
            fps,
            start_monotonic_ns: None,
            end_monotonic_ns: None,
            state_selection: StateSelection::All,
            geometry_solver,
            kind: CaptureKind::Video,
            destination: None,
            scale: None,
            dpi: None,
        },
    )?;

    return Ok(());
}

fn plan_video(
    request: &VideoRequest,
    session: &Path,
    preset: &PresetConfig,
    provisional_fps: u16,
    source: PresentationSourceRequest,
) -> Result<PresentationPlan> {
    if let Some(edit_path) = &request.edit {
        let edit = read_edit_plan(edit_path)?;

        return Ok(plan_presentation(session, &edit, source)?);
    }

    validate_completed_session(session)?;

    let summary = EventLogSummary::read(SessionLayout::new(session).event_log())?;
    let final_monotonic_ns = summary
        .terminal
        .as_ref()
        .map_or(0, |record| return record.monotonic_ns);

    let mut selected = resolve_session_source_selection(
        session,
        None,
        None,
        request.start_marker.as_deref(),
        request.end_marker.as_deref(),
    )?
    .unwrap_or(SourceRange {
        start_monotonic_ns: 0,
        end_monotonic_ns: final_monotonic_ns,
    });
    selected.end_monotonic_ns = selected.end_monotonic_ns.min(final_monotonic_ns);

    if selected.start_monotonic_ns >= selected.end_monotonic_ns {
        return Err(invalid("video range must contain positive recorded time"));
    }

    let ranges = video_clip_ranges_session(
        session,
        selected,
        request.trim_idle.then_some(VideoIdlePolicy {
            threshold_ns: VIDEO_IDLE_THRESHOLD_NS,
            context_ns: VIDEO_IDLE_CONTEXT_NS,
        }),
    )?;

    let hold_start = request.hold_start.unwrap_or(preset.hold_start);
    let hold_end = request.hold_end.unwrap_or(preset.hold_end);
    let provisional = facade_edit_plan(
        request,
        preset,
        &ranges,
        provisional_fps,
        hold_start,
        hold_end,
    )?;

    let first_plan = plan_presentation(session, &provisional, source)?;
    let edit = facade_edit_plan(
        request,
        preset,
        &ranges,
        first_plan.fps,
        hold_start,
        hold_end,
    )?;

    if edit == provisional {
        return Ok(first_plan);
    }
    return Ok(plan_presentation(session, &edit, source)?);
}

fn facade_edit_plan(
    request: &VideoRequest,
    preset: &PresetConfig,
    ranges: &[SourceRange],
    fps: u16,
    hold_start: Duration,
    hold_end: Duration,
) -> Result<EditPlan> {
    let hold_start_ms = duration_millis(hold_start, "--hold-start")?;
    let hold_end_ms = duration_millis(hold_end, "--hold-end")?;
    let clips = video_clips(ranges, fps, hold_start_ms, hold_end_ms)?;

    let mut transforms: Vec<PixelTransform> = Vec::new();

    if request.cursor_style == CursorStyle::Hidden {
        transforms.push(PixelTransform::CursorMask);
    }

    if let Some(resolution) = request.resolution {
        transforms.push(PixelTransform::Resize {
            width: resolution.width,
            height: resolution.height,
        });
    }

    if let Some(scale) = request.scale
        && scale != 1
    {
        transforms.push(PixelTransform::Zoom {
            numerator: scale,
            denominator: 1,
        });
    }

    return Ok(EditPlan {
        version: EDIT_PLAN_VERSION,
        clips,
        delivery: preset.delivery(),
        transforms,
        render_config: Some(VideoRenderConfig {
            preset: preset.name.to_owned(),
            renderer: request.renderer.as_str().to_owned(),
            fps,
            resolution: request
                .resolution
                .map(|resolution| return [resolution.width, resolution.height]),
            scale: request.scale,
            start_marker: request.start_marker.clone(),
            end_marker: request.end_marker.clone(),
            trim_idle: request.trim_idle,
            idle_threshold_ms: request
                .trim_idle
                .then_some(VIDEO_IDLE_THRESHOLD_NS / 1_000_000),
            hold_start_ms,
            hold_end_ms,
            cursor_style: match request.cursor_style {
                CursorStyle::Recorded => "recorded",
                CursorStyle::Hidden => "hidden",
            }
            .to_owned(),
        }),
        ..EditPlan::default()
    });
}

struct PresetConfig {
    name: &'static str,
    fps: u16,
    hold_start: Duration,
    hold_end: Duration,
    codec: &'static str,
    pixel_format: &'static str,
    crf: u8,
    encoder_preset: &'static str,
    default_raster_profile: SemanticRasterProfile,
}

impl PresetConfig {
    const fn from_preset(preset: VideoPreset) -> Self {
        return match preset {
            VideoPreset::Docs => Self {
                name: "docs",
                fps: 30,
                hold_start: Duration::from_millis(500),
                hold_end: Duration::from_millis(1_000),
                codec: "h264",
                pixel_format: "yuv420p",
                crf: 18,
                encoder_preset: "slow",
                default_raster_profile: SemanticRasterProfile::Direct,
            },
            VideoPreset::Social => Self {
                name: "social",
                fps: 60,
                hold_start: Duration::from_millis(250),
                hold_end: Duration::from_millis(750),
                codec: "h264",
                pixel_format: "yuv420p",
                crf: 20,
                encoder_preset: "medium",
                default_raster_profile: SemanticRasterProfile::Direct,
            },
            VideoPreset::Lossless => Self {
                name: "lossless",
                fps: 60,
                hold_start: Duration::ZERO,
                hold_end: Duration::ZERO,
                codec: "h264_rgb",
                pixel_format: "rgb24",
                crf: 0,
                encoder_preset: "veryslow",
                default_raster_profile: SemanticRasterProfile::Supersampled2x,
            },
        };
    }

    fn delivery(&self) -> PresentationDelivery {
        return PresentationDelivery {
            codec: self.codec.to_owned(),
            pixel_format: self.pixel_format.to_owned(),
            crf: self.crf,
            encoder_preset: self.encoder_preset.to_owned(),
        };
    }
}

fn require_render_capabilities(source: PresentationSourceRequest) -> Result<()> {
    if let PresentationSourceRequest::Semantic { .. } = source {
        SystemService::require(SEMANTIC_VIDEO_CAPABILITY)?;
    }

    return SystemService::require(PRESENTATION_CAPABILITY);
}

/// Replace `out` with the rendered video, hard-linking when the filesystem allows it.
fn publish(video: &Path, out: &Path) -> Result<()> {
    let filesystem = |source| {
        return Error::Filesystem {
            path: out.to_path_buf(),
            source,
        };
    };

    if out.symlink_metadata().is_ok() {
        if out.is_dir() {
            fs_err::remove_dir_all(out).map_err(filesystem)?;
        } else {
            fs_err::remove_file(out).map_err(filesystem)?;
        }
    }

    return match fs_err::hard_link(video, out) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == IoErrorKind::CrossesDevices => {
            fs_err::copy(video, out).map(drop).map_err(filesystem)
        }
        Err(source) => Err(filesystem(source)),
    };
}

fn duration_millis(duration: Duration, flag: &str) -> Result<u64> {
    return u64::try_from(duration.as_millis())
        .map_err(|_error| return invalid(format!("{flag} is too large")));
}

fn video_sidecar_path(output: &Path) -> PathBuf {
    let mut sidecar = output.as_os_str().to_os_string();
    sidecar.push(SIDECAR_SUFFIX);
    let sidecar = PathBuf::from(sidecar);
    if sidecar
        .parent()
        .is_some_and(|parent| return !parent.as_os_str().is_empty())
    {
        return sidecar;
    }
    return Path::new(".").join(sidecar);
}

fn invalid(reason: impl Into<String>) -> Error {
    return Error::InvalidOperation {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_raster_profiles_follow_source_and_preset_precedence() {
        let profile = |preset, explicit: Option<SemanticRasterProfile>| {
            return explicit.unwrap_or_else(|| {
                return PresetConfig::from_preset(preset).default_raster_profile;
            });
        };

        assert_eq!(
            profile(VideoPreset::Docs, None),
            SemanticRasterProfile::Direct
        );
        assert_eq!(
            profile(VideoPreset::Social, None),
            SemanticRasterProfile::Direct
        );
        assert_eq!(
            profile(VideoPreset::Lossless, None),
            SemanticRasterProfile::Supersampled2x
        );
        assert_eq!(
            profile(VideoPreset::Lossless, Some(SemanticRasterProfile::Direct)),
            SemanticRasterProfile::Direct
        );
    }
}
