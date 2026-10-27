//! Runtime orchestration for one-shot PTY recording.

use super::{
    actor::SessionActor,
    control::control_socket_name,
    environment::EffectiveEnvironment,
    model::{RecordOptions, RecordOutcome, TerminalGeometry},
    presentation::{geometry_cell_size, LiveTerminal},
    process::{
        create_session_directory, invalid_geometry, register_host_signals,
        resolve_working_directory, spawn_command,
    },
    raw_terminal::RawTerminalGuard,
    support::{finish_with_cleanup, terminal_error},
};
use crate::{Error, Result};
use portable_pty::PtySize;
#[cfg(test)]
use std::cell::Cell;
use std::cell::RefCell;
use std::{
    env::consts::{ARCH, OS},
    ffi::OsStr,
    io::{self, Write},
    os::unix::ffi::OsStrExt as _,
    path::Path,
};
use termtrace_evidence::{
    artifact::write_secure,
    event_log::{CanonicalBoundary, EventLogSummary, EventLogWriter, SessionClock},
    provenance::{collect_execution_provenance, RecorderBuildIdentity},
    request_journal::MutationJournal,
    session::{canonical_manifest_bytes, manifest_hash, validate_completion_summary_candidate},
};
use termtrace_model::execution::layout::{self, SessionLayout};
use termtrace_model::{
    execution::event::{Event, EventLogHeader, LifecycleKind, TerminalColors},
    execution::session::{
        SessionCompletion, SessionFailure, SessionGeometry, SessionManifest,
        COMPLETION_VALIDATION_VERSION, SESSION_COMPLETION_FORMAT, SESSION_COMPLETION_VERSION,
        SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION,
    },
    terminal::replay::GHOSTTY_REVISION,
    terminal::state::supported_geometry_cell_count,
    value::ByteString,
};
use termtrace_terminal::{GhosttyVt, PresentationTracker, QueryTracker};
use uuid::Uuid;

const MUTATION_CACHE_CAPACITY: usize = 256;
const SESSION_FAILURE_FORMAT: &str = "termtrace-session-failure";
const SESSION_FAILURE_VERSION: u32 = 1;

#[cfg(test)]
thread_local! {
    static POISON_FATAL_APPEND: Cell<bool> = const { Cell::new(false) };
}

use super::outer_resize::ResizeAuthority;
use super::runtime::run;

#[derive(serde::Serialize)]
struct IncompleteSessionDiagnostic<'diagnostic> {
    format: &'static str,
    version: u32,
    lifecycle: &'static str,
    session_id: Uuid,
    session_manifest_hash: &'diagnostic str,
    failure: &'diagnostic SessionFailure,
    closure_failure: Option<&'diagnostic SessionFailure>,
    durable_boundary: Option<CanonicalBoundary>,
}

#[cfg(test)]
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(super) struct IncompleteSessionDiagnosticRecord {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) lifecycle: String,
    pub(super) session_id: Uuid,
    pub(super) session_manifest_hash: String,
    pub(super) failure: SessionFailure,
    pub(super) closure_failure: Option<SessionFailure>,
    pub(super) durable_boundary: Option<CanonicalBoundary>,
}

/// Record a command running in a real pseudo-terminal and mirror its output.
///
/// # Errors
///
/// Returns an error when configuration is invalid, secure artifacts cannot be
/// created, PTY setup or child execution fails, output cannot be mirrored, or
/// the canonical event log cannot be durably committed.
pub fn record(options: &RecordOptions) -> Result<RecordOutcome> {
    let mut stdout = io::stdout();

    return record_with_output(options, &mut stdout);
}

/// Record a command while sending mirrored PTY bytes to a caller-provided writer.
///
/// # Errors
///
/// Returns the same errors as [`record`], including failures from `output`.
pub fn record_with_output(
    options: &RecordOptions,
    output: &mut impl Write,
) -> Result<RecordOutcome> {
    return record_with_resize_authority(options, output, &ResizeAuthority::ChildPty);
}

/// Record a command whose terminal grid is owned by `resize_authority`.
///
/// With [`ResizeAuthority::OuterTerminal`], control resizes are forwarded to the supervisor
/// and canonical resize events follow the grid observed on the recorder's own terminal.
///
/// # Errors
///
/// Returns the same errors as [`record`], including failures from `output`.
pub fn record_with_resize_authority(
    options: &RecordOptions,
    output: &mut impl Write,
    resize_authority: &ResizeAuthority,
) -> Result<RecordOutcome> {
    let mut terminal_mode = RawTerminalGuard::enter(options.forward_stdin)?;
    let result = record_to(options, output, resize_authority);

    return finish_with_cleanup(result, terminal_mode.restore());
}

fn record_to(
    options: &RecordOptions,
    output: &mut impl Write,
    resize_authority: &ResizeAuthority,
) -> Result<RecordOutcome> {
    let pty_size = validate_options(options)?;
    let mut effective_environment =
        EffectiveEnvironment::resolve_validated(&options.environment, &options.term)?;

    // Resolve this once and pass the same owned path to provenance and spawn.
    // Leaving cwd unset is not equivalent: portable-pty falls back to $HOME,
    // which would make a wrapper command silently leave the caller's project.
    let working_directory = resolve_working_directory(options.working_directory.as_deref())?;

    if let Some(profile) = &options.ghostty_profile {
        profile.validate()?;
    }

    // Install handlers before the child exists so cancellation during startup
    // is buffered until the actor can audit it and reap the owned process group.
    let host_signals = register_host_signals()?;

    create_session_directory(&options.output_directory)?;
    effective_environment.set_recording_session(&options.output_directory)?;

    if let Some(profile) = &options.ghostty_profile {
        write_secure(
            &SessionLayout::new(&options.output_directory).ghostty_replay_profile(),
            &profile.replay_config,
        )?;
    }

    let clock = SessionClock::now()?;
    let session_id = Uuid::now_v7();
    let control_socket = control_socket_name(session_id);
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

    let header_bytes = serde_json::to_vec(&header).map_err(|source| {
        return Error::InvalidJson {
            path: SessionLayout::new(&options.output_directory).event_log(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let event_header_hash = blake3::hash(&header_bytes).to_hex().to_string();
    let event_log_path = SessionLayout::new(&options.output_directory).event_log();
    let mut event_log = EventLogWriter::create_with_policy(
        &event_log_path,
        &header,
        clock,
        options.durability_policy.clone(),
    )?;

    let actor_state: Result<(LiveTerminal, MutationJournal)> = (|| {
        let live = initialize_live_terminal(TerminalGeometry::from(pty_size), &options.colors)?;
        let journal = MutationJournal::create(&options.output_directory)?;

        return Ok((live, journal));
    })();
    let (live, journal) = match actor_state {
        Ok(state) => state,
        Err(error) => {
            let failure = SessionFailure {
                code: recorder_failure_code(&error).to_owned(),
                message: error.to_string(),
            };
            let closure_result = close_failed_recording(
                options,
                &mut event_log,
                session_id,
                &session_manifest_hash,
                &event_header_hash,
                &failure,
            );

            return finish_with_cleanup(Err(error), closure_result);
        }
    };

    let spawned = match spawn_command(
        options,
        &working_directory,
        pty_size,
        &effective_environment,
    ) {
        Ok(spawned) => spawned,
        Err(error) => {
            let failure = SessionFailure {
                code: recorder_failure_code(&error).to_owned(),
                message: error.to_string(),
            };
            let closure_result = close_failed_recording(
                options,
                &mut event_log,
                session_id,
                &session_manifest_hash,
                &event_header_hash,
                &failure,
            );

            return finish_with_cleanup(Err(error), closure_result);
        }
    };

    let startup_error = event_log
        .append(Event::Lifecycle {
            lifecycle_kind: LifecycleKind::Spawned,
            pid: spawned.pid,
        })
        .err()
        .map(Error::from);
    let mut actor = SessionActor::new(
        spawned,
        event_log,
        session_id,
        output,
        options.continue_after_output_mirror_closes,
        live,
        journal,
        MUTATION_CACHE_CAPACITY,
        options.resource_limits.clone(),
    );
    actor.failure_policy = options.failure_policy;
    actor.live.suppress_mirrored_queries = options.forward_stdin;
    let runtime = match run(
        &mut actor,
        options.forward_stdin,
        TerminalGeometry::from(pty_size),
        &control_socket,
        host_signals,
        startup_error,
        resize_authority,
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            let failure = SessionFailure {
                code: recorder_failure_code(&error).to_owned(),
                message: error.to_string(),
            };
            let closure_result = close_failed_recording(
                options,
                &mut actor.event_log,
                session_id,
                &session_manifest_hash,
                &event_header_hash,
                &failure,
            );

            return finish_with_cleanup(Err(error), closure_result);
        }
    };

    let SessionActor {
        spawned,
        mut event_log,
        ..
    } = actor;

    drop(spawned);

    let exit_code = i32::try_from(runtime.status.exit_code()).ok();
    let signal_name = runtime.status.signal().map(str::to_owned);
    let terminal_event_seq = match event_log.append(Event::Exit {
        code: exit_code,
        signal: None,
        signal_name: signal_name.clone(),
    }) {
        Ok(terminal_event_seq) => terminal_event_seq,
        Err(error) => {
            let error = Error::from(error);
            let failure = canonical_log_failure(&error);
            let diagnostic_result = write_incomplete_session_diagnostic(
                options,
                session_id,
                &session_manifest_hash,
                &failure,
                None,
                Some(event_log.durable_boundary()),
            );

            return finish_with_cleanup(Err(error), diagnostic_result);
        }
    };

    let completion = SessionCompletion {
        format: SESSION_COMPLETION_FORMAT.to_owned(),
        version: SESSION_COMPLETION_VERSION,
        validation_version: COMPLETION_VALIDATION_VERSION,
        session_id,
        session_manifest_hash,
        event_header_hash,
        exit_code,
        signal_name: signal_name.clone(),
        output_bytes: runtime.output_bytes,
        final_event_hash: event_log.current_hash(),
        event_count: event_log.next_seq(),
        terminal_event_seq,
        durability: event_log.durability_record(),
        resource_limit: runtime.resource_limit.clone(),
        failure: None,
    };

    publish_session_completion(options, &completion)?;

    return Ok(RecordOutcome {
        exit_code,
        signal_name,
        output_bytes: runtime.output_bytes,
        resource_limit: runtime.resource_limit,
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
        failure_policy: options.failure_policy,
        format: SESSION_MANIFEST_FORMAT.to_owned(),
        version: SESSION_MANIFEST_VERSION,
        session_id,
        creation_alias: options.creation_alias.clone(),
        lineage: options.lineage.clone(),
        import_provenance: options.import_provenance.clone(),
        tmux_pane: options.tmux_pane.clone(),
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
            effective_environment.variables(),
            &options.command,
            working_directory,
            RecorderBuildIdentity {
                version: env!("CARGO_PKG_VERSION"),
                source_revision: env!("TERMTRACE_BUILD_REVISION"),
                semantic_engine_revision: GHOSTTY_REVISION,
            },
        ),
        child_environment: effective_environment.policy().clone(),
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: OS.to_owned(),
        architecture: ARCH.to_owned(),
        event_log: layout::EVENT_LOG.to_owned(),
        control_socket: control_socket_name(session_id),
        request_journal: layout::MUTATION_JOURNAL.to_owned(),
        contains_sensitive_terminal_data: true,
        durability_policy: options.durability_policy.clone(),
        resource_limits: options.resource_limits.clone(),
    };
    let encoded = canonical_manifest_bytes(&manifest)?;
    let hash = manifest_hash(&encoded);

    write_secure(
        &SessionLayout::new(&options.output_directory).manifest(),
        &encoded,
    )?;

    return Ok(hash);
}

fn publish_session_completion(
    options: &RecordOptions,
    completion: &SessionCompletion,
) -> Result<()> {
    let validation_result = (|| {
        let log = EventLogSummary::read(SessionLayout::new(&options.output_directory).event_log())?;

        return validate_completion_summary_candidate(&options.output_directory, &log, completion);
    })();

    if let Err(error) = validation_result {
        let error = Error::from(error);
        let validation_failure = SessionFailure {
            code: "completion_validation_failed".to_owned(),
            message: error.to_string(),
        };
        let failure = completion.failure.as_ref().unwrap_or(&validation_failure);
        let closure_failure = completion
            .failure
            .as_ref()
            .map(|_| return &validation_failure);

        let diagnostic_result = write_incomplete_session_diagnostic(
            options,
            completion.session_id,
            &completion.session_manifest_hash,
            failure,
            closure_failure,
            None,
        );

        return finish_with_cleanup(Err(error), diagnostic_result);
    }

    return write_json_secure(
        &SessionLayout::new(&options.output_directory).completion(),
        completion,
    );
}

fn close_failed_recording(
    options: &RecordOptions,
    event_log: &mut EventLogWriter,
    session_id: Uuid,
    session_manifest_hash: &str,
    event_header_hash: &str,
    failure: &SessionFailure,
) -> Result<()> {
    let terminal_event_seq = match event_log
        .append(Event::HostError {
            code: failure.code.clone(),
            message: failure.message.clone(),
        })
        .and_then(|_| {
            #[cfg(test)]
            POISON_FATAL_APPEND.with(|poison| {
                if poison.replace(false) {
                    return Err(termtrace_evidence::Error::EventLogPoisoned {
                        path: SessionLayout::new(&options.output_directory).event_log(),
                    });
                }
                return Ok(());
            })?;

            return event_log.append(Event::Exit {
                code: None,
                signal: None,
                signal_name: None,
            });
        }) {
        Ok(terminal_event_seq) => terminal_event_seq,
        Err(error) => {
            let error = Error::from(error);
            let closure_failure = canonical_log_failure(&error);
            let diagnostic_result = write_incomplete_session_diagnostic(
                options,
                session_id,
                session_manifest_hash,
                failure,
                Some(&closure_failure),
                Some(event_log.durable_boundary()),
            );

            return finish_with_cleanup(Err(error), diagnostic_result);
        }
    };

    let completion = SessionCompletion {
        format: SESSION_COMPLETION_FORMAT.to_owned(),
        version: SESSION_COMPLETION_VERSION,
        validation_version: COMPLETION_VALIDATION_VERSION,
        session_id,
        session_manifest_hash: session_manifest_hash.to_owned(),
        event_header_hash: event_header_hash.to_owned(),
        exit_code: None,
        signal_name: None,
        output_bytes: event_log.observed_boundary().output_bytes,
        final_event_hash: event_log.current_hash(),
        event_count: event_log.next_seq(),
        terminal_event_seq,
        durability: event_log.durability_record(),
        resource_limit: None,
        failure: Some(failure.clone()),
    };

    return publish_session_completion(options, &completion);
}

fn write_incomplete_session_diagnostic(
    options: &RecordOptions,
    session_id: Uuid,
    session_manifest_hash: &str,
    failure: &SessionFailure,
    closure_failure: Option<&SessionFailure>,
    durable_boundary: Option<CanonicalBoundary>,
) -> Result<()> {
    return write_json_secure(
        &SessionLayout::new(&options.output_directory).failure_diagnostic(),
        &IncompleteSessionDiagnostic {
            format: SESSION_FAILURE_FORMAT,
            version: SESSION_FAILURE_VERSION,
            lifecycle: "incomplete",
            session_id,
            session_manifest_hash,
            failure,
            closure_failure,
            durable_boundary,
        },
    );
}

fn canonical_log_failure(error: &Error) -> SessionFailure {
    return SessionFailure {
        code: "canonical_log_failed".to_owned(),
        message: error.to_string(),
    };
}

fn write_json_secure(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let encoded = serde_json::to_vec_pretty(value).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    write_secure(path, &encoded).map_err(Error::from)?;

    return Ok(());
}

fn recorder_failure_code(error: &Error) -> &'static str {
    return match error {
        Error::EventLogPoisoned { .. } | Error::InvalidEventLog { .. } => "canonical_log_failed",
        Error::Filesystem { path, .. }
            if path
                .file_name()
                .is_some_and(|name| return name == OsStr::new(layout::EVENT_LOG)) =>
        {
            "canonical_log_failed"
        }
        Error::TerminalOperation { operation, .. } if *operation == "spawn terminal command" => {
            "spawn_failed"
        }
        Error::TerminalOperation { operation, .. } | Error::ThreadPanicked { operation, .. }
            if operation.contains("PTY") =>
        {
            "pty_failed"
        }
        Error::GhosttyVt { .. } | Error::InvalidTerminalText { .. } => "semantic_invariant_failed",
        _ => "recorder_actor_failed",
    };
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

    if options.cell_width_px == 0 {
        return Err(terminal_error(
            "validate geometry",
            "resolved terminal cell width must be greater than zero",
        ));
    }

    if options.cell_height_px == 0 {
        return Err(terminal_error(
            "validate geometry",
            "resolved terminal cell height must be greater than zero",
        ));
    }

    if supported_geometry_cell_count(options.cols, options.rows).is_none() {
        return Err(invalid_geometry(options));
    }

    let pixel_width = options
        .cols
        .checked_mul(options.cell_width_px)
        .ok_or_else(|| return invalid_geometry(options))?;
    let pixel_height = options
        .rows
        .checked_mul(options.cell_height_px)
        .ok_or_else(|| return invalid_geometry(options))?;

    return Ok(PtySize {
        rows: options.rows,
        cols: options.cols,
        pixel_width,
        pixel_height,
    });
}

fn initialize_live_terminal(
    geometry: TerminalGeometry,
    colors: &TerminalColors,
) -> Result<LiveTerminal> {
    let mut semantic = GhosttyVt::new(geometry.cols, geometry.rows, 100_000)?;
    semantic.apply_terminal_colors(colors)?;
    semantic.resize(
        geometry.cols,
        geometry.rows,
        geometry_cell_size(geometry.pixel_width, geometry.cols),
        geometry_cell_size(geometry.pixel_height, geometry.rows),
    )?;
    let presented = semantic.snapshot()?;

    return Ok(LiveTerminal {
        semantic,
        presentation_tracker: PresentationTracker::new(presented.modes.synchronized_output),
        presented,
        presented_stale: false,
        row_hashes: RefCell::new(Vec::new()),
        cached_state_hash: RefCell::new(None),
        query_tracker: QueryTracker::default(),
        pending_query_responses: Vec::new(),
        source_event_seq: None,
        source_byte_offset: None,
        suppress_mirrored_queries: false,
        mirror_output: Vec::new(),
        mirror_sequence: Vec::new(),
        geometry,
    });
}

#[cfg(test)]
mod tests;
