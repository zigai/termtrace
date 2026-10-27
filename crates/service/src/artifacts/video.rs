//! Presentation videos of completed sessions: plan, preview, or render.

use super::ArtifactService;
use crate::{error::Error, system::SystemService, Result};
use std::{
    fmt,
    io::{self, ErrorKind as IoErrorKind},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};
use termtrace_engine::{
    replay::index_session_for_video,
    timeline::{resolve_session_source_selection, SourceRange},
    Error as EngineError,
};
use termtrace_evidence::{
    artifact::open_regular_file_nofollow, event_log::EventLogSummary,
    session::validate_completed_session,
};
use termtrace_media::{
    native_capture::{
        capture, native_capture_target, CaptureKind, CaptureOptions, VISUAL_DIRECTORY,
    },
    presentation::{
        export as export_presentation, export_derivatives, plan_with_capture_directory,
        preview as preview_presentation, read_edit_plan, verify_source_selection,
        video_clip_ranges_session, video_clips, EditPlan, PixelTransform, PresentationDelivery,
        PresentationPlan, PresentationPreviewOutcome, PresentationSource,
        PresentationSourceRequest, PreviewOptions, VideoIdlePolicy, VideoRenderConfig,
        EDIT_PLAN_VERSION,
    },
    Error as MediaError,
};
use termtrace_model::{
    artifact::{
        derivative::{DerivativeExportOptions, DerivativeFormat, GifLoop},
        save::SemanticRasterProfile,
        sdk::{ArtifactSourceKind, MediaArtifactResult},
        visual::VisualManifest,
    },
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
    /// Explicit native replay pixels; never original execution or semantic substitution.
    ReplayNative,
    /// Reuse verified original-execution CFR pixels only; never replay the application.
    OriginalExecution,
}

impl VideoRenderer {
    const fn as_str(self) -> &'static str {
        return match self {
            Self::Native => "native",
            Self::Semantic => "semantic",
            Self::ReplayNative => "native_replay",
            Self::OriginalExecution => "original_execution",
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
    /// Render MP4 (legacy replacement) or GIF/VP9 `WebM` (no-replace), retaining a `.termtrace` sidecar.
    Render {
        /// Lowercase `.mp4`, `.gif`, or `.webm` output path.
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
    /// GIF loop policy; None defaults to infinite. Invalid for other outputs.
    pub gif_loop: Option<GifLoop>,
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
            gif_loop: None,
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
    /// A no-replace GIF/VP9 `WebM` with the shared artifact envelope.
    Derived {
        /// Normalized common plan.
        plan: Box<PresentationPlan>,
        /// External no-replace delivery path.
        output: PathBuf,
        /// Shared lossless SDK result envelope for the retained sidecar.
        artifact: MediaArtifactResult,
    },
    /// A published legacy MP4 and its manifested sidecar (overwrite-compatible).
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
        let format = match &request.mode {
            VideoMode::Render { out } => match out.extension().and_then(|s| return s.to_str()) {
                Some("mp4") => None,
                Some("gif") => Some(DerivativeFormat::Gif {
                    loop_policy: request.gif_loop.unwrap_or(GifLoop::Infinite),
                }),
                Some("webm") => Some(DerivativeFormat::Webm {}),
                _ => {
                    return Err(invalid(
                        "video output must use lowercase `.mp4`, `.gif` or `.webm`",
                    ))
                }
            },
            _ => None,
        };

        if let Some(fps) = request.fps
            && format.is_some()
            && (fps == 0
                || fps > 120
                || (matches!(format, Some(DerivativeFormat::Gif { .. })) && fps > 100))
        {
            return Err(invalid(
                "derivative FPS must be positive, GIF at most 100 and WebM at most 120",
            ));
        }

        if request.gif_loop.is_some() && !matches!(format, Some(DerivativeFormat::Gif { .. })) {
            return Err(invalid("GIF loop policy is only valid for .gif output"));
        }

        if format.is_some() && request.preset == VideoPreset::Lossless {
            return Err(invalid(
                "GIF/VP9 WebM are lossy derivatives; the lossless preset is not supported",
            ));
        }

        let sidecar = match &request.mode {
            VideoMode::Render { out } => {
                let sidecar = video_sidecar_path(out);
                if format.is_some() {
                    for path in [out, &sidecar] {
                        if fs_err::symlink_metadata(path).is_ok() {
                            return Err(invalid(format!(
                                "derivative destination already exists: {}",
                                path.display()
                            )));
                        }
                    }
                }

                Some(sidecar)
            }
            _ => None,
        };

        if request.renderer != VideoRenderer::Semantic && request.semantic_raster_profile.is_some()
        {
            return Err(invalid(
                "--semantic-raster-profile configures semantic video only",
            ));
        }

        let session = request.session.as_path();
        let preset = PresetConfig::from_preset(request.preset);
        let provisional_fps = request.fps.unwrap_or(preset.fps);
        let source_kind = match request.renderer {
            VideoRenderer::Native | VideoRenderer::ReplayNative => ArtifactSourceKind::NativeReplay,
            VideoRenderer::OriginalExecution => ArtifactSourceKind::OriginalExecution,
            VideoRenderer::Semantic => ArtifactSourceKind::SemanticRendering,
        };

        let capture_directory = if source_kind == ArtifactSourceKind::OriginalExecution {
            let original = session.join("visual-original");
            if !original.join(VISUAL_MANIFEST).is_file() {
                return Err(MediaError::CapabilityBlocked { id: "native.original_recording".into(), code: "original_execution_source_unavailable".into(), detail: "verified original-execution CFR pixels are absent; replay and semantic pixels cannot substitute".into(), remediation: "supply a completed recording with verified CFR visual-original pixels (task07 live adapter)".into() }.into());
            }

            Some(original)
        } else if source_kind == ArtifactSourceKind::NativeReplay {
            Some(ensure_native_capture(
                session,
                provisional_fps,
                helper,
                request,
                progress,
            )?)
        } else {
            None
        };

        let source = match request.renderer {
            VideoRenderer::Native
            | VideoRenderer::ReplayNative
            | VideoRenderer::OriginalExecution => PresentationSourceRequest::Native,
            VideoRenderer::Semantic => PresentationSourceRequest::Semantic {
                fps: provisional_fps,
                raster_profile: request
                    .semantic_raster_profile
                    .unwrap_or(preset.default_raster_profile),
            },
        };

        progress(format_args!("Planning video from {}...", session.display()));

        let plan = plan_video(
            request,
            session,
            &preset,
            provisional_fps,
            source,
            capture_directory.as_deref(),
        )?;

        verify_source_selection(&plan, session, source_kind)?;
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

        if format.is_none() {
            require_render_capabilities(source)?;
        }

        progress(format_args!(
            "Rendering {} frames at {} FPS with the {} renderer...",
            plan.output_frame_count,
            plan.fps,
            source_name(&plan.source),
        ));

        if let Some(format) = format {
            let artifact = export_derivatives(
                &plan,
                session,
                &sidecar,
                &DerivativeExportOptions {
                    formats: vec![format],
                    source_kind,
                },
            )?;

            let video = sidecar.join(format.filename());
            publish_no_replace(&video, out).map_err(|error| {
                return invalid(format!(
                    "external publication failed: {error}; valid sidecar retained at {}",
                    sidecar.display()
                ));
            })?;

            return Ok(VideoOutcome::Derived {
                plan: Box::new(plan),
                output: out.clone(),
                artifact,
            });
        }

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
) -> Result<PathBuf> {
    let visual = session.join(VISUAL_DIRECTORY);
    if visual.join(VISUAL_MANIFEST).is_file() {
        // Do not relabel original pixels as replay or recapture over an invalid artifact.
        let path = visual.join(VISUAL_MANIFEST);
        let bytes = fs_err::read(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        let manifest: VisualManifest = serde_json::from_slice(&bytes).map_err(|source| {
            return Error::InvalidJson {
                path,
                json_path: "$".into(),
                source,
            };
        })?;

        if manifest.capture_kind == "video" {
            return Ok(visual);
        }
    }

    SystemService::require(NATIVE_VIDEO_CAPABILITY, &AtomicBool::new(false))?;

    match index_session_for_video(session, u32::from(fps)) {
        Ok(_) | Err(EngineError::ArtifactExists { .. }) => {}
        Err(error) => return Err(error.into()),
    }

    // A fresh sibling derivative, never remove/replace an existing capture (even incomplete).
    let parent = session
        .parent()
        .filter(|p| return !p.as_os_str().is_empty())
        .unwrap_or_else(|| return Path::new("."));

    let destination = parent.join(format!("native-replay-{}.termtrace", uuid::Uuid::now_v7()));
    progress(format_args!("Rendering canonical bytes in headless Ghostty (not rerunning the application), retained at {}...", destination.display()));

    let (target, geometry_solver) = native_capture_target(helper, session, request.resolution)?;
    capture(
        helper,
        &CaptureOptions {
            session_directory: session.to_path_buf(),
            width: target.width,
            height: target.height,
            padding: target.padding,
            fps,
            start_monotonic_ns: None,
            end_monotonic_ns: None,
            state_selection: StateSelection::All,
            geometry_solver,
            kind: CaptureKind::Video,
            destination: Some(destination.clone()),
            scale: None,
            dpi: None,
        },
    )?;

    return Ok(destination);
}

fn plan_video(
    request: &VideoRequest,
    session: &Path,
    preset: &PresetConfig,
    provisional_fps: u16,
    source: PresentationSourceRequest,
    capture_directory: Option<&Path>,
) -> Result<PresentationPlan> {
    if let Some(edit_path) = &request.edit {
        let edit = read_edit_plan(edit_path)?;

        return Ok(plan_with_capture_directory(
            session,
            &edit,
            source,
            capture_directory,
        )?);
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

    let first_plan = plan_with_capture_directory(session, &provisional, source, capture_directory)?;
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
    return Ok(plan_with_capture_directory(
        session,
        &edit,
        source,
        capture_directory,
    )?);
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
        SystemService::require(SEMANTIC_VIDEO_CAPABILITY, &AtomicBool::new(false))?;
    }

    return SystemService::require(PRESENTATION_CAPABILITY, &AtomicBool::new(false));
}

/// Replace `out` with the rendered video, hard-linking when the filesystem allows it.
fn publish(video: &Path, out: &Path) -> Result<()> {
    let filesystem = |source| {
        return Error::Filesystem {
            path: out.to_path_buf(),
            source,
        };
    };

    let parent = out
        .parent()
        .filter(|path| return !path.as_os_str().is_empty())
        .unwrap_or_else(|| return Path::new("."));
    let directory = fs_err::File::open(parent).map_err(filesystem)?;
    let staging = tempfile::Builder::new()
        .prefix(".video-")
        .tempdir_in(parent)
        .map_err(filesystem)?;

    let staged = staging.path().join("video.mp4");

    match fs_err::hard_link(video, &staged) {
        Ok(()) => {}
        Err(source) if source.kind() == IoErrorKind::CrossesDevices => {
            fs_err::copy(video, &staged).map_err(filesystem)?;
        }
        Err(source) => return Err(filesystem(source)),
    }

    fs_err::File::open(&staged)
        .map_err(filesystem)?
        .sync_all()
        .map_err(filesystem)?;
    fs_err::rename(&staged, out).map_err(filesystem)?;
    directory.sync_all().map_err(filesystem)?;

    return Ok(());
}

/// Same-parent staged file and atomic no-replace rename, unlike legacy MP4 publish.
fn publish_no_replace(video: &Path, out: &Path) -> Result<()> {
    use nix::{
        errno::Errno,
        fcntl::{open, openat, renameat2, OFlag, RenameFlags},
        sys::stat::Mode,
        unistd::{unlinkat, UnlinkatFlags},
    };
    use std::{fs::File, path::Component};
    let parent = out
        .parent()
        .filter(|p| return !p.as_os_str().is_empty())
        .unwrap_or_else(|| return Path::new("."));

    let filesystem = |source| {
        return Error::Filesystem {
            path: out.into(),
            source,
        };
    };

    let nix_error = |e: Errno| return filesystem(io::Error::from_raw_os_error(e as i32));
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut directory = open(
        if parent.is_absolute() { "/" } else { "." },
        flags,
        Mode::empty(),
    )
    .map_err(nix_error)?;

    // Descriptor-relative traversal rejects symlinks in every parent component.
    for component in parent.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                directory = openat(&directory, name, flags, Mode::empty()).map_err(nix_error)?;
            }
            _ => return Err(invalid("derivative output parent must not contain '..'")),
        }
    }

    let name = out
        .file_name()
        .ok_or_else(|| return invalid("output has no basename"))?;

    let temporary = format!(".video-{}.tmp", uuid::Uuid::now_v7());
    let fd = openat(
        &directory,
        temporary.as_str(),
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(nix_error)?;

    let primary = (|| {
        let mut destination = File::from(fd);
        let (mut source, _) = open_regular_file_nofollow(video)?;
        io::copy(&mut source, &mut destination).map_err(filesystem)?;
        destination.sync_all().map_err(filesystem)?;

        renameat2(
            &directory,
            temporary.as_str(),
            &directory,
            name,
            RenameFlags::RENAME_NOREPLACE,
        )
        .map_err(nix_error)?;

        File::from(directory.try_clone().map_err(filesystem)?)
            .sync_all()
            .map_err(|e| {
                return invalid(format!(
                    "video published at {} but parent sync failed: {e}",
                    out.display()
                ));
            })?;

        return Ok(());
    })();

    let cleanup = match unlinkat(&directory, temporary.as_str(), UnlinkatFlags::NoRemoveDir) {
        Ok(()) | Err(Errno::ENOENT) => Ok(()),
        Err(e) => Err(nix_error(e)),
    };

    match (primary, cleanup) {
        (Ok(()), Ok(())) => return Ok(()),
        (Err(primary), Err(cleanup)) => {
            return Err(invalid(format!(
                "{primary}; cleanup also failed: {cleanup}"
            )))
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => return Err(error),
    }
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
        source: None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_video_publication_preserves_the_previous_file() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let out = temporary.path().join("output.mp4");
        fs_err::write(&out, b"previous video").expect("previous output");

        let error = publish(&temporary.path().join("missing.mp4"), &out)
            .expect_err("missing source must fail");
        assert!(matches!(error, Error::Filesystem { source, .. }
            if source.kind() == IoErrorKind::NotFound));
        assert_eq!(
            fs_err::read(out).expect("previous output"),
            b"previous video"
        );
    }

    #[test]
    fn video_publication_preserves_an_existing_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let video = temporary.path().join("source.mp4");
        let out = temporary.path().join("output.mp4");
        fs_err::write(&video, b"new video").expect("source video");

        fs_err::create_dir(&out).expect("existing directory");
        fs_err::write(out.join("keep"), b"unrelated data").expect("directory contents");

        assert!(matches!(
            publish(&video, &out),
            Err(Error::Filesystem { .. })
        ));
        assert_eq!(
            fs_err::read(out.join("keep")).expect("directory contents remain"),
            b"unrelated data"
        );
    }

    #[test]
    fn video_publication_replaces_the_previous_file() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let video = temporary.path().join("source.mp4");
        let out = temporary.path().join("output.mp4");
        fs_err::write(&video, b"new complete video").expect("source video");

        fs_err::write(&out, b"previous video").expect("previous output");

        publish(&video, &out).expect("publish video");

        assert_eq!(
            fs_err::read(out).expect("published video"),
            b"new complete video"
        );
    }

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

#[cfg(test)]
mod derivative_publication_tests {
    use super::*;
    use std::{
        os::unix::fs::symlink,
        sync::{Arc, Barrier},
        thread,
    };

    #[test]
    fn no_replace_preserves_files_symlinks_directories_and_legacy_mp4_still_replaces() {
        let temp = tempfile::tempdir().unwrap();
        let video = temp.path().join("source");
        fs_err::write(&video, b"new").unwrap();
        for name in ["file.gif", "link.webm", "directory.gif"] {
            let out = temp.path().join(name);
            if name.starts_with("file") {
                fs_err::write(&out, b"old").unwrap();
            } else if name.starts_with("link") {
                symlink(&video, &out).unwrap();
            } else {
                fs_err::create_dir(&out).unwrap();
            }

            assert!(publish_no_replace(&video, &out).is_err());
        }

        assert_eq!(fs_err::read(temp.path().join("file.gif")).unwrap(), b"old");
        assert!(fs_err::symlink_metadata(temp.path().join("link.webm"))
            .unwrap()
            .is_symlink());
        assert!(temp.path().join("directory.gif").is_dir());

        publish(&video, &temp.path().join("file.gif")).unwrap(); // legacy only

        assert_eq!(fs_err::read(temp.path().join("file.gif")).unwrap(), b"new");
    }

    #[test]
    fn derivative_file_publication_rejects_symlink_parent_components() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("real");
        fs_err::create_dir(&target).unwrap();
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();
        let source = temp.path().join("source");
        fs_err::write(&source, b"pixels").unwrap();

        assert!(publish_no_replace(&source, &link.join("out.gif")).is_err());
        assert!(!target.join("out.gif").exists());
    }

    #[test]
    #[expect(
        clippy::needless_collect,
        reason = "spawn every racer before joining; a lazy iterator would deadlock the barrier"
    )]
    fn concurrent_derivative_publishers_have_exactly_one_winner() {
        let temp = tempfile::tempdir().unwrap();
        let out = temp.path().join("race.webm");
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|i| {
                let source = temp.path().join(format!("source-{i}"));
                fs_err::write(&source, [i]).unwrap();
                let out = out.clone();
                let barrier = Arc::clone(&barrier);

                return thread::spawn(move || {
                    barrier.wait();

                    return (i, publish_no_replace(&source, &out).is_ok());
                });
            })
            .collect();

        let results: Vec<_> = workers
            .into_iter()
            .map(|w| return w.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|(_, ok)| return *ok).count(), 1);

        let winner = results.iter().find(|(_, ok)| return *ok).unwrap().0;

        assert_eq!(fs_err::read(out).unwrap(), [winner]);
        assert!(fs_err::read_dir(temp.path()).unwrap().all(|e| return !e
            .unwrap()
            .file_name()
            .as_encoded_bytes()
            .starts_with(b".video-")));
    }
}
