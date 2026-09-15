#![expect(
    dead_code,
    reason = "Legacy artifact subcommands preserved for test fixtures"
)]

use crate::cli::output::ResponseWriter;

pub(super) mod args;
mod native_screenshot;

use crate::cli::{
    analysis::args::ShowRenderer,
    artifacts::args::{
        BundleArgs, BundleProfileArg, BundleVerifyArgs, CaptureArgs, ExportArgs, ImportArgs,
        SaveArgs, ScreenshotArgs, ScreenshotFormat, SemanticRasterProfileArg, VideoCursorStyleArg,
        VideoFacadeArgs, VideoPresetArg, VideoSourceArg,
    },
    output::{write_json_line, write_progress, write_stdout},
    session_helpers::{
        resolve_cli_range, resolve_session, resolve_session_path, resolve_state_point,
        write_session_lifecycle,
    },
    tooling::require_cli_capability,
};
use args::ArtifactSubcommand;
use miette::{IntoDiagnostic as _, Result};
use std::{
    env,
    ffi::OsStr,
    io::{self, Read as _},
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, Stdio},
    time::Duration,
};
use termtrace_engine::{
    replay::index_session_for_video,
    timeline::{resolve_session_source_selection, SourceRange},
    Error,
};
use termtrace_evidence::{
    artifact::write_secure,
    event_log::{EventLogCursor, EventLogSummary},
    session::{read_manifest, validate_completed_session},
};
use termtrace_export::{
    archive::{create as create_bundle_archive, verify as verify_bundle_archive},
    asciicast::{export as export_asciicast, export_to_writer as export_asciicast_to_writer},
    bug_report::create as create_bug_report,
    bundle::{create as create_bundle, plan as plan_bundle, verify_profile},
};
use termtrace_media::native_capture::{
    capture, native_capture_target, CaptureKind, CaptureOptions, VISUAL_DIRECTORY,
};
use termtrace_media::presentation::{
    export as export_presentation, plan_with_source as plan_presentation,
    preview as preview_presentation, read_edit_plan, video_clip_ranges_session, video_clips,
    EditPlan, PixelTransform, PresentationDelivery, PresentationSource, PresentationSourceRequest,
    PreviewOptions, VideoIdlePolicy, VideoRenderConfig, EDIT_PLAN_VERSION,
};
use termtrace_media::save::{save, save_screenshot_batch};
use termtrace_model::{
    artifact::bundle::BundleVerificationPolicy,
    artifact::save::{SaveFormat, SemanticRasterProfile},
    execution::event::Event,
    execution::session::CaptureTarget,
    protocol::capability::{
        NATIVE_VIDEO_CAPABILITY, PRESENTATION_CAPABILITY, SEMANTIC_VIDEO_CAPABILITY,
    },
    registry::RegistryLifecycle,
    terminal::observation::StatePoint,
    terminal::replay::StateSelection,
    value::ByteString,
};

pub(super) fn run_export_asciicast(
    response: &mut ResponseWriter,
    arguments: ExportArgs,
) -> Result<ExitCode> {
    let output = arguments.out;
    if output == Path::new("-") {
        let mut buffer = Vec::new();
        export_asciicast_to_writer(arguments.session, &mut buffer, "<stdout>").into_diagnostic()?;

        return write_stdout(response, buffer);
    }

    export_asciicast(arguments.session, &output).into_diagnostic()?;

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "asciicast",
            "output": ByteString::from(output.as_os_str().as_bytes()),
            "output_display": output.display().to_string(),
        }),
    );
}

pub(super) fn run_import(response: &mut ResponseWriter, arguments: ImportArgs) -> Result<ExitCode> {
    let bytes = if arguments.input == Path::new("-") {
        let mut bytes: Vec<u8> = Vec::new();
        io::stdin().read_to_end(&mut bytes).into_diagnostic()?;
        bytes
    } else {
        fs_err::read(&arguments.input).into_diagnostic()?
    };

    if bytes.is_empty() {
        return Err(miette::miette!("ANSI import input must not be empty"));
    }

    if arguments.out.exists() {
        return Err(Error::ArtifactExists {
            path: arguments.out,
        })
        .into_diagnostic();
    }

    let parent = arguments
        .out
        .parent()
        .ok_or_else(|| miette::miette!("ANSI import destination has no parent"))?;

    fs_err::create_dir_all(parent).into_diagnostic()?;
    let input = parent.join(format!(".ansi-import-{}.tmp", uuid::Uuid::now_v7()));
    write_secure(&input, &bytes).into_diagnostic()?;
    let hash = blake3::hash(&bytes).to_hex().to_string();

    let process = ProcessCommand::new(env::current_exe().into_diagnostic()?)
        .env_remove("TERMTRACE_RESPONSE")
        .args(["record", "--no-input", "--out"])
        .arg(&arguments.out)
        .args([
            "--cols",
            &arguments.cols.to_string(),
            "--rows",
            &arguments.rows.to_string(),
        ])
        .args(["--import-source-kind", "ansi_stream"])
        .args(["--import-source-byte-count", &bytes.len().to_string()])
        .args(["--import-source-blake3", &hash])
        .arg("--import-source")
        .arg(arguments.input.as_os_str())
        .args([
            "--",
            "/bin/sh",
            "-c",
            "stty raw -echo < /dev/tty; exec /bin/cat -- \"$1\"",
            "termtrace-ansi-import",
        ])
        .arg(&input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    let cleanup = fs_err::remove_file(&input);
    let process = process.into_diagnostic()?;

    cleanup.into_diagnostic()?;

    if !process.status.success() {
        return Err(miette::miette!(
            "ANSI import recorder failed with {}: {}",
            process.status,
            String::from_utf8_lossy(&process.stderr)
        ));
    }

    validate_completed_session(&arguments.out).into_diagnostic()?;

    let mut cursor = EventLogCursor::open(arguments.out.join("events.jsonl")).into_diagnostic()?;
    let mut offset = 0_usize;

    while let Some(record) = cursor.next_record().into_diagnostic()? {
        if let Event::Output { data } = record.event {
            let end = offset.saturating_add(data.as_bytes().len());
            if bytes.get(offset..end) != Some(data.as_bytes()) {
                return Err(miette::miette!(
                    "ANSI import canonical output does not match the exact source bytes"
                ));
            }

            offset = end;
        }
    }

    if offset != bytes.len() {
        return Err(miette::miette!(
            "ANSI import canonical output does not match the exact source bytes"
        ));
    }

    let manifest = read_manifest(&arguments.out).into_diagnostic()?;
    if manifest
        .import_provenance
        .as_ref()
        .is_none_or(|provenance| {
            return provenance.source_kind != "ansi_stream"
                || provenance.byte_count != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                || provenance.blake3 != hash;
        })
    {
        return Err(miette::miette!(
            "ANSI import session did not bind its source provenance"
        ));
    }
    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "ansi_import",
            "session": arguments.out,
            "byte_count": bytes.len(),
            "blake3": hash,
        }),
    );
}

const VIDEO_IDLE_THRESHOLD_NS: u64 = 2_000_000_000;
const VIDEO_IDLE_CONTEXT_NS: u64 = 500_000_000;

pub(super) fn run_video_facade(
    response: &mut ResponseWriter,
    arguments: VideoFacadeArgs,
) -> Result<ExitCode> {
    let sidecar = if let Some(out) = arguments.out.as_ref() {
        if out.extension() != Some(OsStr::new("mp4")) {
            return Err(miette::miette!(
                "video output must use the `.mp4` extension"
            ));
        }

        Some(video_sidecar_path(out))
    } else {
        None
    };

    let resolved = resolve_session(arguments.session.clone())?;
    if resolved.lifecycle != RegistryLifecycle::Completed {
        return write_session_lifecycle(response, &resolved, "video", ExitCode::FAILURE);
    }

    let session = resolved.path;

    if arguments.renderer == VideoSourceArg::Native && arguments.semantic_raster_profile.is_some() {
        return Err(miette::miette!(
            "--semantic-raster-profile configures semantic video only"
        ));
    }

    let preset_config = VideoPresetConfig::from_preset(arguments.preset);
    let requested_fps = arguments.fps;
    let provisional_fps = requested_fps.unwrap_or(preset_config.fps);

    if arguments.renderer == VideoSourceArg::Native
        && !session.join("visual/manifest.json").is_file()
    {
        require_cli_capability(NATIVE_VIDEO_CAPABILITY)?;

        match index_session_for_video(&session, u32::from(provisional_fps)) {
            Ok(_) | Err(Error::ArtifactExists { .. }) => {}
            Err(error) => return Err(error).into_diagnostic(),
        }

        let visual_dir = session.join(VISUAL_DIRECTORY);
        if visual_dir.exists() && !visual_dir.join("manifest.json").is_file() {
            let _ = fs_err::remove_dir_all(&visual_dir);
        }

        write_progress(
            response,
            format_args!("Capturing native video for {}...", session.display()),
        );

        let helper = env::current_exe().into_diagnostic()?;
        let (target, geometry_solver) =
            native_capture_target(&helper, &session, arguments.resolution).into_diagnostic()?;

        capture(
            &helper,
            &CaptureOptions {
                session_directory: session.clone(),
                width: target.width,
                height: target.height,
                fps: provisional_fps,
                start_monotonic_ns: None,
                end_monotonic_ns: None,
                state_selection: StateSelection::All,
                geometry_solver,
                kind: CaptureKind::Video,
                destination: None,
                scale: None,
                dpi: None,
            },
        )
        .into_diagnostic()?;
    }

    let raster_profile = preset_config.raster_profile(arguments.semantic_raster_profile);
    let source = match arguments.renderer {
        VideoSourceArg::Native => PresentationSourceRequest::Native,
        VideoSourceArg::Semantic => PresentationSourceRequest::Semantic {
            fps: provisional_fps,
            raster_profile,
        },
    };

    let hold_start = arguments.hold_start.unwrap_or(preset_config.hold_start);
    let hold_end = arguments.hold_end.unwrap_or(preset_config.hold_end);

    write_progress(
        response,
        format_args!("Planning video from {}...", session.display()),
    );

    let plan = if let Some(edit_path) = arguments.edit.as_ref() {
        let edit = read_edit_plan(edit_path).into_diagnostic()?;

        plan_presentation(&session, &edit, source).into_diagnostic()?
    } else {
        validate_completed_session(&session).into_diagnostic()?;
        let summary = EventLogSummary::read(session.join("events.jsonl")).into_diagnostic()?;
        let final_monotonic_ns = summary
            .terminal
            .as_ref()
            .map_or(0, |record| return record.monotonic_ns);

        let mut selected = resolve_session_source_selection(
            &session,
            None,
            None,
            arguments.start_marker.as_deref(),
            arguments.end_marker.as_deref(),
        )
        .into_diagnostic()?
        .unwrap_or(SourceRange {
            start_monotonic_ns: 0,
            end_monotonic_ns: final_monotonic_ns,
        });
        selected.end_monotonic_ns = selected.end_monotonic_ns.min(final_monotonic_ns);

        if selected.start_monotonic_ns >= selected.end_monotonic_ns {
            return Err(miette::miette!(
                "video range must contain positive recorded time"
            ));
        }

        let ranges = video_clip_ranges_session(
            &session,
            selected,
            arguments.trim_idle.then_some(VideoIdlePolicy {
                threshold_ns: VIDEO_IDLE_THRESHOLD_NS,
                context_ns: VIDEO_IDLE_CONTEXT_NS,
            }),
        )
        .into_diagnostic()?;

        let provisional =
            facade_edit_plan(&arguments, &ranges, provisional_fps, hold_start, hold_end)?;
        let first_plan = plan_presentation(&session, &provisional, source).into_diagnostic()?;
        let edit = facade_edit_plan(&arguments, &ranges, first_plan.fps, hold_start, hold_end)?;

        if edit == provisional {
            first_plan
        } else {
            plan_presentation(&session, &edit, source).into_diagnostic()?
        }
    };

    if requested_fps.is_some_and(|fps| return fps != plan.fps) {
        return Err(miette::miette!(
            "requested {} FPS but the verified native capture is {} FPS; recapture at the requested rate",
            requested_fps.unwrap_or(plan.fps),
            plan.fps
        ));
    }

    if arguments.dry_run {
        return write_json_line(response, &plan);
    }

    require_video_render_capabilities(source)?;

    if let Some(preview_out) = arguments.preview_out {
        let outcome = preview_presentation(
            &plan,
            preview_out,
            PreviewOptions {
                columns: arguments.columns,
                tile_width: arguments.tile_width,
                max_frames: arguments.max_frames,
                change_only: arguments.change_only,
                preview_width: arguments.preview_width,
            },
        )
        .into_diagnostic()?;

        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "presentation_preview",
                "directory": outcome.directory,
                "contact_sheet": outcome.contact_sheet,
                "video": outcome.video,
                "edit_plan_hash": plan.edit_plan_hash,
                "source": match &plan.source {
                    PresentationSource::Native { .. } => "native",
                    PresentationSource::Semantic { .. } => "semantic",
                },
            }),
        );
    }

    let output = arguments
        .out
        .ok_or_else(|| miette::miette!("video export requires --out"))?;

    let sidecar = sidecar.ok_or_else(|| miette::miette!("video export sidecar is missing"))?;

    write_progress(
        response,
        format_args!(
            "Rendering {} frames at {} FPS with the {} renderer...",
            plan.output_frame_count,
            plan.fps,
            match &plan.source {
                PresentationSource::Native { .. } => "native",
                PresentationSource::Semantic { .. } => "semantic",
            }
        ),
    );

    let outcome = export_presentation(&plan, &sidecar).into_diagnostic()?;

    if output.exists() || output.symlink_metadata().is_ok() {
        if output.is_dir() {
            fs_err::remove_dir_all(&output)
                .map_err(|source| {
                    return Error::Filesystem {
                        path: output.clone(),
                        source,
                    };
                })
                .into_diagnostic()?;
        } else {
            fs_err::remove_file(&output)
                .map_err(|source| {
                    return Error::Filesystem {
                        path: output.clone(),
                        source,
                    };
                })
                .into_diagnostic()?;
        }
    }

    if let Err(source) = fs_err::hard_link(&outcome.video, &output) {
        if source.kind() == io::ErrorKind::CrossesDevices || source.raw_os_error() == Some(18) {
            fs_err::copy(&outcome.video, &output)
                .map_err(|source| {
                    return Error::Filesystem {
                        path: output.clone(),
                        source,
                    };
                })
                .into_diagnostic()?;
        } else {
            return Err(Error::Filesystem {
                path: output,
                source,
            })
            .into_diagnostic();
        }
    }

    write_progress(
        response,
        "Published video and manifested render configuration.",
    );

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "video",
            "output": ByteString::from(output.as_os_str().as_bytes()),
            "output_display": output.display().to_string(),
            "manifest": sidecar.join("manifest.json"),
            "artifact_directory": sidecar,
            "renderer": match &plan.source {
                PresentationSource::Native { .. } => "native",
                PresentationSource::Semantic { .. } => "semantic",
            },
            "fps": plan.fps,
            "width": plan.output_width,
            "height": plan.output_height,
            "frame_count": plan.output_frame_count,
            "edit_plan_hash": plan.edit_plan_hash,
        }),
    );
}

fn facade_edit_plan(
    arguments: &VideoFacadeArgs,
    ranges: &[SourceRange],
    fps: u16,
    hold_start: Duration,
    hold_end: Duration,
) -> Result<EditPlan> {
    let hold_start_ms = duration_millis(hold_start, "--hold-start")?;
    let hold_end_ms = duration_millis(hold_end, "--hold-end")?;
    let clips = video_clips(ranges, fps, hold_start_ms, hold_end_ms).into_diagnostic()?;

    let mut transforms: Vec<PixelTransform> = Vec::new();

    if arguments.cursor_style == VideoCursorStyleArg::Hidden {
        transforms.push(PixelTransform::CursorMask);
    }

    if let Some(resolution) = arguments.resolution {
        transforms.push(PixelTransform::Resize {
            width: resolution.width,
            height: resolution.height,
        });
    }

    if let Some(scale) = arguments.scale
        && scale != 1
    {
        transforms.push(PixelTransform::Zoom {
            numerator: scale,
            denominator: 1,
        });
    }

    let preset_config = VideoPresetConfig::from_preset(arguments.preset);
    let delivery = preset_config.delivery;

    return Ok(EditPlan {
        version: EDIT_PLAN_VERSION,
        clips,
        delivery,
        transforms,
        render_config: Some(VideoRenderConfig {
            preset: preset_config.name.to_owned(),
            renderer: match arguments.renderer {
                VideoSourceArg::Native => "native",
                VideoSourceArg::Semantic => "semantic",
            }
            .to_owned(),
            fps,
            resolution: arguments
                .resolution
                .map(|resolution| return [resolution.width, resolution.height]),
            scale: arguments.scale,
            start_marker: arguments.start_marker.clone(),
            end_marker: arguments.end_marker.clone(),
            trim_idle: arguments.trim_idle,
            idle_threshold_ms: arguments
                .trim_idle
                .then_some(VIDEO_IDLE_THRESHOLD_NS / 1_000_000),
            hold_start_ms,
            hold_end_ms,
            cursor_style: match arguments.cursor_style {
                VideoCursorStyleArg::Recorded => "recorded",
                VideoCursorStyleArg::Hidden => "hidden",
            }
            .to_owned(),
        }),
        ..EditPlan::default()
    });
}

struct VideoPresetConfig {
    name: &'static str,
    fps: u16,
    hold_start: Duration,
    hold_end: Duration,
    delivery: PresentationDelivery,
    default_raster_profile: SemanticRasterProfile,
}

impl VideoPresetConfig {
    fn from_preset(preset: VideoPresetArg) -> Self {
        match preset {
            VideoPresetArg::Docs => {
                return Self {
                    name: "docs",
                    fps: 30,
                    hold_start: Duration::from_millis(500),
                    hold_end: Duration::from_millis(1_000),
                    delivery: PresentationDelivery {
                        codec: "h264".to_owned(),
                        pixel_format: "yuv420p".to_owned(),
                        crf: 18,
                        encoder_preset: "slow".to_owned(),
                    },
                    default_raster_profile: SemanticRasterProfile::Direct,
                };
            }
            VideoPresetArg::Social => {
                return Self {
                    name: "social",
                    fps: 60,
                    hold_start: Duration::from_millis(250),
                    hold_end: Duration::from_millis(750),
                    delivery: PresentationDelivery {
                        codec: "h264".to_owned(),
                        pixel_format: "yuv420p".to_owned(),
                        crf: 20,
                        encoder_preset: "medium".to_owned(),
                    },
                    default_raster_profile: SemanticRasterProfile::Direct,
                };
            }
            VideoPresetArg::Lossless => {
                return Self {
                    name: "lossless",
                    fps: 60,
                    hold_start: Duration::ZERO,
                    hold_end: Duration::ZERO,
                    delivery: PresentationDelivery {
                        codec: "h264_rgb".to_owned(),
                        pixel_format: "rgb24".to_owned(),
                        crf: 0,
                        encoder_preset: "veryslow".to_owned(),
                    },
                    default_raster_profile: SemanticRasterProfile::Supersampled2x,
                };
            }
        }
    }

    fn raster_profile(&self, explicit: Option<SemanticRasterProfileArg>) -> SemanticRasterProfile {
        return explicit.map_or(self.default_raster_profile, Into::into);
    }
}

#[cfg(test)]
fn semantic_raster_profile(
    preset: VideoPresetArg,
    explicit: Option<SemanticRasterProfileArg>,
) -> SemanticRasterProfile {
    return VideoPresetConfig::from_preset(preset).raster_profile(explicit);
}

fn duration_millis(duration: Duration, flag: &str) -> Result<u64> {
    return u64::try_from(duration.as_millis())
        .map_err(|_error| return miette::miette!("{flag} is too large"));
}

fn video_sidecar_path(output: &Path) -> PathBuf {
    let mut sidecar = output.as_os_str().to_os_string();
    sidecar.push(".termtrace");
    let sidecar = PathBuf::from(sidecar);
    if sidecar
        .parent()
        .is_some_and(|parent| return !parent.as_os_str().is_empty())
    {
        return sidecar;
    }
    return Path::new(".").join(sidecar);
}

fn video_render_capability_ids(
    source: PresentationSourceRequest,
) -> (Option<&'static str>, &'static str) {
    return (
        match source {
            PresentationSourceRequest::Native => None,
            PresentationSourceRequest::Semantic { .. } => Some(SEMANTIC_VIDEO_CAPABILITY),
        },
        PRESENTATION_CAPABILITY,
    );
}

fn require_video_render_capabilities(source: PresentationSourceRequest) -> Result<()> {
    let (source_capability, presentation_capability) = video_render_capability_ids(source);
    if let Some(source_capability) = source_capability {
        require_cli_capability(source_capability)?;
    }

    return require_cli_capability(presentation_capability);
}

pub(super) fn run_capture(
    response: &mut ResponseWriter,
    arguments: CaptureArgs,
) -> Result<ExitCode> {
    let (start_monotonic_ns, end_monotonic_ns) =
        resolve_cli_range(&arguments.session, &arguments.range)?;

    require_cli_capability(NATIVE_VIDEO_CAPABILITY)?;

    let viewport = match (arguments.width, arguments.height) {
        (Some(width), Some(height)) => Some(CaptureTarget { width, height }),
        (None, None) => None,
        _ => {
            return Err(miette::miette!(
                "--width and --height must be supplied together"
            ));
        }
    };

    let helper = env::current_exe().into_diagnostic()?;
    let (target, geometry_solver) =
        native_capture_target(&helper, &arguments.session, viewport).into_diagnostic()?;

    let outcome = capture(
        &helper,
        &CaptureOptions {
            session_directory: arguments.session,
            width: target.width,
            height: target.height,
            fps: arguments.fps,
            start_monotonic_ns,
            end_monotonic_ns,
            state_selection: if arguments.presented_only {
                StateSelection::Presented
            } else {
                StateSelection::All
            },
            geometry_solver,
            kind: CaptureKind::Video,
            destination: None,
            scale: None,
            dpi: None,
        },
    )
    .into_diagnostic()?;

    return write_json_line(
        response,
        &serde_json::json!({
            "directory": outcome.directory,
            "video": outcome.video,
            "screenshot": outcome.screenshot,
        }),
    );
}

pub(super) fn run_save(response: &mut ResponseWriter, arguments: SaveArgs) -> Result<ExitCode> {
    let session = resolve_session_path(arguments.session)?;
    let point = resolve_state_point(
        arguments.state,
        arguments.state_hash,
        arguments.event_seq,
        arguments.at,
        arguments.at_marker,
        Some(StatePoint::LatestPresented),
        "save",
    )?;

    let formats = arguments
        .format
        .into_iter()
        .map(SaveFormat::from)
        .collect::<Vec<_>>();

    let outcome = save(
        &session,
        &arguments.out,
        point,
        &formats,
        arguments.include_hidden,
        None,
    )
    .into_diagnostic()?;

    return write_json_line(
        response,
        &serde_json::json!({
            "directory": outcome.directory,
            "state_id": outcome.manifest.state_id,
            "state_hash": outcome.manifest.state_hash,
            "files": outcome.manifest.files,
            "manifest": outcome.directory.join("save-manifest.json"),
        }),
    );
}

pub(super) fn run_screenshot(
    response: &mut ResponseWriter,
    arguments: ScreenshotArgs,
) -> Result<ExitCode> {
    let session = resolve_session_path(arguments.session)?;
    let mut points = Vec::new();
    points.extend(
        arguments
            .state
            .into_iter()
            .map(|state_id| return StatePoint::StateId { state_id }),
    );
    points.extend(
        arguments
            .state_hash
            .into_iter()
            .map(|state_hash| return StatePoint::StateHash { state_hash }),
    );
    points.extend(
        arguments
            .event_seq
            .into_iter()
            .map(|event_seq| return StatePoint::EventSeq { event_seq }),
    );
    points.extend(
        arguments
            .at
            .into_iter()
            .map(|monotonic_ns| return StatePoint::MonotonicNs { monotonic_ns }),
    );
    points.extend(
        arguments
            .at_marker
            .into_iter()
            .map(|label| return StatePoint::Marker { label }),
    );

    if points.is_empty() {
        points.push(StatePoint::LatestPresented);
    }

    if arguments.renderer == ShowRenderer::Native {
        if arguments.format != ScreenshotFormat::Png {
            return Err(miette::miette!(
                "native screenshots are PNG; semantic SVG fallback is disabled"
            ));
        }

        if arguments.include_hidden {
            return Err(miette::miette!(
                "native screenshots cannot render hidden synchronized-output states"
            ));
        }
        return native_screenshot::run(
            response,
            &session,
            &points,
            &arguments.out,
            arguments.viewport,
        );
    }

    if arguments.viewport.is_some() || arguments.strict {
        return Err(miette::miette!(
            "native viewport options require --renderer native"
        ));
    }

    let format = match arguments.format {
        ScreenshotFormat::Svg => SaveFormat::Svg,
        ScreenshotFormat::Png => SaveFormat::Png,
    };

    let filename = match arguments.format {
        ScreenshotFormat::Svg => "screen.svg",
        ScreenshotFormat::Png => "screen.png",
    };

    if points.len() > 1 {
        let manifest = save_screenshot_batch(
            &session,
            &arguments.out,
            &points,
            format,
            arguments.include_hidden,
        )
        .into_diagnostic()?;

        let manifest_path = arguments.out.join("batch-manifest.json");

        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "semantic_screenshot_batch",
                "truth_layer": "semantic_rendering",
                "directory": ByteString::from(arguments.out.as_os_str().as_bytes()),
                "directory_display": arguments.out.display().to_string(),
                "manifest": ByteString::from(manifest_path.as_os_str().as_bytes()),
                "manifest_display": manifest_path.display().to_string(),
                "entries": manifest.entries,
            }),
        );
    }

    let point = points
        .into_iter()
        .next()
        .ok_or_else(|| return miette::miette!("missing screenshot point"))?;

    let outcome = save(
        &session,
        &arguments.out,
        point,
        &[format],
        arguments.include_hidden,
        None,
    )
    .into_diagnostic()?;

    let image_path = outcome.directory.join(filename);
    let manifest_path = outcome.directory.join("save-manifest.json");

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "semantic_screenshot",
            "truth_layer": "semantic_rendering",
            "directory": ByteString::from(outcome.directory.as_os_str().as_bytes()),
            "directory_display": outcome.directory.display().to_string(),
            "image": ByteString::from(image_path.as_os_str().as_bytes()),
            "image_display": image_path.display().to_string(),
            "manifest": ByteString::from(manifest_path.as_os_str().as_bytes()),
            "manifest_display": manifest_path.display().to_string(),
            "state_hash": outcome.manifest.state_hash,
        }),
    );
}

pub(super) fn run_bundle(response: &mut ResponseWriter, arguments: BundleArgs) -> Result<ExitCode> {
    let profile = arguments.profile.into();

    if arguments.dry_run {
        return write_json_line(response, &plan_bundle(profile));
    }

    let session = resolve_session_path(arguments.session)?;

    if arguments.archive {
        if arguments.out.exists() {
            return Err(Error::ArtifactExists {
                path: arguments.out,
            })
            .into_diagnostic();
        }

        let parent = arguments
            .out
            .parent()
            .ok_or_else(|| return miette::miette!("bundle archive path has no parent"))?;

        let staging = parent.join(format!(".bundle-source-{}.tmp", uuid::Uuid::now_v7()));
        let manifest = create_bundle(&session, &staging, profile, None).into_diagnostic()?;
        let policy = BundleVerificationPolicy::new(&session, profile);
        let result = create_bundle_archive(&staging, &arguments.out, &policy);
        let _cleanup = fs_err::remove_dir_all(&staging);

        result.into_diagnostic()?;

        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "bundle_archive",
                "profile": manifest.profile,
                "archive": arguments.out,
                "member_count": manifest.members.len(),
                "includes_canonical_evidence": manifest.includes_canonical_evidence,
            }),
        );
    }

    if arguments.profile == BundleProfileArg::Debug {
        let manifest =
            create_bug_report(&session, &arguments.out, arguments.screenshot).into_diagnostic()?;

        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "bundle",
                "profile": "debug",
                "directory": arguments.out,
                "manifest": manifest,
            }),
        );
    }

    let manifest = create_bundle(&session, &arguments.out, profile, None).into_diagnostic()?;

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "bundle",
            "directory": arguments.out,
            "manifest": manifest,
        }),
    );
}

pub(super) fn run_bundle_verify(
    response: &mut ResponseWriter,
    arguments: BundleVerifyArgs,
) -> Result<ExitCode> {
    let source = resolve_session_path(arguments.source)?;
    let policy = BundleVerificationPolicy::new(source, arguments.profile.into());
    let manifest = if arguments.bundle.is_dir() {
        verify_profile(&arguments.bundle, &policy).into_diagnostic()?
    } else {
        verify_bundle_archive(&arguments.bundle, &policy).into_diagnostic()?
    };

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "bundle_verification",
            "valid": true,
            "bundle": arguments.bundle,
            "manifest": manifest,
        }),
    );
}

pub(super) fn dispatch(
    response: &mut ResponseWriter,
    command: ArtifactSubcommand,
) -> Result<ExitCode> {
    return match command {
        ArtifactSubcommand::Import(arguments) => run_import(response, arguments),
        ArtifactSubcommand::Capture(arguments) => run_capture(response, arguments),
        ArtifactSubcommand::Save(arguments) => run_save(response, arguments),
        ArtifactSubcommand::Asciicast(arguments) => run_export_asciicast(response, arguments),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_raster_profiles_follow_source_and_preset_precedence() {
        assert_eq!(
            semantic_raster_profile(VideoPresetArg::Docs, None),
            SemanticRasterProfile::Direct
        );
        assert_eq!(
            semantic_raster_profile(VideoPresetArg::Social, None),
            SemanticRasterProfile::Direct
        );
        assert_eq!(
            semantic_raster_profile(VideoPresetArg::Lossless, None),
            SemanticRasterProfile::Supersampled2x
        );
        assert_eq!(
            semantic_raster_profile(
                VideoPresetArg::Lossless,
                Some(SemanticRasterProfileArg::Direct),
            ),
            SemanticRasterProfile::Direct
        );
        assert_eq!(
            semantic_raster_profile(
                VideoPresetArg::Docs,
                Some(SemanticRasterProfileArg::Supersampled2x),
            ),
            SemanticRasterProfile::Supersampled2x
        );
    }
}
