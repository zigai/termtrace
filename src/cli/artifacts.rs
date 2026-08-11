use super::*;

use termtrace::presentation::EDIT_PLAN_VERSION;

pub(super) fn run_export_asciicast(arguments: ExportArgs) -> Result<ExitCode> {
    let output = arguments.out;
    export_asciicast(arguments.session, &output)?;
    return write_json_line(&serde_json::json!({
        "kind": "asciicast",
        "output": ByteString::from(output.as_os_str().as_bytes()),
        "output_display": output.display().to_string(),
    }));
}

pub(super) fn run_video(arguments: VideoArgs) -> Result<ExitCode> {
    return match arguments.command {
        VideoCommand::Plan(arguments) => {
            let session = resolve_session_ref(arguments.session)?;
            let edit = EditPlan::read(arguments.edit)?;
            let source = video_source_request(arguments.source)?;
            write_json_line(&plan_presentation(session, &edit, source)?)
        }
        VideoCommand::Preview(arguments) => {
            let session = resolve_session_ref(arguments.session)?;
            let edit = EditPlan::read(arguments.edit)?;
            let source = video_source_request(arguments.source)?;
            let plan = plan_presentation(session, &edit, source)?;
            let outcome = preview_presentation(
                &plan,
                arguments.out,
                PreviewOptions {
                    columns: arguments.columns,
                    tile_width: arguments.tile_width,
                    max_frames: arguments.max_frames,
                    change_only: arguments.change_only,
                    preview_width: arguments.preview_width,
                },
            )?;
            write_json_line(&serde_json::json!({
                "kind": "presentation_preview",
                "directory": outcome.directory,
                "contact_sheet": outcome.contact_sheet,
                "video": outcome.video,
                "edit_plan_hash": plan.edit_plan_hash,
                "source": match &plan.source { PresentationSource::Native { .. } => "native", PresentationSource::Semantic { .. } => "semantic" },
            }))
        }
        VideoCommand::Export(arguments) => {
            let session = resolve_session_ref(arguments.session)?;
            let edit = EditPlan::read(arguments.edit)?;
            let source = video_source_request(arguments.source)?;
            let plan = plan_presentation(session, &edit, source)?;
            let outcome = export_presentation(&plan, arguments.out)?;
            write_json_line(&serde_json::json!({
                "kind": "presentation",
                "directory": outcome.directory,
                "video": outcome.video,
                "edit_plan_hash": plan.edit_plan_hash,
                "output_frame_count": plan.output_frame_count,
                "source": match &plan.source { PresentationSource::Native { .. } => "native", PresentationSource::Semantic { .. } => "semantic" },
            }))
        }
    };
}
const VIDEO_IDLE_THRESHOLD_NS: u64 = 2_000_000_000;
const VIDEO_IDLE_CONTEXT_NS: u64 = 500_000_000;

pub(super) fn run_video_facade(arguments: VideoFacadeArgs) -> Result<ExitCode> {
    if arguments.out.extension() != Some(OsStr::new("mp4")) {
        return Err(miette::miette!(
            "video output must use the `.mp4` extension"
        ));
    }
    if arguments.out.exists() {
        return Err(Error::ArtifactExists {
            path: arguments.out,
        }
        .into());
    }
    let sidecar = video_sidecar_path(&arguments.out);
    if sidecar.exists() {
        return Err(Error::ArtifactExists { path: sidecar }.into());
    }

    let resolved = resolve_session(arguments.session.clone())?;
    if resolved.lifecycle != RegistryLifecycle::Completed {
        return write_session_lifecycle(&resolved, "video", ExitCode::FAILURE);
    }
    let session = resolved.path;
    if arguments.renderer == VideoSourceArg::Native
        && !session.join("visual/manifest.json").is_file()
    {
        return Err(miette::miette!(
            "native renderer requires a verified capture; run `termtrace artifact capture --session {}` or explicitly choose `--renderer semantic`",
            session.display()
        ));
    }
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
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
    )?
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
    let (default_fps, default_hold_start, default_hold_end) = video_preset_timing(arguments.preset);
    let requested_fps = arguments.fps;
    let provisional_fps = requested_fps.unwrap_or(default_fps);
    let hold_start = arguments.hold_start.unwrap_or(default_hold_start);
    let hold_end = arguments.hold_end.unwrap_or(default_hold_end);
    let source = match arguments.renderer {
        VideoSourceArg::Native => PresentationSourceRequest::Native,
        VideoSourceArg::Semantic => PresentationSourceRequest::Semantic {
            fps: provisional_fps,
        },
    };

    write_progress(format_args!("Planning video from {}...", session.display()));
    let provisional = facade_edit_plan(&arguments, &ranges, provisional_fps, hold_start, hold_end)?;
    let first_plan = plan_presentation(&session, &provisional, source)?;
    if requested_fps.is_some_and(|fps| return fps != first_plan.fps) {
        return Err(miette::miette!(
            "requested {} FPS but the verified native capture is {} FPS; recapture at the requested rate",
            requested_fps.unwrap_or(first_plan.fps),
            first_plan.fps
        ));
    }
    let edit = facade_edit_plan(&arguments, &ranges, first_plan.fps, hold_start, hold_end)?;
    let plan = if edit == provisional {
        first_plan
    } else {
        plan_presentation(&session, &edit, source)?
    };

    write_progress(format_args!(
        "Rendering {} frames at {} FPS with the {} renderer...",
        plan.output_frame_count,
        plan.fps,
        match plan.source {
            PresentationSource::Native { .. } => "native",
            PresentationSource::Semantic { .. } => "semantic",
        }
    ));
    let outcome = export_presentation(&plan, &sidecar)?;
    fs_err::hard_link(&outcome.video, &arguments.out).map_err(|source| {
        return Error::Filesystem {
            path: arguments.out.clone(),
            source,
        };
    })?;
    write_progress("Published video and manifested render configuration.");
    return write_json_line(&serde_json::json!({
        "kind": "video",
        "output": ByteString::from(arguments.out.as_os_str().as_bytes()),
        "output_display": arguments.out.display().to_string(),
        "manifest": sidecar.join("manifest.json"),
        "artifact_directory": sidecar,
        "renderer": match plan.source { PresentationSource::Native { .. } => "native", PresentationSource::Semantic { .. } => "semantic" },
        "fps": plan.fps,
        "width": plan.output_width,
        "height": plan.output_height,
        "frame_count": plan.output_frame_count,
        "edit_plan_hash": plan.edit_plan_hash,
    }));
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

fn video_source_request(arguments: VideoSourceArgs) -> Result<PresentationSourceRequest> {
    return match arguments.source {
        VideoSourceArg::Native if arguments.fps.is_some() => Err(miette::miette!(
            "--fps configures semantic video only; native FPS comes from the capture manifest"
        )),
        VideoSourceArg::Native => Ok(PresentationSourceRequest::Native),
        VideoSourceArg::Semantic => {
            let fps = arguments.fps.unwrap_or(24);
            if !(1..=120).contains(&fps) {
                return Err(miette::miette!(
                    "semantic video FPS must be between 1 and 120"
                ));
            }
            Ok(PresentationSourceRequest::Semantic { fps })
        }
    };
}

pub(super) fn run_capture(arguments: CaptureArgs) -> Result<ExitCode> {
    let (start_monotonic_ns, end_monotonic_ns) =
        resolve_cli_range(&arguments.session, &arguments.range)?;
    if arguments.width.is_some() != arguments.height.is_some() {
        return Err(miette::miette!(
            "--width and --height must be supplied together"
        ));
    }
    require_capability(NATIVE_VIDEO_CAPABILITY)?;
    let (target, geometry_solver) = match (arguments.width, arguments.height) {
        (Some(width), Some(height)) => (CaptureTarget { width, height }, None),
        (None, None) => {
            let manifest = read_manifest(&arguments.session)?;
            let initial = manifest
                .geometry_provenance
                .capture_target
                .unwrap_or_else(|| {
                    return CaptureTarget {
                        width: manifest.geometry.pixel_width.saturating_add(128).max(320),
                        height: manifest.geometry.pixel_height.saturating_add(128).max(200),
                    };
                });
            let profile = capture_profile(&arguments.session, &manifest)?;
            let report = solve_ghostty_geometry(
                manifest.geometry.cols,
                manifest.geometry.rows,
                initial,
                &profile,
            )?;
            (report.selected, Some(report))
        }
        _ => {
            return Err(miette::miette!(
                "--width and --height must be supplied together"
            ));
        }
    };
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
    })?;
    return write_json_line(&serde_json::json!({
        "directory": outcome.directory,
        "video": outcome.video,
        "screenshot": outcome.screenshot,
    }));
}

fn capture_profile(session: &Path, manifest: &SessionManifest) -> Result<ResolvedGhosttyProfile> {
    let Some(provenance) = &manifest.ghostty_visual else {
        return Ok(resolve_ghostty_profile(
            None,
            GhosttyVisualOverrides::default(),
        )?);
    };
    if provenance.replay_profile != GHOSTTY_REPLAY_PROFILE_NAME {
        return Err(miette::miette!(
            "unsupported recorded Ghostty profile path `{}`",
            provenance.replay_profile
        ));
    }
    let path = session.join(GHOSTTY_REPLAY_PROFILE_NAME);
    let replay_profile = fs_err::read(&path).into_diagnostic()?;
    return restore_ghostty_profile(replay_profile, provenance.clone()).into_diagnostic();
}

pub(super) fn run_save(arguments: SaveArgs) -> Result<ExitCode> {
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
    )?;
    return write_json_line(&serde_json::json!({
        "directory": outcome.directory,
        "state_id": outcome.manifest.state_id,
        "state_hash": outcome.manifest.state_hash,
        "files": outcome.manifest.files,
        "manifest": outcome.directory.join("save-manifest.json"),
    }));
}

pub(super) fn run_screenshot(arguments: ScreenshotArgs) -> Result<ExitCode> {
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
        let log = EventLog::read(session.join("events.jsonl"))?;
        validate_completed_log(&session, &log)?;
        let inspection = inspect_point(&log, point)?;
        if !inspection.record.presented {
            return Err(miette::miette!(
                "native screenshots reject states that were not presentation eligible"
            ));
        }
        let event_seq = inspection.record.event_seq.ok_or_else(|| {
            return miette::miette!("the initial blank state has no native replay endpoint");
        })?;
        require_capability(NATIVE_SCREENSHOT_CAPABILITY)?;
        let (target, geometry_solver) = native_capture_target(&session, arguments.viewport)?;
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
        })?;
        return write_json_line(&serde_json::json!({
            "kind": "native_screenshot",
            "truth_layer": "native",
            "directory": outcome.directory,
            "image": outcome.screenshot,
            "manifest": outcome.directory.join("manifest.json"),
            "presentation_evidence": "post_endpoint_stable_capture",
        }));
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
        )?;
        let manifest_path = arguments.out.join("batch-manifest.json");
        return write_json_line(&serde_json::json!({
            "kind": "semantic_screenshot_batch",
            "truth_layer": "semantic_rendering",
            "directory": ByteString::from(arguments.out.as_os_str().as_bytes()),
            "directory_display": arguments.out.display().to_string(),
            "manifest": ByteString::from(manifest_path.as_os_str().as_bytes()),
            "manifest_display": manifest_path.display().to_string(),
            "entries": manifest.entries,
        }));
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
    )?;
    let image_path = outcome.directory.join(filename);
    let manifest_path = outcome.directory.join("save-manifest.json");
    return write_json_line(&serde_json::json!({
        "kind": "semantic_screenshot",
        "truth_layer": "semantic_rendering",
        "directory": ByteString::from(outcome.directory.as_os_str().as_bytes()),
        "directory_display": outcome.directory.display().to_string(),
        "image": ByteString::from(image_path.as_os_str().as_bytes()),
        "image_display": image_path.display().to_string(),
        "manifest": ByteString::from(manifest_path.as_os_str().as_bytes()),
        "manifest_display": manifest_path.display().to_string(),
        "state_hash": outcome.manifest.state_hash,
    }));
}

pub(super) fn run_screenshot_facade(arguments: ScreenshotFacadeArgs) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Completed {
        return write_session_lifecycle(&resolved, "screenshot", ExitCode::FAILURE);
    }
    let log = EventLog::read(resolved.path.join("events.jsonl"))?;
    validate_completed_log(&resolved.path, &log)?;
    let inspection = inspect_point(&log, &StatePoint::LatestPresented)?;
    let image = render_png(&inspection.state)?;
    write_secure(&arguments.out, &image)?;
    return write_json_line(&serde_json::json!({
        "kind": "semantic_screenshot",
        "truth_layer": "semantic_rendering",
        "file": ByteString::from(arguments.out.as_os_str().as_bytes()),
        "file_display": arguments.out.display().to_string(),
        "state_id": inspection.record.state_id,
        "state_hash": inspection.record.state_hash,
        "presentation_eligible": inspection.record.presented,
    }));
}

pub(super) fn run_bundle_facade(arguments: BundleFacadeArgs) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Completed {
        return write_session_lifecycle(&resolved, "bundle", ExitCode::FAILURE);
    }
    return run_bundle(BundleArgs {
        session: resolved.path,
        profile: BundleProfileArg::Screen,
        out: arguments.out,
        dry_run: false,
        archive: false,
    });
}

pub(super) fn run_bug_report_facade(arguments: BugReportFacadeArgs) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Completed {
        return write_session_lifecycle(&resolved, "bug_report", ExitCode::FAILURE);
    }
    let manifest = create_bug_report(&resolved.path, &arguments.out, arguments.screenshot)?;
    return write_json_line(&serde_json::json!({
        "kind": "bug_report",
        "directory": arguments.out,
        "manifest": manifest,
    }));
}

pub(super) fn native_capture_target(
    session: &Path,
    viewport: Option<CaptureTarget>,
) -> Result<(CaptureTarget, Option<GeometrySolveReport>)> {
    if let Some(target) = viewport {
        return Ok((target, None));
    }
    let manifest = read_manifest(session)?;
    let initial = manifest
        .geometry_provenance
        .capture_target
        .unwrap_or_else(|| {
            return CaptureTarget {
                width: manifest.geometry.pixel_width.saturating_add(128).max(320),
                height: manifest.geometry.pixel_height.saturating_add(128).max(200),
            };
        });
    let profile = capture_profile(session, &manifest)?;
    let report = solve_ghostty_geometry(
        manifest.geometry.cols,
        manifest.geometry.rows,
        initial,
        &profile,
    )?;
    return Ok((report.selected, Some(report)));
}

pub(super) fn run_bundle(arguments: BundleArgs) -> Result<ExitCode> {
    let profile = arguments.profile.into();
    if arguments.dry_run {
        return write_json_line(&plan_bundle(profile));
    }
    let session = resolve_session_ref(arguments.session)?;
    if arguments.archive {
        if arguments.out.exists() {
            return Err(Error::ArtifactExists {
                path: arguments.out,
            }
            .into());
        }
        let parent = arguments
            .out
            .parent()
            .ok_or_else(|| return miette::miette!("bundle archive path has no parent"))?;
        let staging = parent.join(format!(".bundle-source-{}.tmp", uuid::Uuid::now_v7()));
        let manifest = create_bundle(&session, &staging, profile)?;
        let policy = BundleVerificationPolicy::new(&session, profile);
        let result = archive_bundle(&staging, &arguments.out, &policy);
        let _cleanup = fs_err::remove_dir_all(&staging);
        result?;
        return write_json_line(&serde_json::json!({
            "kind": "bundle_archive",
            "profile": manifest.profile,
            "archive": arguments.out,
            "member_count": manifest.members.len(),
            "includes_canonical_evidence": manifest.includes_canonical_evidence,
        }));
    }
    let manifest = create_bundle(&session, &arguments.out, profile)?;
    return write_json_line(&serde_json::json!({
        "kind": "bundle",
        "directory": arguments.out,
        "manifest": manifest,
    }));
}

pub(super) fn run_bundle_verify(arguments: BundleVerifyArgs) -> Result<ExitCode> {
    let source = resolve_session_ref(arguments.source)?;
    let policy = BundleVerificationPolicy::new(source, arguments.profile.into());
    let manifest = if arguments.bundle.is_dir() {
        verify_bundle(&arguments.bundle, &policy)?
    } else {
        verify_bundle_archive(&arguments.bundle, &policy)?
    };
    return write_json_line(&serde_json::json!({
        "kind": "bundle_verification",
        "valid": true,
        "bundle": arguments.bundle,
        "manifest": manifest,
    }));
}
