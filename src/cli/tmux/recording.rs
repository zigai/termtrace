use std::{
    env,
    ffi::OsString,
    os::unix::{ffi::OsStringExt as _, process::CommandExt as _},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode},
    sync::mpsc,
    thread,
    time::Duration,
};

use miette::{IntoDiagnostic as _, Result};
use termtrace_engine::{
    analysis::profile::{builtin_terminal_colors, terminal_colors_for_profile},
    recording::{RecordOptions, inherited_geometry, record, resolve_ghostty_profile},
    registry::SessionRegistry,
};
use termtrace_evidence::event_log::DurabilityPolicy;
use termtrace_model::{
    execution::environment::EnvironmentSpec,
    execution::provenance::ProvenancePolicy,
    execution::session::{
        GeometryProvenance, GeometrySource, GhosttyVisualOverrides, ResourceLimits,
        TMUX_PANE_PROVENANCE_VERSION, TmuxPaneLaunchKind, TmuxPaneProvenance,
    },
    value::ByteString,
};
use termtrace_sdk::control_endpoint_is_reachable;

use super::{
    OPTION_CELL_HEIGHT, OPTION_CELL_WIDTH, OPTION_GHOSTTY_CONFIG, OPTION_ORIGINAL_COMMAND,
    OPTION_ORIGINAL_SHELL, OPTION_OUTPUT_ROOT, OPTION_PANE_SESSION, OPTION_PROFILE_MODE,
    OPTION_TERM, OPTION_TERMTRACE, OPTION_VERSION, TMUX_INTEGRATION_VERSION,
    args::TmuxRunArgs,
    client::{
        TmuxClient, option_path, option_text, parse_option, query_number, query_text,
        same_executable, valid_tmux_identifier,
    },
    scope::{mark_pane_scope_decision, pane_matches_scope, scope_from_options},
};

pub(in crate::cli) fn run_tmux_pane(arguments: TmuxRunArgs) -> Result<ExitCode> {
    let pane = env::var("TMUX_PANE").map_err(|_| {
        return miette::miette!("termtrace tmux run must execute inside a tmux pane");
    })?;
    if !valid_tmux_identifier(&pane, b'%') {
        return Err(miette::miette!(
            "TMUX_PANE contains an invalid pane identifier"
        ));
    }
    let client = TmuxClient::inherited();
    if option_text(&client, OPTION_VERSION)? != TMUX_INTEGRATION_VERSION {
        return Err(miette::miette!(
            "tmux recording is not enabled for this server; run `termtrace tmux enable` or source a generated configuration"
        ));
    }
    let configured_executable = PathBuf::from(OsString::from_vec(client.option(OPTION_TERMTRACE)?));
    if !same_executable(
        &env::current_exe().into_diagnostic()?,
        &configured_executable,
    ) {
        return Err(miette::miette!(
            "the active tmux integration points to a different Termtrace executable: {}",
            configured_executable.display()
        ));
    }

    let explicitly_selected =
        arguments.force || arguments.shell_command.is_some() || !arguments.command.is_empty();
    let (command, launch_kind) = tmux_child_command(&client, arguments)?;
    client.unset_pane_option(&pane, OPTION_PANE_SESSION)?;
    let scope = scope_from_options(&client)?;
    if !explicitly_selected && !pane_matches_scope(&client, &pane, &scope)? {
        mark_pane_scope_decision(&client, &pane, false)?;
        return exec_unrecorded_command(command);
    }
    mark_pane_scope_decision(&client, &pane, true)?;
    let mut tmux_pane = capture_tmux_pane(&client, &pane, launch_kind)?;
    let cell_width = parse_option::<u16>(&client, OPTION_CELL_WIDTH)?;
    let cell_height = parse_option::<u16>(&client, OPTION_CELL_HEIGHT)?;
    let geometry = inherited_geometry(cell_width, cell_height).into_diagnostic()?;
    tmux_pane.pane_width = geometry.cols;
    tmux_pane.pane_height = geometry.rows;
    let configured_term = option_text(&client, OPTION_TERM)?;
    let term = if configured_term.is_empty() {
        env::var("TERM").map_err(|_| {
            return miette::miette!("tmux pane did not provide a terminal identity in TERM");
        })?
    } else {
        configured_term
    };
    let ghostty_config = option_path(&client, OPTION_GHOSTTY_CONFIG)?;
    let profile_mode = option_text(&client, OPTION_PROFILE_MODE)?;
    let ghostty_profile = match profile_mode.as_str() {
        "off" => None,
        "required" => Some(
            resolve_ghostty_profile(ghostty_config.as_deref(), GhosttyVisualOverrides::default())
                .into_diagnostic()?,
        ),
        "auto" => match resolve_ghostty_profile(
            ghostty_config.as_deref(),
            GhosttyVisualOverrides::default(),
        ) {
            Ok(profile) => Some(profile),
            Err(error) if ghostty_config.is_none() => {
                tracing::debug!(%error, "Ghostty profile is unavailable for tmux pane recording");
                None
            }
            Err(error) => return Err(error).into_diagnostic(),
        },
        other => {
            return Err(miette::miette!(
                "unsupported configured Ghostty profile mode `{other}`"
            ));
        }
    };
    let colors = ghostty_profile
        .as_ref()
        .map_or_else(builtin_terminal_colors, terminal_colors_for_profile)
        .into_diagnostic()?;
    let output_root = option_path(&client, OPTION_OUTPUT_ROOT)?
        .ok_or_else(|| return miette::miette!("configured tmux output root is empty"))?;
    let registry = SessionRegistry::open(output_root).into_diagnostic()?;
    let paths = registry.allocate_detached_paths().into_diagnostic()?;
    let creation_alias = Some(format!(
        "tmux-{}-{}-{}",
        tmux_pane.session_id.trim_start_matches('$'),
        tmux_pane.window_id.trim_start_matches('@'),
        tmux_pane.pane_id.trim_start_matches('%'),
    ));
    let cell_width_px = geometry
        .pixel_width
        .checked_div(geometry.cols)
        .ok_or_else(|| {
            return miette::miette!("inherited tmux pane has no terminal columns");
        })?;
    let cell_height_px = geometry
        .pixel_height
        .checked_div(geometry.rows)
        .ok_or_else(|| {
            return miette::miette!("inherited tmux pane has no terminal rows");
        })?;
    let (publication_cancel, publication) =
        publish_pane_recording_when_ready(client, pane.clone(), paths.session.clone())?;
    let options = RecordOptions {
        output_directory: paths.session,
        creation_alias,
        lineage: None,
        import_provenance: None,
        tmux_pane: Some(tmux_pane),
        command,
        working_directory: None,
        cols: geometry.cols,
        rows: geometry.rows,
        cell_width_px,
        cell_height_px,
        geometry_provenance: GeometryProvenance {
            source: GeometrySource::Inherited,
            capture_target: None,
        },
        ghostty_profile,
        term,
        environment: EnvironmentSpec::default(),
        colors,
        forward_stdin: true,
        continue_after_output_mirror_closes: true,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
    let outcome = record(&options);
    let _cancelled = publication_cancel.try_send(());
    let publication = publication
        .join()
        .map_err(|_| return miette::miette!("tmux recording publication thread panicked"))?;
    if let Err(error) = publication {
        tracing::debug!(%error, pane = %pane, "could not publish tmux pane recording path");
    }
    let outcome = outcome.into_diagnostic()?;
    return outcome
        .exit_code
        .and_then(|code| return u8::try_from(code).ok())
        .map_or(Ok(ExitCode::FAILURE), |code| {
            return Ok(ExitCode::from(code));
        });
}

fn publish_pane_recording_when_ready(
    client: TmuxClient,
    pane: String,
    session: PathBuf,
) -> Result<(mpsc::SyncSender<()>, thread::JoinHandle<Result<()>>)> {
    let (cancel, cancellation) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("termtrace-tmux-publication".to_owned())
        .spawn(move || {
            loop {
                if control_endpoint_is_reachable(&session) {
                    return client.set_pane_option(&pane, OPTION_PANE_SESSION, session.as_os_str());
                }
                match cancellation.recv_timeout(Duration::from_millis(5)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        })
        .into_diagnostic()?;
    return Ok((cancel, thread));
}

fn exec_unrecorded_command(command: Vec<OsString>) -> Result<ExitCode> {
    let mut arguments = command.into_iter();
    let program = arguments
        .next()
        .ok_or_else(|| return miette::miette!("tmux pane command is empty"))?;
    let error = ProcessCommand::new(&program).args(arguments).exec();
    return Err(miette::miette!(
        "could not execute unrecorded tmux pane command {}: {error}",
        Path::new(&program).display()
    ));
}

fn tmux_child_command(
    client: &TmuxClient,
    arguments: TmuxRunArgs,
) -> Result<(Vec<OsString>, TmuxPaneLaunchKind)> {
    if let Some(shell_command) = arguments.shell_command {
        return Ok((
            vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                shell_command,
            ],
            TmuxPaneLaunchKind::ShellCommand,
        ));
    }
    if !arguments.command.is_empty() {
        return Ok((arguments.command, TmuxPaneLaunchKind::ExplicitArgv));
    }
    let default_command = client.option(OPTION_ORIGINAL_COMMAND)?;
    if !default_command.is_empty() {
        let shell = client.option(OPTION_ORIGINAL_SHELL)?;
        if shell.is_empty() {
            return Err(miette::miette!("saved tmux default shell is empty"));
        }
        return Ok((
            vec![
                OsString::from_vec(shell),
                OsString::from("-c"),
                OsString::from_vec(default_command),
            ],
            TmuxPaneLaunchKind::DefaultCommand,
        ));
    }
    let shell = client.option(OPTION_ORIGINAL_SHELL)?;
    if shell.is_empty() {
        return Err(miette::miette!("saved tmux default shell is empty"));
    }
    return Ok((
        vec![OsString::from_vec(shell), OsString::from("-l")],
        TmuxPaneLaunchKind::DefaultShell,
    ));
}

fn capture_tmux_pane(
    client: &TmuxClient,
    pane: &str,
    launch_kind: TmuxPaneLaunchKind,
) -> Result<TmuxPaneProvenance> {
    let tmux_version = query_text(client, pane, "#{version}")?;
    let socket_path = client.pane_format(pane, "#{socket_path}")?;
    let server_pid = query_number(client, pane, "#{pid}")?;
    let session_id = query_text(client, pane, "#{session_id}")?;
    let session_name = client.pane_format(pane, "#{session_name}")?;
    let session_created_unix_seconds = query_number(client, pane, "#{session_created}")?;
    let window_id = query_text(client, pane, "#{window_id}")?;
    let window_index = query_number(client, pane, "#{window_index}")?;
    let window_name = client.pane_format(pane, "#{window_name}")?;
    let pane_id = query_text(client, pane, "#{pane_id}")?;
    let pane_index = query_number(client, pane, "#{pane_index}")?;
    let pane_pid = query_number(client, pane, "#{pane_pid}")?;
    let pane_width = query_number(client, pane, "#{pane_width}")?;
    let pane_height = query_number(client, pane, "#{pane_height}")?;
    if !valid_tmux_identifier(&session_id, b'$')
        || !valid_tmux_identifier(&window_id, b'@')
        || !valid_tmux_identifier(&pane_id, b'%')
    {
        return Err(miette::miette!(
            "tmux returned invalid stable object identifiers"
        ));
    }
    return Ok(TmuxPaneProvenance {
        version: TMUX_PANE_PROVENANCE_VERSION,
        tmux_version,
        socket_path: ByteString::from(socket_path.as_slice()),
        server_pid,
        session_id,
        session_name: ByteString::from(session_name.as_slice()),
        session_created_unix_seconds,
        window_id,
        window_index,
        window_name: ByteString::from(window_name.as_slice()),
        pane_id,
        pane_index,
        pane_pid,
        pane_width,
        pane_height,
        pane_start_command: ByteString::from(
            client
                .pane_format(pane, "#{pane_start_command}")?
                .as_slice(),
        ),
        launch_kind,
    });
}
