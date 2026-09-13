use super::{
    args::{TmuxBundleArgs, TmuxCoverageMode, TmuxRecordArgs, TmuxStopArgs, TmuxVideoArgs},
    client::{option_path, option_text, same_executable, TmuxClient},
    setup::{enable_prepared_tmux_setup, prepare_tmux_setup, tmux_config_fragment},
    supervisor::tmux_status_report,
    OPTION_AGGREGATE_MANIFEST, OPTION_COVERAGE, OPTION_PANE_SESSION,
};
use crate::cli::output::write_json_line;
use crate::cli::tmux::{args::TmuxConnectionArgs, setup::run_tmux_disable};
use crate::cli::{
    artifacts::{
        args::{VideoCursorStyleArg, VideoFacadeArgs},
        run_video_facade,
    },
    control::{args::StopArgs, run_stop},
    output::ResponseWriter,
};
use miette::{IntoDiagnostic as _, Result};
use nix::{
    fcntl::{Flock, FlockArg},
    libc::{O_NOFOLLOW, S_IFMT, S_IFREG},
    unistd::geteuid,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{File, OpenOptions, Permissions},
    io::{self, Write as _},
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::{FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    },
    path::{Path, PathBuf},
    process::{self, Command as ProcessCommand, ExitCode, Stdio},
    str, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use termtrace_engine::{inspection::wait_for_completion, registry::SessionRegistry};
use termtrace_evidence::artifact::sync_directory;
use termtrace_evidence::session::{read_completion, read_manifest};
use termtrace_evidence::tmux_catalog::TmuxCatalog;
use termtrace_evidence::tmux_journal;
use termtrace_export::tmux;
use termtrace_model::{
    artifact::tmux::{
        TmuxCompositeTrack, TmuxCoverageGap, TmuxLifecycleEvent, TmuxPaneSnapshot, TmuxPaneTrack,
        TmuxRecordingCoverage, TmuxRecordingLifecycle, TmuxRecordingManifest,
        TMUX_RECORDING_FORMAT, TMUX_RECORDING_VERSION,
    },
    value::ByteString,
};

const TMUX_RECORDINGS_DIRECTORY: &str = "tmux-recordings";

pub(in crate::cli) fn run_tmux_record(
    response: &mut ResponseWriter,
    mut arguments: TmuxRecordArgs,
) -> Result<ExitCode> {
    validate_recording_name(&arguments.name)?;

    let recording_id = uuid::Uuid::now_v7();

    if arguments.new
        && arguments.connection.socket_name.is_none()
        && arguments.connection.socket_path.is_none()
    {
        arguments.connection.socket_name = Some(OsString::from(format!(
            "termtrace-recording-{recording_id}"
        )));
    }

    let registry = match arguments.setup.output_root.as_ref() {
        Some(root) => SessionRegistry::open(root).into_diagnostic()?,
        None => SessionRegistry::open_default().into_diagnostic()?,
    };

    let directory = registry
        .state_root()
        .join(TMUX_RECORDINGS_DIRECTORY)
        .join(&arguments.name);

    create_recording_directory(&directory)?;
    let manifest_path = directory.join("manifest.json");
    arguments.setup.aggregate_manifest = Some(manifest_path.clone());
    let setup = prepare_tmux_setup(arguments.setup, arguments.connection.tmux.clone())?;

    let alias = format!("tmux-{}-composite", arguments.name);
    registry.ensure_available(&alias).into_diagnostic()?;

    let client = TmuxClient::from_connection(arguments.connection.clone());
    let now = unix_time_ns()?;
    let mut manifest = TmuxRecordingManifest {
        format: TMUX_RECORDING_FORMAT.to_owned(),
        version: TMUX_RECORDING_VERSION,
        recording_id,
        name: arguments.name,
        lifecycle: TmuxRecordingLifecycle::Starting,
        created_unix_ns: now,
        updated_unix_ns: now,
        tmux_executable: ByteString::from(client.executable.as_os_str().as_bytes()),
        coverage: match setup.coverage {
            TmuxCoverageMode::Warn => TmuxRecordingCoverage::Warn,
            TmuxCoverageMode::Strict => TmuxRecordingCoverage::Strict,
        },
        socket_name: client
            .socket_name
            .as_ref()
            .map(|value| return ByteString::from(value.as_bytes())),
        socket_path: client
            .socket_path
            .as_ref()
            .map(|value| return ByteString::from(value.as_os_str().as_bytes())),
        server_pid: None,
        server_socket_path: None,
        server_socket_device: None,
        server_socket_inode: None,
        composite_track: TmuxCompositeTrack {
            alias: alias.clone(),
            session_id: None,
            session: None,
            lifecycle: TmuxRecordingLifecycle::Starting,
        },
        panes: Vec::new(),
        pane_tracks: BTreeMap::new(),
        track_history: BTreeMap::new(),
        catalog: None,
        lifecycle_journal: None,
        events: vec![TmuxLifecycleEvent {
            sequence: 1,
            observed_unix_ns: now,
            kind: "recording_started".to_owned(),
            session_id: None,
            window_id: None,
            pane_id: None,
            value: Some(setup.coverage.as_str().to_owned()),
        }],
        coverage_gaps: permanent_coverage_gaps(now),
    };
    write_tmux_manifest(&manifest_path, &mut manifest)?;

    let composite_command = if arguments.new {
        let config = directory.join("tmux.conf");
        write_private_file(&config, &tmux_config_fragment(&setup)?)?;

        configured_tmux_command(&client, Some(&config), &arguments.command)?
    } else {
        enable_prepared_tmux_setup(&client, &setup)?;

        configured_tmux_command(&client, None, &arguments.command)?
    };

    let output = ProcessCommand::new(&setup.termtrace)
        .args(["record", "-d"])
        .arg(&alias)
        .arg("--")
        .args(&composite_command)
        .env("TERMTRACE_STATE_DIR", &setup.output_root)
        .stderr(Stdio::inherit())
        .output()
        .into_diagnostic()?;

    if !output.status.success() {
        return Err(miette::miette!(
            "composite tmux client track start exited with {}",
            output.status
        ));
    }

    let entry = registry.resolve(&alias).into_diagnostic()?;
    {
        let _manifest_lock = lock_tmux_manifest(&manifest_path)?;
        let mut manifest = read_tmux_manifest(&manifest_path)?;
        manifest.lifecycle = TmuxRecordingLifecycle::Live;
        manifest.composite_track.lifecycle = TmuxRecordingLifecycle::Live;
        manifest.composite_track.session_id = Some(entry.session_id);
        manifest.composite_track.session = Some(ByteString::from(
            entry.session_path().as_os_str().as_bytes(),
        ));

        manifest.updated_unix_ns = unix_time_ns()?;
        push_manifest_event(
            &mut manifest,
            "composite_track_linked",
            None,
            None,
            None,
            Some(&entry.session_path().display().to_string()),
        )?;
        write_tmux_manifest(&manifest_path, &mut manifest)?;
    }

    if let Err(error) = refresh_aggregate_manifest(&manifest_path, &client) {
        tracing::warn!(%error, "aggregate tmux recording started before its first server snapshot");
    }

    let manifest = read_tmux_manifest(&manifest_path)?;

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "tmux_recording",
            "recording_id": manifest.recording_id,
            "name": manifest.name,
            "lifecycle": manifest.lifecycle,
            "manifest": manifest_path,
            "composite_track": manifest.composite_track,
            "pane_tracks": manifest.pane_tracks,
            "coverage": manifest.coverage,
            "coverage_gaps": manifest.coverage_gaps,
        }),
    );
}

pub(in crate::cli) fn run_tmux_recording_status(
    response: &mut ResponseWriter,
    output_root: Option<&Path>,
    name: &str,
) -> Result<ExitCode> {
    let manifest_path = recording_manifest_path(output_root, name)?;
    let manifest = read_tmux_manifest(&manifest_path)?;
    let client = client_from_manifest(&manifest);

    if let Ok(server_identity) = observed_tmux_server_identity(&client) {
        let _manifest_lock = lock_tmux_manifest(&manifest_path)?;
        let mut manifest = read_tmux_manifest(&manifest_path)?;
        bind_or_validate_server_identity(&mut manifest, &server_identity)?;
        write_tmux_manifest(&manifest_path, &mut manifest)?;
    }

    if let Err(error) = refresh_aggregate_manifest(&manifest_path, &client) {
        tracing::debug!(%error, "tmux server is unavailable while reading aggregate status");
    }

    return write_json_line(response, &read_tmux_manifest(&manifest_path)?);
}

pub(in crate::cli) fn run_tmux_events(
    response: &mut ResponseWriter,
    arguments: super::args::TmuxEventsArgs,
) -> Result<ExitCode> {
    let path = recording_manifest_path(arguments.output_root.as_deref(), &arguments.name)?;
    let manifest = read_tmux_manifest(&path)?;
    let mut events = Vec::new();
    let mut bytes = 0_usize;
    let mut next_sequence = None;

    let mut collect = |event: &TmuxLifecycleEvent| -> termtrace_evidence::Result<bool> {
        if event.sequence < arguments.from_sequence || next_sequence.is_some() {
            return Ok(next_sequence.is_none());
        }

        let encoded = serde_json::to_vec(event).map_err(|source| {
            return termtrace_evidence::Error::InvalidJson {
                path: path.clone(),
                json_path: String::new(),
                source,
            };
        })?;

        if events.len() >= usize::from(arguments.limit)
            || bytes.saturating_add(encoded.len()) > 4 * 1024 * 1024
        {
            next_sequence = Some(event.sequence);
        } else {
            bytes += encoded.len();
            events.push(event.clone());
        }

        return Ok(next_sequence.is_none());
    };

    if manifest.catalog.is_some()
        && let Some(boundary) = &manifest.lifecycle_journal
    {
        let directory = path
            .parent()
            .ok_or_else(|| miette::miette!("tmux manifest has no parent"))?;

        let catalog = TmuxCatalog::read(directory, &manifest).into_diagnostic()?;

        for sequence in arguments.from_sequence.max(1)..=boundary.sequence {
            if !collect(&catalog.event(sequence).into_diagnostic()?).into_diagnostic()? {
                break;
            }
        }
    } else if let Some(boundary) = &manifest.lifecycle_journal {
        tmux_journal::visit(
            &path.with_file_name(tmux_journal::JOURNAL_NAME),
            boundary,
            |event| return collect(event).map(drop),
        )
        .into_diagnostic()?;
    } else {
        for event in &manifest.events {
            collect(event).into_diagnostic()?;
        }
    }

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "tmux_events", "recording_id": manifest.recording_id,
            "events": events, "next_sequence": next_sequence,
        }),
    );
}

pub(in crate::cli) fn run_tmux_catalog(
    response: &mut ResponseWriter,
    arguments: super::args::TmuxCatalogArgs,
    gaps: bool,
) -> Result<ExitCode> {
    let path =
        recording_manifest_path(arguments.page.output_root.as_deref(), &arguments.page.name)?;
    let manifest = read_tmux_manifest(&path)?;
    let directory = path
        .parent()
        .ok_or_else(|| miette::miette!("tmux manifest has no parent"))?;

    let catalog = TmuxCatalog::read(directory, &manifest).into_diagnostic()?;
    let limit = arguments.page.limit;
    let entries = if gaps {
        serde_json::to_value(
            catalog
                .gaps(arguments.page.from_sequence, limit, arguments.active_only)
                .into_diagnostic()?,
        )
        .into_diagnostic()?
    } else {
        serde_json::to_value(
            catalog
                .tracks(arguments.page.from_sequence, limit, arguments.active_only)
                .into_diagnostic()?,
        )
        .into_diagnostic()?
    };

    let next_sequence = entries
        .as_array()
        .filter(|entries| return entries.len() == usize::from(limit))
        .and_then(|entries| return entries.last())
        .and_then(|entry| return entry.get("sequence"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|sequence| return sequence.checked_add(1));

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": if gaps { "tmux_coverage_gaps" } else { "tmux_track_inventory" },
            "recording_id": manifest.recording_id, "boundary": manifest.lifecycle_journal,
            "summary": catalog.summary(), "entries": entries, "next_sequence": next_sequence,
        }),
    );
}

pub(in crate::cli) fn run_tmux_stop(
    response: &mut ResponseWriter,
    arguments: TmuxStopArgs,
) -> Result<ExitCode> {
    let manifest_path = recording_manifest_path(arguments.output_root.as_deref(), &arguments.name)?;
    let initial_manifest = {
        let _manifest_lock = lock_tmux_manifest(&manifest_path)?;
        let mut manifest = read_tmux_manifest(&manifest_path)?;
        manifest.lifecycle = TmuxRecordingLifecycle::Stopping;
        manifest.composite_track.lifecycle = TmuxRecordingLifecycle::Stopping;
        manifest.updated_unix_ns = unix_time_ns()?;
        push_manifest_event(&mut manifest, "recording_stopping", None, None, None, None)?;
        write_tmux_manifest(&manifest_path, &mut manifest)?;

        manifest
    };

    let session_path = match &initial_manifest.composite_track.session {
        Some(session) => PathBuf::from(OsString::from_vec(session.as_bytes().to_vec())),
        None => {
            let registry = match arguments.output_root.as_ref() {
                Some(root) => SessionRegistry::open(root).into_diagnostic()?,
                None => SessionRegistry::open_default().into_diagnostic()?,
            };

            registry
                .resolve(&initial_manifest.composite_track.alias)
                .into_diagnostic()?
                .session_path()
        }
    };

    let stop_args = StopArgs {
        session: session_path,
        wait: arguments.wait,
        timeout_ms: arguments.timeout_ms,
        expected_presented_state_hash: None,
        request_id: None,
        json: false,
    };

    let mut quiet_response = ResponseWriter::quiet();
    let exit_code = run_stop(&mut quiet_response, stop_args)?;
    if exit_code != ExitCode::SUCCESS {
        return Err(miette::miette!(
            "composite tmux client track stop exited with failure"
        ));
    }

    let client = client_from_manifest(&initial_manifest);
    // A wrapper can be running before its asynchronous track registration is
    // published. Include those panes in shutdown as well as the saved links.
    if let Ok(report) = tmux_status_report(&client) {
        for pane in report.panes {
            if pane.selected && pane.recorded {
                let _ = client.run(["kill-pane", "-t", pane.pane_id.as_str()]);
            }
        }
    }

    for (pane_id, track) in &initial_manifest.pane_tracks {
        let _ = client.run(["kill-pane", "-t", pane_id.as_str()]);

        if arguments.wait {
            let session = PathBuf::from(OsString::from_vec(track.as_bytes().to_vec()));
            wait_for_completion(&session, Duration::from_millis(arguments.timeout_ms)).map_err(
                |error| {
                    return miette::miette!("could not complete pane track {pane_id}: {error}");
                },
            )?;
        }
    }

    if arguments.wait {
        let directory = manifest_path
            .parent()
            .ok_or_else(|| miette::miette!("tmux manifest has no parent"))?;

        let catalog = TmuxCatalog::read(directory, &initial_manifest).into_diagnostic()?;
        let deadline = Instant::now() + Duration::from_millis(arguments.timeout_ms);
        let mut next = 0;

        loop {
            let page = catalog.tracks(next, 128, true).into_diagnostic()?;
            if page.is_empty() {
                break;
            }

            for entry in page {
                next = entry
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| miette::miette!("tmux track sequence overflow"))?;

                let session = PathBuf::from(OsString::from_vec(entry.track.session.into_bytes()));
                wait_for_completion(&session, deadline.saturating_duration_since(Instant::now()))
                    .into_diagnostic()?;
            }
        }
    }

    let mut quiet_response = ResponseWriter::quiet();
    let _ = run_tmux_disable(
        &mut quiet_response,
        TmuxConnectionArgs {
            tmux: client.executable.clone(),
            socket_name: client.socket_name.clone(),
            socket_path: client.socket_path,
        },
    );

    let deadline = Instant::now() + Duration::from_millis(arguments.timeout_ms);
    let manifest = loop {
        let _manifest_lock = lock_tmux_manifest(&manifest_path)?;
        let mut manifest = read_tmux_manifest(&manifest_path)?;

        if arguments.wait && !matches!(manifest.lifecycle, TmuxRecordingLifecycle::Completed) {
            refresh_track_lifecycle(&manifest_path, &mut manifest)?;

            if !matches!(manifest.lifecycle, TmuxRecordingLifecycle::Completed) {
                if Instant::now() >= deadline {
                    return Err(miette::miette!(
                        "tmux recording has unfinished tracks: composite={:?}, catalog={:?}",
                        manifest.composite_track.session,
                        manifest.catalog,
                    ));
                }

                drop(_manifest_lock);

                thread::sleep(Duration::from_millis(20));

                continue;
            }
        }

        break manifest;
    };

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "tmux_recording_stopped",
            "recording_id": manifest.recording_id,
            "name": manifest.name,
            "lifecycle": manifest.lifecycle,
            "manifest": manifest_path,
            "composite_track": manifest.composite_track,
            "pane_tracks": manifest.pane_tracks,
            "coverage_gaps": manifest.coverage_gaps,
        }),
    );
}

pub(in crate::cli) fn run_tmux_video(
    response: &mut ResponseWriter,
    arguments: TmuxVideoArgs,
) -> Result<ExitCode> {
    let manifest_path = recording_manifest_path(arguments.output_root.as_deref(), &arguments.name)?;
    let manifest = read_tmux_manifest(&manifest_path)?;
    let session = manifest.composite_track.session.ok_or_else(|| {
        return miette::miette!("aggregate tmux recording has no linked composite client track");
    })?;

    let video_args = VideoFacadeArgs {
        session: PathBuf::from(OsString::from_vec(session.into_bytes())),
        out: Some(arguments.out),
        dry_run: false,
        preview_out: None,
        edit: None,
        preset: arguments.preset,
        fps: arguments.fps,
        renderer: arguments.renderer,
        semantic_raster_profile: None,
        resolution: None,
        scale: None,
        start_marker: None,
        end_marker: None,
        trim_idle: false,
        hold_start: None,
        hold_end: None,
        cursor_style: VideoCursorStyleArg::Recorded,
        columns: 4,
        tile_width: 320,
        max_frames: 12,
        change_only: false,
        preview_width: 640,
        json: false,
    };

    return run_video_facade(response, video_args);
}

pub(in crate::cli) fn run_tmux_bundle(
    response: &mut ResponseWriter,
    arguments: TmuxBundleArgs,
) -> Result<ExitCode> {
    let manifest_path = recording_manifest_path(arguments.output_root.as_deref(), &arguments.name)?;
    let manifest = tmux::streaming::create(&manifest_path, &arguments.out).into_diagnostic()?;

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "tmux_bundle",
            "directory": arguments.out,
            "recording_id": manifest.recording_id,
            "recording_name": manifest.recording_name,
            "track_count": manifest.track_count,
            "manifest": manifest,
        }),
    );
}

pub(super) fn coverage_from_options(client: &TmuxClient) -> Result<TmuxCoverageMode> {
    let text = option_text(client, OPTION_COVERAGE)?;

    return <TmuxCoverageMode as clap::ValueEnum>::from_str(&text, true)
        .map_err(|_| return miette::miette!("unsupported tmux coverage mode `{text}`"));
}

pub(super) fn refresh_configured_aggregate_manifest(client: &TmuxClient) -> Result<()> {
    let Some(path) = option_path(client, OPTION_AGGREGATE_MANIFEST)? else {
        return Ok(());
    };

    return refresh_aggregate_manifest(&path, client);
}

pub(super) fn record_observer_start(client: &TmuxClient) -> Result<()> {
    let Some(path) = option_path(client, OPTION_AGGREGATE_MANIFEST)? else {
        return Ok(());
    };
    let _lock = lock_tmux_manifest(&path)?;
    let mut manifest = read_tmux_manifest(&path)?;
    let now = unix_time_ns()?;
    let mut gap = TmuxCoverageGap {
        pane_id: None,
        kind: "observer_start".to_owned(),
        detail: "Aggregate observation resumed; transitions since the preceding snapshot may be missing. Pane recording runs independently.".to_owned(),
        first_observed_unix_ns: manifest.updated_unix_ns,
        last_observed_unix_ns: now,
        resolved_unix_ns: None,
    };
    push_manifest_event(
        &mut manifest,
        "coverage_gap_opened",
        None,
        None,
        None,
        Some(&serde_json::to_string(&gap).into_diagnostic()?),
    )?;

    gap.resolved_unix_ns = Some(now);
    push_manifest_event(
        &mut manifest,
        "coverage_gap_resolved",
        None,
        None,
        None,
        Some(&serde_json::to_string(&gap).into_diagnostic()?),
    )?;
    push_manifest_event(
        &mut manifest,
        "observer_started",
        None,
        None,
        None,
        Some(&process::id().to_string()),
    )?;

    return write_tmux_manifest(&path, &mut manifest);
}

fn refresh_track_lifecycle(path: &Path, manifest: &mut TmuxRecordingManifest) -> Result<()> {
    if matches!(manifest.lifecycle, TmuxRecordingLifecycle::Completed) {
        return Ok(());
    }

    let composite_completed = manifest
        .composite_track
        .session
        .as_ref()
        .is_some_and(|session| {
            return PathBuf::from(OsString::from_vec(session.as_bytes().to_vec()))
                .join("completion.json")
                .is_file();
        });

    if !composite_completed {
        return Ok(());
    }

    let directory = path
        .parent()
        .ok_or_else(|| miette::miette!("tmux manifest has no parent"))?;

    let catalog = TmuxCatalog::read(directory, manifest).into_diagnostic()?;
    let mut next_sequence = 0;
    let mut remaining = catalog.summary().unfinished_tracks;

    loop {
        let page = catalog.tracks(next_sequence, 128, true).into_diagnostic()?;
        if page.is_empty() {
            break;
        }

        for entry in page {
            next_sequence = entry
                .sequence
                .checked_add(1)
                .ok_or_else(|| miette::miette!("tmux track sequence overflow"))?;

            let session = PathBuf::from(OsString::from_vec(entry.track.session.into_bytes()));

            if session.join("completion.json").is_file() {
                let completion = read_completion(&session).into_diagnostic()?;
                if completion.session_id != entry.track.session_id {
                    return Err(miette::miette!("tmux track completion identity changed"));
                }

                push_manifest_event(
                    manifest,
                    "pane_track_completed",
                    None,
                    None,
                    Some(&entry.track.pane_id),
                    Some(&entry.track.session_id.to_string()),
                )?;

                remaining = remaining
                    .checked_sub(1)
                    .ok_or_else(|| miette::miette!("tmux unfinished track count underflow"))?;
            }
        }
    }

    drop(catalog);

    if remaining != 0 {
        return write_tmux_manifest(path, manifest);
    }

    manifest.lifecycle = TmuxRecordingLifecycle::Completed;
    manifest.composite_track.lifecycle = TmuxRecordingLifecycle::Completed;
    manifest.updated_unix_ns = unix_time_ns()?;
    push_manifest_event(manifest, "recording_completed", None, None, None, None)?;

    return write_tmux_manifest(path, manifest);
}

struct ObservedTmuxServerIdentity {
    pid: u32,
    socket_path: ByteString,
    socket_device: u64,
    socket_inode: u64,
}

fn observed_tmux_server_identity(client: &TmuxClient) -> Result<ObservedTmuxServerIdentity> {
    let pid_bytes = client.output(["display-message", "-p", "#{pid}"])?;
    let pid = str::from_utf8(&pid_bytes)
        .into_diagnostic()?
        .parse::<u32>()
        .into_diagnostic()?;

    let socket_bytes = client.output(["display-message", "-p", "#{socket_path}"])?;
    if socket_bytes.is_empty() {
        return Err(miette::miette!("tmux returned an empty server socket path"));
    }

    let socket = PathBuf::from(OsString::from_vec(socket_bytes.clone()));
    let metadata = fs_err::symlink_metadata(&socket)
        .map_err(|error| return miette::miette!("inspect tmux server socket: {error}"))?;

    if !metadata.file_type().is_socket() {
        return Err(miette::miette!(
            "tmux server path is not a Unix socket: {}",
            socket.display()
        ));
    }
    return Ok(ObservedTmuxServerIdentity {
        pid,
        socket_path: ByteString::from(socket_bytes.as_slice()),
        socket_device: metadata.dev(),
        socket_inode: metadata.ino(),
    });
}

fn bind_or_validate_server_identity(
    manifest: &mut TmuxRecordingManifest,
    observed: &ObservedTmuxServerIdentity,
) -> Result<bool> {
    match (
        manifest.server_pid,
        manifest.server_socket_path.as_ref(),
        manifest.server_socket_device,
        manifest.server_socket_inode,
    ) {
        (None, None, None, None) => {
            manifest.server_pid = Some(observed.pid);
            manifest.server_socket_path = Some(observed.socket_path.clone());
            manifest.server_socket_device = Some(observed.socket_device);
            manifest.server_socket_inode = Some(observed.socket_inode);

            return Ok(true);
        }
        (Some(pid), Some(socket_path), Some(socket_device), Some(socket_inode))
            if pid == observed.pid
                && socket_path == &observed.socket_path
                && socket_device == observed.socket_device
                && socket_inode == observed.socket_inode =>
        {
            return Ok(false);
        }
        (Some(pid), Some(socket_path), Some(socket_device), Some(socket_inode)) => {
            return Err(miette::miette!(
                "tmux server identity changed for aggregate {}: expected pid {pid}, socket {:?} ({socket_device}:{socket_inode}); observed pid {}, socket {:?} ({}:{})",
                manifest.name,
                socket_path,
                observed.pid,
                observed.socket_path,
                observed.socket_device,
                observed.socket_inode,
            ));
        }
        _ => {
            return Err(miette::miette!(
                "aggregate {} contains an incomplete tmux server identity",
                manifest.name
            ));
        }
    }
}

fn refresh_aggregate_manifest(path: &Path, client: &TmuxClient) -> Result<()> {
    let _manifest_lock = lock_tmux_manifest(path)?;
    let mut manifest = read_tmux_manifest(path)?;
    refresh_track_lifecycle(path, &mut manifest)?;

    let server_identity = observed_tmux_server_identity(client)?;

    if bind_or_validate_server_identity(&mut manifest, &server_identity)? {
        manifest.updated_unix_ns = unix_time_ns()?;
        push_manifest_event(
            &mut manifest,
            "server_identity_bound",
            None,
            None,
            None,
            Some(&server_identity.pid.to_string()),
        )?;
        write_tmux_manifest(path, &mut manifest)?;
    }

    let panes = tmux_pane_snapshots(client)?;
    let old = manifest
        .panes
        .iter()
        .cloned()
        .map(|pane| return (pane.pane_id.clone(), pane))
        .collect::<BTreeMap<_, _>>();

    let current = panes
        .iter()
        .map(|pane| return pane.pane_id.clone())
        .collect::<BTreeSet<_>>();

    for pane in &panes {
        if !old.contains_key(&pane.pane_id) {
            push_manifest_event(
                &mut manifest,
                "pane_created",
                Some(&pane.session_id),
                Some(&pane.window_id),
                Some(&pane.pane_id),
                None,
            )?;
        }

        if let Some(previous) = old.get(&pane.pane_id) {
            append_pane_transitions(&mut manifest, previous, pane)?;
        }

        if let Some(track) = pane.recording_session.as_ref()
            && manifest.pane_tracks.get(&pane.pane_id) != Some(track)
        {
            let linked = retain_pane_track(path, &mut manifest, &pane.pane_id, track)?;
            manifest
                .pane_tracks
                .insert(pane.pane_id.clone(), track.clone());
            push_manifest_event(
                &mut manifest,
                "pane_track_linked",
                Some(&pane.session_id),
                Some(&pane.window_id),
                Some(&pane.pane_id),
                Some(&serde_json::to_string(&linked).into_diagnostic()?),
            )?;
        }
    }

    for pane in old.values() {
        if !current.contains(&pane.pane_id) {
            push_manifest_event(
                &mut manifest,
                "pane_destroyed",
                Some(&pane.session_id),
                Some(&pane.window_id),
                Some(&pane.pane_id),
                None,
            )?;
        }
    }

    append_container_transitions(&mut manifest, &panes)?;

    let observed_unix_ns = unix_time_ns()?;
    let report = tmux_status_report(client)?;
    let mut gaps = permanent_coverage_gaps(observed_unix_ns);
    gaps.extend(report.panes.into_iter().filter_map(|pane| {
        if !pane.selected || pane.recorded {
            return None;
        }
        return Some(TmuxCoverageGap {
            pane_id: Some(pane.pane_id),
            kind: "uncovered_pane".to_owned(),
            detail: "selected pane root process is not a Termtrace pane wrapper".to_owned(),
            first_observed_unix_ns: observed_unix_ns,
            last_observed_unix_ns: observed_unix_ns,
            resolved_unix_ns: None,
        });
    }));
    merge_coverage_gaps(&mut manifest, &gaps, observed_unix_ns)?;
    manifest.panes = panes;
    manifest
        .pane_tracks
        .retain(|pane, _| return current.contains(pane));

    manifest.updated_unix_ns = observed_unix_ns;

    return write_tmux_manifest(path, &mut manifest);
}

fn retain_pane_track(
    manifest_path: &Path,
    manifest: &mut TmuxRecordingManifest,
    pane_id: &str,
    session: &ByteString,
) -> Result<TmuxPaneTrack> {
    let path = PathBuf::from(OsString::from_vec(session.as_bytes().to_vec()));
    let canonical = read_manifest(&path).into_diagnostic()?;
    let mut track = TmuxPaneTrack {
        pane_id: pane_id.to_owned(),
        session_id: canonical.session_id,
        session: session.clone(),
        linked_unix_ns: unix_time_ns()?,
    };

    let directory = manifest_path
        .parent()
        .ok_or_else(|| miette::miette!("tmux manifest has no parent"))?;

    let catalog = TmuxCatalog::read(directory, manifest).into_diagnostic()?;

    if let Some(existing) = catalog.track(canonical.session_id).into_diagnostic()? {
        let existing = existing.track;
        if existing.pane_id != track.pane_id || existing.session != track.session {
            return Err(miette::miette!(
                "tmux recording identity was rebound to another pane or path"
            ));
        }

        track.linked_unix_ns = existing.linked_unix_ns;
    }

    manifest
        .track_history
        .insert(canonical.session_id, track.clone());

    return Ok(track);
}

pub(super) fn register_pane_track(path: &Path, pane: &str, session: &Path) -> Result<()> {
    let _lock = lock_tmux_manifest(path)?;
    let mut manifest = read_tmux_manifest(path)?;
    let track = ByteString::from(session.as_os_str().as_bytes());
    let linked = retain_pane_track(path, &mut manifest, pane, &track)?;
    manifest.pane_tracks.insert(pane.to_owned(), track);
    push_manifest_event(
        &mut manifest,
        "pane_track_linked",
        None,
        None,
        Some(pane),
        Some(&serde_json::to_string(&linked).into_diagnostic()?),
    )?;

    if matches!(manifest.lifecycle, TmuxRecordingLifecycle::Completed) {
        manifest.lifecycle = TmuxRecordingLifecycle::Stopping;
    }

    return write_tmux_manifest(path, &mut manifest);
}

fn tmux_pane_snapshots(client: &TmuxClient) -> Result<Vec<TmuxPaneSnapshot>> {
    let format = format!(
        "#{{session_id}}\t#{{session_name}}\t#{{window_id}}\t#{{window_index}}\t#{{window_layout}}\t#{{window_active}}\t#{{window_zoomed_flag}}\t#{{pane_id}}\t#{{pane_active}}\t#{{pane_in_mode}}\t#{{pane_mode}}\t#{{{OPTION_PANE_SESSION}}}"
    );

    let output = client.output(["list-panes", "-a", "-F", format.as_str()])?;
    let mut panes = Vec::new();

    for line in output
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
    {
        let fields = line
            .splitn(12, |byte| return *byte == b'\t')
            .collect::<Vec<_>>();

        let [session_id, session_name, window_id, window_index, layout, window_active, zoomed, pane_id, pane_active, copy_mode, pane_mode, recording_session] =
            fields.as_slice()
        else {
            return Err(miette::miette!(
                "tmux returned an invalid aggregate pane row"
            ));
        };

        let text = |field: &[u8]| -> Result<String> {
            return str::from_utf8(field).map(str::to_owned).into_diagnostic();
        };
        panes.push(TmuxPaneSnapshot {
            session_id: text(session_id)?,
            session_name: text(session_name)?,
            window_id: text(window_id)?,
            window_index: text(window_index)?.parse().into_diagnostic()?,
            layout: text(layout)?,
            window_active: *window_active == b"1",
            zoomed: *zoomed == b"1",
            pane_id: text(pane_id)?,
            pane_active: *pane_active == b"1",
            copy_mode: *copy_mode != b"0",
            pane_mode: text(pane_mode)?,
            recording_session: (!recording_session.is_empty())
                .then(|| return ByteString::from(*recording_session)),
        });
    }

    return Ok(panes);
}

fn append_pane_transitions(
    manifest: &mut TmuxRecordingManifest,
    previous: &TmuxPaneSnapshot,
    pane: &TmuxPaneSnapshot,
) -> Result<()> {
    for (changed, kind, value) in [
        (
            previous.window_id != pane.window_id,
            "pane_moved",
            Some(pane.window_id.as_str()),
        ),
        (
            previous.layout != pane.layout,
            "layout_changed",
            Some(pane.layout.as_str()),
        ),
        (
            previous.window_active != pane.window_active,
            "active_window_changed",
            Some(if pane.window_active { "active" } else { "inactive" }),
        ),
        (
            previous.pane_active != pane.pane_active,
            "active_pane_changed",
            Some(if pane.pane_active { "active" } else { "inactive" }),
        ),
        (
            previous.zoomed != pane.zoomed,
            "zoom_changed",
            Some(if pane.zoomed { "zoomed" } else { "unzoomed" }),
        ),
        (
            previous.copy_mode != pane.copy_mode || previous.pane_mode != pane.pane_mode,
            "copy_mode_changed",
            Some(if pane.copy_mode { pane.pane_mode.as_str() } else { "off" }),
        ),
    ] {
        if changed {
            push_manifest_event(
                manifest,
                kind,
                Some(&pane.session_id),
                Some(&pane.window_id),
                Some(&pane.pane_id),
                value,
            )?;
        }
    }

    return Ok(());
}

fn append_container_transitions(
    manifest: &mut TmuxRecordingManifest,
    panes: &[TmuxPaneSnapshot],
) -> Result<()> {
    let old_sessions = manifest
        .panes
        .iter()
        .map(|pane| return pane.session_id.clone())
        .collect::<BTreeSet<_>>();

    let new_sessions = panes
        .iter()
        .map(|pane| return pane.session_id.clone())
        .collect::<BTreeSet<_>>();

    let old_windows = manifest
        .panes
        .iter()
        .map(|pane| return pane.window_id.clone())
        .collect::<BTreeSet<_>>();

    let new_windows = panes
        .iter()
        .map(|pane| return pane.window_id.clone())
        .collect::<BTreeSet<_>>();

    for session in new_sessions.difference(&old_sessions) {
        push_manifest_event(manifest, "session_created", Some(session), None, None, None)?;
    }

    for session in old_sessions.difference(&new_sessions) {
        push_manifest_event(
            manifest,
            "session_destroyed",
            Some(session),
            None,
            None,
            None,
        )?;
    }

    for window in new_windows.difference(&old_windows) {
        push_manifest_event(manifest, "window_created", None, Some(window), None, None)?;
    }

    for window in old_windows.difference(&new_windows) {
        push_manifest_event(manifest, "window_destroyed", None, Some(window), None, None)?;
    }

    return Ok(());
}

fn push_manifest_event(
    manifest: &mut TmuxRecordingManifest,
    kind: &str,
    session_id: Option<&str>,
    window_id: Option<&str>,
    pane_id: Option<&str>,
    value: Option<&str>,
) -> Result<()> {
    manifest.events.push(TmuxLifecycleEvent {
        sequence: manifest
            .events
            .last()
            .map_or_else(
                || {
                    return manifest
                        .lifecycle_journal
                        .as_ref()
                        .map_or(0, |boundary| return boundary.sequence);
                },
                |event| return event.sequence,
            )
            .checked_add(1)
            .ok_or_else(|| miette::miette!("tmux lifecycle sequence overflow"))?,
        observed_unix_ns: unix_time_ns()?,
        kind: kind.to_owned(),
        session_id: session_id.map(str::to_owned),
        window_id: window_id.map(str::to_owned),
        pane_id: pane_id.map(str::to_owned),
        value: value.map(str::to_owned),
    });

    return Ok(());
}

fn merge_coverage_gaps(
    manifest: &mut TmuxRecordingManifest,
    current: &[TmuxCoverageGap],
    observed_unix_ns: u128,
) -> Result<()> {
    for gap in current {
        let existing = manifest.coverage_gaps.iter_mut().find(|existing| {
            return existing.resolved_unix_ns.is_none() && same_coverage_gap(existing, gap);
        });
        if let Some(existing) = existing {
            if existing.first_observed_unix_ns == 0 {
                existing.first_observed_unix_ns = observed_unix_ns;
            }

            existing.last_observed_unix_ns = observed_unix_ns;
            continue;
        }

        push_manifest_event(
            manifest,
            "coverage_gap_opened",
            None,
            None,
            gap.pane_id.as_deref(),
            Some(&serde_json::to_string(gap).into_diagnostic()?),
        )?;

        manifest.coverage_gaps.push(gap.clone());
    }

    let resolved: Vec<(usize, Option<String>, String)> = manifest
        .coverage_gaps
        .iter()
        .enumerate()
        .filter(|(_, gap)| {
            return gap.resolved_unix_ns.is_none()
                && !current
                    .iter()
                    .any(|candidate| return same_coverage_gap(gap, candidate));
        })
        .map(|(index, gap)| return (index, gap.pane_id.clone(), gap.kind.clone()))
        .collect();

    for (index, pane_id, _kind) in resolved {
        let gap = manifest.coverage_gaps.get_mut(index).ok_or_else(|| {
            return miette::miette!("resolved tmux coverage gap index is out of bounds");
        })?;
        gap.resolved_unix_ns = Some(observed_unix_ns);
        let value = serde_json::to_string(gap).into_diagnostic()?;

        push_manifest_event(
            manifest,
            "coverage_gap_resolved",
            None,
            None,
            pane_id.as_deref(),
            Some(&value),
        )?;
    }

    return Ok(());
}

fn same_coverage_gap(left: &TmuxCoverageGap, right: &TmuxCoverageGap) -> bool {
    return left.pane_id == right.pane_id && left.kind == right.kind && left.detail == right.detail;
}

fn permanent_coverage_gaps(observed_unix_ns: u128) -> Vec<TmuxCoverageGap> {
    return vec![
        TmuxCoverageGap {
            pane_id: None,
            kind: "polling_interval".to_owned(),
            detail: "tmux state transitions shorter than the supervisor polling interval may only be visible in the composite client track".to_owned(),
            first_observed_unix_ns: observed_unix_ns,
            last_observed_unix_ns: observed_unix_ns,
            resolved_unix_ns: None,
        },
        TmuxCoverageGap {
            pane_id: None,
            kind: "client_overlay_index".to_owned(),
            detail: "tmux messages and command prompts are captured by the composite client track but are not separately indexed in aggregate lifecycle events".to_owned(),
            first_observed_unix_ns: observed_unix_ns,
            last_observed_unix_ns: observed_unix_ns,
            resolved_unix_ns: None,
        },
    ];
}

fn configured_tmux_command(
    client: &TmuxClient,
    config: Option<&Path>,
    requested: &[OsString],
) -> Result<Vec<OsString>> {
    let requested_executable = requested.first().map(PathBuf::from).ok_or_else(|| {
        return miette::miette!("tmux record command is empty");
    })?;

    if !same_executable(&requested_executable, &client.executable)
        && requested_executable.file_name() != client.executable.file_name()
    {
        return Err(miette::miette!(
            "tmux record command executable {} does not match configured tmux executable {}",
            requested_executable.display(),
            client.executable.display()
        ));
    }

    let mut command = Vec::with_capacity(requested.len().saturating_add(4));
    command.push(client.executable.as_os_str().to_owned());

    if let Some(socket_name) = &client.socket_name {
        command.push(OsString::from("-L"));
        command.push(socket_name.clone());
    }

    if let Some(socket_path) = &client.socket_path {
        command.push(OsString::from("-S"));
        command.push(socket_path.as_os_str().to_owned());
    }

    if let Some(config) = config {
        command.push(OsString::from("-f"));
        command.push(config.as_os_str().to_owned());
    }

    command.extend(requested.iter().skip(1).cloned());

    return Ok(command);
}

fn client_from_manifest(manifest: &TmuxRecordingManifest) -> TmuxClient {
    return TmuxClient {
        executable: PathBuf::from(OsString::from_vec(
            manifest.tmux_executable.as_bytes().to_vec(),
        )),
        socket_name: manifest
            .socket_name
            .as_ref()
            .map(|value| return OsString::from_vec(value.as_bytes().to_vec())),
        socket_path: manifest
            .socket_path
            .as_ref()
            .map(|value| return PathBuf::from(OsString::from_vec(value.as_bytes().to_vec()))),
    };
}

fn recording_manifest_path(output_root: Option<&Path>, name: &str) -> Result<PathBuf> {
    validate_recording_name(name)?;

    let registry = match output_root {
        Some(root) => SessionRegistry::open(root).into_diagnostic()?,
        None => SessionRegistry::open_default().into_diagnostic()?,
    };

    return Ok(registry
        .state_root()
        .join(TMUX_RECORDINGS_DIRECTORY)
        .join(name)
        .join("manifest.json"));
}

fn validate_recording_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 40
        || !name
            .bytes()
            .all(|byte| return byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        || !name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(miette::miette!(
            "tmux recording name must be 1-40 ASCII letters, digits, '-' or '_', beginning with a letter or digit"
        ));
    }
    return Ok(());
}

fn create_recording_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| return miette::miette!("tmux recording directory has no parent"))?;

    fs_err::create_dir_all(parent).into_diagnostic()?;
    fs_err::set_permissions(parent, Permissions::from_mode(0o700)).into_diagnostic()?;

    if let Err(error) = fs_err::create_dir(path) {
        if error.kind() == io::ErrorKind::AlreadyExists {
            return Err(miette::miette!(
                "tmux recording `{}` already exists",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        return Err(error).into_diagnostic();
    }

    fs_err::set_permissions(path, Permissions::from_mode(0o700)).into_diagnostic()?;

    return Ok(());
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).into_diagnostic()?;
    file.write_all(bytes).into_diagnostic()?;
    file.sync_all().into_diagnostic()?;

    return Ok(());
}

fn lock_tmux_manifest(path: &Path) -> Result<Flock<File>> {
    let directory = path
        .parent()
        .ok_or_else(|| return miette::miette!("tmux manifest has no parent directory"))?;

    if !directory.is_dir() {
        let name = directory.file_name().map_or_else(
            || return "unknown".into(),
            |name| return name.to_string_lossy(),
        );

        return Err(miette::miette!("tmux recording `{name}` was not found"));
    }

    let lock_path = directory.join(".manifest.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(&lock_path)
        .into_diagnostic()?;

    let metadata = file.metadata().into_diagnostic()?;
    if metadata.mode() & S_IFMT != S_IFREG || metadata.uid() != geteuid().as_raw() {
        return Err(miette::miette!(
            "tmux manifest lock must be a regular file owned by the current user"
        ));
    }
    return Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, error)| {
        return miette::miette!("could not lock tmux manifest: {error}");
    });
}

fn write_tmux_manifest(path: &Path, manifest: &mut TmuxRecordingManifest) -> Result<()> {
    let mut snapshot = manifest.clone();
    let directory = path
        .parent()
        .ok_or_else(|| miette::miette!("tmux manifest has no parent"))?;
    tmux_journal::append(directory, &mut snapshot).into_diagnostic()?;
    let mut bytes = serde_json::to_vec_pretty(&snapshot).into_diagnostic()?;
    bytes.push(b'\n');

    let temporary = path.with_extension(format!("json.{}.tmp", uuid::Uuid::now_v7()));
    write_private_file(&temporary, &bytes)?;
    fs_err::rename(&temporary, path).into_diagnostic()?;

    sync_directory(directory).into_diagnostic()?;
    *manifest = snapshot;

    return Ok(());
}

fn read_tmux_manifest(path: &Path) -> Result<TmuxRecordingManifest> {
    if !path.is_file() {
        let name = path
            .parent()
            .and_then(|parent| return parent.file_name())
            .map_or_else(
                || return "unknown".into(),
                |name| return name.to_string_lossy(),
            );

        return Err(miette::miette!("tmux recording `{name}` was not found"));
    }

    let bytes = fs_err::read(path).into_diagnostic()?;
    let manifest: TmuxRecordingManifest = serde_json::from_slice(&bytes).into_diagnostic()?;
    if manifest.format != TMUX_RECORDING_FORMAT || manifest.version != TMUX_RECORDING_VERSION {
        return Err(miette::miette!(
            "unsupported aggregate tmux recording manifest"
        ));
    }
    return Ok(manifest);
}

fn unix_time_ns() -> Result<u128> {
    return SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| return duration.as_nanos())
        .into_diagnostic();
}
