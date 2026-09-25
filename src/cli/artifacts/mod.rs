use crate::cli::output::ResponseWriter;
use termtrace_model::execution::layout::SessionLayout;

pub(super) mod args;
mod native_screenshot;

use crate::cli::{
    analysis::args::ShowRenderer,
    artifacts::args::{
        BundleArgs, BundleProfileArg, BundleVerifyArgs, CaptureArgs, ExportArgs, ImportArgs,
        SaveArgs, ScreenshotArgs, ScreenshotFormat, VideoFacadeArgs,
    },
    output::{write_json_line, write_progress, write_stdout},
    session_helpers::{
        resolve_cli_range, resolve_session, resolve_session_path, resolve_state_point,
        write_session_lifecycle,
    },
    tooling::{capability_report, require_cli_capability},
};
use args::ArtifactSubcommand;
use miette::{IntoDiagnostic as _, Result};
use std::{
    env,
    io::{self, Read as _},
    os::unix::ffi::OsStrExt as _,
    path::Path,
    process::{Command as ProcessCommand, ExitCode, Stdio},
};
use termtrace_engine::Error;
use termtrace_evidence::{
    artifact::write_secure,
    event_log::EventLogCursor,
    session::{read_manifest, validate_completed_session},
};
use termtrace_export::{
    archive::{create as create_bundle_archive, verify as verify_bundle_archive},
    asciicast::{export as export_asciicast, export_to_writer as export_asciicast_to_writer},
    bug_report::create as create_bug_report,
    bundle::{create as create_bundle, plan as plan_bundle, verify_profile},
};
use termtrace_media::native_capture::{
    capture, native_capture_target, CaptureKind, CaptureOptions,
};
use termtrace_media::presentation::PreviewOptions;
use termtrace_media::save::{save, save_screenshot_batch};
use termtrace_model::{
    artifact::bundle::BundleVerificationPolicy, artifact::save::SaveFormat,
    execution::event::Event, execution::session::CaptureTarget, execution::session::SessionPhase,
    protocol::capability::NATIVE_VIDEO_CAPABILITY, terminal::observation::StatePoint,
    terminal::replay::StateSelection, value::ByteString,
};
use termtrace_service::artifacts::{
    source_name, ArtifactService, VideoMode, VideoOutcome, VideoRequest,
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

    let mut cursor =
        EventLogCursor::open(SessionLayout::new(&arguments.out).event_log()).into_diagnostic()?;

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

pub(super) fn run_video_facade(
    response: &mut ResponseWriter,
    arguments: VideoFacadeArgs,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session.clone())?;
    if resolved.phase != SessionPhase::Completed {
        return write_session_lifecycle(response, &resolved, "video", ExitCode::FAILURE);
    }

    let mode = match (arguments.out, arguments.preview_out) {
        (Some(out), _) => VideoMode::Render { out },
        (None, Some(directory)) => VideoMode::Preview {
            directory,
            options: PreviewOptions {
                columns: arguments.columns,
                tile_width: arguments.tile_width,
                max_frames: arguments.max_frames,
                change_only: arguments.change_only,
                preview_width: arguments.preview_width,
            },
        },
        (None, None) => VideoMode::Plan,
    };

    let request = VideoRequest {
        edit: arguments.edit,
        preset: arguments.preset.into(),
        fps: arguments.fps,
        renderer: arguments.renderer.into(),
        semantic_raster_profile: arguments.semantic_raster_profile.map(Into::into),
        resolution: arguments.resolution,
        scale: arguments.scale,
        start_marker: arguments.start_marker,
        end_marker: arguments.end_marker,
        trim_idle: arguments.trim_idle,
        hold_start: arguments.hold_start,
        hold_end: arguments.hold_end,
        cursor_style: arguments.cursor_style.into(),
        ..VideoRequest::new(resolved.path, mode)
    };

    return render_video(response, &request);
}

/// Run a video request and write its outcome.
pub(super) fn render_video(
    response: &mut ResponseWriter,
    request: &VideoRequest,
) -> Result<ExitCode> {
    let helper = env::current_exe().into_diagnostic()?;
    let outcome = ArtifactService::video(request, &helper, &mut |message| {
        return write_progress(response, message);
    })
    .map_err(capability_report)?;

    return match outcome {
        VideoOutcome::Planned(plan) => write_json_line(response, &plan),
        VideoOutcome::Previewed { plan, preview } => write_json_line(
            response,
            &serde_json::json!({
                "kind": "presentation_preview",
                "directory": preview.directory,
                "contact_sheet": preview.contact_sheet,
                "video": preview.video,
                "edit_plan_hash": plan.edit_plan_hash,
                "source": source_name(&plan.source),
            }),
        ),
        VideoOutcome::Rendered {
            plan,
            output,
            sidecar,
        } => write_json_line(
            response,
            &serde_json::json!({
                "kind": "video",
                "output": ByteString::from(output.as_os_str().as_bytes()),
                "output_display": output.display().to_string(),
                "manifest": sidecar.join("manifest.json"),
                "artifact_directory": sidecar,
                "renderer": source_name(&plan.source),
                "fps": plan.fps,
                "width": plan.output_width,
                "height": plan.output_height,
                "frame_count": plan.output_frame_count,
                "edit_plan_hash": plan.edit_plan_hash,
            }),
        ),
    };
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
