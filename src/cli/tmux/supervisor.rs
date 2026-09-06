use crate::cli::output::ResponseWriter;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fmt::Write as _,
    io,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
    process::{self, ExitCode},
    str, thread,
    time::{Duration, Instant},
};

use miette::{IntoDiagnostic as _, Result};
use termtrace_model::value::ByteString;

use crate::cli::output::{optional_display, write_json_line, write_stdout};

use super::{
    OPTION_EXCLUDED_PID, OPTION_GRANDFATHERED_PID, OPTION_OUTPUT_ROOT, OPTION_PANE_SESSION,
    OPTION_SELECTED_PID, OPTION_SUPERVISOR_PID, OPTION_TERMTRACE, OPTION_TMUX, OPTION_VERSION,
    SUPERVISOR_INTERVAL, TMUX_INTEGRATION_VERSION, UNRECORDED_GRACE,
    aggregate::{
        coverage_from_options, refresh_configured_aggregate_manifest, run_tmux_recording_status,
    },
    args::{TmuxCoverageMode, TmuxGuardArgs, TmuxStatusArgs},
    client::{TmuxClient, option_text, query_number, same_executable, valid_tmux_identifier},
    scope::{
        TmuxScopeMode, TmuxScopePolicy, mark_pane_scope_decision, pane_matches_scope,
        pane_scope_decision, parse_scope_identifier, scope_from_options,
    },
    setup::{launch_option, setup_from_options, tmux_wrapper_command},
};

#[derive(serde::Serialize)]
pub(super) struct TmuxPaneStatus {
    pub(super) pane_id: String,
    pub(super) session_id: String,
    pub(super) window_id: String,
    pub(super) pane_pid: Option<u32>,
    pub(super) current_command: ByteString,
    pub(super) selected: bool,
    pub(super) recorded: bool,
    pub(super) recording_session: Option<ByteString>,
}

#[derive(serde::Serialize)]
pub(super) struct TmuxStatusReport {
    pub(super) integration_version: u32,
    pub(super) enabled: bool,
    pub(super) strict: bool,
    pub(super) default_command_matches: bool,
    pub(super) configured_executable: Option<ByteString>,
    pub(super) output_root: Option<ByteString>,
    pub(super) scope: Option<TmuxScopePolicy>,
    pub(super) pane_count: usize,
    pub(super) selected_panes: usize,
    pub(super) excluded_panes: usize,
    pub(super) recorded_panes: usize,
    pub(super) unrecorded_panes: usize,
    pub(super) panes: Vec<TmuxPaneStatus>,
}

pub(super) struct PaneProcessRow {
    pub(super) pane_id: String,
    pub(super) pid: u32,
    pub(super) grandfathered_pid: Option<u32>,
    pub(super) selected_pid: Option<u32>,
    pub(super) excluded_pid: Option<u32>,
    pub(super) session_id: String,
    pub(super) window_id: String,
}

pub(in crate::cli) fn run_tmux_status(
    response: &mut ResponseWriter,
    arguments: TmuxStatusArgs,
) -> Result<ExitCode> {
    if let Some(recording) = arguments.recording.as_deref() {
        return run_tmux_recording_status(response, arguments.output_root.as_deref(), recording);
    }
    let client = TmuxClient::from_connection(arguments.connection);
    let report = tmux_status_report(&client)?;
    if arguments.json {
        return write_json_line(response, &report);
    }
    let scope = report
        .scope
        .as_ref()
        .map_or("disabled", TmuxScopePolicy::mode_name);
    let mut output = format!(
        "enabled={} strict={} scope={} default-command={} panes={} selected={} excluded={} recorded={} unrecorded={}\n",
        report.enabled,
        report.strict,
        scope,
        if report.default_command_matches {
            "managed"
        } else {
            "unmanaged"
        },
        report.pane_count,
        report.selected_panes,
        report.excluded_panes,
        report.recorded_panes,
        report.unrecorded_panes,
    );
    for pane in report.panes {
        let recording = pane.recording_session.map_or_else(
            || return "-".to_owned(),
            |path| {
                return PathBuf::from(OsString::from_vec(path.into_bytes()))
                    .display()
                    .to_string();
            },
        );
        writeln!(
            output,
            "{} {} {} pid={} selected={} recorded={} {}",
            pane.session_id,
            pane.window_id,
            pane.pane_id,
            optional_display(pane.pane_pid),
            pane.selected,
            pane.recorded,
            recording,
        )
        .into_diagnostic()?;
    }
    return write_stdout(response, output);
}

pub(in crate::cli) fn run_tmux_guard(arguments: TmuxGuardArgs) -> Result<ExitCode> {
    if !valid_tmux_identifier(&arguments.pane, b'%') {
        return Err(miette::miette!(
            "tmux guard received an invalid pane identifier"
        ));
    }
    let client = TmuxClient::inherited();
    if option_text(&client, OPTION_VERSION)? != TMUX_INTEGRATION_VERSION {
        return Ok(ExitCode::SUCCESS);
    }
    let policy = scope_from_options(&client)?;
    let pane_pid = match query_number::<u32>(&client, &arguments.pane, "#{pane_pid}") {
        Ok(pid) => pid,
        Err(_) => return Ok(ExitCode::SUCCESS),
    };
    if !pane_scope_decision(&client, &arguments.pane, pane_pid, &policy)? {
        mark_pane_scope_decision(&client, &arguments.pane, false)?;
        return Ok(ExitCode::SUCCESS);
    }
    let expected = PathBuf::from(OsString::from_vec(client.option(OPTION_TERMTRACE)?));
    let deadline = Instant::now() + UNRECORDED_GRACE;
    loop {
        match pane_process_is_wrapper(&client, &arguments.pane, &expected) {
            Ok(Some(true)) => return Ok(ExitCode::SUCCESS),
            Ok(None) => return Ok(ExitCode::SUCCESS),
            Ok(Some(false)) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(Some(false)) => break,
            Err(error) if Instant::now() < deadline => {
                tracing::debug!(%error, pane = %arguments.pane, "waiting for tmux pane wrapper");
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
    let coverage = coverage_from_options(&client)?;
    let message = format!(
        "Termtrace found unrecorded pane {} (coverage={})",
        arguments.pane,
        coverage.as_str(),
    );
    if coverage == TmuxCoverageMode::Strict {
        let _display = client.run(["display-message", "-d", "5000", message.as_str()]);
        client.run(["kill-pane", "-t", arguments.pane.as_str()])?;
    } else {
        tracing::warn!(pane = %arguments.pane, "{message}");
    }
    return Ok(ExitCode::SUCCESS);
}

pub(in crate::cli) fn run_tmux_supervisor() -> Result<ExitCode> {
    let client = TmuxClient::inherited();
    if option_text(&client, OPTION_VERSION)? != TMUX_INTEGRATION_VERSION {
        return Ok(ExitCode::SUCCESS);
    }
    let expected = PathBuf::from(OsString::from_vec(client.option(OPTION_TERMTRACE)?));
    let Some(_lock) = super::watchdog::claim(&client, "supervisor")? else {
        return Ok(ExitCode::SUCCESS);
    };
    client.set_option(
        OPTION_SUPERVISOR_PID,
        OsStr::new(&process::id().to_string()),
    )?;
    let result = super::aggregate::record_observer_start(&client)
        .and_then(|()| return supervise_tmux_panes(&client, &expected));
    let cleanup = release_supervisor(&client);
    match (result, cleanup) {
        (Ok(()), Ok(())) => return Ok(ExitCode::SUCCESS),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => return Err(error),
        (Err(primary), Err(cleanup)) => {
            return Err(miette::miette!(
                "tmux supervisor failed: {primary}; cleanup also failed: {cleanup}"
            ));
        }
    }
}

pub(super) fn release_supervisor(client: &TmuxClient) -> Result<()> {
    let current = process::id().to_string();
    if option_text(client, OPTION_SUPERVISOR_PID).ok().as_deref() == Some(current.as_str()) {
        client.unset_option(OPTION_SUPERVISOR_PID)?;
    }
    return Ok(());
}

pub(super) fn supervise_tmux_panes(client: &TmuxClient, expected: &Path) -> Result<()> {
    let mut suspects = BTreeMap::<String, Instant>::new();
    loop {
        let version = match option_text(client, OPTION_VERSION) {
            Ok(version) => version,
            Err(_) => return Ok(()),
        };
        if version != TMUX_INTEGRATION_VERSION {
            return Ok(());
        }
        let coverage = coverage_from_options(client)?;
        if let Err(error) = refresh_configured_aggregate_manifest(client) {
            tracing::warn!(%error, "could not refresh aggregate tmux manifest");
        }
        let policy = scope_from_options(client)?;
        let panes = match pane_process_rows(client) {
            Ok(panes) => panes,
            Err(_) => return Ok(()),
        };
        let present = panes
            .iter()
            .map(|pane| return pane.pane_id.clone())
            .collect::<BTreeSet<_>>();
        suspects.retain(|pane, _| return present.contains(pane));
        for pane in panes {
            if pane.grandfathered_pid == Some(pane.pid) {
                suspects.remove(&pane.pane_id);
                continue;
            }
            let selected = match pane_row_is_selected(client, &pane, &policy) {
                Ok(selected) => selected,
                Err(error) => {
                    tracing::debug!(%error, pane = %pane.pane_id, "tmux pane disappeared during scope audit");
                    suspects.remove(&pane.pane_id);
                    continue;
                }
            };
            if !selected {
                if pane.excluded_pid != Some(pane.pid)
                    && let Err(error) = mark_pane_scope_decision(client, &pane.pane_id, false)
                {
                    tracing::debug!(%error, pane = %pane.pane_id, "could not cache excluded tmux pane process");
                }
                suspects.remove(&pane.pane_id);
                continue;
            }
            if process_id_has_role(pane.pid, expected, b"run")?.unwrap_or(false) {
                suspects.remove(&pane.pane_id);
                continue;
            }
            let first_seen = suspects
                .entry(pane.pane_id.clone())
                .or_insert_with(Instant::now);
            if first_seen.elapsed() < UNRECORDED_GRACE {
                continue;
            }
            let message = format!(
                "Termtrace found unrecorded pane {} after its process changed (coverage={})",
                pane.pane_id,
                coverage.as_str(),
            );
            if coverage == TmuxCoverageMode::Strict {
                let _display = client.run(["display-message", "-d", "5000", message.as_str()]);
                let _kill = client.run(["kill-pane", "-t", pane.pane_id.as_str()]);
            } else {
                tracing::warn!(pane = %pane.pane_id, "{message}");
            }
            suspects.remove(&pane.pane_id);
        }
        thread::sleep(SUPERVISOR_INTERVAL);
    }
}

pub(super) fn pane_row_is_selected(
    client: &TmuxClient,
    pane: &PaneProcessRow,
    policy: &TmuxScopePolicy,
) -> Result<bool> {
    if pane.selected_pid == Some(pane.pid) {
        return Ok(true);
    }
    if pane.excluded_pid == Some(pane.pid) {
        return Ok(false);
    }
    return match policy.mode {
        TmuxScopeMode::All => Ok(true),
        TmuxScopeMode::Explicit => Ok(false),
        TmuxScopeMode::Selected => {
            if policy.pane_ids.contains(&pane.pane_id)
                || policy.session_ids.contains(&pane.session_id)
                || policy.window_ids.contains(&pane.window_id)
            {
                return Ok(true);
            }
            if policy.session_names.is_empty() && policy.window_names.is_empty() {
                return Ok(false);
            }
            pane_matches_scope(client, &pane.pane_id, policy)
        }
    };
}

pub(super) fn tmux_status_report(client: &TmuxClient) -> Result<TmuxStatusReport> {
    let enabled = option_text(client, OPTION_VERSION)? == TMUX_INTEGRATION_VERSION;
    let scope = enabled
        .then(|| return scope_from_options(client))
        .transpose()?;
    let configured_executable = enabled
        .then(|| return client.option(OPTION_TERMTRACE))
        .transpose()?;
    let expected = configured_executable
        .as_ref()
        .map(|bytes| return PathBuf::from(OsString::from_vec(bytes.clone())));
    let expected_wrapper = if enabled {
        let setup = setup_from_options(
            client,
            configured_executable.clone().unwrap_or_default(),
            client.option(OPTION_TMUX)?,
        )?;
        Some((launch_option(&setup), tmux_wrapper_command(&setup)))
    } else {
        None
    };
    let default_command_matches = expected_wrapper.as_ref().is_some_and(|(option, wrapper)| {
        return client.option(option).ok().as_deref() == Some(wrapper.as_bytes());
    });
    let pane_rows = pane_process_rows(client)?;
    let mut panes = Vec::with_capacity(pane_rows.len());
    for pane in pane_rows {
        // Commands and recording paths can contain tabs or newlines, so keep
        // these arbitrary-byte fields out of the framed identity rows.
        let current_command = client.pane_format(&pane.pane_id, "#{pane_current_command}")?;
        let recording_session = client.pane_option(&pane.pane_id, OPTION_PANE_SESSION)?;
        let selected = match &scope {
            Some(policy) => pane_row_is_selected(client, &pane, policy)?,
            None => false,
        };
        let recorded = match &expected {
            Some(executable) => process_id_has_role(pane.pid, executable, b"run")?.unwrap_or(false),
            None => false,
        };
        panes.push(TmuxPaneStatus {
            pane_id: pane.pane_id,
            session_id: pane.session_id,
            window_id: pane.window_id,
            pane_pid: Some(pane.pid),
            current_command: ByteString::from(current_command.as_slice()),
            selected,
            recorded,
            recording_session: (!recording_session.is_empty())
                .then(|| return ByteString::from(recording_session.as_slice())),
        });
    }
    let selected_panes = panes.iter().filter(|pane| return pane.selected).count();
    let recorded_panes = panes.iter().filter(|pane| return pane.recorded).count();
    let unrecorded_panes = panes
        .iter()
        .filter(|pane| return pane.selected && !pane.recorded)
        .count();
    return Ok(TmuxStatusReport {
        integration_version: 1,
        enabled,
        strict: enabled && coverage_from_options(client)? == TmuxCoverageMode::Strict,
        default_command_matches,
        configured_executable: configured_executable
            .map(|bytes| return ByteString::from(bytes.as_slice())),
        output_root: enabled
            .then(|| return client.option(OPTION_OUTPUT_ROOT))
            .transpose()?
            .map(|bytes| return ByteString::from(bytes.as_slice())),
        scope,
        pane_count: panes.len(),
        selected_panes,
        excluded_panes: panes.len().saturating_sub(selected_panes),
        recorded_panes,
        unrecorded_panes,
        panes,
    });
}

pub(super) fn pane_process_is_wrapper(
    client: &TmuxClient,
    pane: &str,
    expected_executable: &Path,
) -> Result<Option<bool>> {
    let pid = match query_number::<u32>(client, pane, "#{pane_pid}") {
        Ok(pid) => pid,
        Err(_) => return Ok(None),
    };
    return process_id_has_role(pid, expected_executable, b"run");
}

pub(super) fn process_id_has_role(
    pid: u32,
    expected_executable: &Path,
    role: &[u8],
) -> Result<Option<bool>> {
    let process = PathBuf::from(format!("/proc/{pid}"));
    let actual_executable = match fs_err::read_link(process.join("exe")) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(miette::miette!(
                "could not inspect process executable {}: {source}",
                process.join("exe").display()
            ));
        }
    };
    if !same_executable(&actual_executable, expected_executable) {
        return Ok(Some(false));
    }
    let command_line = match fs_err::read(process.join("cmdline")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(miette::miette!(
                "could not inspect process command line {}: {source}",
                process.join("cmdline").display()
            ));
        }
    };
    let mut arguments = command_line.split(|byte| return *byte == 0);
    let _program = arguments.next();
    let mut group = arguments.next();
    while let Some(arg) = group {
        if arg == b"tmux" {
            break;
        }
        group = arguments.next();
    }
    let action = arguments.next();
    return Ok(Some(group == Some(b"tmux") && action == Some(role)));
}

pub(super) fn pane_process_rows(client: &TmuxClient) -> Result<Vec<PaneProcessRow>> {
    let format = format!(
        "#{{pane_dead}}\t#{{pane_id}}\t#{{pane_pid}}\t#{{{OPTION_GRANDFATHERED_PID}}}\t#{{{OPTION_SELECTED_PID}}}\t#{{{OPTION_EXCLUDED_PID}}}\t#{{session_id}}\t#{{window_id}}"
    );
    let output = client.output(["list-panes", "-a", "-F", format.as_str()])?;
    let mut panes = BTreeMap::<String, PaneProcessRow>::new();
    for line in output.split(|byte| return *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.splitn(8, |byte| return *byte == b'\t');
        if fields.next() == Some(b"1") {
            continue;
        }
        let pane_id = parse_scope_identifier(fields.next(), b'%', "pane")?;
        let pid = fields
            .next()
            .and_then(|value| return str::from_utf8(value).ok())
            .and_then(|value| return value.parse::<u32>().ok())
            .ok_or_else(|| return miette::miette!("tmux returned an invalid pane PID"))?;
        let grandfathered_pid = fields
            .next()
            .and_then(|value| return str::from_utf8(value).ok())
            .and_then(|value| return value.parse::<u32>().ok());
        let selected_pid = fields
            .next()
            .and_then(|value| return str::from_utf8(value).ok())
            .and_then(|value| return value.parse::<u32>().ok());
        let excluded_pid = fields
            .next()
            .and_then(|value| return str::from_utf8(value).ok())
            .and_then(|value| return value.parse::<u32>().ok());
        let session_id = parse_scope_identifier(fields.next(), b'$', "session")?;
        let window_id = parse_scope_identifier(fields.next(), b'@', "window")?;
        panes.insert(
            pane_id.clone(),
            PaneProcessRow {
                pane_id,
                pid,
                grandfathered_pid,
                selected_pid,
                excluded_pid,
                session_id,
                window_id,
            },
        );
    }
    return Ok(panes.into_values().collect());
}

pub(super) fn grandfather_current_panes(client: &TmuxClient) -> Result<()> {
    for pane in pane_process_rows(client)? {
        client.set_pane_option(
            &pane.pane_id,
            OPTION_GRANDFATHERED_PID,
            OsStr::new(&pane.pid.to_string()),
        )?;
    }
    return Ok(());
}

pub(super) fn clear_managed_pane_options(client: &TmuxClient) -> Result<()> {
    let panes = client.output(["list-panes", "-a", "-F", "#{pane_id}"])?;
    for line in panes
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
    {
        let pane_id = parse_scope_identifier(Some(line), b'%', "pane")?;
        for option in [
            OPTION_GRANDFATHERED_PID,
            OPTION_SELECTED_PID,
            OPTION_EXCLUDED_PID,
        ] {
            client.unset_pane_option(&pane_id, option)?;
        }
    }
    return Ok(());
}
