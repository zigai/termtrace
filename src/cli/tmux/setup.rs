use super::*;

#[derive(Clone, Debug)]
pub(super) struct PreparedTmuxSetup {
    termtrace: PathBuf,
    tmux: PathBuf,
    output_root: PathBuf,
    profile_mode: TmuxGhosttyProfileMode,
    ghostty_config: Option<PathBuf>,
    allow_unrecorded: bool,
    cell_width: u16,
    cell_height: u16,
    term: String,
    scope: TmuxScopePolicy,
}

pub(in crate::cli) fn run_tmux_config(arguments: TmuxConfigArgs) -> Result<ExitCode> {
    let setup = prepare_tmux_setup(arguments.setup, arguments.tmux)?;
    return write_stdout(tmux_config_fragment(&setup)?);
}

pub(in crate::cli) fn run_tmux_enable(arguments: TmuxEnableArgs) -> Result<ExitCode> {
    let client = TmuxClient::from_connection(arguments.connection.clone());
    let existing_version = option_text(&client, OPTION_VERSION)?;
    if !existing_version.is_empty() {
        if existing_version == TMUX_INTEGRATION_VERSION {
            return write_json_line(&serde_json::json!({
                "enabled": true,
                "already_enabled": true,
                "integration_version": 1,
            }));
        }
        return Err(miette::miette!(
            "tmux recording is already configured with unsupported integration version {existing_version}"
        ));
    }
    let mut setup = prepare_tmux_setup(arguments.setup, arguments.connection.tmux)?;
    setup.scope = resolve_live_scope(&client, &setup.scope)?;

    let original_command = client.option("default-command")?;
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

    let apply = apply_tmux_setup(&client, &setup, &original_command, &original_shell);
    if let Err(error) = apply {
        let _rollback = rollback_tmux_setup(&client, &original_command);
        return Err(error);
    }
    if let Err(error) = grandfather_current_panes(&client).and_then(|()| {
        if setup.allow_unrecorded {
            return Ok(());
        }
        return client.run_shell_background(supervisor_shell_command(&setup).as_os_str());
    }) {
        let _cleanup = clear_managed_pane_options(&client);
        let _rollback = rollback_tmux_setup(&client, &original_command);
        return Err(error);
    }
    let report = tmux_status_report(&client)?;
    return write_json_line(&serde_json::json!({
        "enabled": true,
        "already_enabled": false,
        "integration_version": 1,
        "strict": !setup.allow_unrecorded,
        "output_root": ByteString::from(setup.output_root.as_os_str().as_bytes()),
        "scope": setup.scope,
        "existing_panes": report.pane_count,
        "existing_unrecorded_panes": report.unrecorded_panes,
        "retroactive": false,
    }));
}

pub(in crate::cli) fn run_tmux_disable(connection: TmuxConnectionArgs) -> Result<ExitCode> {
    let client = TmuxClient::from_connection(connection);
    let version = option_text(&client, OPTION_VERSION)?;
    if version.is_empty() {
        return write_json_line(&serde_json::json!({
            "enabled": false,
            "already_disabled": true,
        }));
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
    let current_command = client.option("default-command")?;
    if current_command != expected_wrapper.as_bytes() {
        return Err(miette::miette!(
            "tmux default-command changed after Termtrace was enabled; refusing to overwrite it"
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
    client.set_option("default-command", OsStr::from_bytes(&original))?;
    for name in GUARDED_HOOKS {
        client.unset_hook(name)?;
    }
    for name in RESERVED_OPTIONS {
        client.unset_option(name)?;
    }
    clear_managed_pane_options(&client)?;
    return write_json_line(&serde_json::json!({
        "enabled": false,
        "already_disabled": false,
        "default_command_restored": true,
    }));
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
        Some(root) => SessionRegistry::open(root)?,
        None => SessionRegistry::open_default()?,
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
        termtrace,
        tmux,
        output_root,
        profile_mode: setup.ghostty_profile,
        ghostty_config,
        allow_unrecorded: setup.allow_unrecorded,
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
    let profile_mode = match option_text(client, OPTION_PROFILE_MODE)?.as_str() {
        "auto" => TmuxGhosttyProfileMode::Auto,
        "required" => TmuxGhosttyProfileMode::Required,
        "off" => TmuxGhosttyProfileMode::Off,
        other => {
            return Err(miette::miette!(
                "unsupported Ghostty profile mode `{other}`"
            ));
        }
    };
    return Ok(PreparedTmuxSetup {
        termtrace: PathBuf::from(OsString::from_vec(termtrace)),
        tmux: PathBuf::from(OsString::from_vec(tmux)),
        output_root: option_path(client, OPTION_OUTPUT_ROOT)?
            .ok_or_else(|| return miette::miette!("configured tmux output root is empty"))?,
        profile_mode,
        ghostty_config: option_path(client, OPTION_GHOSTTY_CONFIG)?,
        allow_unrecorded: option_text(client, OPTION_ALLOW_UNRECORDED)? == "1",
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
        OPTION_ALLOW_UNRECORDED,
        if setup.allow_unrecorded { b"1" } else { b"0" },
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
    append_config_option(&mut output, "default-command", wrapper.as_bytes());
    for name in GUARDED_HOOKS {
        let _ = writeln!(
            output,
            "set-hook -g {} {}",
            hook_name(name),
            tmux_config_quote(hook.as_bytes()),
        );
    }
    if !setup.allow_unrecorded {
        let _ = writeln!(
            output,
            "run-shell -b {}",
            tmux_config_quote(supervisor_shell_command(setup).as_bytes()),
        );
    }
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
    client.set_option(
        OPTION_ALLOW_UNRECORDED,
        OsStr::new(if setup.allow_unrecorded { "1" } else { "0" }),
    )?;
    client.set_option(OPTION_CELL_WIDTH, OsStr::new(&setup.cell_width.to_string()))?;
    client.set_option(
        OPTION_CELL_HEIGHT,
        OsStr::new(&setup.cell_height.to_string()),
    )?;
    client.set_option(OPTION_TERM, OsStr::new(&setup.term))?;
    set_scope_policy(client, &setup.scope)?;
    client.set_option(OPTION_VERSION, OsStr::new(TMUX_INTEGRATION_VERSION))?;
    client.set_option("default-command", tmux_wrapper_command(setup).as_os_str())?;
    let hook = tmux_hook_command();
    for name in GUARDED_HOOKS {
        client.set_hook(name, hook.as_os_str())?;
    }
    return Ok(());
}

pub(super) fn rollback_tmux_setup(client: &TmuxClient, original_command: &[u8]) -> Result<()> {
    client.set_option("default-command", OsStr::from_bytes(original_command))?;
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
    command.extend_from_slice(b" tmux run");
    return OsString::from_vec(command);
}

pub(super) fn tmux_hook_command() -> OsString {
    return OsString::from(
        "run-shell -b \"exec env TERMTRACE_TMUX_BINARY=#{q:@termtrace-tmux-executable} #{q:@termtrace-executable} tmux guard --pane '#{pane_id}'\"",
    );
}

pub(super) fn supervisor_shell_command(setup: &PreparedTmuxSetup) -> OsString {
    let mut command = b"exec env ".to_vec();
    command.extend_from_slice(TMUX_WRAPPER_BINARY_ENV.as_bytes());
    command.push(b'=');
    command.extend_from_slice(&shell_quote(setup.tmux.as_os_str().as_bytes()));
    command.push(b' ');
    command.extend_from_slice(&shell_quote(setup.termtrace.as_os_str().as_bytes()));
    command.extend_from_slice(b" tmux supervise");
    return OsString::from_vec(command);
}
