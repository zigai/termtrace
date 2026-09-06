use crate::cli::output::ResponseWriter;

use std::{
    env,
    ffi::{OsStr, OsString},
    fmt::Write as _,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::PathBuf,
    process::ExitCode,
};

use miette::{IntoDiagnostic as _, Result};
use termtrace_engine::registry::SessionRegistry;
use termtrace_model::value::ByteString;

use crate::cli::output::{write_json_line, write_stdout};

use super::{
    GUARDED_HOOKS, OPTION_AGGREGATE_MANIFEST, OPTION_CELL_HEIGHT, OPTION_CELL_WIDTH,
    OPTION_COVERAGE, OPTION_GHOSTTY_CONFIG, OPTION_LAUNCH_HOOK, OPTION_ORIGINAL_COMMAND,
    OPTION_ORIGINAL_LAUNCH_HOOK, OPTION_ORIGINAL_SHELL, OPTION_OUTPUT_ROOT, OPTION_PROFILE_MODE,
    OPTION_SCOPE, OPTION_TERM, OPTION_TERMTRACE, OPTION_TMUX, OPTION_VERSION, RESERVED_OPTIONS,
    TMUX_HOOK_INDEX, TMUX_INTEGRATION_VERSION, TMUX_WRAPPER_BINARY_ENV,
    args::{
        TmuxConfigArgs, TmuxConnectionArgs, TmuxCoverageMode, TmuxEnableArgs,
        TmuxGhosttyProfileMode, TmuxSetupArgs,
    },
    client::{
        TmuxClient, hook_name, option_path, option_text, parse_option, shell_quote,
        tmux_config_quote,
    },
    scope::{
        TmuxScopePolicy, initial_scope_policy, resolve_live_scope, scope_bytes, scope_from_options,
        set_scope_policy,
    },
    supervisor::{clear_managed_pane_options, grandfather_current_panes, tmux_status_report},
};

#[derive(Clone, Debug)]
pub(super) struct PreparedTmuxSetup {
    pub(super) launch_hook: bool,
    pub(super) termtrace: PathBuf,
    pub(super) tmux: PathBuf,
    pub(super) output_root: PathBuf,
    pub(super) profile_mode: TmuxGhosttyProfileMode,
    pub(super) ghostty_config: Option<PathBuf>,
    pub(super) coverage: TmuxCoverageMode,
    pub(super) aggregate_manifest: Option<PathBuf>,
    pub(super) cell_width: u16,
    pub(super) cell_height: u16,
    pub(super) term: String,
    pub(super) scope: TmuxScopePolicy,
}

pub(in crate::cli) fn run_tmux_config(
    response: &mut ResponseWriter,
    arguments: TmuxConfigArgs,
) -> Result<ExitCode> {
    let setup = prepare_tmux_setup(arguments.setup, arguments.tmux)?;
    return write_stdout(response, tmux_config_fragment(&setup)?);
}

pub(in crate::cli) fn run_tmux_enable(
    response: &mut ResponseWriter,
    arguments: TmuxEnableArgs,
) -> Result<ExitCode> {
    let client = TmuxClient::from_connection(arguments.connection.clone());
    let mut setup = prepare_tmux_setup(arguments.setup, arguments.connection.tmux)?;
    setup.scope = resolve_live_scope(&client, &setup.scope)?;
    let already_enabled = enable_prepared_tmux_setup(&client, &setup)?;
    let report = tmux_status_report(&client)?;
    return write_json_line(
        response,
        &serde_json::json!({
            "enabled": true,
            "already_enabled": already_enabled,
            "integration_version": 1,
            "coverage": setup.coverage,
            "strict": setup.coverage == TmuxCoverageMode::Strict,
            "output_root": ByteString::from(setup.output_root.as_os_str().as_bytes()),
            "scope": setup.scope,
            "existing_panes": report.pane_count,
            "existing_unrecorded_panes": report.unrecorded_panes,
            "retroactive": false,
        }),
    );
}

pub(super) fn enable_prepared_tmux_setup(
    client: &TmuxClient,
    setup: &PreparedTmuxSetup,
) -> Result<bool> {
    let existing_version = option_text(client, OPTION_VERSION)?;
    if !existing_version.is_empty() {
        if existing_version != TMUX_INTEGRATION_VERSION {
            return Err(miette::miette!(
                "tmux recording is already configured with unsupported integration version {existing_version}"
            ));
        }
        if (option_text(client, OPTION_LAUNCH_HOOK)? == "1") != setup.launch_hook {
            return Err(miette::miette!(
                "disable the existing tmux integration before changing launch-hook mode"
            ));
        }
        client.set_option(OPTION_COVERAGE, OsStr::new(setup.coverage.as_str()))?;
        client.set_option(
            OPTION_AGGREGATE_MANIFEST,
            setup
                .aggregate_manifest
                .as_ref()
                .map_or_else(|| return OsStr::new(""), |path| return path.as_os_str()),
        )?;
        client.run_shell_background(supervisor_shell_command(setup).as_os_str())?;
        return Ok(true);
    }
    let original_command = client.option("default-command")?;
    if setup.launch_hook {
        let _ = client.output(["show-options", "-gv", "pane-launch-command"])?;
    }
    let original_shell = client.option("default-shell")?;
    if original_shell.is_empty() {
        return Err(miette::miette!("tmux default-shell is empty"));
    }
    let hook_command = tmux_hook_command();
    for name in GUARDED_HOOKS {
        let existing = client.hook(name)?;
        if !existing.is_empty() && existing != hook_command.as_bytes() {
            return Err(miette::miette!(
                "tmux hook {} already uses reserved index {TMUX_HOOK_INDEX}",
                hook_name(name)
            ));
        }
    }
    if let Err(error) = apply_tmux_setup(client, setup, &original_command, &original_shell) {
        let _rollback = rollback_tmux_setup(client, &original_command);
        return Err(error);
    }
    if let Err(error) = grandfather_current_panes(client).and_then(|()| {
        return client.run_shell_background(supervisor_shell_command(setup).as_os_str());
    }) {
        let _cleanup = clear_managed_pane_options(client);
        let _rollback = rollback_tmux_setup(client, &original_command);
        return Err(error);
    }
    return Ok(false);
}

pub(in crate::cli) fn run_tmux_disable(
    response: &mut ResponseWriter,
    connection: TmuxConnectionArgs,
) -> Result<ExitCode> {
    let client = TmuxClient::from_connection(connection);
    let version = option_text(&client, OPTION_VERSION)?;
    if version.is_empty() {
        return write_json_line(
            response,
            &serde_json::json!({
                "enabled": false,
                "already_disabled": true,
            }),
        );
    }
    if version != TMUX_INTEGRATION_VERSION {
        return Err(miette::miette!(
            "cannot disable unsupported tmux recording integration version {version}"
        ));
    }
    let termtrace = client.option(OPTION_TERMTRACE)?;
    let tmux = client.option(OPTION_TMUX)?;
    let expected_setup = setup_from_options(&client, termtrace, tmux)?;
    let expected_wrapper = tmux_wrapper_command(&expected_setup);
    let current_command = client.option(launch_option(&expected_setup))?;
    if current_command != expected_wrapper.as_bytes() {
        return Err(miette::miette!(
            "tmux launch command changed after Termtrace was enabled; refusing to overwrite it"
        ));
    }
    let expected_hook = tmux_hook_command();
    for name in GUARDED_HOOKS {
        let current = client.hook(name)?;
        if !current.is_empty() && current != expected_hook.as_bytes() {
            return Err(miette::miette!(
                "tmux hook {} changed after Termtrace was enabled; refusing to overwrite it",
                hook_name(name)
            ));
        }
    }
    let original = client.option(OPTION_ORIGINAL_COMMAND)?;
    restore_launch_hook(&client)?;
    if !expected_setup.launch_hook {
        client.set_option("default-command", OsStr::from_bytes(&original))?;
    }
    for name in GUARDED_HOOKS {
        client.unset_hook(name)?;
    }
    for name in RESERVED_OPTIONS {
        client.unset_option(name)?;
    }
    clear_managed_pane_options(&client)?;
    return write_json_line(
        response,
        &serde_json::json!({
            "enabled": false,
            "already_disabled": false,
            "default_command_restored": true,
        }),
    );
}

pub(super) fn prepare_tmux_setup(setup: TmuxSetupArgs, tmux: PathBuf) -> Result<PreparedTmuxSetup> {
    if setup.cell_width == 0 || setup.cell_height == 0 {
        return Err(miette::miette!(
            "tmux fallback cell dimensions must be nonzero"
        ));
    }
    if setup
        .term
        .as_ref()
        .is_some_and(|term| return term.is_empty() || term.as_bytes().contains(&0))
    {
        return Err(miette::miette!(
            "tmux terminal identity override must be nonempty"
        ));
    }
    if matches!(setup.ghostty_profile, TmuxGhosttyProfileMode::Off)
        && setup.ghostty_config.is_some()
    {
        return Err(miette::miette!(
            "--ghostty-config cannot be used with --ghostty-profile off"
        ));
    }
    let scope = initial_scope_policy(
        setup.explicit_only,
        &setup.sessions,
        &setup.windows,
        &setup.panes,
    )?;
    let registry = match setup.output_root {
        Some(root) => SessionRegistry::open(root).into_diagnostic()?,
        None => SessionRegistry::open_default().into_diagnostic()?,
    };
    let output_root = fs_err::canonicalize(registry.state_root()).into_diagnostic()?;
    let termtrace =
        fs_err::canonicalize(env::current_exe().into_diagnostic()?).into_diagnostic()?;
    let ghostty_config = setup
        .ghostty_config
        .map(fs_err::canonicalize)
        .transpose()
        .into_diagnostic()?;
    return Ok(PreparedTmuxSetup {
        launch_hook: setup.launch_hook,
        termtrace,
        tmux,
        output_root,
        profile_mode: setup.ghostty_profile,
        ghostty_config,
        coverage: setup.coverage,
        aggregate_manifest: setup.aggregate_manifest,
        cell_width: setup.cell_width,
        cell_height: setup.cell_height,
        term: setup.term.unwrap_or_default(),
        scope,
    });
}

pub(super) fn setup_from_options(
    client: &TmuxClient,
    termtrace: Vec<u8>,
    tmux: Vec<u8>,
) -> Result<PreparedTmuxSetup> {
    let profile_text = option_text(client, OPTION_PROFILE_MODE)?;
    let profile_mode = <TmuxGhosttyProfileMode as clap::ValueEnum>::from_str(&profile_text, true)
        .map_err(|_| {
        return miette::miette!("unsupported Ghostty profile mode `{profile_text}`");
    })?;
    return Ok(PreparedTmuxSetup {
        launch_hook: option_text(client, OPTION_LAUNCH_HOOK)? == "1",
        termtrace: PathBuf::from(OsString::from_vec(termtrace)),
        tmux: PathBuf::from(OsString::from_vec(tmux)),
        output_root: option_path(client, OPTION_OUTPUT_ROOT)?
            .ok_or_else(|| return miette::miette!("configured tmux output root is empty"))?,
        profile_mode,
        ghostty_config: option_path(client, OPTION_GHOSTTY_CONFIG)?,
        coverage: {
            let cov_text = option_text(client, OPTION_COVERAGE)?;
            <TmuxCoverageMode as clap::ValueEnum>::from_str(&cov_text, true).map_err(|_| {
                return miette::miette!("unsupported tmux coverage mode `{cov_text}`");
            })?
        },
        aggregate_manifest: option_path(client, OPTION_AGGREGATE_MANIFEST)?,
        cell_width: parse_option(client, OPTION_CELL_WIDTH)?,
        cell_height: parse_option(client, OPTION_CELL_HEIGHT)?,
        term: option_text(client, OPTION_TERM)?,
        scope: scope_from_options(client)?,
    });
}

pub(super) fn tmux_config_fragment(setup: &PreparedTmuxSetup) -> Result<Vec<u8>> {
    let wrapper = tmux_wrapper_command(setup);
    let hook = tmux_hook_command();
    let mut output = String::from(
        "# Generated by `termtrace tmux config`. Source this before tmux creates panes.\n\
%if \"#{==:#{@termtrace-recording-version},}\"\n\
set-option -gF @termtrace-original-default-command \"#{default-command}\"\n\
set-option -gF @termtrace-original-default-shell \"#{default-shell}\"\n\
%endif\n",
    );
    if setup.launch_hook {
        output.push_str("%if \"#{==:#{@termtrace-launch-hook},}\"\nset-option -gF @termtrace-original-pane-launch-command \"#{pane-launch-command}\"\n%endif\n");
    }
    append_config_option(
        &mut output,
        OPTION_LAUNCH_HOOK,
        if setup.launch_hook { b"1" } else { b"0" },
    );
    append_config_option(
        &mut output,
        OPTION_VERSION,
        TMUX_INTEGRATION_VERSION.as_bytes(),
    );
    append_config_option(
        &mut output,
        OPTION_TERMTRACE,
        setup.termtrace.as_os_str().as_bytes(),
    );
    append_config_option(&mut output, OPTION_TMUX, setup.tmux.as_os_str().as_bytes());
    append_config_option(
        &mut output,
        OPTION_OUTPUT_ROOT,
        setup.output_root.as_os_str().as_bytes(),
    );
    append_config_option(
        &mut output,
        OPTION_PROFILE_MODE,
        setup.profile_mode.as_str().as_bytes(),
    );
    append_config_option(
        &mut output,
        OPTION_GHOSTTY_CONFIG,
        setup
            .ghostty_config
            .as_ref()
            .map_or(&[][..], |path| return path.as_os_str().as_bytes()),
    );
    append_config_option(
        &mut output,
        OPTION_COVERAGE,
        setup.coverage.as_str().as_bytes(),
    );
    append_config_option(
        &mut output,
        OPTION_AGGREGATE_MANIFEST,
        setup
            .aggregate_manifest
            .as_ref()
            .map_or(&[][..], |path| return path.as_os_str().as_bytes()),
    );
    append_config_option(
        &mut output,
        OPTION_CELL_WIDTH,
        setup.cell_width.to_string().as_bytes(),
    );
    append_config_option(
        &mut output,
        OPTION_CELL_HEIGHT,
        setup.cell_height.to_string().as_bytes(),
    );
    append_config_option(&mut output, OPTION_TERM, setup.term.as_bytes());
    append_config_option(&mut output, OPTION_SCOPE, &scope_bytes(&setup.scope)?);
    append_config_option(&mut output, launch_option(setup), wrapper.as_bytes());
    for name in GUARDED_HOOKS {
        let _ = writeln!(
            output,
            "set-hook -g {} {}",
            hook_name(name),
            tmux_config_quote(hook.as_bytes()),
        );
    }
    let _ = writeln!(
        output,
        "run-shell -b {}",
        tmux_config_quote(supervisor_shell_command(setup).as_bytes()),
    );
    return Ok(output.into_bytes());
}

pub(super) fn append_config_option(output: &mut String, name: &str, value: &[u8]) {
    let _ = writeln!(output, "set-option -gq {name} {}", tmux_config_quote(value));
}

pub(super) fn apply_tmux_setup(
    client: &TmuxClient,
    setup: &PreparedTmuxSetup,
    original_command: &[u8],
    original_shell: &[u8],
) -> Result<()> {
    if setup.launch_hook {
        let original = client.output(["show-options", "-gv", "pane-launch-command"])?;
        client.set_option(OPTION_ORIGINAL_LAUNCH_HOOK, OsStr::from_bytes(&original))?;
    }
    client.set_option(
        OPTION_LAUNCH_HOOK,
        OsStr::new(if setup.launch_hook { "1" } else { "0" }),
    )?;
    client.set_option(OPTION_ORIGINAL_COMMAND, OsStr::from_bytes(original_command))?;
    client.set_option(OPTION_ORIGINAL_SHELL, OsStr::from_bytes(original_shell))?;
    client.set_option(OPTION_TERMTRACE, setup.termtrace.as_os_str())?;
    client.set_option(OPTION_TMUX, setup.tmux.as_os_str())?;
    client.set_option(OPTION_OUTPUT_ROOT, setup.output_root.as_os_str())?;
    client.set_option(OPTION_PROFILE_MODE, OsStr::new(setup.profile_mode.as_str()))?;
    client.set_option(
        OPTION_GHOSTTY_CONFIG,
        setup
            .ghostty_config
            .as_ref()
            .map_or_else(|| return OsStr::new(""), |path| return path.as_os_str()),
    )?;
    client.set_option(OPTION_COVERAGE, OsStr::new(setup.coverage.as_str()))?;
    client.set_option(
        OPTION_AGGREGATE_MANIFEST,
        setup
            .aggregate_manifest
            .as_ref()
            .map_or_else(|| return OsStr::new(""), |path| return path.as_os_str()),
    )?;
    client.set_option(OPTION_CELL_WIDTH, OsStr::new(&setup.cell_width.to_string()))?;
    client.set_option(
        OPTION_CELL_HEIGHT,
        OsStr::new(&setup.cell_height.to_string()),
    )?;
    client.set_option(OPTION_TERM, OsStr::new(&setup.term))?;
    set_scope_policy(client, &setup.scope)?;
    client.set_option(OPTION_VERSION, OsStr::new(TMUX_INTEGRATION_VERSION))?;
    client.set_option(
        launch_option(setup),
        tmux_wrapper_command(setup).as_os_str(),
    )?;
    let hook = tmux_hook_command();
    for name in GUARDED_HOOKS {
        client.set_hook(name, hook.as_os_str())?;
    }
    return Ok(());
}

pub(super) fn rollback_tmux_setup(client: &TmuxClient, original_command: &[u8]) -> Result<()> {
    let launch_hook = option_text(client, OPTION_LAUNCH_HOOK)? == "1";
    restore_launch_hook(client)?;
    if !launch_hook {
        client.set_option("default-command", OsStr::from_bytes(original_command))?;
    }
    for name in GUARDED_HOOKS {
        let _ = client.unset_hook(name);
    }
    for name in RESERVED_OPTIONS {
        let _ = client.unset_option(name);
    }
    return Ok(());
}

pub(super) fn tmux_wrapper_command(setup: &PreparedTmuxSetup) -> OsString {
    let mut command = b"exec env ".to_vec();
    command.extend_from_slice(TMUX_WRAPPER_BINARY_ENV.as_bytes());
    command.push(b'=');
    command.extend_from_slice(&shell_quote(setup.tmux.as_os_str().as_bytes()));
    command.push(b' ');
    command.extend_from_slice(&shell_quote(setup.termtrace.as_os_str().as_bytes()));
    command.extend_from_slice(b" tmux run --response quiet");
    if setup.launch_hook {
        command.extend_from_slice(b" --pane-launch -- \"$@\"");
    }
    return OsString::from_vec(command);
}

pub(super) fn launch_option(setup: &PreparedTmuxSetup) -> &'static str {
    return if setup.launch_hook {
        "pane-launch-command"
    } else {
        "default-command"
    };
}

fn restore_launch_hook(client: &TmuxClient) -> Result<()> {
    if option_text(client, OPTION_LAUNCH_HOOK)? == "1" {
        let original = client.option(OPTION_ORIGINAL_LAUNCH_HOOK)?;
        client.set_option("pane-launch-command", OsStr::from_bytes(&original))?;
    }
    return Ok(());
}

pub(super) fn tmux_hook_command() -> OsString {
    return OsString::from(
        "run-shell -b \"exec env TERMTRACE_TMUX_BINARY=#{q:@termtrace-tmux-executable} #{q:@termtrace-executable} tmux guard --response quiet --pane '#{pane_id}'\"",
    );
}

pub(super) fn supervisor_shell_command(setup: &PreparedTmuxSetup) -> OsString {
    let mut command = b"exec env ".to_vec();
    command.extend_from_slice(TMUX_WRAPPER_BINARY_ENV.as_bytes());
    command.push(b'=');
    command.extend_from_slice(&shell_quote(setup.tmux.as_os_str().as_bytes()));
    command.push(b' ');
    command.extend_from_slice(&shell_quote(setup.termtrace.as_os_str().as_bytes()));
    command.extend_from_slice(b" tmux watchdog --response quiet");
    return OsString::from_vec(command);
}
