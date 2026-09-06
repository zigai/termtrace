use std::{fs, path::Path, process::Command, sync::atomic::AtomicBool, thread, time::Duration};

use termtrace_engine::{replay::visit_session_states_while, timeline::resolve_source_range};
use termtrace_evidence::{
    artifact::{
        append_artifact_root, collect_artifact_members, commit_directory, create_private_directory,
        sync_directory, verify_artifact_members, write_secure,
    },
    event_log::EventLogSummary,
    session::{read_completion, read_manifest, validate_completion_summary_candidate},
};
use termtrace_model::{
    artifact::metadata::ArtifactSourceRange,
    artifact::playback::PlaybackTiming,
    artifact::visual::{
        CaptureKind, CaptureOptions, CaptureOutcome, GHOSTTY_EFFECTIVE_CONFIG_NAME,
        VISUAL_MANIFEST_VERSION, VisualManifest, visual_directory_name_for,
    },
    execution::session::GHOSTTY_REPLAY_PROFILE_NAME,
    terminal::replay::SourceRange,
};

use crate::error::finish_with_cleanup;

use super::{
    Error, Result,
    geometry::validate_capture_geometry,
    index::{
        StateIndexSource, validate_acquisition_frames, validate_replay_manifest, write_video_map,
    },
    media::{
        capture_screenshot, capture_stable_x11_frame, probe_screenshot, probe_video,
        spawn_video_ffmpeg,
    },
    process::{
        DisplayLease, ghostty_overrides, spawn_ghostty, spawn_xvfb, stop_ffmpeg, wait_for_file,
        wait_for_playback_handshake, wait_for_success, wait_for_x_socket,
    },
    profile::materialize_profile,
    util::{command_output, command_output_in_profile, outcome, secure_permissions, unix_now_ns},
    validate_visual_manifest,
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
    if !options.session_directory.join("completion.json").is_file() {
        return Err(Error::SessionIncomplete {
            session: options.session_directory.clone(),
        });
    }
    let log = EventLogSummary::read(options.session_directory.join("events.jsonl"))?;
    validate_completion_summary_candidate(
        &options.session_directory,
        &log,
        &read_completion(&options.session_directory)?,
    )?;
    let final_ns = log
        .terminal
        .as_ref()
        .map_or(0, |record| return record.monotonic_ns);
    let range = resolve_source_range(
        options.start_monotonic_ns,
        options.end_monotonic_ns,
        final_ns,
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
            };
        })?;
    require_native_tools(&options.kind)?;
    let staging = parent.join(format!(".visual.{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;
    if let Err(error) = capture_staged(helper, options, &log, states.as_ref(), range, &staging) {
        return Err(preserve_failed(parent, &staging, error));
    }
    if let Err(error) = commit_directory(parent, &staging, &destination)
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
    let profile_root = if let Some(provenance) = &manifest.ghostty_visual {
        if provenance.replay_profile != GHOSTTY_REPLAY_PROFILE_NAME {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "unsupported recorded Ghostty profile path `{}`",
                    provenance.replay_profile
                ),
            });
        }
        let path = options.session_directory.join(GHOSTTY_REPLAY_PROFILE_NAME);
        let replay = fs::read(&path).map_err(|source| return Error::Filesystem { path, source })?;
        if blake3::hash(&replay).to_hex().as_str() != provenance.replay_profile_hash {
            return Err(Error::InvalidReplay {
                reason: "recorded Ghostty replay profile hash does not match session provenance"
                    .to_owned(),
            });
        }
        Some(materialize_profile(
            &staging.join("ghostty-profile"),
            &replay,
        )?)
    } else {
        None
    };
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
        });
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
    let lease = DisplayLease::acquire()?;
    let display = format!(":{}", lease.number);
    let mut xvfb = spawn_xvfb(Some(helper), &lease, options.width, options.height, staging)?;
    wait_for_x_socket(&lease, &mut xvfb, Duration::from_secs(5))?;
    let geometry_path = staging.join("geometry-preflight.json");
    let timing_file = staging.join("playback-timing.json");
    let error_file = staging.join("playback-error.json");
    let ready_file = staging.join("playback-ready");
    let overrides = ghostty_overrides();
    let mut ghostty = spawn_ghostty(
        helper,
        options,
        staging,
        &display,
        &timing_file,
        &error_file,
        &geometry_path,
        &ready_file,
        &overrides,
        profile_root.as_deref(),
    )?;
    let geometry_report =
        super::process::wait_for_json(&geometry_path, &mut ghostty, Duration::from_secs(20))?;
    validate_capture_geometry(
        manifest.geometry,
        manifest.geometry_provenance.source,
        geometry_report,
        options.width,
        options.height,
    )?;
    let timing: PlaybackTiming = wait_for_playback_handshake(
        &timing_file,
        &error_file,
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
        });
    }
    let mut capture_started = unix_now_ns()?;
    let screenshot = staging.join("final.png");
    let (video_probe, frame_samples) = match &options.kind {
        CaptureKind::Video => {
            thread::sleep(Duration::from_millis(200));
            let video = staging.join("session.mkv");
            let mut ffmpeg = spawn_video_ffmpeg(options, staging, &display, &video)?;
            let duration = range.map_or_else(
                || {
                    return log.terminal.as_ref().map_or(Duration::ZERO, |event| {
                        return Duration::from_nanos(event.monotonic_ns);
                    });
                },
                |range| return Duration::from_nanos(range.duration_ns()),
            );
            wait_for_file(
                &ready_file,
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
                    };
                })?;
            capture_started =
                validate_acquisition_frames(&staging.join("ffmpeg-video.log"), packets)?;
            write_video_map(
                &staging.join("video-map.jsonl"),
                states.ok_or_else(|| {
                    return Error::InvalidReplay {
                        reason: "video capture has no validated state index".to_owned(),
                    };
                })?,
                &staging.join("ffmpeg-video.log"),
                timing.replay_origin_unix_ns,
                range,
            )?;
            (Some(probe), Vec::new())
        }
        CaptureKind::Screenshot { .. } => {
            wait_for_file(&ready_file, &mut ghostty, Duration::from_secs(25))?;
            let samples = capture_stable_x11_frame(staging, &display, &screenshot)?;
            wait_for_success(&mut ghostty, Duration::from_secs(10), staging)?;
            (None, samples)
        }
    };
    xvfb.terminate();
    secure_permissions(&screenshot)?;
    let screenshot_probe = probe_screenshot(&screenshot, options.width, options.height)?;
    for transient in [&timing_file, &ready_file] {
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
    if options.width < 320 || options.height < 200 || !matches!(options.fps, 24 | 30 | 60) {
        return Err(Error::InvalidReplay {
            reason: "native capture requires at least 320x200 pixels and 24, 30, or 60 FPS"
                .to_owned(),
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
    match commit_directory(parent, staging, &evidence).and_then(|()| return sync_directory(parent))
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use termtrace_model::terminal::replay::StateSelection;

    use super::*;

    fn options(fps: u16) -> CaptureOptions {
        return CaptureOptions {
            session_directory: PathBuf::from("/unused"),
            width: 640,
            height: 480,
            fps,
            start_monotonic_ns: None,
            end_monotonic_ns: None,
            state_selection: StateSelection::Presented,
            geometry_solver: None,
            kind: CaptureKind::Video,
            destination: None,
        };
    }

    #[test]
    fn native_capture_accepts_public_video_frame_rates_only() {
        for fps in [24, 30, 60] {
            validate_options(&options(fps)).expect("public native FPS should validate");
        }
        assert!(validate_options(&options(25)).is_err());
    }
}
