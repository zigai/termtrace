use super::{
    capture::{preserve_failed, require_native_tools, validate_options},
    index::{validate_acquisition_frames, write_original_acquisition_map},
    media::{
        capture_screenshot, probe_screenshot, probe_variable_video, probe_video, spawn_video_ffmpeg,
    },
    portal,
    process::{
        ghostty_overrides, owned_command, probe_output, spawn_xvfb, stop_ffmpeg, wait_for_x_socket,
        DisplayLease, ManagedChild,
    },
    profile::materialize_profile,
    resources::capture_resources,
    util::{
        command_output, command_output_in_profile, external_error, outcome, secure_permissions,
    },
    validate_visual_manifest, Error, Result,
};
use std::{
    ffi::OsString,
    fs,
    io::Read as _,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use termtrace_evidence::{
    artifact::{
        append_artifact_root, collect_artifact_members, commit_directory, create_private_directory,
        secure_file, sync_directory, write_secure,
    },
    event_log::EventLogSummary,
    session::{read_completion, read_manifest, validate_completion_summary_candidate},
    Error as EvidenceError,
};
use termtrace_model::{
    artifact::visual::{
        CaptureKind, CaptureOptions, CaptureOutcome, VisualManifest, GHOSTTY_EFFECTIVE_CONFIG_NAME,
        VISUAL_MANIFEST_VERSION,
    },
    execution::session::SessionCompletion,
    value::geometry::TerminalGeometryReport,
};

/// Pixel acquisition backend for a managed original execution.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OriginalCaptureBackend {
    /// An isolated X11 display with a configured acquisition rate.
    #[default]
    PrivateX11,
    /// A user-selected Wayland window, with the compositor's variable frame timing.
    WaylandPortal,
}

/// Record a managed command inside Ghostty while acquiring the original X11 pixels.
///
/// The argument builder receives the isolated Ghostty configuration file. It must return
/// arguments for an attached `record --on-recording-error stop` invocation of `helper`,
/// writing the new session selected by `options`. The caller owns cancellation signalling.
/// No replay or semantic presentation claim is made by this capture.
///
/// # Errors
/// Returns an error on cancellation, invalid configuration, failed recording or native
/// acquisition, or publication failure. Failed acquisition evidence is preserved.
pub fn capture_original(
    helper: &Path,
    options: &CaptureOptions,
    profile: &[u8],
    cancelled: &AtomicBool,
    arguments: impl FnOnce(&Path) -> Vec<OsString>,
) -> Result<CaptureOutcome> {
    return capture_original_with_backend(
        helper,
        options,
        profile,
        cancelled,
        OriginalCaptureBackend::PrivateX11,
        arguments,
    );
}

/// Record original execution pixels using the selected native backend.
///
/// The Wayland backend opens a visible Ghostty window, waits for portal window
/// selection and the first acquired frame, then starts the command. The selected
/// window identity is user supplied and is not independently verified. Its frame
/// rate is compositor controlled; `options.fps` applies only to private X11.
///
/// # Errors
/// Returns an error on cancellation, missing dependencies, portal refusal,
/// incomplete recording or acquisition, or artifact publication failure.
pub fn capture_original_with_backend(
    helper: &Path,
    options: &CaptureOptions,
    profile: &[u8],
    cancelled: &AtomicBool,
    backend: OriginalCaptureBackend,
    arguments: impl FnOnce(&Path) -> Vec<OsString>,
) -> Result<CaptureOutcome> {
    validate_options(options)?;

    if !matches!(options.kind, CaptureKind::Video)
        || options.start_monotonic_ns.is_some()
        || options.end_monotonic_ns.is_some()
        || !options.session_directory.is_absolute()
        || !helper.is_absolute()
        || options.session_directory.exists()
    {
        return Err(external_error(
            "original capture",
            "requires a new session and full execution video",
        ));
    }

    let parent = options
        .session_directory
        .parent()
        .filter(|path| return path.is_dir())
        .ok_or_else(|| return external_error("original capture", "session parent must exist"))?;

    let destination = options
        .destination
        .clone()
        .unwrap_or_else(|| return options.session_directory.join("visual-original"));

    if destination.exists() {
        return Err(Error::ArtifactExists { path: destination });
    }

    let destination_parent = destination
        .parent()
        .filter(|parent| return parent.is_dir() || *parent == options.session_directory)
        .ok_or_else(|| {
            return external_error(
                "original capture",
                "destination parent must exist or be the new session",
            );
        })?;

    match backend {
        OriginalCaptureBackend::PrivateX11 => require_native_tools(&CaptureKind::Video)?,
        OriginalCaptureBackend::WaylandPortal => portal::require_tools()?,
    }

    let staging = parent.join(format!(".original-visual.{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;
    let result = capture_staged(
        helper, options, profile, cancelled, backend, arguments, &staging,
    );
    let (manifest, completion) = match result {
        Ok(manifest) => manifest,
        Err(error) => {
            // The attached recorder also receives terminal hangup when Ghostty is dropped.
            // Request a durable stop if its control socket is still reachable.
            let _ = probe_output(
                "stop original recording",
                Command::new(helper)
                    .arg("stop")
                    .arg(&options.session_directory)
                    .args(["--wait", "--timeout-ms", "3000"]),
                Duration::from_secs(4),
            );

            return Err(preserve_failed(parent, &staging, error));
        }
    };

    // Recording creates the session itself. Only after completion can the native
    // staging directory become its direct child for the common artifact commit protocol.
    let publication_staging =
        destination_parent.join(format!(".visual.{}.tmp", uuid::Uuid::now_v7()));

    if let Err(source) = fs::rename(&staging, &publication_staging) {
        return Err(preserve_failed(
            parent,
            &staging,
            Error::Filesystem {
                path: publication_staging,
                source,
            },
        ));
    }

    if let Err(error) = commit_directory(destination_parent, &publication_staging, &destination) {
        return Err(preserve_failed(
            destination_parent,
            &publication_staging,
            Error::from(error),
        ));
    }

    sync_directory(destination_parent)?;

    let session = fs::canonicalize(&options.session_directory).map_err(|source| {
        return Error::Filesystem {
            path: options.session_directory.clone(),
            source,
        };
    })?;
    let canonical_destination = fs::canonicalize(&destination).map_err(|source| {
        return Error::Filesystem {
            path: destination.clone(),
            source,
        };
    })?;

    if canonical_destination.starts_with(&session) {
        append_artifact_root(
            &session,
            &canonical_destination,
            "native_original_capture",
            &manifest,
            &completion.session_manifest_hash,
            Some(completion.final_event_hash.as_str()),
            None,
        )?;
    }

    return Ok(outcome(destination, &CaptureKind::Video));
}

fn capture_staged(
    helper: &Path,
    options: &CaptureOptions,
    profile: &[u8],
    cancelled: &AtomicBool,
    backend: OriginalCaptureBackend,
    arguments: impl FnOnce(&Path) -> Vec<OsString>,
    staging: &Path,
) -> Result<(Vec<u8>, SessionCompletion)> {
    let root = materialize_profile(&staging.join("ghostty-profile"), profile)?;
    let effective = command_output_in_profile("ghostty", &["+show-config"], Some(&root))?;
    let effective_hash = blake3::hash(&effective).to_hex().to_string();
    write_secure(&staging.join(GHOSTTY_EFFECTIVE_CONFIG_NAME), &effective)?;

    let ghostty_version =
        String::from_utf8_lossy(&command_output("ghostty", &["+version"])?).into_owned();
    let ffmpeg_version = String::from_utf8_lossy(&command_output("ffmpeg", &["-version"])?)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();

    let lease = if backend == OriginalCaptureBackend::PrivateX11 {
        Some(DisplayLease::acquire()?)
    } else {
        None
    };

    let display = lease
        .as_ref()
        .map(|lease| return format!(":{}", lease.number));

    let mut xvfb = if let Some(lease) = lease.as_ref() {
        let mut server = spawn_xvfb(Some(helper), lease, options.width, options.height, staging)?;
        wait_for_x_socket(lease, &mut server, Duration::from_secs(5))?;
        Some(server)
    } else {
        None
    };

    let video = staging.join("session.mkv");
    let mut ffmpeg = if let Some(display) = display.as_ref() {
        let mut capture = spawn_video_ffmpeg(options, staging, display, &video)?;
        wait_for_acquisition(&mut capture, staging, cancelled)?;
        Some(capture)
    } else {
        None
    };

    let finished = staging.join("recording-exit-status");
    let stderr = secure_file(&staging.join("ghostty.stderr.log"))?;
    let stdout = secure_file(&staging.join("ghostty.stdout.log"))?;

    let mut overrides = ghostty_overrides();

    if backend == OriginalCaptureBackend::WaylandPortal {
        overrides.retain(|arg| {
            return !arg.starts_with("--window-width=")
                && !arg.starts_with("--window-height=")
                && !arg.starts_with("--window-position-");
        });
        overrides.extend([
            "--window-width=100".to_owned(),
            "--window-height=30".to_owned(),
            "--title=Termtrace original capture".to_owned(),
        ]);
    }

    let gate = staging.join("execution-ready");

    if backend == OriginalCaptureBackend::PrivateX11 {
        write_secure(&gate, b"ready\n")?;
    }

    let mut command = if backend == OriginalCaptureBackend::PrivateX11 {
        let mut command = owned_command(helper, "dbus-run-session")?;
        command.arg("--").arg("ghostty");
        command
    } else {
        owned_command(helper, "ghostty")?
    };

    if let Some(display) = display.as_ref() {
        command
            .env("DISPLAY", display)
            .env("GDK_BACKEND", "x11")
            .env("LIBGL_ALWAYS_SOFTWARE", "1");
    } else {
        command.env("GDK_BACKEND", "wayland");
    }

    // All user paths and arguments are positional shell parameters, never shell source.
    let mut ghostty = ManagedChild::spawn_logged("ghostty", command
        .args(&overrides).arg("-e").arg("/bin/sh")
        .args(["-c", "finished=$1; log=$2; gate=$3; shift 3; while ! test -f \"$gate\"; do sleep 0.02; done; \"$@\" 2>\"$log\"; status=$?; printf '%s\\n' \"$status\" >\"$finished\"; while :; do sleep 1; done", "termtrace-original"])
        .arg(&finished).arg(staging.join("recorder.stderr.log")).arg(&gate).arg(helper)
        .args(arguments(&root.join("ghostty/config.ghostty")))
        .env("XDG_CONFIG_HOME", &root)
        .stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr)))?;

    if backend == OriginalCaptureBackend::WaylandPortal {
        let mut capture = portal::spawn(helper, options, staging)?;
        portal::wait_for_acquisition(&mut capture, &mut ghostty, staging, cancelled)?;
        write_secure(&gate, b"ready\n")?;

        ffmpeg = Some(capture);
    }

    let mut ffmpeg = ffmpeg.ok_or_else(|| {
        return external_error("original capture", "capture process was not started");
    })?;

    while !finished.is_file() {
        check_cancelled(cancelled)?;

        ghostty.check_running()?;
        ffmpeg.check_running()?;

        if let Some(xvfb) = xvfb.as_mut() {
            xvfb.check_running()?;
        }

        thread::sleep(Duration::from_millis(20));
    }

    let completion = read_completion(&options.session_directory)?;
    let summary =
        EventLogSummary::read_with_check(options.session_directory.join("events.jsonl"), || {
            if cancelled.load(Ordering::Relaxed) {
                return Err(EvidenceError::InvalidEvidence {
                    reason: "original capture cancelled".to_owned(),
                });
            }
            return Ok(());
        })?;

    validate_completion_summary_candidate(&options.session_directory, &summary, &completion)?;

    if completion.failure.is_some() {
        return Err(external_error(
            "original capture",
            "recorder reported an evidence failure",
        ));
    }

    let status = fs::read_to_string(&finished).map_err(|source| {
        return Error::Filesystem {
            path: finished.clone(),
            source,
        };
    })?;
    let recorder_status = status
        .trim()
        .parse::<u8>()
        .map_err(|error| return external_error("original capture", error))?;

    let expected_status = if completion.resource_limit.is_some() || completion.signal_name.is_some()
    {
        1
    } else {
        completion
            .exit_code
            .and_then(|code| return u8::try_from(code).ok())
            .unwrap_or(1)
    };

    if recorder_status != expected_status {
        return Err(external_error(
            "original capture",
            "recorder exit status does not match canonical completion",
        ));
    }

    let session = read_manifest(&options.session_directory)?;

    if session
        .ghostty_visual
        .as_ref()
        .map(|profile| return profile.effective_config_hash.as_str())
        != Some(effective_hash.as_str())
    {
        return Err(external_error(
            "original capture",
            "recorded configuration differs from original terminal configuration",
        ));
    }

    // Retain the terminal after command completion; this frame is an acquisition,
    // not an assertion that a particular semantic state has been presented.
    let screenshot = staging.join("final.png");

    if let Some(display) = display.as_ref() {
        capture_screenshot(options, staging, display, &screenshot)?;
    }

    capture_resources(&ghostty, staging, cancelled, "after_execution")?;

    stop_ffmpeg(&mut ffmpeg, Duration::from_secs(10), staging)?;
    ghostty.terminate();

    if let Some(xvfb) = xvfb.as_mut() {
        xvfb.terminate();
    }

    secure_permissions(&video)?;
    secure_permissions(&screenshot)?;

    let video_probe = match backend {
        OriginalCaptureBackend::PrivateX11 => {
            probe_video(&video, options.fps, options.width, options.height)?
        }
        OriginalCaptureBackend::WaylandPortal => {
            probe_variable_video(&video, options.width, options.height)?
        }
    };

    let packets = video_probe
        .pointer("/streams/0/nb_read_packets")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| return value.parse().ok())
        .ok_or_else(|| return external_error("original capture", "missing encoded frame count"))?;

    let capture_started = match backend {
        OriginalCaptureBackend::PrivateX11 => {
            validate_acquisition_frames(&staging.join("ffmpeg-video.log"), packets)?
        }
        OriginalCaptureBackend::WaylandPortal => portal::validate_completion(staging, packets)?,
    };

    if capture_started > session.started_unix_ns {
        return Err(external_error(
            "original capture",
            "first frame was acquired after execution began",
        ));
    }

    if backend == OriginalCaptureBackend::PrivateX11 {
        write_original_acquisition_map(
            &staging.join("acquisition-map.jsonl"),
            &staging.join("ffmpeg-video.log"),
            session.started_unix_ns,
        )?;
    }

    let correlation = match backend {
        OriginalCaptureBackend::PrivateX11 => "acquisition_clock_only_no_presentation_claim",
        OriginalCaptureBackend::WaylandPortal => {
            "pipewire_pts_and_collector_receipt_no_presentation_claim"
        }
    };

    write_secure(
        &staging.join("execution-timing.json"),
        format!(
            "{{\"execution_origin_unix_ns\":{},\"correlation\":\"{correlation}\"}}\n",
            session.started_unix_ns
        )
        .as_bytes(),
    )?;

    let geometry = session.geometry;
    let manifest = VisualManifest {
        version: VISUAL_MANIFEST_VERSION,
        session_id: session.session_id,
        source_event_hash: Some(completion.final_event_hash.clone()),
        source_range: None,
        state_selection: options.state_selection,
        geometry_report: TerminalGeometryReport {
            cols: geometry.cols,
            rows: geometry.rows,
            cell_width: geometry.pixel_width.checked_div(geometry.cols).unwrap_or(0),
            cell_height: geometry
                .pixel_height
                .checked_div(geometry.rows)
                .unwrap_or(0),
            text_width: geometry.pixel_width,
            text_height: geometry.pixel_height,
        },
        fps: if backend == OriginalCaptureBackend::WaylandPortal { 0 } else { options.fps },
        width: options.width,
        height: options.height,
        capture_started_unix_ns: capture_started,
        replay_origin_unix_ns: 0,
        effective_config_hash: effective_hash,
        ghostty_version,
        ffmpeg_version: Some(ffmpeg_version),
        ghostty_overrides: overrides,
        video_probe: Some(video_probe),
        screenshot_probe: probe_screenshot(&screenshot, options.width, options.height)?,
        members: collect_artifact_members(staging)?,
        capture_kind: match backend {
            OriginalCaptureBackend::PrivateX11 => "original_execution",
            OriginalCaptureBackend::WaylandPortal => "original_execution_wayland_portal",
        }.to_owned(),
        selected_state_id: None,
        selected_state_hash: None,
        endpoint_event_seq: None,
        endpoint_event_byte_offset: None,
        presentation_evidence: match backend {
            OriginalCaptureBackend::PrivateX11 => "original_execution_pixels_no_semantic_presentation_claim",
            OriginalCaptureBackend::WaylandPortal => "user_selected_window_pixels_no_verified_window_identity_or_semantic_presentation_claim",
        }.to_owned(),
        frame_samples: Vec::new(),
        guarantee_level: "native_capture_environment".to_owned(),
    };

    validate_visual_manifest(&manifest)?;
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("manifest.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    write_secure(&staging.join("manifest.json"), &bytes)?;
    return Ok((bytes, completion));
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(external_error("original capture", "capture cancelled"));
    }
    return Ok(());
}

fn wait_for_acquisition(
    ffmpeg: &mut ManagedChild,
    staging: &Path,
    cancelled: &AtomicBool,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        check_cancelled(cancelled)?;

        ffmpeg.check_running()?;

        let path = staging.join("ffmpeg-video.log");
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|file| return file.take(65536).read_to_end(&mut bytes))
            .map_err(|source| return Error::Filesystem { path, source })?;

        if String::from_utf8_lossy(&bytes).contains(" pts_time:") {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(external_error(
                "original capture",
                "timed out waiting for first acquired video frame",
            ));
        }

        thread::sleep(Duration::from_millis(20));
    }
}
