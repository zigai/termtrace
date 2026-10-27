//! One classification of every pane: selected, recorded, and uncovered.

use crate::{
    artifact::TmuxRecordingCoverage,
    client::{same_executable, PaneRow, TmuxClient},
    error::{Error, Result},
    observer::{ObserverRole, Observers},
    options,
    scope::TmuxScopePolicy,
    setup::{PaneRetention, PreparedTmuxSetup},
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    ffi::OsString,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
};
use termtrace_model::value::ByteString;
use termtrace_process::ProcessImage;

/// Integration settings read once per observation.
#[derive(Clone, Debug)]
pub struct Integration {
    /// Configured Termtrace executable.
    pub termtrace: PathBuf,
    /// Coverage policy.
    pub coverage: TmuxRecordingCoverage,
    /// Recording scope.
    pub scope: TmuxScopePolicy,
    /// Managed state root.
    pub output_root: PathBuf,
    /// Aggregate manifest updated by the supervisor.
    pub aggregate_manifest: Option<PathBuf>,
    /// Automatic deletion of completed pane recordings.
    pub retention: PaneRetention,
}

impl Integration {
    /// Read the settings, or none when the current integration version is not enabled.
    ///
    /// # Errors
    /// Returns tmux failures or malformed options.
    pub fn read(client: &TmuxClient) -> Result<Option<Self>> {
        if client.option_text(options::VERSION)? != options::INTEGRATION_VERSION {
            return Ok(None);
        }
        return Ok(Some(Self {
            termtrace: PathBuf::from(OsString::from_vec(client.option(options::TERMTRACE)?)),
            coverage: client
                .option_text(options::COVERAGE)?
                .parse()
                .map_err(Error::invalid)?,
            scope: TmuxScopePolicy::from_options(client)?,
            output_root: client
                .option_path(options::OUTPUT_ROOT)?
                .ok_or_else(|| return Error::invalid("configured tmux output root is empty"))?,
            aggregate_manifest: client.option_path(options::AGGREGATE_MANIFEST)?,
            retention: PaneRetention::from_options(client)?,
        }));
    }
}

/// Coverage of one live pane.
#[derive(Clone, Debug)]
pub struct PaneCoverage {
    /// The pane as listed.
    pub row: PaneRow,
    /// The pane's current process is in scope.
    pub selected: bool,
    /// The pane's root process is a Termtrace pane wrapper.
    pub recorded: bool,
}

impl PaneCoverage {
    /// Selected but not recorded.
    pub fn is_uncovered(&self) -> bool {
        return self.selected && !self.recorded;
    }
}

/// Classify every live pane; `recorded` answers whether a PID runs the pane wrapper.
pub fn classify(
    integration: Option<&Integration>,
    rows: &[PaneRow],
    mut recorded: impl FnMut(u32) -> bool,
) -> Vec<PaneCoverage> {
    return rows
        .iter()
        .filter(|row| return !row.dead)
        .map(|row| {
            return PaneCoverage {
                row: row.clone(),
                selected: integration
                    .is_some_and(|integration| return integration.scope.selects(row)),
                recorded: integration.is_some() && row.pid.is_some_and(&mut recorded),
            };
        })
        .collect();
}

/// Pane IDs selected for recording but not run by a wrapper.
pub fn uncovered(panes: &[PaneCoverage]) -> BTreeSet<String> {
    return panes
        .iter()
        .filter(|pane| return pane.is_uncovered())
        .map(|pane| return pane.row.snapshot.pane_id.clone())
        .collect();
}

/// Whether `pid` runs `termtrace tmux <role>` from `executable`; none when it exited.
///
/// # Errors
/// Returns an unreadable process image.
pub fn process_has_role(pid: u32, executable: &Path, role: &[u8]) -> Result<Option<bool>> {
    let Some(image) = ProcessImage::of(pid)
        .map_err(|error| return Error::filesystem(format!("/proc/{pid}"), error))?
    else {
        return Ok(None);
    };
    if !same_executable(&image.executable, executable) {
        return Ok(Some(false));
    }

    let mut arguments = image
        .arguments
        .iter()
        .skip(1)
        .map(|argument| return argument.as_bytes());
    let found_group = arguments.any(|argument| return argument == b"tmux");

    return Ok(Some(found_group && arguments.next() == Some(role)));
}

/// Whether `pid` is a pane wrapper of `executable`.
pub fn is_pane_wrapper(pid: u32, executable: &Path) -> bool {
    return process_has_role(pid, executable, b"run")
        .ok()
        .flatten()
        .unwrap_or(false);
}

/// One pane in a status report.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct TmuxPaneStatus {
    /// Stable pane ID.
    pub pane_id: String,
    /// Stable session ID.
    pub session_id: String,
    /// Stable window ID.
    pub window_id: String,
    /// Pane root process.
    pub pane_pid: Option<u32>,
    /// Current foreground command.
    pub current_command: ByteString,
    /// In scope.
    pub selected: bool,
    /// Run by a pane wrapper.
    pub recorded: bool,
    /// Published recording session.
    pub recording_session: Option<ByteString>,
}

/// Integration and coverage status of one tmux server.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct TmuxStatusReport {
    /// Integration contract version.
    pub integration_version: u32,
    /// The current integration version is enabled.
    pub enabled: bool,
    /// Coverage is strict.
    pub strict: bool,
    /// The launch option still runs the pane wrapper.
    pub default_command_matches: bool,
    /// Configured Termtrace executable.
    pub configured_executable: Option<ByteString>,
    /// Managed state root.
    pub output_root: Option<ByteString>,
    /// Recording scope.
    pub scope: Option<TmuxScopePolicy>,
    /// Live observer processes.
    pub observers: Option<Observers>,
    /// Live panes.
    pub pane_count: usize,
    /// Selected panes.
    pub selected_panes: usize,
    /// Panes not selected.
    pub excluded_panes: usize,
    /// Panes run by a wrapper.
    pub recorded_panes: usize,
    /// Selected panes not run by a wrapper.
    pub unrecorded_panes: usize,
    /// Every live pane.
    pub panes: Vec<TmuxPaneStatus>,
}

/// Observe a server's integration and pane coverage.
///
/// # Errors
/// Returns tmux failures or malformed options.
pub fn observe(client: &TmuxClient) -> Result<TmuxStatusReport> {
    let integration = Integration::read(client)?;
    let rows = client.list_panes()?;
    let panes = classify(integration.as_ref(), &rows, |pid| {
        return integration
            .as_ref()
            .is_some_and(|integration| return is_pane_wrapper(pid, &integration.termtrace));
    });

    let default_command_matches = match &integration {
        Some(_) => {
            let setup = PreparedTmuxSetup::from_options(client)?;

            client.option(setup.launch_option()).ok().as_deref()
                == Some(setup.wrapper_command().as_bytes())
        }
        None => false,
    };

    let observers = match &integration {
        Some(integration) => {
            let identity = client.server_identity()?;

            Some(Observers {
                supervisor: ObserverRole::Supervisor.holder(&integration.output_root, &identity),
                watchdog: ObserverRole::Watchdog.holder(&integration.output_root, &identity),
            })
        }
        None => None,
    };

    let selected_panes = panes.iter().filter(|pane| return pane.selected).count();
    let recorded_panes = panes.iter().filter(|pane| return pane.recorded).count();
    let unrecorded_panes = panes
        .iter()
        .filter(|pane| return pane.is_uncovered())
        .count();
    let pane_count = panes.len();

    return Ok(TmuxStatusReport {
        integration_version: 1,
        enabled: integration.is_some(),
        strict: integration.as_ref().is_some_and(|integration| {
            return integration.coverage == TmuxRecordingCoverage::Strict;
        }),
        default_command_matches,
        configured_executable: integration.as_ref().map(|integration| {
            return ByteString::from(integration.termtrace.as_os_str().as_encoded_bytes());
        }),
        output_root: integration.as_ref().map(|integration| {
            return ByteString::from(integration.output_root.as_os_str().as_encoded_bytes());
        }),
        scope: integration.map(|integration| return integration.scope),
        observers,
        pane_count,
        selected_panes,
        excluded_panes: pane_count.saturating_sub(selected_panes),
        recorded_panes,
        unrecorded_panes,
        panes: panes
            .into_iter()
            .map(|pane| {
                let snapshot = pane.row.snapshot;

                return TmuxPaneStatus {
                    pane_id: snapshot.pane_id,
                    session_id: snapshot.session_id,
                    window_id: snapshot.window_id,
                    pane_pid: pane.row.pid,
                    current_command: pane.row.current_command,
                    selected: pane.selected,
                    recorded: pane.recorded,
                    recording_session: snapshot.recording_session,
                };
            })
            .collect(),
    });
}
