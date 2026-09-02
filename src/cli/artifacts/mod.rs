use crate::cli::output::ResponseWriter;

pub(super) mod args;
use args::ArtifactSubcommand;

use std::{
    env,
    ffi::OsStr,
    io::{self, Read as _},
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, Stdio},
    time::Duration,
};

use miette::{IntoDiagnostic as _, Result};
use termtrace_engine::{
    Error,
    diff::inspect_point,
    timeline::{SourceRange, resolve_source_selection},
};
use termtrace_evidence::{
    artifact::write_secure,
    event_log::EventLog,
    session::{read_manifest, validate_completed_log},
};
use termtrace_export::{
    archive::{create as create_bundle_archive, verify as verify_bundle_archive},
    asciicast::export as export_asciicast,
    bug_report::create as create_bug_report,
    bundle::{create as create_bundle, plan as plan_bundle, verify_profile},
};
use termtrace_media::native_capture::{
    CaptureKind, CaptureOptions, capture, native_capture_target,
};
use termtrace_media::presentation::{
    EDIT_PLAN_VERSION, EditClip, EditPlan, EditSelector, PixelTransform, PresentationDelivery,
    PresentationSource, PresentationSourceRequest, PreviewOptions, RationalSpeed,
    VideoRenderConfig, export as export_presentation, plan_with_source as plan_presentation,
    preview as preview_presentation, read_edit_plan,
};
use termtrace_media::save::{save, save_screenshot_batch};
use termtrace_model::{
    artifact::bundle::BundleVerificationPolicy,
    artifact::save::{SaveFormat, SemanticRasterProfile},
    execution::event::Event,
    execution::session::CaptureTarget,
    protocol::capability::{
        NATIVE_SCREENSHOT_CAPABILITY, NATIVE_VIDEO_CAPABILITY, PRESENTATION_CAPABILITY,
        SEMANTIC_VIDEO_CAPABILITY,
    },
    registry::RegistryLifecycle,
    terminal::observation::StatePoint,
    terminal::replay::StateSelection,
    value::ByteString,
};

use crate::cli::{
    analysis::args::ShowRenderer,
    artifacts::args::{
        BugReportFacadeArgs, BundleArgs, BundleVerifyArgs, CaptureArgs, ExportArgs, ImportArgs,
        SaveArgs, ScreenshotArgs, ScreenshotFormat, SemanticRasterProfileArg, VideoCursorStyleArg,
        VideoFacadeArgs, VideoPresetArg, VideoSourceArg,
    },
    output::{write_json_line, write_progress},
    session_helpers::{
        resolve_cli_range, resolve_session, resolve_session_ref, selection, write_session_lifecycle,
    },
    tooling::require_cli_capability,
};

pub(super) fn run_export_asciicast(
    response: &mut ResponseWriter,
    arguments: ExportArgs,
) -> Result<ExitCode> {
    let output = arguments.out;
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
    let log = EventLog::read(arguments.out.join("events.jsonl")).into_diagnostic()?;
    validate_completed_log(&arguments.out, &log).into_diagnostic()?;
    let imported: Vec<u8> = log
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::Output { data } => return Some(data.as_bytes()),
            _ => return None,
        })
        .flatten()
        .copied()
        .collect();
    if imported != bytes {
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
    let mode_count = usize::from(arguments.out.is_some())
        + usize::from(arguments.dry_run)
        + usize::from(arguments.preview_out.is_some());
    if mode_count != 1 {
        return Err(miette::miette!(
            "exactly one of --out, --dry-run, or --preview-out is required"
        ));
    }
    let sidecar = if let Some(out) = arguments.out.as_ref() {
        if out.extension() != Some(OsStr::new("mp4")) {
            return Err(miette::miette!(
                "video output must use the `.mp4` extension"
            ));
        }
        if out.exists() {
            return Err(Error::ArtifactExists { path: out.clone() }).into_diagnostic();
        }
        let sidecar = video_sidecar_path(out);
        if sidecar.exists() {
            return Err(Error::ArtifactExists { path: sidecar }).into_diagnostic();
        }
        Some(sidecar)
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
    if arguments.renderer == VideoSourceArg::Native
        && !session.join("visual/manifest.json").is_file()
    {
        return Err(miette::miette!(
            "native renderer requires a verified capture manifest; run `termtrace artifact capture --session {}` after checking `termtrace doctor --require native-video --explain`, or explicitly choose `--renderer semantic`",
            session.display()
        ));
    }
    let (default_fps, default_hold_start, default_hold_end) = video_preset_timing(arguments.preset);
    let requested_fps = arguments.fps;
    let provisional_fps = requested_fps.unwrap_or(default_fps);
    let raster_profile =
        semantic_raster_profile(arguments.preset, arguments.semantic_raster_profile);
    let source = match arguments.renderer {
        VideoSourceArg::Native => PresentationSourceRequest::Native,
        VideoSourceArg::Semantic => PresentationSourceRequest::Semantic {
            fps: provisional_fps,
            raster_profile,
        },
    };

    write_progress(
        response,
        format_args!("Planning video from {}...", session.display()),
    );
    let plan = if let Some(edit_path) = arguments.edit.as_ref() {
        let edit = read_edit_plan(edit_path).into_diagnostic()?;
        plan_presentation(&session, &edit, source).into_diagnostic()?
    } else {
        let log = EventLog::read(session.join("events.jsonl")).into_diagnostic()?;
        validate_completed_log(&session, &log).into_diagnostic()?;
        let final_monotonic_ns = log
            .events
            .last()
            .map_or(0, |record| return record.monotonic_ns);
        let mut selected = resolve_source_selection(
            &log,
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
        let ranges = video_clip_ranges(&log, selected, arguments.trim_idle);
        let hold_start = arguments.hold_start.unwrap_or(default_hold_start);
        let hold_end = arguments.hold_end.unwrap_or(default_hold_end);
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
    fs_err::hard_link(&outcome.video, &output)
        .map_err(|source| {
            return Error::Filesystem {
                path: output.clone(),
                source,
            };
        })
        .into_diagnostic()?;
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
    let clips = facade_video_clips(ranges, fps, hold_start_ms, hold_end_ms)?;
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
    let delivery = video_preset_delivery(arguments.preset);
    return Ok(EditPlan {
        version: EDIT_PLAN_VERSION,
        clips,
        delivery,
        transforms,
        render_config: Some(VideoRenderConfig {
            preset: video_preset_name(arguments.preset).to_owned(),
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

fn video_preset_timing(preset: VideoPresetArg) -> (u16, Duration, Duration) {
    return match preset {
        VideoPresetArg::Docs => (30, Duration::from_millis(500), Duration::from_millis(1_000)),
        VideoPresetArg::Social => (60, Duration::from_millis(250), Duration::from_millis(750)),
        VideoPresetArg::Lossless => (60, Duration::ZERO, Duration::ZERO),
    };
}

fn semantic_raster_profile(
    preset: VideoPresetArg,
    explicit: Option<SemanticRasterProfileArg>,
) -> SemanticRasterProfile {
    if let Some(explicit) = explicit {
        return explicit.into();
    }
    return match preset {
        VideoPresetArg::Docs | VideoPresetArg::Social => SemanticRasterProfile::Direct,
        VideoPresetArg::Lossless => SemanticRasterProfile::Supersampled2x,
    };
}

fn video_preset_delivery(preset: VideoPresetArg) -> PresentationDelivery {
    return match preset {
        VideoPresetArg::Docs => PresentationDelivery {
            codec: "h264".to_owned(),
            pixel_format: "yuv420p".to_owned(),
            crf: 18,
            encoder_preset: "slow".to_owned(),
        },
        VideoPresetArg::Social => PresentationDelivery {
            codec: "h264".to_owned(),
            pixel_format: "yuv420p".to_owned(),
            crf: 20,
            encoder_preset: "medium".to_owned(),
        },
        VideoPresetArg::Lossless => PresentationDelivery {
            codec: "h264_rgb".to_owned(),
            pixel_format: "rgb24".to_owned(),
            crf: 0,
            encoder_preset: "veryslow".to_owned(),
        },
    };
}

fn video_preset_name(preset: VideoPresetArg) -> &'static str {
    return match preset {
        VideoPresetArg::Docs => "docs",
        VideoPresetArg::Social => "social",
        VideoPresetArg::Lossless => "lossless",
    };
}

fn duration_millis(duration: Duration, flag: &str) -> Result<u64> {
    return u64::try_from(duration.as_millis())
        .map_err(|_error| return miette::miette!("{flag} is too large"));
}

fn facade_video_clips(
    ranges: &[SourceRange],
    fps: u16,
    hold_start_ms: u64,
    hold_end_ms: u64,
) -> Result<Vec<EditClip>> {
    let mut clips: Vec<EditClip> = ranges
        .iter()
        .copied()
        .map(|range| return edit_clip(range, 0))
        .collect();
    if clips.is_empty() {
        return Err(miette::miette!("video range contains no source activity"));
    }
    if hold_start_ms > 0 {
        let frame_ns = 1_000_000_000_u64.div_ceil(u64::from(fps));
        let first = ranges.first().copied().ok_or_else(|| {
            return miette::miette!("video range contains no source activity");
        })?;
        let split = first
            .start_monotonic_ns
            .saturating_add(frame_ns)
            .min(first.end_monotonic_ns);
        *clips.first_mut().ok_or_else(|| {
            return miette::miette!("video range contains no source activity");
        })? = edit_clip(
            SourceRange {
                start_monotonic_ns: first.start_monotonic_ns,
                end_monotonic_ns: split,
            },
            hold_start_ms,
        );
        if split < first.end_monotonic_ns {
            clips.insert(
                1,
                edit_clip(
                    SourceRange {
                        start_monotonic_ns: split,
                        end_monotonic_ns: first.end_monotonic_ns,
                    },
                    0,
                ),
            );
        }
    }
    let last = clips
        .last_mut()
        .ok_or_else(|| return miette::miette!("video range contains no clips"))?;
    last.hold_ms = last.hold_ms.saturating_add(hold_end_ms);
    return Ok(clips);
}

fn edit_clip(range: SourceRange, hold_ms: u64) -> EditClip {
    return EditClip {
        from: EditSelector {
            monotonic_ns: Some(range.start_monotonic_ns),
            ..EditSelector::default()
        },
        to: EditSelector {
            monotonic_ns: Some(range.end_monotonic_ns),
            ..EditSelector::default()
        },
        speed: RationalSpeed::default(),
        hold_ms,
        caption: None,
    };
}

fn video_clip_ranges(log: &EventLog, selected: SourceRange, trim_idle: bool) -> Vec<SourceRange> {
    if !trim_idle {
        return vec![selected];
    }
    let activity: Vec<u64> = log
        .events
        .iter()
        .filter(|record| {
            return selected.contains(record.monotonic_ns)
                && matches!(&record.event, Event::Output { .. });
        })
        .map(|record| return record.monotonic_ns)
        .collect();
    let Some(first) = activity.first().copied() else {
        return vec![selected];
    };
    let mut ranges: Vec<SourceRange> = Vec::new();
    let mut start = first
        .saturating_sub(VIDEO_IDLE_CONTEXT_NS)
        .max(selected.start_monotonic_ns);
    let mut previous = first;
    for timestamp in activity.into_iter().skip(1) {
        if timestamp.saturating_sub(previous) > VIDEO_IDLE_THRESHOLD_NS {
            ranges.push(SourceRange {
                start_monotonic_ns: start,
                end_monotonic_ns: previous
                    .saturating_add(VIDEO_IDLE_CONTEXT_NS)
                    .min(selected.end_monotonic_ns),
            });
            start = timestamp
                .saturating_sub(VIDEO_IDLE_CONTEXT_NS)
                .max(selected.start_monotonic_ns);
        }
        previous = timestamp;
    }
    ranges.push(SourceRange {
        start_monotonic_ns: start,
        end_monotonic_ns: previous
            .saturating_add(VIDEO_IDLE_CONTEXT_NS)
            .min(selected.end_monotonic_ns),
    });
    return ranges;
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
    if arguments.width.is_some() != arguments.height.is_some() {
        return Err(miette::miette!(
            "--width and --height must be supplied together"
        ));
    }
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
    let (target, geometry_solver) =
        native_capture_target(&arguments.session, viewport).into_diagnostic()?;
    let outcome = capture(&CaptureOptions {
        session_directory: arguments.session,
        width: target.width,
        height: target.height,
        fps: arguments.fps,
        start_monotonic_ns,
        end_monotonic_ns,
        state_selection: selection(arguments.presented_only),
        geometry_solver,
        kind: CaptureKind::Video,
        destination: None,
    })
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
    let session = resolve_session_ref(arguments.session)?;
    let point = match (
        arguments.state,
        arguments.state_hash,
        arguments.event_seq,
        arguments.at,
        arguments.at_marker,
    ) {
        (Some(state_id), None, None, None, None) => StatePoint::StateId { state_id },
        (None, Some(state_hash), None, None, None) => StatePoint::StateHash { state_hash },
        (None, None, Some(event_seq), None, None) => StatePoint::EventSeq { event_seq },
        (None, None, None, Some(monotonic_ns), None) => StatePoint::MonotonicNs { monotonic_ns },
        (None, None, None, None, Some(label)) => StatePoint::Marker { label },
        (None, None, None, None, None) => StatePoint::LatestPresented,
        _ => return Err(miette::miette!("exactly one save selector may be supplied")),
    };
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
    let session = resolve_session_ref(arguments.session)?;
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
        if points.len() != 1 {
            return Err(miette::miette!(
                "native screenshot batches are not yet a single-process operation; select one state"
            ));
        }
        let point = points
            .first()
            .ok_or_else(|| return miette::miette!("missing native screenshot point"))?;
        let log = EventLog::read(session.join("events.jsonl")).into_diagnostic()?;
        validate_completed_log(&session, &log).into_diagnostic()?;
        let inspection = inspect_point(&log, point).into_diagnostic()?;
        if !inspection.record.presented {
            return Err(miette::miette!(
                "native screenshots reject states that were not presentation eligible"
            ));
        }
        let event_seq = inspection.record.event_seq.ok_or_else(|| {
            return miette::miette!("the initial blank state has no native replay endpoint");
        })?;
        require_cli_capability(NATIVE_SCREENSHOT_CAPABILITY)?;
        let (target, geometry_solver) =
            native_capture_target(&session, arguments.viewport).into_diagnostic()?;
        let outcome = capture(&CaptureOptions {
            session_directory: session,
            width: target.width,
            height: target.height,
            fps: 24,
            start_monotonic_ns: None,
            end_monotonic_ns: None,
            state_selection: StateSelection::Presented,
            geometry_solver,
            kind: CaptureKind::Screenshot {
                state_id: inspection.record.state_id,
                state_hash: inspection.record.state_hash,
                event_seq,
                event_byte_offset: inspection.record.event_byte_offset,
            },
            destination: Some(arguments.out),
        })
        .into_diagnostic()?;
        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "native_screenshot",
                "truth_layer": "native",
                "directory": outcome.directory,
                "image": outcome.screenshot,
                "manifest": outcome.directory.join("manifest.json"),
                "presentation_evidence": "post_endpoint_stable_capture",
            }),
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

pub(super) fn run_bug_report_facade(
    response: &mut ResponseWriter,
    arguments: BugReportFacadeArgs,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Completed {
        return write_session_lifecycle(response, &resolved, "bug_report", ExitCode::FAILURE);
    }
    let manifest = create_bug_report(&resolved.path, &arguments.out, arguments.screenshot)
        .into_diagnostic()?;
    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "bug_report",
            "directory": arguments.out,
            "manifest": manifest,
        }),
    );
}

pub(super) fn run_bundle(response: &mut ResponseWriter, arguments: BundleArgs) -> Result<ExitCode> {
    let profile = arguments.profile.into();
    if arguments.dry_run {
        return write_json_line(response, &plan_bundle(profile));
    }
    let session = resolve_session_ref(arguments.session)?;
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
        let manifest = create_bundle(&session, &staging, profile).into_diagnostic()?;
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
    let manifest = create_bundle(&session, &arguments.out, profile).into_diagnostic()?;
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
    let source = resolve_session_ref(arguments.source)?;
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
    fn native_video_editing_requires_presentation_without_capture_tooling() {
        assert_eq!(
            video_render_capability_ids(PresentationSourceRequest::Native),
            (None, PRESENTATION_CAPABILITY)
        );
        assert_eq!(
            video_render_capability_ids(PresentationSourceRequest::Semantic {
                fps: 24,
                raster_profile: SemanticRasterProfile::Direct,
            }),
            (Some(SEMANTIC_VIDEO_CAPABILITY), PRESENTATION_CAPABILITY)
        );
    }

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
