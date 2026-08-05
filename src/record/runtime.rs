//! Runtime orchestration for one-shot PTY recording.

use std::{
    env::consts::{ARCH, OS},
    io::{self, Write},
    mem::replace,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

use portable_pty::{ExitStatus, PtySize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::write_secure,
    control::{CONTROL_SOCKET_NAME, ProcessSignal},
    environment::EffectiveEnvironment,
    event::{
        ByteString, Event, EventLogHeader, InputDescriptor, InputKind, InputSource, LifecycleKind,
        TerminalColors,
    },
    event_log::{EventLogWriter, SessionClock},
    ghostty_vt::{GhosttyVt, supported_geometry_cell_count},
    provenance::collect_execution_provenance,
    request_journal::{MUTATION_JOURNAL_NAME, MutationJournal},
    session::{
        COMPLETION_VALIDATION_VERSION, GHOSTTY_REPLAY_PROFILE_NAME, ResourceLimitKind,
        ResourceLimitOutcome, ResourceLimits, SESSION_COMPLETION_FORMAT,
        SESSION_COMPLETION_VERSION, SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION,
        SessionCompletion, SessionGeometry, SessionManifest, canonical_manifest_bytes,
        manifest_hash,
    },
    terminal_mode::RawTerminalGuard,
};

use super::{
    control::{
        MutationHistory, ScreenWaiter, apply_signal, evaluate_screen_waiters, fail_screen_waiters,
        handle_control_request,
    },
    input::{apply_input, apply_resize},
    model::{RecordOptions, RecordOutcome, RuntimeMessage, SpawnedCommand, TerminalGeometry},
    presentation::{
        LiveTerminal, PresentationTracker, QueryTracker, geometry_cell_size,
        process_semantic_bytes, process_semantic_output, resize_semantic,
    },
    process::{
        RuntimeWorkers, create_session_directory, invalid_geometry, resolve_working_directory,
        spawn_command, terminate_process_group,
    },
    support::{finish_with_cleanup, terminal_error},
};

const DESCENDANT_GRACE_PERIOD: Duration = Duration::from_millis(250);
const DESCENDANT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const MUTATION_CACHE_CAPACITY: usize = 256;

struct RuntimeOutcome {
    status: ExitStatus,
    output_bytes: u64,
    resource_limit: Option<ResourceLimitOutcome>,
}

struct RuntimeProgress {
    output_closed: bool,
    output_bytes: u64,
    resource_limit: Option<ResourceLimitOutcome>,
}

/// Record a command running in a real pseudo-terminal and mirror its output.
///
/// # Errors
///
/// Returns an error when configuration is invalid, secure artifacts cannot be
/// created, PTY setup or child execution fails, output cannot be mirrored, or
/// the canonical event log cannot be durably committed.
pub fn record(options: &RecordOptions) -> Result<RecordOutcome> {
    let stdout = io::stdout();
    let mut terminal_mode = RawTerminalGuard::enter(options.forward_stdin)?;
    let result = record_to(options, &mut stdout.lock());
    return finish_with_cleanup(result, terminal_mode.restore());
}

fn record_to(options: &RecordOptions, output: &mut impl Write) -> Result<RecordOutcome> {
    let pty_size = validate_options(options)?;
    let effective_environment = options.environment.resolve(&options.term)?;
    // Resolve this once and pass the same owned path to provenance and spawn.
    // Leaving cwd unset is not equivalent: portable-pty falls back to $HOME,
    // which would make a wrapper command silently leave the caller's project.
    let working_directory = resolve_working_directory(options.working_directory.as_deref())?;
    if let Some(profile) = &options.ghostty_profile {
        profile.validate()?;
    }
    create_session_directory(&options.output_directory)?;
    if let Some(profile) = &options.ghostty_profile {
        write_secure(
            &options.output_directory.join(GHOSTTY_REPLAY_PROFILE_NAME),
            &profile.replay_config,
        )?;
    }
    let clock = SessionClock::now()?;
    let session_id = Uuid::now_v7();
    let session_manifest_hash = write_session_manifest(
        options,
        &working_directory,
        pty_size,
        session_id,
        clock.started_unix_ns(),
        &effective_environment,
    )?;
    let header = EventLogHeader::new(
        session_id,
        &session_manifest_hash,
        clock.started_unix_ns(),
        options.cols,
        options.rows,
        pty_size.pixel_width,
        pty_size.pixel_height,
        &options.term,
    )
    .with_colors(options.colors.clone());
    let event_log_path = options.output_directory.join("events.jsonl");
    let mut event_log = EventLogWriter::create_with_policy(
        &event_log_path,
        &header,
        clock,
        options.durability_policy.clone(),
    )?;

    let mut spawned = spawn_command(
        options,
        &working_directory,
        pty_size,
        &effective_environment,
    )?;
    if let Err(error) = event_log.append(Event::Lifecycle {
        lifecycle_kind: LifecycleKind::Spawned,
        pid: spawned.pid,
    }) {
        terminate_process_group(spawned.pid);
        let _ = spawned.child.kill();
        return Err(error);
    }
    let runtime = match consume_runtime(
        &mut spawned,
        &mut event_log,
        output,
        options.forward_stdin,
        TerminalGeometry::from(pty_size),
        &options.output_directory,
        &options.colors,
        &options.resource_limits,
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            terminate_process_group(spawned.pid);
            let _ = spawned.child.kill();
            return Err(error);
        }
    };
    let exit_code = i32::try_from(runtime.status.exit_code()).ok();
    let signal_name = runtime.status.signal().map(str::to_owned);
    let terminal_event_seq = event_log.append(Event::Exit {
        code: exit_code,
        signal: None,
        signal_name: signal_name.clone(),
    })?;
    let header_bytes = serde_json::to_vec(&header).map_err(|source| {
        return Error::InvalidJson {
            path: event_log_path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    write_session_completion(
        options,
        &SessionCompletion {
            format: SESSION_COMPLETION_FORMAT.to_owned(),
            version: SESSION_COMPLETION_VERSION,
            validation_version: COMPLETION_VALIDATION_VERSION,
            session_id,
            session_manifest_hash,
            event_header_hash: blake3::hash(&header_bytes).to_hex().to_string(),
            exit_code,
            signal_name: signal_name.clone(),
            output_bytes: runtime.output_bytes,
            final_event_hash: event_log.current_hash(),
            event_count: event_log.next_seq(),
            terminal_event_seq,
            durability: event_log.durability_record(),
            resource_limit: runtime.resource_limit,
        },
    )?;

    return Ok(RecordOutcome {
        exit_code,
        signal_name,
        output_bytes: runtime.output_bytes,
        event_log: event_log_path,
    });
}

fn write_session_manifest(
    options: &RecordOptions,
    working_directory: &Path,
    size: PtySize,
    session_id: Uuid,
    started_unix_ns: u128,
    effective_environment: &EffectiveEnvironment,
) -> Result<String> {
    let manifest = SessionManifest {
        format: SESSION_MANIFEST_FORMAT.to_owned(),
        version: SESSION_MANIFEST_VERSION,
        session_id,
        creation_alias: options.creation_alias.clone(),
        lineage: options.lineage.clone(),
        import_provenance: options.import_provenance.clone(),
        started_unix_ns,
        command: options
            .command
            .iter()
            .map(|value| return ByteString::from(value.as_os_str().as_bytes()))
            .collect(),
        working_directory: Some(ByteString::from(working_directory.as_os_str().as_bytes())),
        geometry: SessionGeometry {
            cols: size.cols,
            rows: size.rows,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        },
        geometry_provenance: options.geometry_provenance.clone(),
        ghostty_visual: options
            .ghostty_profile
            .as_ref()
            .map(|profile| return profile.provenance.clone()),
        execution_provenance: collect_execution_provenance(
            &options.provenance_policy,
            effective_environment,
            &options.command,
            working_directory,
        ),
        child_environment: effective_environment.policy().clone(),
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: OS.to_owned(),
        architecture: ARCH.to_owned(),
        event_log: "events.jsonl".to_owned(),
        control_socket: CONTROL_SOCKET_NAME.to_owned(),
        request_journal: MUTATION_JOURNAL_NAME.to_owned(),
        contains_sensitive_terminal_data: true,
        durability_policy: options.durability_policy.clone(),
        resource_limits: options.resource_limits.clone(),
    };
    let encoded = canonical_manifest_bytes(&manifest)?;
    let hash = manifest_hash(&encoded);
    write_secure(&options.output_directory.join("session.json"), &encoded)?;
    return Ok(hash);
}

fn write_session_completion(options: &RecordOptions, completion: &SessionCompletion) -> Result<()> {
    return write_json_secure(
        &options.output_directory.join("completion.json"),
        &completion,
    );
}

fn write_json_secure(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let encoded = serde_json::to_vec_pretty(value).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return write_secure(path, &encoded);
}

fn validate_options(options: &RecordOptions) -> Result<PtySize> {
    if options.command.is_empty() {
        return Err(terminal_error("validate command", "command is empty"));
    }
    if options.resource_limits.output_bytes == Some(0) {
        return Err(terminal_error(
            "validate resource limits",
            "resource budgets must be greater than zero",
        ));
    }
    options.environment.validate()?;
    let pixel_width = options
        .cols
        .checked_mul(options.cell_width_px)
        .ok_or_else(|| return invalid_geometry(options))?;
    let pixel_height = options
        .rows
        .checked_mul(options.cell_height_px)
        .ok_or_else(|| return invalid_geometry(options))?;
    if supported_geometry_cell_count(options.cols, options.rows).is_none()
        || options.cell_width_px == 0
        || options.cell_height_px == 0
    {
        return Err(invalid_geometry(options));
    }
    return Ok(PtySize {
        rows: options.rows,
        cols: options.cols,
        pixel_width,
        pixel_height,
    });
}

fn consume_runtime(
    spawned: &mut SpawnedCommand,
    event_log: &mut EventLogWriter,
    output: &mut impl Write,
    forward_stdin: bool,
    initial_geometry: TerminalGeometry,
    session_directory: &Path,
    colors: &TerminalColors,
    resource_limits: &ResourceLimits,
) -> Result<RuntimeOutcome> {
    let mut semantic = GhosttyVt::new(initial_geometry.cols, initial_geometry.rows, 100_000)?;
    semantic.apply_terminal_colors(colors)?;
    semantic.resize(
        initial_geometry.cols,
        initial_geometry.rows,
        geometry_cell_size(initial_geometry.pixel_width, initial_geometry.cols),
        geometry_cell_size(initial_geometry.pixel_height, initial_geometry.rows),
    )?;
    let presented = semantic.snapshot()?;
    let mut live_terminal = LiveTerminal {
        semantic,
        presentation_tracker: PresentationTracker::new(presented.modes.synchronized_output),
        presented,
        query_tracker: QueryTracker::default(),
        geometry: initial_geometry,
    };
    let (sender, receiver) = mpsc::sync_channel::<RuntimeMessage>(64);
    let journal = MutationJournal::create(session_directory)?;
    let mut mutation_history = MutationHistory::new(journal, MUTATION_CACHE_CAPACITY);
    let mut screen_waiters = Vec::new();
    let reader = replace(&mut spawned.reader, Box::new(io::empty()));
    let workers = RuntimeWorkers::start(
        reader,
        &sender,
        forward_stdin,
        initial_geometry,
        session_directory,
    )?;
    drop(sender);

    let mut progress = RuntimeProgress {
        output_closed: false,
        output_bytes: 0,
        resource_limit: None,
    };
    let mut exit_status = None;
    let mut drain_deadline: Option<Instant> = None;
    let mut process_group_killed = false;
    let mut resource_kill_deadline: Option<Instant> = None;
    let result = (|| {
        while !progress.output_closed || exit_status.is_none() {
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(message) => process_runtime_message(
                    message,
                    spawned,
                    event_log,
                    output,
                    &mut progress,
                    &mut mutation_history,
                    &mut live_terminal,
                    &mut screen_waiters,
                )?,
                Err(RecvTimeoutError::Disconnected) if !progress.output_closed => {
                    return Err(terminal_error(
                        "receive runtime event",
                        "PTY reader disconnected before reporting closure",
                    ));
                }
                Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout) => {}
            }
            event_log.flush_due()?;
            evaluate_screen_waiters(
                event_log,
                &live_terminal,
                &mut screen_waiters,
                false,
                progress.output_closed,
            )?;

            if progress.resource_limit.is_none()
                && let Some(limit) = resource_limits.output_bytes
                && progress.output_bytes > limit
            {
                let outcome = ResourceLimitOutcome {
                    kind: ResourceLimitKind::OutputBytes,
                    limit,
                    observed: progress.output_bytes,
                };
                event_log.append(Event::HostError {
                    code: "output_budget_exceeded".to_owned(),
                    message: format!(
                        "output budget of {limit} bytes was exceeded at {} observed bytes; every observed byte was retained",
                        progress.output_bytes
                    ),
                })?;
                apply_signal(
                    spawned.pid,
                    event_log,
                    Uuid::now_v7(),
                    ProcessSignal::Terminate,
                    true,
                )?;
                progress.resource_limit = Some(outcome);
                resource_kill_deadline = Some(Instant::now() + DESCENDANT_GRACE_PERIOD);
            }

            if exit_status.is_none() {
                exit_status = spawned
                    .child
                    .try_wait()
                    .map_err(|error| return terminal_error("poll child", error.to_string()))?;
                if exit_status.is_some() && !progress.output_closed {
                    drain_deadline = Some(Instant::now() + DESCENDANT_GRACE_PERIOD);
                }
            }
            if exit_status.is_some() && !progress.output_closed {
                evaluate_screen_waiters(
                    event_log,
                    &live_terminal,
                    &mut screen_waiters,
                    true,
                    false,
                )?;
            }
            if exit_status.is_none()
                && resource_kill_deadline.is_some_and(|deadline| return Instant::now() >= deadline)
                && !process_group_killed
            {
                event_log.append(Event::HostError {
                    code: "output_budget_force_kill".to_owned(),
                    message: "child did not exit after output-budget termination; process group was killed"
                        .to_owned(),
                })?;
                terminate_process_group(spawned.pid);
                process_group_killed = true;
            }
            if drain_deadline.is_some_and(|deadline| return Instant::now() >= deadline)
                && !progress.output_closed
            {
                if process_group_killed {
                    return Err(terminal_error(
                        "drain PTY output",
                        "PTY remained open after terminating retained process-group descendants",
                    ));
                }
                terminate_process_group(spawned.pid);
                process_group_killed = true;
                drain_deadline = Some(Instant::now() + DESCENDANT_DRAIN_TIMEOUT);
            }
        }
        evaluate_screen_waiters(
            event_log,
            &live_terminal,
            &mut screen_waiters,
            true,
            progress.output_closed,
        )?;
        fail_screen_waiters(
            &mut screen_waiters,
            "child_completed",
            "child completed before screen predicate matched",
        );
        return exit_status
            .ok_or_else(|| return terminal_error("wait for child", "missing exit status"));
    })();

    if result.is_err() {
        terminate_process_group(spawned.pid);
        let _ = spawned.child.kill();
    }
    let status = workers.finish(result)?;

    return Ok(RuntimeOutcome {
        status,
        output_bytes: progress.output_bytes,
        resource_limit: progress.resource_limit,
    });
}

fn process_runtime_message(
    message: RuntimeMessage,
    spawned: &mut SpawnedCommand,
    event_log: &mut EventLogWriter,
    output: &mut impl Write,
    progress: &mut RuntimeProgress,
    mutation_history: &mut MutationHistory,
    live: &mut LiveTerminal,
    screen_waiters: &mut Vec<ScreenWaiter>,
) -> Result<()> {
    match message {
        RuntimeMessage::Output(data) => {
            progress.output_bytes = progress
                .output_bytes
                .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
            event_log.append_output(Event::Output {
                data: ByteString::from(data.as_slice()),
            })?;
            output
                .write_all(&data)
                .and_then(|()| return output.flush())
                .map_err(|source| {
                    return Error::Filesystem {
                        path: PathBuf::from("<stdout>"),
                        source,
                    };
                })?;
            process_semantic_output(live, spawned.writer.as_mut(), event_log, &data)?;
            let latest = live.semantic.snapshot()?;
            if !latest.modes.synchronized_output {
                live.presented = latest;
            }
        }
        RuntimeMessage::OutputClosed => {
            let pending = live.presentation_tracker.finish();
            if !pending.is_empty() {
                process_semantic_bytes(live, spawned.writer.as_mut(), event_log, &pending)?;
                let latest = live.semantic.snapshot()?;
                if !latest.modes.synchronized_output {
                    live.presented = latest;
                }
            }
            progress.output_closed = true;
            event_log.append(Event::Lifecycle {
                lifecycle_kind: LifecycleKind::OutputClosed,
                pid: spawned.pid,
            })?;
        }
        RuntimeMessage::ReadFailed(reason) => {
            return Err(terminal_error("read PTY output", reason));
        }
        RuntimeMessage::Input(data) => {
            apply_input(
                spawned.writer.as_mut(),
                event_log,
                Uuid::now_v7(),
                InputSource::Human,
                InputKind::Raw,
                InputDescriptor::Raw,
                None,
                &data,
            )?;
        }
        RuntimeMessage::InputClosed => {}
        RuntimeMessage::InputFailed(reason) => {
            return Err(terminal_error("read standard input", reason));
        }
        RuntimeMessage::Resize(geometry) => {
            apply_resize(spawned.master.as_mut(), event_log, Uuid::now_v7(), geometry)?;
            resize_semantic(&mut live.semantic, geometry)?;
            live.geometry = geometry;
            let latest = live.semantic.snapshot()?;
            if !latest.modes.synchronized_output {
                live.presented = latest;
            }
        }
        RuntimeMessage::ResizeFailed(reason) => {
            return Err(terminal_error("read terminal geometry", reason));
        }
        RuntimeMessage::Control { request, response } => {
            handle_control_request(
                spawned,
                event_log,
                request,
                response,
                mutation_history,
                live,
                screen_waiters,
            )?;
        }
        RuntimeMessage::ControlServerFailed(reason) => {
            return Err(terminal_error("serve control socket", reason));
        }
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        ffi::OsString,
        fs::Permissions,
        io::Read as _,
        iter,
        os::unix::fs::PermissionsExt as _,
        os::unix::net::UnixStream,
        thread,
        time::Instant,
    };

    use tempfile::TempDir;

    use super::super::{
        control::spawn_control_server,
        input::encode_paste,
        presentation::{MAX_PRESENTATION_SEQUENCE_BYTES, PresentationAction},
        process::SESSION_DIRECTORY_MODE,
    };
    use super::*;
    use crate::{
        control::{
            ActorBoundaries, CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlRequest,
            ControlResponse, ControlResult,
        },
        environment::{EnvironmentBase, EnvironmentSpec},
        event::Event,
        event_log::{CanonicalBoundary, DurabilityPolicy, EventLog},
        provenance::ProvenancePolicy,
        request_journal::guarded_command_hash,
        session::{GeometryProvenance, SessionCompletion, SessionManifest, validate_completed_log},
    };

    fn cached_test_response(request_id: Uuid, next_event_seq: u64) -> ControlResponse {
        let boundary = CanonicalBoundary {
            next_event_seq,
            event_hash: format!("{next_event_seq:064x}"),
            output_bytes: 0,
        };
        return ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Ok {
                data: ControlData::Acknowledged {
                    boundaries: ActorBoundaries {
                        durable: boundary.clone(),
                        observed: boundary,
                    },
                },
            },
        };
    }

    fn isolated_options(output_directory: PathBuf, command: Vec<OsString>) -> RecordOptions {
        return RecordOptions {
            output_directory,
            creation_alias: None,
            lineage: None,
            import_provenance: None,
            command,
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            geometry_provenance: GeometryProvenance::default(),
            ghostty_profile: None,
            term: "xterm-ghostty".to_owned(),
            environment: EnvironmentSpec::default(),
            colors: TerminalColors::default(),
            forward_stdin: false,
            durability_policy: DurabilityPolicy::default(),
            provenance_policy: ProvenancePolicy::default(),
            resource_limits: ResourceLimits::default(),
        };
    }

    #[test]
    fn clear_environment_is_exact_and_secret_values_do_not_enter_evidence() {
        let temporary = TempDir::new().expect("temporary directory");
        let session = temporary.path().join("isolated");
        let secret = b"not-for-any-artifact";
        let mut options = isolated_options(
            session.clone(),
            vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(
                    "test \"${#TERMTRACE_PRIVATE_TOKEN}\" -eq 20; test -z \"${HOME+x}\"",
                ),
            ],
        );
        options.environment = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            set: BTreeMap::from([(
                "TERMTRACE_PRIVATE_TOKEN".to_owned(),
                ByteString::from(secret.as_slice()),
            )]),
            ..EnvironmentSpec::default()
        };

        let outcome = record(&options).expect("isolated recording");
        assert_eq!(outcome.exit_code, Some(0));
        let manifest: SessionManifest =
            serde_json::from_slice(&fs_err::read(session.join("session.json")).expect("manifest"))
                .expect("manifest JSON");
        assert_eq!(
            manifest.child_environment.explicit_names,
            BTreeSet::from(["TERMTRACE_PRIVATE_TOKEN".to_owned()])
        );
        for entry in fs_err::read_dir(&session).expect("session entries") {
            let path = entry.expect("entry").path();
            if path.is_file() {
                let bytes = fs_err::read(&path).expect("artifact bytes");
                assert!(
                    !bytes
                        .windows(secret.len())
                        .any(|window| return window == secret),
                    "secret leaked into {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn output_budget_retains_observed_bytes_and_records_termination() {
        let temporary = TempDir::new().expect("temporary directory");
        let session = temporary.path().join("budget");
        let mut options = isolated_options(
            session.clone(),
            vec![
                OsString::from("/usr/bin/python3"),
                OsString::from("-c"),
                OsString::from("import os,time; os.write(1,b'x'*131072); time.sleep(30)"),
            ],
        );
        options.resource_limits.output_bytes = Some(1024);

        let mut mirrored_output = Vec::new();
        let outcome = record_to(&options, &mut mirrored_output).expect("budgeted recording");
        assert!(outcome.output_bytes > 1024);
        assert_eq!(mirrored_output.len() as u64, outcome.output_bytes);
        let completion: SessionCompletion = serde_json::from_slice(
            &fs_err::read(session.join("completion.json")).expect("completion"),
        )
        .expect("completion JSON");
        let limit = completion.resource_limit.expect("resource outcome");
        assert_eq!(limit.kind, ResourceLimitKind::OutputBytes);
        assert_eq!(limit.limit, 1024);
        assert!(limit.observed > limit.limit);
        assert_eq!(completion.output_bytes, outcome.output_bytes);
        let log = EventLog::read(session.join("events.jsonl")).expect("event log");
        let retained = log.events.iter().fold(0_u64, |total, record| {
            return total
                + match &record.event {
                    Event::Output { data } => data.as_bytes().len() as u64,
                    _ => 0,
                };
        });
        assert_eq!(retained, completion.output_bytes);
        let signal_operation = log.events.iter().find_map(|record| {
            return match &record.event {
                Event::SignalIntent {
                    operation_id, name, ..
                } if name == "SIGTERM" => Some(*operation_id),
                _ => None,
            };
        });
        assert!(signal_operation.is_some());
        assert!(log.events.iter().any(|record| {
            return matches!(
                &record.event,
                Event::SignalResult { operation_id, .. }
                    if Some(*operation_id) == signal_operation
            );
        }));
        assert!(log.events.iter().any(|record| {
            return matches!(
                &record.event,
                Event::HostError { code, .. } if code == "output_budget_exceeded"
            );
        }));
        validate_completed_log(&session, &log).expect("valid completed budget evidence");
    }

    #[test]
    fn spawn_and_provenance_share_the_effective_child_path() {
        let temporary = TempDir::new().expect("temporary directory");
        let executable = temporary.path().join("child-path-command");
        fs_err::write(&executable, b"#!/bin/sh\nprintf CHILD_PATH\n").expect("fixture");
        fs_err::set_permissions(&executable, Permissions::from_mode(0o700)).expect("mode");
        let session = temporary.path().join("child-path");
        let mut options =
            isolated_options(session.clone(), vec![OsString::from("child-path-command")]);
        options.environment = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            set: BTreeMap::from([(
                "PATH".to_owned(),
                ByteString::from(temporary.path().as_os_str().as_bytes()),
            )]),
            ..EnvironmentSpec::default()
        };
        let mut output = Vec::new();
        let outcome = record_to(&options, &mut output).expect("record child PATH command");
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(output, b"CHILD_PATH");
        let manifest: SessionManifest =
            serde_json::from_slice(&fs_err::read(session.join("session.json")).expect("manifest"))
                .expect("manifest JSON");
        assert_eq!(
            manifest
                .execution_provenance
                .executable
                .resolved
                .as_ref()
                .map(ByteString::as_bytes),
            Some(executable.as_os_str().as_bytes())
        );
    }

    #[test]
    fn evicted_mutation_outcomes_are_loaded_from_the_persistent_journal() {
        let temporary = TempDir::new().expect("temporary directory");
        let journal = MutationJournal::create(temporary.path()).expect("mutation journal");
        let mut history = MutationHistory::new(journal, 1);
        let first_id = Uuid::now_v7();
        let first_command = ControlCommand::Mark {
            label: "first".to_owned(),
        };
        let first_response = cached_test_response(first_id, 1);
        history
            .remember(
                first_id,
                &first_command,
                guarded_command_hash(&first_command, None).expect("command hash"),
                0,
                1,
                "1".repeat(64),
                first_response.clone(),
            )
            .expect("remember first mutation");
        let second_id = Uuid::now_v7();
        let second_command = ControlCommand::Mark {
            label: "second".to_owned(),
        };
        history
            .remember(
                second_id,
                &second_command,
                guarded_command_hash(&second_command, None).expect("command hash"),
                1,
                2,
                "2".repeat(64),
                cached_test_response(second_id, 2),
            )
            .expect("remember second mutation");
        assert!(!history.cached.contains_key(&first_id));

        let (_, retry) = history
            .lookup(first_id, &first_command, None)
            .expect("load evicted mutation");
        assert_eq!(retry, Some(first_response));
        assert_eq!(history.journal.len(), 2);
    }

    #[test]
    fn records_exact_output_and_exit_after_output_close() {
        let temporary = TempDir::new().expect("temporary directory");
        let session = temporary.path().join("recording");
        let options = RecordOptions {
            output_directory: session.clone(),
            creation_alias: None,
            lineage: None,
            import_provenance: None,
            command: vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(r"printf '\033[31mhello\033[0m\377'"),
            ],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            geometry_provenance: GeometryProvenance::default(),
            ghostty_profile: None,
            term: "xterm-ghostty".to_owned(),
            environment: EnvironmentSpec::default(),
            colors: TerminalColors::default(),
            forward_stdin: false,
            durability_policy: DurabilityPolicy::default(),
            provenance_policy: ProvenancePolicy::default(),
            resource_limits: ResourceLimits::default(),
        };

        let mut mirrored_output = Vec::new();
        let outcome = record_to(&options, &mut mirrored_output).expect("record command");
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(
            fs_err::metadata(&session)
                .expect("session metadata")
                .permissions()
                .mode()
                & 0o777,
            SESSION_DIRECTORY_MODE
        );

        let recording = EventLog::read(&outcome.event_log).expect("valid event log");
        let output = recording
            .events
            .iter()
            .filter_map(|record| match &record.event {
                Event::Output { data } => return Some(data.as_bytes()),
                _ => return None,
            })
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(output, b"\x1b[31mhello\x1b[0m\xff");
        assert_eq!(mirrored_output, output);
        assert!(matches!(
            recording.events.last().map(|record| return &record.event),
            Some(Event::Exit { code: Some(0), .. })
        ));
        let output_closed_index = recording
            .events
            .iter()
            .position(|record| {
                return matches!(
                    record.event,
                    Event::Lifecycle {
                        lifecycle_kind: LifecycleKind::OutputClosed,
                        ..
                    }
                );
            })
            .expect("output closed event");
        assert!(output_closed_index < recording.events.len() - 1);
        let manifest: SessionManifest = serde_json::from_slice(
            &fs_err::read(session.join("session.json")).expect("session manifest"),
        )
        .expect("valid session manifest");
        let completion: SessionCompletion = serde_json::from_slice(
            &fs_err::read(session.join("completion.json")).expect("completion manifest"),
        )
        .expect("valid completion manifest");
        assert_eq!(manifest.session_id, recording.header.session_id);
        assert_eq!(completion.session_id, recording.header.session_id);
        assert_eq!(
            completion.final_event_hash,
            recording.events.last().unwrap().hash
        );
        assert_eq!(completion.event_count, recording.events.len() as u64);
        assert_eq!(manifest.command[0].as_bytes(), b"/bin/sh");
    }

    #[test]
    fn paste_encoding_is_explicit_and_rejects_delimiter_injection() {
        assert_eq!(
            encode_paste(b"one\ntwo", true).expect("bracketed paste"),
            b"\x1b[200~one\ntwo\x1b[201~"
        );
        assert_eq!(
            encode_paste(b"one\ntwo", false).expect("plain paste"),
            b"one\rtwo"
        );
        assert!(encode_paste(b"unsafe\x1b[201~command", true).is_err());
    }

    #[test]
    fn refuses_to_overwrite_existing_session_directory() {
        let temporary = TempDir::new().expect("temporary directory");
        let options = RecordOptions {
            output_directory: temporary.path().to_path_buf(),
            creation_alias: None,
            lineage: None,
            import_provenance: None,
            command: vec![OsString::from("true")],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            geometry_provenance: GeometryProvenance::default(),
            ghostty_profile: None,
            term: "xterm-ghostty".to_owned(),
            environment: EnvironmentSpec::default(),
            colors: TerminalColors::default(),
            forward_stdin: false,
            durability_policy: DurabilityPolicy::default(),
            provenance_policy: ProvenancePolicy::default(),
            resource_limits: ResourceLimits::default(),
        };

        let error = record(&options).expect_err("existing directory must fail");
        assert!(matches!(error, Error::SessionExists { .. }));
    }

    #[test]
    fn terminates_descendants_that_retain_the_slave_pty() {
        let temporary = TempDir::new().expect("temporary directory");
        let options = RecordOptions {
            output_directory: temporary.path().join("recording"),
            creation_alias: None,
            lineage: None,
            import_provenance: None,
            command: vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from("(trap '' HUP; exec sleep 30) & printf done"),
            ],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            geometry_provenance: GeometryProvenance::default(),
            ghostty_profile: None,
            term: "xterm-ghostty".to_owned(),
            environment: EnvironmentSpec::default(),
            colors: TerminalColors::default(),
            forward_stdin: false,
            durability_policy: DurabilityPolicy::default(),
            provenance_policy: ProvenancePolicy::default(),
            resource_limits: ResourceLimits::default(),
        };
        let started = Instant::now();
        let mut output = Vec::new();

        let outcome = record_to(&options, &mut output).expect("record command");

        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(output, b"done");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "retained PTY was not terminated promptly"
        );
    }

    #[test]
    fn answers_and_audits_terminal_queries() {
        let temporary = TempDir::new().expect("temporary directory");
        let options = RecordOptions {
            output_directory: temporary.path().join("query"),
            creation_alias: None,
            lineage: None,
            import_provenance: None,
            command: vec![
                OsString::from("/usr/bin/python3"),
                OsString::from("-c"),
                OsString::from(
                    "import os,tty; tty.setraw(0); os.write(1,b'\\x1b[6n'); r=os.read(0,32); os.write(1,b'GOT:'+r.hex().encode())",
                ),
            ],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            geometry_provenance: GeometryProvenance::default(),
            ghostty_profile: None,
            term: "xterm-ghostty".to_owned(),
            environment: EnvironmentSpec::default(),
            colors: TerminalColors::default(),
            forward_stdin: false,
            durability_policy: DurabilityPolicy::default(),
            provenance_policy: ProvenancePolicy::default(),
            resource_limits: ResourceLimits::default(),
        };
        let mut output = Vec::new();

        let outcome = record_to(&options, &mut output).expect("record query command");
        let log = EventLog::read(outcome.event_log).expect("valid query log");

        assert!(output.windows(4).any(|window| return window == b"GOT:"));
        assert!(log.events.iter().any(|record| matches!(
            &record.event,
            Event::QueryResponse { query, response, .. }
                if query.as_bytes() == b"\x1b[6n" && response.as_bytes() == b"\x1b[1;1R"
        )));
        assert!(log.events.iter().any(|record| matches!(
            record.event,
            Event::InputIntent {
                source: InputSource::Host,
                ..
            }
        )));
    }

    #[test]
    fn preserves_complete_osc_query_when_response_precedes_st() {
        let temporary = TempDir::new().expect("temporary directory");
        let options = RecordOptions {
            output_directory: temporary.path().join("osc-query"),
            creation_alias: None,
            lineage: None,
            import_provenance: None,
            command: vec![
                OsString::from("/usr/bin/python3"),
                OsString::from("-c"),
                OsString::from(
                    "import os,tty; tty.setraw(0); q=b'\\x1b]11;?\\x1b\\\\'; os.write(1,q); r=b'';\nwhile not r.endswith(b'\\x1b\\\\'): r+=os.read(0,99)\nos.write(1,b'GOT:'+r.hex().encode())",
                ),
            ],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            geometry_provenance: GeometryProvenance::default(),
            ghostty_profile: None,
            term: "xterm-ghostty".to_owned(),
            environment: EnvironmentSpec::default(),
            colors: TerminalColors::default(),
            forward_stdin: false,
            durability_policy: DurabilityPolicy::default(),
            provenance_policy: ProvenancePolicy::default(),
            resource_limits: ResourceLimits::default(),
        };
        let mut output = Vec::new();

        let outcome = record_to(&options, &mut output).expect("record OSC query command");
        let log = EventLog::read(outcome.event_log).expect("valid OSC query log");

        assert!(log.events.iter().any(|record| matches!(
            &record.event,
            Event::QueryResponse { query, response, .. }
                if query.as_bytes() == b"\x1b]11;?\x1b\\"
                    && response.as_bytes() == b"\x1b]11;rgb:0000/0000/0000\x1b\\"
        )));
    }

    #[test]
    fn presentation_tracker_recognizes_only_synchronized_output_mode_commands() {
        for (input, expected) in [
            (
                b"visible\x1b[?2026hhidden\x1b[?2026l".as_slice(),
                vec![true, false],
            ),
            (b"\x1b[?25;2026h".as_slice(), vec![true]),
            (b"\x9b?2026l".as_slice(), vec![false]),
            (b"\x1b[2026h".as_slice(), Vec::new()),
            (b"\x1b[?20260h".as_slice(), Vec::new()),
            (b"\x1b[?2026:1h".as_slice(), Vec::new()),
            (b"\x1b[?2026\x18h".as_slice(), Vec::new()),
        ] {
            let mut tracker = PresentationTracker::new(false);
            let mut commands = Vec::new();
            let mut framed = Vec::new();
            for byte in input {
                match tracker.push(*byte) {
                    PresentationAction::Immediate => framed.push(*byte),
                    PresentationAction::Buffered => {}
                    PresentationAction::Sequence { bytes, command } => {
                        framed.extend(bytes);
                        if let Some(command) = command {
                            commands.push(command);
                        }
                    }
                }
            }
            framed.extend(tracker.finish());
            assert_eq!(commands, expected, "input: {input:?}");
            assert_eq!(framed, input, "framing must be lossless");
        }

        let mut tracker = PresentationTracker::new(false);
        let mut longest_pending = 0;
        for byte in b"\x1b[?"
            .iter()
            .copied()
            .chain(iter::repeat_n(b'0', MAX_PRESENTATION_SEQUENCE_BYTES * 2))
        {
            let _ = tracker.push(byte);
            longest_pending = longest_pending.max(tracker.pending.len());
        }
        assert!(longest_pending <= MAX_PRESENTATION_SEQUENCE_BYTES + 1);
    }

    #[test]
    fn control_shutdown_cancels_an_inflight_request() {
        let temporary = TempDir::new().expect("temporary directory");
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = spawn_control_server(temporary.path(), sender).expect("control server");
        let socket = temporary.path().join(CONTROL_SOCKET_NAME);
        let client = thread::spawn(move || {
            let mut stream = UnixStream::connect(socket).expect("connect control client");
            let request = ControlRequest::new(ControlCommand::Ping);
            serde_json::to_writer(&mut stream, &request).expect("encode request");
            stream.write_all(b"\n").expect("terminate request");
            let mut response = Vec::new();
            stream.read_to_end(&mut response).expect("read shutdown");
        });
        let pending = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("inflight request");

        let started = Instant::now();
        worker.stop().expect("cancel control server");
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(pending);
        client.join().expect("join control client");
    }

    #[test]
    fn control_server_accepts_a_second_request_while_the_first_is_pending() {
        let temporary = TempDir::new().expect("temporary directory");
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = spawn_control_server(temporary.path(), sender).expect("control server");
        let socket = temporary.path().join(CONTROL_SOCKET_NAME);
        let first_socket = socket.clone();
        let first_client = thread::spawn(move || {
            let mut stream = UnixStream::connect(first_socket).expect("connect first client");
            let request = ControlRequest::new(ControlCommand::Ping);
            serde_json::to_writer(&mut stream, &request).expect("encode first request");
            stream.write_all(b"\n").expect("terminate first request");
            let mut response = Vec::new();
            stream
                .read_to_end(&mut response)
                .expect("read first shutdown");
        });
        let first = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("first inflight request");

        let second_client = thread::spawn(move || {
            let mut stream = UnixStream::connect(socket).expect("connect second client");
            let request = ControlRequest::new(ControlCommand::Ping);
            serde_json::to_writer(&mut stream, &request).expect("encode second request");
            stream.write_all(b"\n").expect("terminate second request");
            let mut response = Vec::new();
            stream
                .read_to_end(&mut response)
                .expect("read second shutdown");
        });
        let second = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("second concurrent request");

        worker.stop().expect("cancel control server");
        drop(first);
        drop(second);
        first_client.join().expect("join first client");
        second_client.join().expect("join second client");
    }
}
