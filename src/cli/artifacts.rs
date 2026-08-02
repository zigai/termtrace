use super::*;

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
        return write_json_line(&serde_json::json!({
            "kind": "semantic_screenshot_batch",
            "truth_layer": "semantic_rendering",
            "directory": arguments.out,
            "manifest": arguments.out.join("batch-manifest.json"),
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
    return write_json_line(&serde_json::json!({
        "kind": "semantic_screenshot",
        "truth_layer": "semantic_rendering",
        "directory": outcome.directory,
        "image": outcome.directory.join(filename),
        "manifest": outcome.directory.join("save-manifest.json"),
        "state_hash": outcome.manifest.state_hash,
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
