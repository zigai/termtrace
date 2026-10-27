//! Installing, describing, and removing the integration's tmux options and hooks.

use crate::{
    artifact::TmuxRecordingCoverage,
    client::{config_quote, hook_name, shell_quote, TmuxClient},
    error::{Error, Result},
    options::{
        self, GUARDED_HOOKS, HOOK_INDEX, INTEGRATION_VERSION, OBSERVED_HOOKS, OBSERVER_CHANNEL,
        WRAPPER_BINARY_ENV,
    },
    scope::{ScopeTargets, TmuxScopePolicy},
};
use std::{
    env,
    ffi::{OsStr, OsString},
    fmt::{self, Write as _},
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
};

/// Option the supplied tmux launch-hook patch adds.
pub const LAUNCH_HOOK_OPTION: &str = "pane-launch-command";

/// Whether the tmux at `executable` has the launch-hook patch.
///
/// Starts a server with no configuration on `socket`; tmux stops it as soon as the probe
/// disconnects because it holds no sessions. The caller owns and removes the socket's
/// directory.
///
/// # Errors
/// Returns [`Error::Tmux`] when tmux cannot run or fails for another reason.
pub fn launch_hook_supported(executable: &Path, socket: &Path) -> Result<bool> {
    let output = Command::new(executable)
        .arg("-S")
        .arg(socket)
        .args([
            "-f",
            "/dev/null",
            "start-server",
            ";",
            "show-options",
            "-gv",
        ])
        .arg(LAUNCH_HOOK_OPTION)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            return Error::Tmux(format!(
                "could not run tmux executable {}: {error}",
                executable.display()
            ));
        })?;

    if output.status.success() {
        return Ok(true);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("invalid option") {
        return Ok(false);
    }
    return Err(Error::Tmux(format!(
        "tmux command failed with {}: {}",
        output.status,
        stderr.trim()
    )));
}

/// Ghostty profile capture policy for new panes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GhosttyProfileMode {
    /// Save the current Ghostty profile when Ghostty is available.
    Auto,
    /// Require and save the current Ghostty profile for every pane.
    Required,
    /// Record semantic evidence without a saved Ghostty profile.
    Off,
}

impl GhosttyProfileMode {
    /// Stable option spelling.
    pub fn as_str(self) -> &'static str {
        return match self {
            Self::Auto => "auto",
            Self::Required => "required",
            Self::Off => "off",
        };
    }
}

impl FromStr for GhosttyProfileMode {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        return match value {
            "auto" => Ok(Self::Auto),
            "required" => Ok(Self::Required),
            "off" => Ok(Self::Off),
            _ => Err(Error::invalid(format!(
                "unsupported Ghostty profile mode `{value}`"
            ))),
        };
    }
}

/// Automatic deletion of completed pane recordings; nothing is deleted when both are unset.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PaneRetention {
    /// Delete completed pane recordings older than this many days.
    pub older_than_days: Option<u64>,
    /// Delete the oldest completed pane recordings while their total exceeds this many bytes.
    pub max_bytes: Option<u64>,
}

impl PaneRetention {
    /// Whether any limit is set.
    pub const fn is_enabled(self) -> bool {
        return self.older_than_days.is_some() || self.max_bytes.is_some();
    }

    /// Read the configured limits.
    ///
    /// # Errors
    /// Returns tmux failures or malformed limits.
    pub fn from_options(client: &TmuxClient) -> Result<Self> {
        let limit = |name: &str| -> Result<Option<u64>> {
            let value = client.option_text(name)?;
            if value.is_empty() {
                return Ok(None);
            }
            return value
                .parse()
                .map(Some)
                .map_err(|_invalid| return Error::invalid(format!("{name} is not a number")));
        };

        return Ok(Self {
            older_than_days: limit(options::RETAIN_DAYS)?,
            max_bytes: limit(options::RETAIN_BYTES)?,
        });
    }
}

/// Requested integration settings before validation.
#[derive(Clone, Debug)]
pub struct SetupRequest {
    /// Use the patched `pane-launch-command` hook instead of `default-command`.
    pub launch_hook: bool,
    /// Ghostty profile policy.
    pub profile_mode: GhosttyProfileMode,
    /// Ghostty configuration override.
    pub ghostty_config: Option<PathBuf>,
    /// Coverage policy.
    pub coverage: TmuxRecordingCoverage,
    /// Aggregate manifest updated by the supervisor.
    pub aggregate_manifest: Option<PathBuf>,
    /// Record only explicitly launched panes.
    pub explicit_only: bool,
    /// Initial scope targets.
    pub targets: ScopeTargets,
    /// Fallback cell width.
    pub cell_width: u16,
    /// Fallback cell height.
    pub cell_height: u16,
    /// Terminal identity override.
    pub term: Option<String>,
    /// Automatic deletion of completed pane recordings.
    pub retention: PaneRetention,
}

/// Validated integration settings with canonical paths.
#[derive(Clone, Debug)]
pub struct PreparedTmuxSetup {
    /// Use the patched launch hook.
    pub launch_hook: bool,
    /// Canonical Termtrace executable.
    pub termtrace: PathBuf,
    /// tmux executable used by wrappers and hooks.
    pub tmux: PathBuf,
    /// Canonical managed state root.
    pub output_root: PathBuf,
    /// Ghostty profile policy.
    pub profile_mode: GhosttyProfileMode,
    /// Canonical Ghostty configuration override.
    pub ghostty_config: Option<PathBuf>,
    /// Coverage policy.
    pub coverage: TmuxRecordingCoverage,
    /// Aggregate manifest updated by the supervisor.
    pub aggregate_manifest: Option<PathBuf>,
    /// Fallback cell width.
    pub cell_width: u16,
    /// Fallback cell height.
    pub cell_height: u16,
    /// Terminal identity override; empty inherits the pane's TERM.
    pub term: String,
    /// Recording scope.
    pub scope: TmuxScopePolicy,
    /// Automatic deletion of completed pane recordings.
    pub retention: PaneRetention,
}

/// Outcome of enabling on a running server.
#[derive(Clone, Debug)]
pub struct Enabled {
    /// The integration was already enabled; only coverage and aggregate were replaced.
    pub already_enabled: bool,
    previous: Option<(Vec<u8>, Vec<u8>)>,
}

impl PreparedTmuxSetup {
    /// Validate a request against the canonical `state_root`.
    ///
    /// # Errors
    /// Returns invalid settings or unresolvable paths.
    pub fn prepare(request: SetupRequest, tmux: PathBuf, state_root: &Path) -> Result<Self> {
        return Self::prepare_inner(request, tmux, state_root, None);
    }

    /// Prepare integration using an explicitly selected Termtrace executable.
    /// Embedded callers must use this instead of their host process executable.
    /// Quoting, canonicalization, scope, and capture policies are unchanged.
    /// # Errors
    /// Returns invalid settings or unresolvable paths; never falls back to the host.
    pub fn prepare_with_executable(
        request: SetupRequest,
        tmux: PathBuf,
        state_root: &Path,
        termtrace: &Path,
    ) -> Result<Self> {
        return Self::prepare_inner(request, tmux, state_root, Some(termtrace));
    }

    fn prepare_inner(
        request: SetupRequest,
        tmux: PathBuf,
        state_root: &Path,
        termtrace: Option<&Path>,
    ) -> Result<Self> {
        if request.cell_width == 0 || request.cell_height == 0 {
            return Err(Error::invalid(
                "tmux fallback cell dimensions must be nonzero",
            ));
        }

        if request
            .term
            .as_ref()
            .is_some_and(|term| return term.is_empty() || term.as_bytes().contains(&0))
        {
            return Err(Error::invalid(
                "tmux terminal identity override must be nonempty",
            ));
        }

        if request.profile_mode == GhosttyProfileMode::Off && request.ghostty_config.is_some() {
            return Err(Error::invalid(
                "--ghostty-config cannot be used with --ghostty-profile off",
            ));
        }

        let scope = TmuxScopePolicy::initial(request.explicit_only, &request.targets)?;
        let canonical = |path: &Path| {
            return fs_err::canonicalize(path)
                .map_err(|error| return Error::filesystem(path, error));
        };

        let executable = termtrace.map_or_else(
            || {
                return env::current_exe()
                    .map_err(|error| return Error::filesystem("/proc/self/exe", error));
            },
            |binary| return Ok(binary.to_path_buf()),
        )?;

        return Ok(Self {
            launch_hook: request.launch_hook,
            termtrace: canonical(&executable)?,
            tmux,
            output_root: canonical(state_root)?,
            profile_mode: request.profile_mode,
            ghostty_config: request
                .ghostty_config
                .as_deref()
                .map(canonical)
                .transpose()?,
            coverage: request.coverage,
            aggregate_manifest: request.aggregate_manifest,
            cell_width: request.cell_width,
            cell_height: request.cell_height,
            term: request.term.unwrap_or_default(),
            scope,
            retention: request.retention,
        });
    }

    /// Read the settings installed on a server.
    ///
    /// # Errors
    /// Returns missing or malformed options.
    pub fn from_options(client: &TmuxClient) -> Result<Self> {
        return Ok(Self {
            launch_hook: client.option_text(options::LAUNCH_HOOK)? == "1",
            termtrace: PathBuf::from(OsString::from_vec(client.option(options::TERMTRACE)?)),
            tmux: PathBuf::from(OsString::from_vec(client.option(options::TMUX)?)),
            output_root: client
                .option_path(options::OUTPUT_ROOT)?
                .ok_or_else(|| return Error::invalid("configured tmux output root is empty"))?,
            profile_mode: client.option_text(options::PROFILE_MODE)?.parse()?,
            ghostty_config: client.option_path(options::GHOSTTY_CONFIG)?,
            coverage: client
                .option_text(options::COVERAGE)?
                .parse()
                .map_err(Error::invalid)?,
            aggregate_manifest: client.option_path(options::AGGREGATE_MANIFEST)?,
            cell_width: client.parse_option(options::CELL_WIDTH)?,
            cell_height: client.parse_option(options::CELL_HEIGHT)?,
            term: client.option_text(options::TERM)?,
            scope: TmuxScopePolicy::from_options(client)?,
            retention: PaneRetention::from_options(client)?,
        });
    }

    /// Option naming the command tmux runs for new panes.
    pub fn launch_option(&self) -> &'static str {
        return if self.launch_hook { LAUNCH_HOOK_OPTION } else { "default-command" };
    }

    /// Pane wrapper installed in [`Self::launch_option`].
    pub fn wrapper_command(&self) -> OsString {
        let mut command = self.env_prefix();
        command.extend_from_slice(b" tmux run -q");

        if self.launch_hook {
            command.extend_from_slice(b" --pane-launch -- \"$@\"");
        }

        return OsString::from_vec(command);
    }

    /// tmux-format-escaped background command that starts the observer watchdog.
    pub fn watchdog_command(&self) -> OsString {
        let mut command = self.env_prefix();
        command.extend_from_slice(b" tmux watchdog -q");

        let mut escaped = Vec::new();
        for byte in command {
            escaped.push(byte);

            if byte == b'#' {
                escaped.push(byte);
            }
        }

        return OsString::from_vec(escaped);
    }

    fn env_prefix(&self) -> Vec<u8> {
        let mut command = b"exec env ".to_vec();
        command.extend_from_slice(WRAPPER_BINARY_ENV.as_bytes());
        command.push(b'=');
        command.extend_from_slice(&shell_quote(self.tmux.as_os_str().as_bytes()));
        command.push(b' ');
        command.extend_from_slice(&shell_quote(self.termtrace.as_os_str().as_bytes()));

        return command;
    }

    /// tmux configuration fragment that enables recording from server startup.
    ///
    /// # Errors
    /// Returns malformed scope policies.
    pub fn config_fragment(&self) -> Result<Vec<u8>> {
        let mut output = String::from(
            "# Generated by `termtrace tmux config`. Source this before tmux creates panes.\n\
%if \"#{==:#{@termtrace-recording-version},}\"\n\
set-option -gF @termtrace-original-default-command \"#{default-command}\"\n\
set-option -gF @termtrace-original-default-shell \"#{default-shell}\"\n\
%endif\n",
        );

        if self.launch_hook {
            output.push_str("%if \"#{==:#{@termtrace-launch-hook},}\"\nset-option -gF @termtrace-original-pane-launch-command \"#{pane-launch-command}\"\n%endif\n");
        }

        for (name, value) in self.option_values()? {
            writeln!(output, "set-option -gq {name} {}", config_quote(&value))
                .map_err(config_format_error)?;
        }

        for (name, hook) in reserved_hooks() {
            writeln!(
                output,
                "set-hook -g {} {}",
                hook_name(name),
                config_quote(hook.as_bytes()),
            )
            .map_err(config_format_error)?;
        }

        writeln!(
            output,
            "run-shell -b {}",
            config_quote(self.watchdog_command().as_bytes()),
        )
        .map_err(config_format_error)?;

        return Ok(output.into_bytes());
    }

    /// Every configured option in installation order; the version and launch command come last
    /// so hooks never observe a partially installed integration.
    fn option_values(&self) -> Result<Vec<(&'static str, Vec<u8>)>> {
        let path = |path: Option<&PathBuf>| {
            return path.map_or_else(Vec::new, |path| return path.as_os_str().as_bytes().to_vec());
        };

        return Ok(vec![
            (
                options::LAUNCH_HOOK,
                if self.launch_hook { b"1".to_vec() } else { b"0".to_vec() },
            ),
            (
                options::TERMTRACE,
                self.termtrace.as_os_str().as_bytes().to_vec(),
            ),
            (options::TMUX, self.tmux.as_os_str().as_bytes().to_vec()),
            (
                options::OUTPUT_ROOT,
                self.output_root.as_os_str().as_bytes().to_vec(),
            ),
            (
                options::PROFILE_MODE,
                self.profile_mode.as_str().as_bytes().to_vec(),
            ),
            (options::GHOSTTY_CONFIG, path(self.ghostty_config.as_ref())),
            (
                options::COVERAGE,
                self.coverage.as_str().as_bytes().to_vec(),
            ),
            (
                options::AGGREGATE_MANIFEST,
                path(self.aggregate_manifest.as_ref()),
            ),
            (
                options::CELL_WIDTH,
                self.cell_width.to_string().into_bytes(),
            ),
            (
                options::CELL_HEIGHT,
                self.cell_height.to_string().into_bytes(),
            ),
            (options::TERM, self.term.as_bytes().to_vec()),
            (
                options::RETAIN_DAYS,
                self.retention
                    .older_than_days
                    .map_or_else(Vec::new, |days| return days.to_string().into_bytes()),
            ),
            (
                options::RETAIN_BYTES,
                self.retention
                    .max_bytes
                    .map_or_else(Vec::new, |bytes| return bytes.to_string().into_bytes()),
            ),
            (options::SCOPE, self.scope.to_bytes()?),
            (options::VERSION, INTEGRATION_VERSION.as_bytes().to_vec()),
            (self.launch_option(), self.wrapper_command().into_vec()),
        ]);
    }

    /// Enable recording for future panes of a running server.
    ///
    /// An already enabled server keeps its settings except coverage and aggregate.
    ///
    /// # Errors
    /// Returns conflicting existing settings or tmux failures; a fresh enable is rolled back.
    pub fn enable(&self, client: &TmuxClient) -> Result<Enabled> {
        let existing_version = client.option_text(options::VERSION)?;
        if !existing_version.is_empty() {
            if existing_version != INTEGRATION_VERSION {
                return Err(Error::invalid(format!(
                    "tmux recording is already configured with unsupported integration version {existing_version}"
                )));
            }

            if (client.option_text(options::LAUNCH_HOOK)? == "1") != self.launch_hook {
                return Err(Error::invalid(
                    "disable the existing tmux integration before changing launch-hook mode",
                ));
            }

            let previous = (
                client.option(options::COVERAGE)?,
                client.option(options::AGGREGATE_MANIFEST)?,
            );

            client.set_option(options::COVERAGE, OsStr::new(self.coverage.as_str()))?;
            client.set_option(
                options::AGGREGATE_MANIFEST,
                self.aggregate_manifest
                    .as_ref()
                    .map_or_else(|| return OsStr::new(""), |path| return path.as_os_str()),
            )?;
            client.run_shell_background(&self.watchdog_command())?;
            client.signal_observer()?;

            return Ok(Enabled {
                already_enabled: true,
                previous: Some(previous),
            });
        }

        let original_command = client.option("default-command")?;

        if self.launch_hook {
            let _ = client.output(["show-options", "-gv", LAUNCH_HOOK_OPTION])?;
        }

        let original_shell = client.option("default-shell")?;
        if original_shell.is_empty() {
            return Err(Error::invalid("tmux default-shell is empty"));
        }

        for (name, hook) in reserved_hooks() {
            let existing = client.hook(name)?;
            if !existing.is_empty() && existing != hook.as_bytes() {
                return Err(Error::invalid(format!(
                    "tmux hook {} already uses reserved index {HOOK_INDEX}",
                    hook_name(name)
                )));
            }
        }

        let installed = self
            .install(client, &original_command, &original_shell)
            .and_then(|()| return grandfather_current_panes(client))
            .and_then(|()| return client.run_shell_background(&self.watchdog_command()));
        if let Err(error) = installed {
            let _cleanup = clear_pane_marks(client);
            let _rollback = remove(client, &original_command);

            return Err(error);
        }

        return Ok(Enabled {
            already_enabled: false,
            previous: None,
        });
    }

    fn install(
        &self,
        client: &TmuxClient,
        original_command: &[u8],
        original_shell: &[u8],
    ) -> Result<()> {
        if self.launch_hook {
            let original = client.output(["show-options", "-gv", LAUNCH_HOOK_OPTION])?;
            client.set_option(options::ORIGINAL_LAUNCH_HOOK, OsStr::from_bytes(&original))?;
        }

        client.set_option(
            options::ORIGINAL_COMMAND,
            OsStr::from_bytes(original_command),
        )?;
        client.set_option(options::ORIGINAL_SHELL, OsStr::from_bytes(original_shell))?;

        for (name, value) in self.option_values()? {
            client.set_option(name, OsStr::from_bytes(&value))?;
        }

        for (name, hook) in reserved_hooks() {
            client.set_hook(name, &hook)?;
        }

        return Ok(());
    }
}

impl Enabled {
    /// Undo this enable after a failed start.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn roll_back(self, client: &TmuxClient) -> Result<()> {
        return match self.previous {
            Some((coverage, aggregate)) => {
                client.set_option(options::COVERAGE, OsStr::from_bytes(&coverage))?;

                client.set_option(options::AGGREGATE_MANIFEST, OsStr::from_bytes(&aggregate))
            }
            None => disable(client).map(drop),
        };
    }
}

/// Outcome of disabling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Disabled {
    /// No integration was enabled.
    AlreadyDisabled,
    /// Saved launch settings were restored.
    Restored,
}

/// Restore the launch settings saved by enable and remove every integration option.
///
/// # Errors
/// Returns unsupported versions, externally changed launch settings, or tmux failures.
pub fn disable(client: &TmuxClient) -> Result<Disabled> {
    let version = client.option_text(options::VERSION)?;
    if version.is_empty() {
        return Ok(Disabled::AlreadyDisabled);
    }

    if version != INTEGRATION_VERSION {
        return Err(Error::invalid(format!(
            "cannot disable unsupported tmux recording integration version {version}"
        )));
    }

    let expected = PreparedTmuxSetup::from_options(client)?;
    if client.option(expected.launch_option())? != expected.wrapper_command().as_bytes() {
        return Err(Error::invalid(
            "tmux launch command changed after Termtrace was enabled; refusing to overwrite it",
        ));
    }

    for (name, hook) in reserved_hooks() {
        let current = client.hook(name)?;
        if !current.is_empty() && current != hook.as_bytes() {
            return Err(Error::invalid(format!(
                "tmux hook {} changed after Termtrace was enabled; refusing to overwrite it",
                hook_name(name)
            )));
        }
    }

    let original = client.option(options::ORIGINAL_COMMAND)?;
    remove(client, &original)?;
    clear_pane_marks(client)?;
    client.signal_observer()?;

    return Ok(Disabled::Restored);
}

fn remove(client: &TmuxClient, original_command: &[u8]) -> Result<()> {
    if client.option_text(options::LAUNCH_HOOK)? == "1" {
        let original = client.option(options::ORIGINAL_LAUNCH_HOOK)?;
        client.set_option(LAUNCH_HOOK_OPTION, OsStr::from_bytes(&original))?;
    } else {
        client.set_option("default-command", OsStr::from_bytes(original_command))?;
    }

    // Removal continues past individual failures so every remaining hook and option is cleared.
    for (name, _) in reserved_hooks() {
        if let Err(error) = client.unset_hook(name) {
            tracing::warn!(%error, hook = name, "could not unset termtrace tmux hook");
        }
    }

    for name in options::RESERVED {
        if let Err(error) = client.unset_option(name) {
            tracing::warn!(%error, option = name, "could not unset termtrace tmux option");
        }
    }

    return Ok(());
}

fn config_format_error(source: fmt::Error) -> Error {
    return Error::invalid_with_source("could not format tmux configuration", source);
}

/// Guard hook command run for every new pane.
pub fn hook_command() -> OsString {
    return OsString::from(
        "run-shell -b \"exec env TERMTRACE_TMUX_BINARY=#{q:@termtrace-tmux-executable} #{q:@termtrace-executable} tmux guard -q --pane '#{pane_id}'\"",
    );
}

/// Every hook installed at Termtrace's reserved index, with its command.
pub fn reserved_hooks() -> impl Iterator<Item = (&'static str, OsString)> {
    let guard = hook_command();
    let signal = OsString::from(format!("wait-for -S {OBSERVER_CHANNEL}"));

    return GUARDED_HOOKS
        .iter()
        .map(move |name| return (*name, guard.clone()))
        .chain(
            OBSERVED_HOOKS
                .iter()
                .map(move |name| return (*name, signal.clone())),
        );
}

/// Exempt every current pane process from enforcement.
///
/// # Errors
/// Returns tmux failures.
pub fn grandfather_current_panes(client: &TmuxClient) -> Result<()> {
    for row in client.list_panes()? {
        if row.dead {
            continue;
        }

        if let Some(pid) = row.pid {
            client.set_pane_option(
                &row.snapshot.pane_id,
                options::GRANDFATHERED_PID,
                OsStr::new(&pid.to_string()),
            )?;
        }
    }

    return Ok(());
}

fn clear_pane_marks(client: &TmuxClient) -> Result<()> {
    for row in client.list_panes()? {
        for option in options::PANE_MARKS {
            client.unset_pane_option(&row.snapshot.pane_id, option)?;
        }
    }

    return Ok(());
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
