use std::{path::Path, thread, time::Duration};

use uuid::Uuid;

use super::{
    CaptureKind, CaptureOptions, CaptureOutcome, GHOSTTY_EFFECTIVE_CONFIG_NAME,
    VISUAL_MANIFEST_VERSION, VisualManifest, validate_visual_manifest, visual_directory_name_for,
};
use crate::{
    Error, Result,
    artifact::{
        ArtifactSourceRange, append_artifact_root, collect_artifact_members, commit_directory,
        create_private_directory, sync_directory, verify_artifact_members, write_secure,
    },
    capability::{NATIVE_SCREENSHOT_CAPABILITY, NATIVE_VIDEO_CAPABILITY, require_capability},
    event_log::EventLog,
    geometry::TerminalGeometryReport,
    playback::PlaybackTiming,
    profile::materialize_profile,
    replay::{StateIndexEntry, StateSelector, inspect},
    session::{GHOSTTY_REPLAY_PROFILE_NAME, read_manifest, validate_completed_log},
    timeline::{SourceRange, resolve_source_range},
};

use super::{
    geometry::validate_capture_geometry,
    index::{read_state_index, validate_replay_manifest, write_video_map},
    media::{
        capture_screenshot, capture_stable_x11_frame, probe_screenshot, probe_video,
        spawn_video_ffmpeg,
    },
    process::{
        DisplayLease, ghostty_overrides, spawn_ghostty, spawn_xvfb, stop_ffmpeg, wait_for_file,
        wait_for_playback_handshake, wait_for_success, wait_for_x_socket,
    },
    util::{
        command_output, command_output_in_profile, finish_with_cleanup, json_error, outcome,
        secure_permissions, unix_now_ns,
    },
};

/// Capture timed replay with the actual Ghostty GTK/OpenGL application.
///
/// # Errors
///
/// Returns an error for invalid dimensions, missing semantic index, process
/// launch/exit failures, timeouts, probe mismatches, or artifact I/O failure.
pub fn capture(options: &CaptureOptions) -> Result<CaptureOutcome> {
    validate_options(options)?;
    if !options.session_directory.join("completion.json").is_file() {
        return Err(Error::SessionIncomplete {
            session: options.session_directory.clone(),
        });
    }
    let event_log = EventLog::read(options.session_directory.join("events.jsonl"))?;
    validate_completed_log(&options.session_directory, &event_log)?;
    let final_monotonic_ns = event_log
        .events
        .last()
        .map_or(0, |record| return record.monotonic_ns);
    let source_range = resolve_source_range(
        options.start_monotonic_ns,
        options.end_monotonic_ns,
        final_monotonic_ns,
    )?;
    // A range-specific directory name is not sufficient provenance. Verify the
    // semantic index against the canonical log before using its hashes to build
    // a video map, or renamed/stale artifacts could appear internally consistent.
    let state_index = match &options.kind {
        CaptureKind::Video => {
            let state_index = read_state_index(
                &options.session_directory,
                source_range,
                options.state_selection,
            )?;
            validate_replay_manifest(
                &options.session_directory,
                &event_log,
                &state_index,
                source_range,
                options.state_selection,
            )?;
            state_index
        }
        CaptureKind::Screenshot {
            state_id,
            state_hash,
            event_seq,
            event_byte_offset,
        } => {
            if source_range.is_some() {
                return Err(Error::InvalidReplay {
                    reason: "native screenshot endpoints cannot use a time range".to_owned(),
                });
            }
            let selected = inspect(&event_log, StateSelector::StateId(*state_id))?;
            if !selected.record.presented
                || selected.record.state_hash != *state_hash
                || selected.record.event_seq != Some(*event_seq)
                || selected.record.event_byte_offset != *event_byte_offset
            {
                return Err(Error::InvalidReplay {
                    reason:
                        "native screenshot endpoint does not match one presented semantic state"
                            .to_owned(),
                });
            }
            vec![StateIndexEntry {
                state_id: *state_id,
                state_hash: state_hash.clone(),
                event_seq: Some(*event_seq),
                event_byte_offset: *event_byte_offset,
                monotonic_ns: selected.record.monotonic_ns,
                presented: true,
                checkpoint: false,
                timeline_rank: None,
                control_index_offset: None,
                control_rank: None,
            }]
        }
    };
    let destination = options.destination.clone().unwrap_or_else(|| {
        return match &options.kind {
            CaptureKind::Video => options.session_directory.join(visual_directory_name_for(
                source_range,
                options.state_selection,
            )),
            CaptureKind::Screenshot { state_id, .. } => options
                .session_directory
                .join(format!("native-screenshot-state-{state_id}")),
        };
    });
    if destination.exists() {
        return Err(Error::ArtifactExists { path: destination });
    }
    let parent = destination.parent().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "native capture destination has no parent".to_owned(),
        };
    })?;
    if !parent.is_dir() {
        return Err(Error::InvalidReplay {
            reason: "native capture destination parent does not exist".to_owned(),
        });
    }
    require_capability(required_capture_capability(&options.kind))?;
    let staging = parent.join(format!(".visual.{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    if let Err(error) = capture_staged(options, &event_log, &state_index, source_range, &staging) {
        return Err(preserve_failed_capture(parent, &staging, error));
    }
    let commit = commit_directory(parent, &staging, &destination)
        .and_then(|()| return sync_directory(parent));
    if let Err(error) = commit {
        let cleanup = fs_err::remove_dir_all(&staging).map_err(|source| {
            return Error::Filesystem {
                path: staging,
                source,
            };
        });
        return finish_with_cleanup(Err(error), cleanup);
    }
    let manifest_path = destination.join("manifest.json");
    let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
        return Error::Filesystem {
            path: manifest_path,
            source,
        };
    })?;
    let canonical_session = options.session_directory.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: options.session_directory.clone(),
            source,
        };
    })?;
    let canonical_destination = destination.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: destination.clone(),
            source,
        };
    })?;
    if canonical_destination.starts_with(&canonical_session) {
        append_artifact_root(
            &canonical_session,
            &canonical_destination,
            match options.kind {
                CaptureKind::Video => "native_capture",
                CaptureKind::Screenshot { .. } => "native_screenshot",
            },
            &manifest_bytes,
            &event_log.header.session_manifest_hash,
            event_log
                .events
                .last()
                .map(|record| return record.hash.as_str()),
            source_range.map(|range| {
                return ArtifactSourceRange {
                    start_monotonic_ns: range.start_monotonic_ns,
                    end_monotonic_ns: range.end_monotonic_ns,
                };
            }),
        )?;
    }
    return Ok(outcome(destination, &options.kind));
}

pub(super) fn required_capture_capability(kind: &CaptureKind) -> &'static str {
    return match kind {
        CaptureKind::Video => NATIVE_VIDEO_CAPABILITY,
        CaptureKind::Screenshot { .. } => NATIVE_SCREENSHOT_CAPABILITY,
    };
}

fn preserve_failed_capture(session: &Path, staging: &Path, error: Error) -> Error {
    let evidence = session.join(format!("visual-failed-{}", Uuid::now_v7()));
    match commit_directory(session, staging, &evidence)
        .and_then(|()| return sync_directory(session))
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
                cleanup: Box::new(cleanup),
            };
        }
    }
}

fn capture_staged(
    options: &CaptureOptions,
    event_log: &EventLog,
    state_index: &[StateIndexEntry],
    source_range: Option<SourceRange>,
    staging: &Path,
) -> Result<VisualManifest> {
    let session_manifest = read_manifest(&options.session_directory)?;
    if let Some(report) = &options.geometry_solver {
        let encoded = serde_json::to_vec_pretty(report).map_err(|source| {
            return json_error(staging.join("geometry-solver.json"), source);
        })?;
        write_secure(&staging.join("geometry-solver.json"), &encoded)?;
    }
    let profile_root = if let Some(provenance) = &session_manifest.ghostty_visual {
        if provenance.replay_profile != GHOSTTY_REPLAY_PROFILE_NAME {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "unsupported recorded Ghostty profile path `{}`",
                    provenance.replay_profile
                ),
            });
        }
        let replay_profile_path = options.session_directory.join(GHOSTTY_REPLAY_PROFILE_NAME);
        let replay_profile = fs_err::read(&replay_profile_path).map_err(|source| {
            return Error::Filesystem {
                path: replay_profile_path,
                source,
            };
        })?;
        let replay_profile_hash = blake3::hash(&replay_profile).to_hex().to_string();
        if replay_profile_hash != provenance.replay_profile_hash {
            return Err(Error::InvalidReplay {
                reason: "recorded Ghostty replay profile hash does not match session provenance"
                    .to_owned(),
            });
        }
        Some(materialize_profile(
            &staging.join("ghostty-profile"),
            &replay_profile,
        )?)
    } else {
        None
    };
    let effective_config =
        command_output_in_profile("ghostty", &["+show-config"], profile_root.as_deref())?;
    let effective_config_hash = blake3::hash(&effective_config).to_hex().to_string();
    write_secure(
        &staging.join(GHOSTTY_EFFECTIVE_CONFIG_NAME),
        &effective_config,
    )?;
    if let Some(provenance) = &session_manifest.ghostty_visual {
        if provenance.effective_config_hash != effective_config_hash {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "recorded Ghostty effective configuration hash does not match native capture (recorded {}, current {effective_config_hash})",
                    provenance.effective_config_hash
                ),
            });
        }
    } else if let Some(expected) = event_log
        .header
        .colors
        .source
        .strip_prefix("ghostty_show_config_blake3:")
        && expected != effective_config_hash
    {
        return Err(Error::ExternalCommand {
            program: "ghostty".to_owned(),
            reason: format!(
                "effective configuration changed since recording (recorded {expected}, current {effective_config_hash})"
            ),
        });
    }
    let ghostty_version =
        String::from_utf8_lossy(&command_output("ghostty", &["+version"])?).into();
    let ffmpeg_version = if matches!(options.kind, CaptureKind::Video) {
        let output = command_output("ffmpeg", &["-version"])?;
        Some(
            String::from_utf8_lossy(&output)
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
    let mut xvfb = spawn_xvfb(&lease, options.width, options.height, staging)?;
    wait_for_x_socket(&lease, &mut xvfb, Duration::from_secs(5))?;
    let geometry_path = staging.join("geometry-preflight.json");
    let overrides = ghostty_overrides();
    let timing_file = staging.join("playback-timing.json");
    let error_file = staging.join("playback-error.json");
    let ready_file = staging.join("playback-ready");
    let mut ghostty = spawn_ghostty(
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
    let geometry_report: TerminalGeometryReport =
        super::process::wait_for_json(&geometry_path, &mut ghostty, Duration::from_secs(20))?;
    validate_capture_geometry(
        session_manifest.geometry,
        session_manifest.geometry_provenance.source,
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
    if timing.source_range != source_range {
        return Err(Error::InvalidReplay {
            reason: "playback timing source range does not match the capture request".to_owned(),
        });
    }
    let expected_endpoint = match &options.kind {
        CaptureKind::Video => (None, None, false),
        CaptureKind::Screenshot {
            event_seq,
            event_byte_offset,
            ..
        } => (Some(*event_seq), *event_byte_offset, true),
    };
    if (
        timing.endpoint_event_seq,
        timing.endpoint_event_byte_offset,
        timing.instant,
    ) != expected_endpoint
    {
        return Err(Error::InvalidReplay {
            reason: "playback timing endpoint does not match capture request".to_owned(),
        });
    }
    let capture_started_unix_ns = unix_now_ns()?;
    let screenshot_path = staging.join("final.png");
    let (video_probe, frame_samples) = match &options.kind {
        CaptureKind::Video => {
            // The query barrier proves terminal parsing is caught up, but GTK still has
            // to present the surface to X11. This margin is not presentation proof.
            thread::sleep(Duration::from_millis(200));
            let video_path = staging.join("session.mkv");
            let mut ffmpeg = spawn_video_ffmpeg(options, staging, &display, &video_path)?;
            let recording_duration = source_range.map_or_else(
                || {
                    return event_log.events.last().map_or(Duration::ZERO, |event| {
                        return Duration::from_nanos(event.monotonic_ns);
                    });
                },
                |range| return Duration::from_nanos(range.duration_ns()),
            );
            wait_for_file(
                &ready_file,
                &mut ghostty,
                Duration::from_secs(25) + recording_duration,
            )?;
            capture_screenshot(options, staging, &display, &screenshot_path)?;
            wait_for_success(&mut ghostty, Duration::from_secs(10), staging)?;
            stop_ffmpeg(&mut ffmpeg, Duration::from_secs(10), staging)?;
            secure_permissions(&video_path)?;
            write_video_map(
                &staging.join("video-map.jsonl"),
                state_index,
                options.fps,
                capture_started_unix_ns,
                timing.replay_origin_unix_ns,
                source_range,
            )?;
            (
                Some(probe_video(
                    &video_path,
                    options.fps,
                    options.width,
                    options.height,
                )?),
                Vec::new(),
            )
        }
        CaptureKind::Screenshot { .. } => {
            wait_for_file(&ready_file, &mut ghostty, Duration::from_secs(25))?;
            let samples = capture_stable_x11_frame(staging, &display, &screenshot_path)?;
            wait_for_success(&mut ghostty, Duration::from_secs(10), staging)?;
            (None, samples)
        }
    };
    xvfb.terminate();

    secure_permissions(&screenshot_path)?;
    let screenshot_probe = probe_screenshot(&screenshot_path, options.width, options.height)?;
    for transient in [&timing_file, &ready_file] {
        fs_err::remove_file(transient).map_err(|source| {
            return Error::Filesystem {
                path: transient.clone(),
                source,
            };
        })?;
    }
    if let Some(profile_root) = profile_root {
        fs_err::remove_dir_all(&profile_root).map_err(|source| {
            return Error::Filesystem {
                path: profile_root,
                source,
            };
        })?;
    }
    let members = collect_artifact_members(staging)?;
    let (
        capture_kind,
        selected_state_id,
        selected_state_hash,
        endpoint_event_seq,
        endpoint_event_byte_offset,
        presentation_evidence,
    ) = match &options.kind {
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
    let manifest = VisualManifest {
        version: VISUAL_MANIFEST_VERSION,
        session_id: event_log.header.session_id,
        source_event_hash: event_log
            .events
            .last()
            .map(|event| return event.hash.clone()),
        source_range,
        state_selection: options.state_selection,
        geometry_report,
        fps: options.fps,
        width: options.width,
        height: options.height,
        capture_started_unix_ns,
        replay_origin_unix_ns: timing.replay_origin_unix_ns,
        effective_config_hash,
        ghostty_version,
        ffmpeg_version,
        ghostty_overrides: overrides,
        video_probe,
        screenshot_probe,
        members,
        capture_kind,
        selected_state_id,
        selected_state_hash,
        endpoint_event_seq,
        endpoint_event_byte_offset,
        presentation_evidence,
        frame_samples,
        guarantee_level: "native_capture_environment".to_owned(),
    };
    validate_visual_manifest(&manifest)?;
    let encoded = serde_json::to_vec_pretty(&manifest)
        .map_err(|source| return json_error(staging.join("manifest.json"), source))?;
    write_secure(&staging.join("manifest.json"), &encoded)?;
    verify_artifact_members(staging, &manifest.members)?;
    sync_directory(staging)?;
    drop(lease);
    return Ok(manifest);
}

fn validate_options(options: &CaptureOptions) -> Result<()> {
    if options.width < 320 || options.height < 200 || !matches!(options.fps, 24 | 30 | 60) {
        return Err(Error::InvalidReplay {
            reason: "native capture requires at least 320x200 pixels and 24, 30, or 60 FPS"
                .to_owned(),
        });
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::replay::StateSelection;

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
