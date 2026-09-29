//! Deciding, launching, and publishing one pane process recording (`termtrace tmux run`).

use crate::{
    client::{same_executable, valid_identifier, TmuxClient},
    error::{Error, Result},
    options, recording,
    scope::{mark_decision, TmuxScopePolicy},
    setup::GhosttyProfileMode,
};
use std::{
    env,
    ffi::OsString,
    os::unix::{ffi::OsStringExt as _, process::CommandExt as _},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};
use termtrace_control::control_endpoint_is_reachable;
use termtrace_model::{
    error::ThreadPanic,
    execution::{
        layout::SessionLayout,
        session::{TmuxPaneLaunchKind, TmuxPaneProvenance, TMUX_PANE_PROVENANCE_VERSION},
    },
    value::ByteString,
};

/// Arguments of one pane wrapper invocation.
#[derive(Clone, Debug, Default)]
pub struct PaneRunRequest {
    /// Arguments came from the patched tmux launch hook.
    pub pane_launch: bool,
    /// Record the saved default command even outside the scope.
    pub force: bool,
    /// One command string for `/bin/sh -c`.
    pub shell_command: Option<OsString>,
    /// Exact argv; empty restores tmux's saved default.
    pub command: Vec<OsString>,
}

/// What a pane wrapper does with its pane.
#[derive(Debug)]
pub enum PaneLaunch {
    /// Replace this process with the command, unrecorded.
    Unrecorded(Vec<OsString>),
    /// Record the command in a Termtrace-owned PTY.
    Record(Box<PaneRecording>),
}

/// Everything needed to record one pane process.
#[derive(Clone, Debug)]
pub struct PaneRecording {
    /// Client of the pane's server.
    pub client: TmuxClient,
    /// Stable pane ID.
    pub pane: String,
    /// Child argv.
    pub command: Vec<OsString>,
    /// Pane identity captured before the child starts; width and height are the pane's
    /// cell size until the recorder measures the inherited PTY.
    pub provenance: TmuxPaneProvenance,
    /// Fallback cell width.
    pub cell_width: u16,
    /// Fallback cell height.
    pub cell_height: u16,
    /// Terminal identity.
    pub term: String,
    /// Ghostty profile policy.
    pub profile_mode: GhosttyProfileMode,
    /// Ghostty configuration override.
    pub ghostty_config: Option<PathBuf>,
    /// Managed state root.
    pub output_root: PathBuf,
    /// Aggregate manifest that links this recording.
    pub aggregate_manifest: Option<PathBuf>,
}

impl PaneRecording {
    /// Registry alias recorded at creation.
    pub fn creation_alias(&self) -> String {
        return format!(
            "tmux-{}-{}-{}",
            self.provenance.session_id.trim_start_matches('$'),
            self.provenance.window_id.trim_start_matches('@'),
            self.provenance.pane_id.trim_start_matches('%'),
        );
    }
}

/// Decide what the pane wrapper running in `$TMUX_PANE` does.
///
/// # Errors
/// Returns a missing or disabled integration, a foreign executable, or tmux failures.
pub fn prepare(request: PaneRunRequest) -> Result<PaneLaunch> {
    let pane = env::var("TMUX_PANE").map_err(|source| {
        return Error::invalid_with_source(
            "termtrace tmux run must execute inside a tmux pane",
            source,
        );
    })?;

    if !valid_identifier(&pane, b'%') {
        return Err(Error::invalid(
            "TMUX_PANE contains an invalid pane identifier",
        ));
    }

    let client = TmuxClient::inherited();
    if !client.integration_enabled() {
        return Err(Error::invalid(
            "tmux recording is not enabled for this server; run `termtrace tmux enable` or source a generated configuration",
        ));
    }

    let configured = PathBuf::from(OsString::from_vec(client.option(options::TERMTRACE)?));
    let current =
        env::current_exe().map_err(|error| return Error::filesystem("/proc/self/exe", error))?;

    if !same_executable(&current, &configured) {
        return Err(Error::invalid(format!(
            "the active tmux integration points to a different Termtrace executable: {}",
            configured.display()
        )));
    }

    let explicitly_selected = !request.pane_launch
        && (request.force || request.shell_command.is_some() || !request.command.is_empty());

    let (command, launch_kind) = child_command(&client, request, &current)?;
    client.unset_pane_option(&pane, options::PANE_SESSION)?;
    let row = client.pane_row(&pane)?;
    let pid = row
        .pid
        .ok_or_else(|| return Error::Tmux(format!("tmux pane {pane} has no process")))?;

    let scope = TmuxScopePolicy::from_options(&client)?;
    if !explicitly_selected && !scope.matches(&row) {
        mark_decision(&client, &pane, pid, false)?;

        return Ok(PaneLaunch::Unrecorded(command));
    }

    mark_decision(&client, &pane, pid, true)?;

    let provenance = capture_provenance(&client, &pane, launch_kind)?;
    let configured_term = client.option_text(options::TERM)?;
    let term = if configured_term.is_empty() {
        env::var("TERM").map_err(|source| {
            return Error::invalid_with_source(
                "tmux pane did not provide a terminal identity in TERM",
                source,
            );
        })?
    } else {
        configured_term
    };

    return Ok(PaneLaunch::Record(Box::new(PaneRecording {
        pane,
        command,
        provenance,
        cell_width: client.parse_option(options::CELL_WIDTH)?,
        cell_height: client.parse_option(options::CELL_HEIGHT)?,
        term,
        profile_mode: client.option_text(options::PROFILE_MODE)?.parse()?,
        ghostty_config: client.option_path(options::GHOSTTY_CONFIG)?,
        output_root: client
            .option_path(options::OUTPUT_ROOT)?
            .ok_or_else(|| return Error::invalid("configured tmux output root is empty"))?,
        aggregate_manifest: client.option_path(options::AGGREGATE_MANIFEST)?,
        client,
    })));
}

fn child_command(
    client: &TmuxClient,
    request: PaneRunRequest,
    current_exe: &Path,
) -> Result<(Vec<OsString>, TmuxPaneLaunchKind)> {
    if request.pane_launch {
        let launch_kind = validate_pane_launch(&request.command)?;

        let mut command = vec![
            current_exe.as_os_str().to_owned(),
            OsString::from("tmux"),
            OsString::from("exec-pane"),
            OsString::from("--"),
        ];
        command.extend(request.command);

        return Ok((command, launch_kind));
    }

    if let Some(shell_command) = request.shell_command {
        return Ok((
            vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                shell_command,
            ],
            TmuxPaneLaunchKind::ShellCommand,
        ));
    }

    if !request.command.is_empty() {
        return Ok((request.command, TmuxPaneLaunchKind::ExplicitArgv));
    }

    let shell = client.option(options::ORIGINAL_SHELL)?;
    if shell.is_empty() {
        return Err(Error::invalid("saved tmux default shell is empty"));
    }

    let default_command = client.option(options::ORIGINAL_COMMAND)?;
    if !default_command.is_empty() {
        return Ok((
            vec![
                OsString::from_vec(shell),
                OsString::from("-c"),
                OsString::from_vec(default_command),
            ],
            TmuxPaneLaunchKind::DefaultCommand,
        ));
    }
    return Ok((
        vec![OsString::from_vec(shell), OsString::from("-l")],
        TmuxPaneLaunchKind::DefaultShell,
    ));
}

/// Classify arguments from the patched launch hook: `MODE SHELL [ARGS...]`.
///
/// # Errors
/// Returns an unknown mode or a wrong argument count.
pub fn validate_pane_launch(arguments: &[OsString]) -> Result<TmuxPaneLaunchKind> {
    let [mode, shell, rest @ ..] = arguments else {
        return Err(Error::invalid("pane launch requires a mode and shell"));
    };
    if shell.is_empty() {
        return Err(Error::invalid("pane launch shell is empty"));
    }
    return match (mode.to_str(), rest.len()) {
        (Some("login"), 0) => Ok(TmuxPaneLaunchKind::DefaultShell),
        (Some("shell"), 1) => Ok(TmuxPaneLaunchKind::ShellCommand),
        (Some("exec"), 2..) => Ok(TmuxPaneLaunchKind::ExplicitArgv),
        _ => Err(Error::invalid("invalid pane launch mode or argument count")),
    };
}

/// Replace this process with an unrecorded pane command; returns only on failure.
pub fn exec_unrecorded(command: Vec<OsString>) -> Error {
    let mut arguments = command.into_iter();
    let Some(program) = arguments.next() else {
        return Error::invalid("tmux pane command is empty");
    };
    let error = Command::new(&program)
        .args(arguments)
        .env_remove("TERMTRACE_RESPONSE")
        .exec();

    return Error::invalid(format!(
        "could not execute unrecorded tmux pane command {}: {error}",
        Path::new(&program).display()
    ));
}

/// Restore exact tmux launch semantics inside the recorded PTY; returns only on failure.
pub fn exec_pane_launch(arguments: &[OsString]) -> Error {
    let kind = match validate_pane_launch(arguments) {
        Ok(kind) => kind,
        Err(error) => return error,
    };

    let [_, shell, rest @ ..] = arguments else {
        return Error::invalid("pane launch requires a mode and shell");
    };
    let mut command = match (kind, rest) {
        (TmuxPaneLaunchKind::ExplicitArgv, [program, arguments @ ..]) => {
            let mut command = Command::new(program);
            command.args(arguments);
            command
        }
        (TmuxPaneLaunchKind::DefaultShell, []) => {
            let mut argv0 = OsString::from("-");
            argv0.push(Path::new(shell).file_name().unwrap_or(shell));

            let mut command = Command::new(shell);
            command.arg0(argv0);
            command
        }
        (TmuxPaneLaunchKind::ShellCommand, [source]) => {
            let mut command = Command::new(shell);
            command
                .arg0(Path::new(shell).file_name().unwrap_or(shell))
                .arg("-c")
                .arg(source);
            command
        }
        _ => return Error::invalid("unsupported pane launch kind"),
    };

    return Error::invalid(format!(
        "could not execute tmux pane launch: {}",
        command.exec()
    ));
}

fn capture_provenance(
    client: &TmuxClient,
    pane: &str,
    launch_kind: TmuxPaneLaunchKind,
) -> Result<TmuxPaneProvenance> {
    let session_id = client.pane_text(pane, "#{session_id}")?;
    let window_id = client.pane_text(pane, "#{window_id}")?;
    let pane_id = client.pane_text(pane, "#{pane_id}")?;

    if !valid_identifier(&session_id, b'$')
        || !valid_identifier(&window_id, b'@')
        || !valid_identifier(&pane_id, b'%')
    {
        return Err(Error::Tmux(
            "tmux returned invalid stable object identifiers".to_owned(),
        ));
    }
    return Ok(TmuxPaneProvenance {
        version: TMUX_PANE_PROVENANCE_VERSION,
        tmux_version: client.pane_text(pane, "#{version}")?,
        socket_path: ByteString::from(client.pane_format(pane, "#{socket_path}")?),
        server_pid: client.pane_number(pane, "#{pid}")?,
        session_id,
        session_name: ByteString::from(client.pane_format(pane, "#{session_name}")?),
        session_created_unix_seconds: client.pane_number(pane, "#{session_created}")?,
        window_id,
        window_index: client.pane_number(pane, "#{window_index}")?,
        window_name: ByteString::from(client.pane_format(pane, "#{window_name}")?),
        pane_id,
        pane_index: client.pane_number(pane, "#{pane_index}")?,
        pane_pid: client.pane_number(pane, "#{pane_pid}")?,
        pane_width: client.pane_number(pane, "#{pane_width}")?,
        pane_height: client.pane_number(pane, "#{pane_height}")?,
        pane_start_command: ByteString::from(client.pane_format(pane, "#{pane_start_command}")?),
        launch_kind,
    });
}

/// Background publication of a pane recording's session path.
pub struct Publication {
    cancel: mpsc::SyncSender<()>,
    thread: thread::JoinHandle<Result<()>>,
}

impl Publication {
    /// Link the session to the aggregate once its manifest exists, then publish the
    /// session path on the pane once its control endpoint answers.
    ///
    /// # Errors
    /// Returns a failure to start the publication thread.
    pub fn start(recording: &PaneRecording, session: PathBuf) -> Result<Self> {
        // Retain the aggregate before recording begins: the pane or the whole server may
        // be gone by the time a short recording finishes publishing its manifest.
        let aggregate = recording.aggregate_manifest.clone();
        let client = recording.client.clone();
        let pane = recording.pane.clone();
        let (cancel, cancellation) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("termtrace-tmux-publication".to_owned())
            .spawn(move || {
                let manifest = SessionLayout::new(&session).manifest();
                let mut registered = false;
                let register = |registered: &mut bool| -> Result<()> {
                    if !*registered && manifest.is_file() {
                        if let Some(path) = &aggregate {
                            recording::register_pane_track(path, &pane, &session)?;
                        }

                        *registered = true;
                    }

                    return Ok(());
                };

                loop {
                    register(&mut registered)?;

                    if control_endpoint_is_reachable(&session) {
                        return client.set_pane_option(
                            &pane,
                            options::PANE_SESSION,
                            session.as_os_str(),
                        );
                    }

                    match cancellation.recv_timeout(Duration::from_millis(5)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return register(&mut registered);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            })
            .map_err(|error| return Error::filesystem("termtrace-tmux-publication", error))?;

        return Ok(Self { cancel, thread });
    }

    /// Stop publishing after the recording ended.
    ///
    /// # Errors
    /// Returns publication failures.
    pub fn finish(self) -> Result<()> {
        let _cancelled = self.cancel.try_send(());

        return self.thread.join().map_err(|payload| {
            return Error::invalid_with_source(
                "tmux recording publication thread panicked",
                ThreadPanic::from_payload(payload),
            );
        })?;
    }
}
