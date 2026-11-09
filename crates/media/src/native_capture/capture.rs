use super::{
    capture_io::{
        command_output, command_output_in_profile, outcome, secure_permissions, unix_now_ns,
    },
    geometry::validate_capture,
    index::{
        validate_acquisition_frames, validate_replay_manifest, write_video_map, StateIndexSource,
    },
    media::{
        capture_screenshot, capture_stable_x11_frame, probe_screenshot, probe_video,
        spawn_video_ffmpeg,
    },
    process::{
        ghostty_overrides, spawn_ghostty, spawn_xvfb, stop_ffmpeg, wait_for_file,
        wait_for_playback_handshake, wait_for_success, wait_for_x_socket, DisplayLease,
        GhosttyEnvironment, PlaybackFiles,
    },
    profile::materialize,
    validate_visual_manifest, Error, Result,
};
use crate::error::finish_with_cleanup;
use std::{
    env, fs, path::Path, process::Command, str, sync::atomic::AtomicBool, thread, time::Duration,
};
use termtrace_control::read_completed;
use termtrace_engine::{
    recording::verify_profile_fonts, replay::visit_session_states_while,
    timeline::resolve_source_range,
};
use termtrace_evidence::{
    artifact::{
        append_artifact_root, collect_artifact_members, commit_directory, create_private_directory,
        sync_directory, verify_artifact_members, write_secure,
    },
    event_log::EventLogSummary,
    session::{read_manifest, validate_completion_summary_candidate},
};
use termtrace_model::execution::layout;
use termtrace_model::{
    artifact::metadata::ArtifactSourceRange,
    artifact::playback::PlaybackTiming,
    artifact::visual::{
        visual_directory_name_for, CaptureKind, CaptureOptions, CaptureOutcome, VisualManifest,
        GHOSTTY_EFFECTIVE_CONFIG_NAME, VISUAL_MANIFEST_VERSION,
    },
    execution::layout::SessionLayout,
    execution::session::{CaptureTarget, GeometrySource, SessionManifest},
    terminal::replay::{SourceRange, StateSelection},
};

/// Capture a completed session in a private display and atomically publish its visual artifacts.
///
/// `helper` is the Termtrace executable providing the playback subprocess protocol.
/// The selected capture kind, source range, profile, geometry, and output location are taken
/// from `options`. Existing destinations are never replaced.
///
/// # Errors
///
/// Returns an error when the session or options are invalid, a required tool is unavailable,
/// native playback or capture fails, or the resulting artifacts cannot be verified and published.
pub fn capture(helper: &Path, options: &CaptureOptions) -> Result<CaptureOutcome> {
    validate_options(options)?;
    let (log, range) = completed_source(
        &options.session_directory,
        options.start_monotonic_ns,
        options.end_monotonic_ns,
    )?;
    let states = selected_states(options, &log, range)?;
    let destination = options
        .destination
        .clone()
        .unwrap_or_else(|| match &options.kind {
            CaptureKind::Video => {
                return options
                    .session_directory
                    .join(visual_directory_name_for(range, options.state_selection));
            }
            CaptureKind::Screenshot { state_id, .. } => {
                return options
                    .session_directory
                    .join(format!("native-screenshot-state-{state_id}"));
            }
        });

    if destination.exists() {
        return Err(Error::ArtifactExists { path: destination });
    }

    let parent = destination
        .parent()
        .filter(|parent| return parent.is_dir())
        .ok_or_else(|| {
            return Error::InvalidReplay {
                reason: "native capture destination parent does not exist".to_owned(),
                source: None,
            };
        })?;

    require_native_tools(&options.kind)?;
    let staging = parent.join(format!(".visual.{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;

    if let Err(error) = capture_staged(helper, options, &log, states.as_ref(), range, &staging) {
        return Err(preserve_failed(parent, &staging, error));
    }

    if let Err(error) = commit_directory(parent, &staging, &destination, false)
        .and_then(|()| return sync_directory(parent))
    {
        let cleanup = fs::remove_dir_all(&staging).map_err(|source| {
            return Error::Filesystem {
                path: staging,
                source,
            };
        });

        return finish_with_cleanup(Err(Error::from(error)), cleanup);
    }

    let manifest_bytes = fs::read(destination.join("manifest.json")).map_err(|source| {
        return Error::Filesystem {
            path: destination.join("manifest.json"),
            source,
        };
    })?;

    let canonical_session = fs::canonicalize(&options.session_directory).map_err(|source| {
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

    if canonical_destination.starts_with(&canonical_session) {
        append_artifact_root(
            &canonical_session,
            &canonical_destination,
            if matches!(&options.kind, CaptureKind::Video) {
                "native_capture"
            } else {
                "native_screenshot"
            },
            &manifest_bytes,
            &log.header.session_manifest_hash,
            log.terminal
                .as_ref()
                .map(|record| return record.hash.as_str()),
            range.map(|range| {
                return ArtifactSourceRange {
                    start_monotonic_ns: range.start_monotonic_ns,
                    end_monotonic_ns: range.end_monotonic_ns,
                };
            }),
        )?;
    }

    return Ok(outcome(destination, &options.kind));
}

/// Check that a completed session has the semantic index a native video capture of the
/// selected range needs, without touching native tools.
///
/// # Errors
///
/// Returns an error when the session is incomplete or invalid, the range does not resolve, or the
/// matching semantic index is missing or stale.
pub fn check_video_source(
    session: &Path,
    start_monotonic_ns: Option<u64>,
    end_monotonic_ns: Option<u64>,
    selection: StateSelection,
) -> Result<()> {
    let (log, range) = completed_source(session, start_monotonic_ns, end_monotonic_ns)?;
    validate_replay_manifest(session, &log, range, selection)?;

    return Ok(());
}

fn completed_source(
    session: &Path,
    start_monotonic_ns: Option<u64>,
    end_monotonic_ns: Option<u64>,
) -> Result<(EventLogSummary, Option<SourceRange>)> {
    let Some(completion) = read_completed(session)? else {
        return Err(Error::SessionIncomplete {
            session: session.to_path_buf(),
        });
    };

    let log = EventLogSummary::read(SessionLayout::new(session).event_log())?;
    validate_completion_summary_candidate(session, &log, &completion)?;

    let final_ns = log
        .terminal
        .as_ref()
        .map_or(0, |record| return record.monotonic_ns);

    let range = resolve_source_range(start_monotonic_ns, end_monotonic_ns, final_ns)?;

    return Ok((log, range));
}

fn selected_states(
    options: &CaptureOptions,
    log: &EventLogSummary,
    range: Option<SourceRange>,
) -> Result<Option<StateIndexSource>> {
    match &options.kind {
        CaptureKind::Video => {
            return validate_replay_manifest(
                &options.session_directory,
                log,
                range,
                options.state_selection,
            )
            .map(Some);
        }
        CaptureKind::Screenshot {
            state_id,
            state_hash,
            event_seq,
            event_byte_offset,
        } => {
            if range.is_some() {
                return Err(Error::InvalidReplay {
                    reason: "native screenshot endpoints cannot use a time range".to_owned(),
                    source: None,
                });
            }

            let mut selected = None;

            visit_session_states_while(
                &options.session_directory,
                &AtomicBool::new(false),
                |record, _| {
                    if record.state_id == *state_id {
                        selected = Some(record.clone());
                    }

                    return Ok(record.state_id < *state_id);
                },
            )?;

            if selected.is_none_or(|record| {
                return !record.presented
                    || record.state_hash != *state_hash
                    || record.event_seq != Some(*event_seq)
                    || record.event_byte_offset != *event_byte_offset;
            }) {
                return Err(Error::InvalidReplay {
                    reason:
                        "native screenshot endpoint does not match one presented semantic state"
                            .to_owned(),
                    source: None,
                });
            }
            return Ok(None);
        }
    }
}

fn capture_staged(
    helper: &Path,
    options: &CaptureOptions,
    log: &EventLogSummary,
    states: Option<&StateIndexSource>,
    range: Option<SourceRange>,
    staging: &Path,
) -> Result<()> {
    let manifest = read_manifest(&options.session_directory)?;
    let canvas = super::geometry::session_canvas(
        &options.session_directory,
        CaptureTarget {
            width: options.width,
            height: options.height,
            padding: options.padding,
        },
    )?;

    let options = &CaptureOptions {
        width: canvas.width,
        height: canvas.height,
        ..options.clone()
    };

    if let Some(report) = &options.geometry_solver {
        write_secure(
            &staging.join("geometry-solver.json"),
            &serde_json::to_vec_pretty(report).map_err(|source| {
                return Error::InvalidJson {
                    path: staging.join("geometry-solver.json"),
                    json_path: "$".to_owned(),
                    source,
                };
            })?,
        )?;
    }

    let replay = if let Some(provenance) = &manifest.ghostty_visual {
        if provenance.replay_profile != layout::GHOSTTY_REPLAY_PROFILE {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "unsupported recorded Ghostty profile path `{}`",
                    provenance.replay_profile
                ),
                source: None,
            });
        }

        let path = SessionLayout::new(&options.session_directory).ghostty_replay_profile();
        let replay = fs::read(&path).map_err(|source| return Error::Filesystem { path, source })?;
        if blake3::hash(&replay).to_hex().as_str() != provenance.replay_profile_hash {
            return Err(Error::InvalidReplay {
                reason: "recorded Ghostty replay profile hash does not match session provenance"
                    .to_owned(),
                source: None,
            });
        }

        Some(replay)
    } else {
        None
    };

    let profile_root = replay
        .as_deref()
        .map(|replay| return materialize(&staging.join("ghostty-profile"), replay))
        .transpose()?;

    let effective_config =
        command_output_in_profile("ghostty", &["+show-config"], profile_root.as_deref())?;
    let effective_hash = blake3::hash(&effective_config).to_hex().to_string();

    write_secure(
        &staging.join(GHOSTTY_EFFECTIVE_CONFIG_NAME),
        &effective_config,
    )?;

    if let Some(provenance) = &manifest.ghostty_visual {
        if provenance.effective_config_hash != effective_hash {
            return Err(Error::InvalidReplay {
                reason: "recorded Ghostty effective configuration changed".to_owned(),
                source: None,
            });
        }
    } else if let Some(expected) = log
        .header
        .colors
        .source
        .strip_prefix("ghostty_show_config_blake3:")
        && expected != effective_hash
    {
        return Err(Error::InvalidReplay {
            reason: "effective Ghostty configuration changed since recording".to_owned(),
            source: None,
        });
    }

    if let Some(replay) = &replay
        && manifest.native_profile_dpi().is_some()
    {
        verify_profile_fonts(replay, &effective_config)?;
    }

    let ghostty_version =
        String::from_utf8_lossy(&command_output("ghostty", &["+version"])?).into_owned();

    let ffmpeg_version = if matches!(&options.kind, CaptureKind::Video) {
        Some(
            String::from_utf8_lossy(&command_output("ffmpeg", &["-version"])?)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned(),
        )
    } else {
        None
    };

    let scaling = resolve_display_scaling(options, Some(&manifest))?;
    let lease = DisplayLease::acquire()?;
    let display = format!(":{}", lease.number);
    let mut xvfb = spawn_xvfb(
        Some(helper),
        &lease,
        options.width,
        options.height,
        staging,
        Some(scaling.dpi),
    )?;
    wait_for_x_socket(&lease, &mut xvfb, Duration::from_secs(5))?;

    if let Some(dpi) = scaling.font_dpi {
        super::window::publish_font_dpi(&display, dpi)?;
    }

    let files = PlaybackFiles::in_staging(staging);
    let overrides = ghostty_overrides(options.padding);
    let mut ghostty = spawn_ghostty(
        helper,
        options,
        staging,
        &files,
        &GhosttyEnvironment {
            display: &display,
            overrides: &overrides,
            profile: profile_root.as_deref(),
            scaling: Some(scaling),
        },
    )?;
    let geometry_report =
        super::process::wait_for_json(&files.geometry, &mut ghostty, Duration::from_secs(20))?;

    validate_capture(
        manifest.geometry,
        manifest.geometry_provenance.source,
        geometry_report,
        options.width,
        options.height,
    )?;

    let timing: PlaybackTiming = wait_for_playback_handshake(
        &files.timing,
        &files.error,
        &mut ghostty,
        Duration::from_secs(20),
    )?;

    let expected = match &options.kind {
        CaptureKind::Video => (None, None, false),
        CaptureKind::Screenshot {
            event_seq,
            event_byte_offset,
            ..
        } => (Some(*event_seq), *event_byte_offset, true),
    };

    if timing.source_event_hash
        != log
            .terminal
            .as_ref()
            .map(|record| return record.hash.clone())
        || timing.source_event_count != log.event_count
        || timing.source_range != range
        || (
            timing.endpoint_event_seq,
            timing.endpoint_event_byte_offset,
            timing.instant,
        ) != expected
    {
        return Err(Error::InvalidReplay {
            reason: "playback timing handshake does not match capture request".to_owned(),
            source: None,
        });
    }

    let mut capture_started = unix_now_ns()?;
    let screenshot = staging.join("final.png");
    let (video_probe, frame_samples) = match &options.kind {
        CaptureKind::Video => {
            thread::sleep(Duration::from_millis(200));

            let video = staging.join("session.mkv");
            let mut ffmpeg = spawn_video_ffmpeg(Some(helper), options, staging, &display, &video)?;
            let duration = range.map_or_else(
                || {
                    return log.terminal.as_ref().map_or(Duration::ZERO, |event| {
                        return Duration::from_nanos(event.monotonic_ns);
                    });
                },
                |range| return Duration::from_nanos(range.duration_ns()),
            );
            wait_for_file(
                &files.ready,
                &mut ghostty,
                Duration::from_secs(25) + duration,
            )?;

            capture_screenshot(options, staging, &display, &screenshot)?;
            wait_for_success(&mut ghostty, Duration::from_secs(10), staging)?;
            stop_ffmpeg(&mut ffmpeg, Duration::from_secs(10), staging)?;

            secure_permissions(&video)?;
            let probe = probe_video(&video, options.fps, options.width, options.height)?;
            let packets = probe
                .get("streams")
                .and_then(serde_json::Value::as_array)
                .and_then(|streams| return streams.first())
                .and_then(|stream| return stream.get("nb_read_packets"))
                .and_then(serde_json::Value::as_str)
                .and_then(|count| return count.parse().ok())
                .ok_or_else(|| {
                    return Error::InvalidReplay {
                        reason: "encoded frame count is missing".to_owned(),
                        source: None,
                    };
                })?;
            capture_started =
                validate_acquisition_frames(&staging.join("ffmpeg-video.log"), packets)?;
            write_video_map(
                &staging.join("video-map.jsonl"),
                states.ok_or_else(|| {
                    return Error::InvalidReplay {
                        reason: "video capture has no validated state index".to_owned(),
                        source: None,
                    };
                })?,
                &staging.join("ffmpeg-video.log"),
                timing.replay_origin_unix_ns,
                range,
            )?;
            (Some(probe), Vec::new())
        }
        CaptureKind::Screenshot { .. } => {
            wait_for_file(&files.ready, &mut ghostty, Duration::from_secs(25))?;
            let samples = capture_stable_x11_frame(staging, &display, &screenshot)?;
            wait_for_success(&mut ghostty, Duration::from_secs(10), staging)?;
            (None, samples)
        }
    };

    xvfb.terminate();

    secure_permissions(&screenshot)?;
    let screenshot_probe = probe_screenshot(&screenshot, options.width, options.height)?;

    for transient in [&files.timing, &files.ready] {
        fs::remove_file(transient).map_err(|source| {
            return Error::Filesystem {
                path: transient.clone(),
                source,
            };
        })?;
    }

    if let Some(root) = profile_root {
        fs::remove_dir_all(&root)
            .map_err(|source| return Error::Filesystem { path: root, source })?;
    }

    let members = collect_artifact_members(staging)?;
    let (kind, state_id, state_hash, event_seq, byte_offset, presentation) = match &options.kind {
        CaptureKind::Video => (
            "video".to_owned(),
            None,
            None,
            None,
            None,
            "post_playback_capture_unproven".to_owned(),
        ),
        CaptureKind::Screenshot {
            state_id,
            state_hash,
            event_seq,
            event_byte_offset,
        } => (
            "screenshot".to_owned(),
            Some(*state_id),
            Some(state_hash.clone()),
            Some(*event_seq),
            *event_byte_offset,
            "post_endpoint_stable_capture".to_owned(),
        ),
    };

    let visual = VisualManifest {
        version: VISUAL_MANIFEST_VERSION,
        session_id: log.header.session_id,
        source_event_hash: log.terminal.as_ref().map(|event| return event.hash.clone()),
        source_range: range,
        state_selection: options.state_selection,
        geometry_report,
        fps: options.fps,
        width: options.width,
        height: options.height,
        capture_started_unix_ns: capture_started,
        replay_origin_unix_ns: timing.replay_origin_unix_ns,
        effective_config_hash: effective_hash,
        ghostty_version,
        ffmpeg_version,
        ghostty_overrides: overrides,
        video_probe,
        screenshot_probe,
        members,
        capture_kind: kind,
        selected_state_id: state_id,
        selected_state_hash: state_hash,
        endpoint_event_seq: event_seq,
        endpoint_event_byte_offset: byte_offset,
        presentation_evidence: presentation,
        frame_samples,
        guarantee_level: "native_capture_environment".to_owned(),
    };

    validate_visual_manifest(&visual)?;
    write_secure(
        &staging.join("manifest.json"),
        &serde_json::to_vec_pretty(&visual).map_err(|source| {
            return Error::InvalidJson {
                path: staging.join("manifest.json"),
                json_path: "$".to_owned(),
                source,
            };
        })?,
    )?;

    verify_artifact_members(staging, &visual.members)?;
    sync_directory(staging)?;

    return Ok(());
}

pub(super) fn validate_options(options: &CaptureOptions) -> Result<()> {
    if options.width < super::MIN_VIEWPORT_WIDTH
        || options.height < super::MIN_VIEWPORT_HEIGHT
        || !matches!(options.fps, 24 | 30 | 60)
    {
        return Err(Error::InvalidReplay {
            reason: format!(
                "native capture requires at least {}x{} pixels and 24, 30, or 60 FPS",
                super::MIN_VIEWPORT_WIDTH,
                super::MIN_VIEWPORT_HEIGHT
            ),
            source: None,
        });
    }

    if options.dpi == Some(0)
        || options
            .scale
            .is_some_and(|scale| return !scale.is_finite() || scale <= 0.0)
    {
        return Err(Error::InvalidReplay {
            reason: "native capture scale and DPI must be positive finite numbers".to_owned(),
            source: None,
        });
    }
    return Ok(());
}

pub(super) fn require_native_tools(kind: &CaptureKind) -> Result<()> {
    for program in ["Xvfb", "dbus-run-session", "ghostty", "ffprobe"] {
        which(program)?;
    }

    if matches!(kind, CaptureKind::Screenshot { .. }) {
        return which("import");
    }

    which("ffmpeg")?;
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map_err(|error| {
            return Error::ExternalCommand {
                operation: "ffmpeg".to_owned(),
                reason: error.to_string(),
            };
        })?;

    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .any(|value| return value == "libx264rgb")
    {
        return Err(Error::ExternalCommand {
            operation: "ffmpeg".to_owned(),
            reason: "required libx264rgb encoder is unavailable; no fallback is permitted"
                .to_owned(),
        });
    }
    return Ok(());
}

fn which(program: &str) -> Result<()> {
    let status = Command::new("sh")
        .args([
            "-c",
            "command -v \"$1\" >/dev/null 2>&1",
            "termtrace-probe",
            program,
        ])
        .status()
        .map_err(|error| {
            return Error::ExternalCommand {
                operation: program.to_owned(),
                reason: error.to_string(),
            };
        })?;

    if !status.success() {
        return Err(Error::ExternalCommand {
            operation: program.to_owned(),
            reason: "required native capture capability is unavailable; no fallback is permitted"
                .to_owned(),
        });
    }
    return Ok(());
}

pub(super) fn preserve_failed(parent: &Path, staging: &Path, error: Error) -> Error {
    let evidence = parent.join(format!("visual-failed-{}", uuid::Uuid::now_v7()));

    match commit_directory(parent, staging, &evidence, false)
        .and_then(|()| return sync_directory(parent))
    {
        Ok(()) => {
            return Error::CaptureFailed {
                evidence,
                source: Box::new(error),
            };
        }
        Err(cleanup) => {
            return Error::CleanupFailed {
                primary: Box::new(error),
                cleanup: Box::new(Error::from(cleanup)),
            };
        }
    }
}

/// Display scaling and DPI configuration for headless X11 capture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayScaling {
    /// X11 screen resolution in dots per inch (passed to Xvfb -dpi).
    pub dpi: u32,
    /// Overall display scale factor (e.g. 1.0, 1.5, 2.0).
    pub scale: f64,
    /// Integer GDK window scaling factor (set as `GDK_SCALE` in Ghostty).
    pub gdk_scale: u32,
    /// Fractional GDK font scaling factor (set as `GDK_DPI_SCALE` in Ghostty).
    pub gdk_dpi_scale: f64,
    /// GTK font DPI that a visual profile fixes through the profile's GTK settings; GTK 4
    /// ignores `GDK_DPI_SCALE`, so this is the only fractional font scale Ghostty honors.
    pub font_dpi: Option<u32>,
}

impl DisplayScaling {
    /// Calculate display scaling from an overall scale factor and optional explicit DPI.
    ///
    /// # Errors
    ///
    /// Returns an error when `scale` is not a positive finite number, or when the DPI or GDK
    /// window factor derived from it does not fit `u32`.
    pub fn new(scale: f64, dpi: Option<u32>) -> Result<Self> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(Error::InvalidReplay {
                reason: format!("display scale {scale} must be a positive finite number"),
                source: None,
            });
        }

        let base_dpi = 96.0;
        let effective_dpi = match dpi {
            Some(dpi) => dpi,
            None => display_units((base_dpi * scale).round())?,
        };

        let effective_scale = if dpi.is_some() && (scale - 1.0).abs() < f64::EPSILON {
            f64::from(effective_dpi) / base_dpi
        } else {
            scale
        };

        let gdk_scale =
            if effective_scale >= 2.0 { display_units(effective_scale.floor())? } else { 1 };
        let gdk_dpi_scale = effective_scale / f64::from(gdk_scale);

        return Ok(Self {
            dpi: effective_dpi,
            scale: effective_scale,
            gdk_scale,
            gdk_dpi_scale,
            font_dpi: None,
        });
    }

    /// Scaling fixed by a visual profile: unscaled windows with Ghostty's font and padding
    /// scaled by `dpi / 96` through GTK's font DPI.
    #[must_use]
    pub fn profile(dpi: u32) -> Self {
        return Self {
            dpi,
            scale: f64::from(dpi) / 96.0,
            gdk_scale: 1,
            gdk_dpi_scale: 1.0,
            font_dpi: Some(dpi),
        };
    }

    /// Calculate display scaling from a given DPI value.
    ///
    /// # Errors
    ///
    /// Returns an error when `dpi` is zero.
    pub fn from_dpi(dpi: u32) -> Result<Self> {
        let scale = f64::from(dpi) / 96.0;

        return Self::new(scale, Some(dpi));
    }

    /// Calculate display scaling from a given scale factor.
    ///
    /// # Errors
    ///
    /// Returns an error when `scale` is not a positive finite number or its derived DPI does not
    /// fit `u32`.
    pub fn from_scale(scale: f64) -> Result<Self> {
        return Self::new(scale, None);
    }
}

/// Convert a whole-valued display measure to `u32`, rejecting values the cast would saturate.
fn display_units(value: f64) -> Result<u32> {
    if !value.is_finite() || value < 0.0 || value > f64::from(u32::MAX) {
        return Err(Error::InvalidReplay {
            reason: format!("display scaling value {value} does not fit a 32-bit unsigned integer"),
            source: None,
        });
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked above to be finite and within `0..=u32::MAX`, and callers round or floor \
                  first, so the cast is exact; std has no checked float-to-integer conversion"
    )]
    let units = value as u32;

    return Ok(units);
}

/// Resolve display scaling for native headless capture from the session's visual profile,
/// explicit options, environment variables, or recorded session provenance/geometry heuristics.
///
/// Environment and provenance values that are not positive finite numbers are ignored like
/// unparsable ones.
///
/// # Errors
///
/// Returns an error when an explicit scale or DPI option is zero, negative, or non-finite or
/// conflicts with the session's visual profile, or when the resolved scale's derived DPI does not
/// fit `u32`.
pub fn resolve_display_scaling(
    options: &CaptureOptions,
    manifest: Option<&SessionManifest>,
) -> Result<DisplayScaling> {
    if let Some(dpi) = manifest.and_then(SessionManifest::native_profile_dpi) {
        if options.dpi.is_some_and(|explicit| return explicit != dpi) || options.scale.is_some() {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "the session's visual profile fixes {dpi} DPI; omit the conflicting DPI or scale"
                ),
                source: None,
            });
        }
        return Ok(DisplayScaling::profile(dpi));
    }

    // 1. Explicit options
    if let Some(dpi) = options.dpi {
        return DisplayScaling::from_dpi(dpi);
    }

    if let Some(scale) = options.scale {
        return DisplayScaling::from_scale(scale);
    }

    // 2. Explicit environment variables
    if let Ok(dpi_str) = env::var("TERMTRACE_DPI")
        && let Ok(dpi) = dpi_str.parse::<u32>()
        && dpi > 0
    {
        return DisplayScaling::from_dpi(dpi);
    }

    if let Ok(scale_str) =
        env::var("TERMTRACE_DISPLAY_SCALE").or_else(|_| return env::var("TERMTRACE_SCALE"))
        && let Ok(scale) = scale_str.parse::<f64>()
        && scale.is_finite()
        && scale > 0.0
    {
        return DisplayScaling::from_scale(scale);
    }

    if let Ok(gdk_scale_str) = env::var("GDK_SCALE")
        && let Ok(gdk_scale) = gdk_scale_str.parse::<u32>()
        && gdk_scale > 0
    {
        let dpi_scale = env::var("GDK_DPI_SCALE")
            .ok()
            .and_then(|s| return s.parse::<f64>().ok())
            .filter(|scale| return scale.is_finite() && *scale > 0.0)
            .unwrap_or(1.0);

        let total_scale = f64::from(gdk_scale) * dpi_scale;

        return DisplayScaling::new(total_scale, None);
    }

    // 3. Manifest provenance environment detection
    if let Some(manifest) = manifest {
        if let Some(env_scale) = manifest.execution_provenance.environment.get("GDK_SCALE")
            && let Some(scale_str) = env_scale
                .value
                .as_ref()
                .and_then(|b| return str::from_utf8(b.as_bytes()).ok())
            && let Ok(gdk_scale) = scale_str.parse::<u32>()
            && gdk_scale > 0
        {
            let dpi_scale = manifest
                .execution_provenance
                .environment
                .get("GDK_DPI_SCALE")
                .and_then(|v| return v.value.as_ref())
                .and_then(|b| return str::from_utf8(b.as_bytes()).ok())
                .and_then(|s| return s.parse::<f64>().ok())
                .filter(|scale| return scale.is_finite() && *scale > 0.0)
                .unwrap_or(1.0);

            let total_scale = f64::from(gdk_scale) * dpi_scale;

            return DisplayScaling::new(total_scale, None);
        }

        // 4. Geometry-based high-DPI detection from recorded cell dimensions
        if manifest.geometry.cols > 0 && manifest.geometry.rows > 0 {
            let cell_w =
                f64::from(manifest.geometry.pixel_width) / f64::from(manifest.geometry.cols);

            let cell_h =
                f64::from(manifest.geometry.pixel_height) / f64::from(manifest.geometry.rows);

            if cell_w >= 18.0 && cell_h >= 36.0 {
                return DisplayScaling::from_scale(2.0);
            }

            if cell_w >= 14.0 && cell_h >= 28.0 {
                return DisplayScaling::from_scale(1.5);
            }
        }

        if manifest.geometry_provenance.source == GeometrySource::GhosttyFit
            && let Some(target) = manifest.geometry_provenance.capture_target
        {
            if target.width >= 3840 || target.height >= 2160 {
                return DisplayScaling::from_scale(2.0);
            }

            if target.width >= 2560 || target.height >= 1440 {
                return DisplayScaling::from_scale(1.5);
            }
        }
    }

    // 5. Viewport resolution heuristic (e.g. 4K canvases >= 2880x1800, QHD >= 2560x1440)
    if options.width >= 2880 || options.height >= 1800 {
        return DisplayScaling::from_scale(2.0);
    }

    if options.width >= 2560 || options.height >= 1440 {
        return DisplayScaling::from_scale(1.5);
    }
    return DisplayScaling::from_scale(1.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, path::PathBuf};
    use termtrace_engine::{
        analysis::profile::resolve,
        recording::{record_with_output, RecordOptions},
    };
    use termtrace_model::{
        execution::session::{RecordingFailurePolicy, WindowPadding},
        execution::visual_profile::{NativeVisualProfile, VisualProfile},
        terminal::replay::StateSelection,
    };

    fn options(fps: u16) -> CaptureOptions {
        return CaptureOptions {
            session_directory: PathBuf::from("/unused"),
            width: 640,
            height: 480,
            padding: WindowPadding::default(),
            fps,
            start_monotonic_ns: None,
            end_monotonic_ns: None,
            state_selection: StateSelection::Presented,
            geometry_solver: None,
            kind: CaptureKind::Video,
            destination: None,
            scale: None,
            dpi: None,
        };
    }

    #[test]
    fn native_capture_accepts_public_video_frame_rates_only() {
        for fps in [24, 30, 60] {
            validate_options(&options(fps)).expect("public native FPS should validate");
        }

        assert!(validate_options(&options(25)).is_err());
        assert!(validate_options(&options(24).with_dpi(0)).is_err());

        for scale in [0.0, -1.5, f64::NAN, f64::INFINITY] {
            assert!(validate_options(&options(24).with_scale(scale)).is_err());
        }
    }

    #[test]
    fn display_scaling_rejects_values_the_conversion_would_saturate() {
        for scale in [0.0, -2.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(
                DisplayScaling::from_scale(scale).is_err(),
                "{scale} must be rejected"
            );
        }

        assert!(DisplayScaling::from_dpi(0).is_err());
    }

    #[test]
    fn display_scaling_computes_dpi_and_gdk_factors() {
        let s1 = DisplayScaling::from_scale(1.0).expect("valid display scaling");
        assert_eq!(s1.dpi, 96);
        assert_eq!(s1.gdk_scale, 1);
        assert!((s1.gdk_dpi_scale - 1.0).abs() < f64::EPSILON);

        let s1_5 = DisplayScaling::from_scale(1.5).expect("valid display scaling");
        assert_eq!(s1_5.dpi, 144);
        assert_eq!(s1_5.gdk_scale, 1);
        assert!((s1_5.gdk_dpi_scale - 1.5).abs() < f64::EPSILON);

        let s2 = DisplayScaling::from_scale(2.0).expect("valid display scaling");
        assert_eq!(s2.dpi, 192);
        assert_eq!(s2.gdk_scale, 2);
        assert!((s2.gdk_dpi_scale - 1.0).abs() < f64::EPSILON);

        let s2_5 = DisplayScaling::from_scale(2.5).expect("valid display scaling");
        assert_eq!(s2_5.dpi, 240);
        assert_eq!(s2_5.gdk_scale, 2);
        assert!((s2_5.gdk_dpi_scale - 1.25).abs() < f64::EPSILON);

        let from_dpi = DisplayScaling::from_dpi(192).expect("valid display scaling");
        assert_eq!(from_dpi.scale, 2.0);
        assert_eq!(from_dpi.gdk_scale, 2);
        assert!((from_dpi.gdk_dpi_scale - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_display_scaling_respects_options_and_viewport_resolution() {
        let opt_1x = options(24);
        assert_eq!(
            resolve_display_scaling(&opt_1x, None).expect("resolve scaling"),
            DisplayScaling::from_scale(1.0).expect("valid display scaling")
        );

        let opt_explicit_dpi = options(24).with_dpi(192);
        assert_eq!(
            resolve_display_scaling(&opt_explicit_dpi, None).expect("resolve scaling"),
            DisplayScaling::from_dpi(192).expect("valid display scaling")
        );

        let opt_explicit_scale = options(24).with_scale(1.5);
        assert_eq!(
            resolve_display_scaling(&opt_explicit_scale, None).expect("resolve scaling"),
            DisplayScaling::from_scale(1.5).expect("valid display scaling")
        );

        let mut opt_2k = options(24);
        opt_2k.width = 2560;
        opt_2k.height = 1440;

        assert_eq!(
            resolve_display_scaling(&opt_2k, None).expect("resolve scaling"),
            DisplayScaling::from_scale(1.5).expect("valid display scaling")
        );

        let mut opt_4k = options(24);
        opt_4k.width = 3840;
        opt_4k.height = 2160;

        assert_eq!(
            resolve_display_scaling(&opt_4k, None).expect("resolve scaling"),
            DisplayScaling::from_scale(2.0).expect("valid display scaling")
        );
    }

    #[test]
    fn recorded_visual_profile_fixes_dpi_over_environment_and_viewport_heuristics() {
        let mut profile = VisualProfile::native_standard();
        if let Some(native) = profile.native.as_mut() {
            native.dpi = Some(144);
        }

        let overrides = profile
            .native
            .as_ref()
            .map(NativeVisualProfile::overrides)
            .expect("native section");

        let resolved = resolve(b"", None, overrides, |config| return Ok(config.to_vec()))
            .expect("resolved profile bytes");

        let temporary = tempfile::TempDir::new().expect("temporary session");
        let session = temporary.path().join("session");
        record_with_output(
            &RecordOptions {
                failure_policy: RecordingFailurePolicy::Stop,
                output_directory: session.clone(),
                creation_alias: None,
                lineage: None,
                import_provenance: None,
                tmux_pane: None,
                command: vec!["/bin/true".into()],
                working_directory: None,
                cols: 80,
                rows: 24,
                cell_width_px: 18,
                cell_height_px: 36,
                geometry_provenance: Default::default(),
                ghostty_profile: Some(resolved),
                visual_profile: Some(profile.record().expect("valid profile")),
                term: "xterm-256color".to_owned(),
                environment: Default::default(),
                colors: Default::default(),
                forward_stdin: false,
                continue_after_output_mirror_closes: false,
                durability_policy: Default::default(),
                provenance_policy: Default::default(),
                resource_limits: Default::default(),
            },
            &mut io::sink(),
        )
        .expect("record profiled session");

        let manifest = read_manifest(&session).expect("profiled manifest");
        let mut large = options(24);
        large.width = 3840;
        large.height = 2160;

        assert_eq!(
            resolve_display_scaling(&large, Some(&manifest)).expect("resolve scaling"),
            DisplayScaling::profile(144)
        );
        assert_eq!(
            resolve_display_scaling(&options(24).with_dpi(144), Some(&manifest))
                .expect("matching explicit DPI"),
            DisplayScaling::profile(144)
        );
        assert!(resolve_display_scaling(&options(24).with_dpi(96), Some(&manifest)).is_err());
        assert!(resolve_display_scaling(&options(24).with_scale(1.5), Some(&manifest)).is_err());
    }
}
