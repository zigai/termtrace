use crate::cli::output::ResponseWriter;

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt as _,
    process::ExitCode,
    str,
};

use miette::{IntoDiagnostic as _, Result};
use termtrace_model::value::ByteString;

use crate::cli::output::write_json_line;

use super::{
    args::TmuxScopeArgs,
    client::{option_text, query_number, valid_tmux_identifier, TmuxClient},
    supervisor::grandfather_current_panes,
    OPTION_EXCLUDED_PID, OPTION_SCOPE, OPTION_SELECTED_PID, OPTION_VERSION,
    TMUX_INTEGRATION_VERSION,
};

const TMUX_SCOPE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TmuxScopeMode {
    All,
    Explicit,
    Selected,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TmuxScopePolicy {
    pub(super) version: u32,
    pub(super) mode: TmuxScopeMode,
    pub(super) session_ids: BTreeSet<String>,
    pub(super) session_names: Vec<ByteString>,
    pub(super) window_ids: BTreeSet<String>,
    pub(super) window_names: Vec<ByteString>,
    pub(super) pane_ids: BTreeSet<String>,
}

impl TmuxScopePolicy {
    pub(super) fn all() -> Self {
        return Self {
            version: TMUX_SCOPE_VERSION,
            mode: TmuxScopeMode::All,
            session_ids: BTreeSet::new(),
            session_names: Vec::new(),
            window_ids: BTreeSet::new(),
            window_names: Vec::new(),
            pane_ids: BTreeSet::new(),
        };
    }

    pub(super) fn explicit() -> Self {
        return Self {
            mode: TmuxScopeMode::Explicit,
            ..Self::all()
        };
    }

    pub(super) fn selected() -> Self {
        return Self {
            mode: TmuxScopeMode::Selected,
            ..Self::all()
        };
    }

    pub(super) fn mode_name(&self) -> &'static str {
        return match self.mode {
            TmuxScopeMode::All => "all",
            TmuxScopeMode::Explicit => "explicit",
            TmuxScopeMode::Selected => "selected",
        };
    }
}

pub(in crate::cli) fn run_tmux_scope(
    response: &mut ResponseWriter,
    arguments: TmuxScopeArgs,
) -> Result<ExitCode> {
    let client = TmuxClient::from_connection(arguments.connection);
    if option_text(&client, OPTION_VERSION)? != TMUX_INTEGRATION_VERSION {
        return Err(miette::miette!(
            "tmux recording is not enabled for this server"
        ));
    }

    let requested = requested_scope_policy(
        &client,
        arguments.all,
        arguments.explicit_only,
        &arguments.sessions,
        &arguments.windows,
        &arguments.panes,
    )?;

    let changed = requested.is_some();
    let policy = match requested {
        Some(policy) => {
            grandfather_current_panes(&client)?;
            set_scope_policy(&client, &policy)?;

            policy
        }
        None => scope_from_options(&client)?,
    };

    return write_json_line(
        response,
        &serde_json::json!({
            "changed": changed,
            "future_processes_only": changed,
            "scope": policy,
        }),
    );
}

pub(super) fn initial_scope_policy(
    explicit_only: bool,
    sessions: &[OsString],
    windows: &[OsString],
    panes: &[OsString],
) -> Result<TmuxScopePolicy> {
    if explicit_only {
        return Ok(TmuxScopePolicy::explicit());
    }

    let has_targets = !sessions.is_empty() || !windows.is_empty() || !panes.is_empty();
    if !has_targets {
        return Ok(TmuxScopePolicy::all());
    }

    let mut policy = TmuxScopePolicy::selected();
    for session in sessions {
        if session.as_bytes().is_empty() {
            return Err(miette::miette!("tmux session names must be nonempty"));
        }

        policy
            .session_names
            .push(ByteString::from(session.as_bytes()));
    }

    for window in windows {
        if window.as_bytes().is_empty() {
            return Err(miette::miette!("tmux window names must be nonempty"));
        }

        policy
            .window_names
            .push(ByteString::from(window.as_bytes()));
    }

    for pane in panes {
        let pane = str::from_utf8(pane.as_bytes()).map_err(|error| {
            return miette::miette!("tmux pane IDs must be UTF-8: {error}");
        })?;
        if !valid_tmux_identifier(pane, b'%') {
            return Err(miette::miette!(
                "tmux pane selector `{pane}` is not a stable pane ID such as %3"
            ));
        }

        policy.pane_ids.insert(pane.to_owned());
    }

    validate_scope_policy(&policy)?;

    return Ok(policy);
}

pub(super) fn requested_scope_policy(
    client: &TmuxClient,
    all: bool,
    explicit_only: bool,
    sessions: &[OsString],
    windows: &[OsString],
    panes: &[OsString],
) -> Result<Option<TmuxScopePolicy>> {
    if all {
        return Ok(Some(TmuxScopePolicy::all()));
    }

    if explicit_only {
        return Ok(Some(TmuxScopePolicy::explicit()));
    }

    let has_targets = !sessions.is_empty() || !windows.is_empty() || !panes.is_empty();
    if !has_targets {
        return Ok(None);
    }

    let mut policy = TmuxScopePolicy::selected();
    for target in sessions {
        policy.session_ids.insert(resolve_target_identifier(
            client,
            target,
            "#{session_id}",
            b'$',
            "session",
        )?);
    }

    for target in windows {
        policy.window_ids.insert(resolve_target_identifier(
            client,
            target,
            "#{window_id}",
            b'@',
            "window",
        )?);
    }

    for target in panes {
        policy.pane_ids.insert(resolve_target_identifier(
            client,
            target,
            "#{pane_id}",
            b'%',
            "pane",
        )?);
    }

    validate_scope_policy(&policy)?;

    return Ok(Some(policy));
}

pub(super) fn resolve_live_scope(
    client: &TmuxClient,
    policy: &TmuxScopePolicy,
) -> Result<TmuxScopePolicy> {
    if policy.mode != TmuxScopeMode::Selected {
        return Ok(policy.clone());
    }

    let mut resolved = TmuxScopePolicy::selected();
    for session in &policy.session_ids {
        resolved.session_ids.insert(resolve_target_identifier(
            client,
            OsStr::new(session),
            "#{session_id}",
            b'$',
            "session",
        )?);
    }

    for session in &policy.session_names {
        let target = OsStr::from_bytes(session.as_bytes());
        resolved.session_ids.insert(resolve_target_identifier(
            client,
            target,
            "#{session_id}",
            b'$',
            "session",
        )?);
    }

    for window in &policy.window_ids {
        resolved.window_ids.insert(resolve_target_identifier(
            client,
            OsStr::new(window),
            "#{window_id}",
            b'@',
            "window",
        )?);
    }

    for window in &policy.window_names {
        let target = OsStr::from_bytes(window.as_bytes());
        resolved.window_ids.insert(resolve_target_identifier(
            client,
            target,
            "#{window_id}",
            b'@',
            "window",
        )?);
    }

    for pane in &policy.pane_ids {
        resolved.pane_ids.insert(resolve_target_identifier(
            client,
            OsStr::new(pane),
            "#{pane_id}",
            b'%',
            "pane",
        )?);
    }

    validate_scope_policy(&resolved)?;

    return Ok(resolved);
}

pub(super) fn resolve_target_identifier(
    client: &TmuxClient,
    target: &OsStr,
    format: &str,
    prefix: u8,
    kind: &str,
) -> Result<String> {
    let bytes = client.target_format(target, format)?;
    let identifier = str::from_utf8(&bytes).map_err(|error| {
        return miette::miette!("tmux returned a non-UTF-8 {kind} ID: {error}");
    })?;

    if !valid_tmux_identifier(identifier, prefix) {
        return Err(miette::miette!(
            "tmux returned invalid {kind} ID `{identifier}`"
        ));
    }

    return Ok(identifier.to_owned());
}

pub(super) fn validate_scope_policy(policy: &TmuxScopePolicy) -> Result<()> {
    if policy.version != TMUX_SCOPE_VERSION {
        return Err(miette::miette!(
            "unsupported tmux recording scope version {}",
            policy.version
        ));
    }

    if policy
        .session_ids
        .iter()
        .any(|identifier| return !valid_tmux_identifier(identifier, b'$'))
        || policy
            .window_ids
            .iter()
            .any(|identifier| return !valid_tmux_identifier(identifier, b'@'))
        || policy
            .pane_ids
            .iter()
            .any(|identifier| return !valid_tmux_identifier(identifier, b'%'))
    {
        return Err(miette::miette!(
            "configured tmux recording scope contains an invalid stable object ID"
        ));
    }

    if policy
        .session_names
        .iter()
        .chain(policy.window_names.iter())
        .any(|name| return name.as_bytes().is_empty())
    {
        return Err(miette::miette!(
            "configured tmux recording scope contains an empty name"
        ));
    }

    let has_targets = !policy.session_ids.is_empty()
        || !policy.session_names.is_empty()
        || !policy.window_ids.is_empty()
        || !policy.window_names.is_empty()
        || !policy.pane_ids.is_empty();

    match policy.mode {
        TmuxScopeMode::Selected if !has_targets => {
            return Err(miette::miette!(
                "selected tmux recording scope has no targets"
            ));
        }
        TmuxScopeMode::All | TmuxScopeMode::Explicit if has_targets => {
            return Err(miette::miette!(
                "tmux recording scope mode {} cannot contain targets",
                policy.mode_name()
            ));
        }
        TmuxScopeMode::All | TmuxScopeMode::Explicit | TmuxScopeMode::Selected => {}
    }

    return Ok(());
}

pub(super) fn scope_bytes(policy: &TmuxScopePolicy) -> Result<Vec<u8>> {
    validate_scope_policy(policy)?;

    return serde_json::to_vec(policy).into_diagnostic();
}

pub(super) fn scope_from_options(client: &TmuxClient) -> Result<TmuxScopePolicy> {
    let bytes = client.option(OPTION_SCOPE)?;
    if bytes.is_empty() {
        return Err(miette::miette!("configured tmux recording scope is empty"));
    }

    let policy = serde_json::from_slice::<TmuxScopePolicy>(&bytes).map_err(|error| {
        return miette::miette!("configured tmux recording scope is invalid: {error}");
    })?;

    validate_scope_policy(&policy)?;

    return Ok(policy);
}

pub(super) fn set_scope_policy(client: &TmuxClient, policy: &TmuxScopePolicy) -> Result<()> {
    let bytes = scope_bytes(policy)?;

    return client.set_option(OPTION_SCOPE, OsStr::from_bytes(&bytes));
}

pub(super) fn pane_matches_scope(
    client: &TmuxClient,
    pane: &str,
    policy: &TmuxScopePolicy,
) -> Result<bool> {
    return match policy.mode {
        TmuxScopeMode::All => Ok(true),
        TmuxScopeMode::Explicit => Ok(false),
        TmuxScopeMode::Selected => {
            let identifiers =
                client.pane_format(pane, "#{pane_id}\t#{session_id}\t#{window_id}")?;
            let mut identifiers = identifiers.splitn(3, |byte| return *byte == b'\t');
            let pane_id = parse_scope_identifier(identifiers.next(), b'%', "pane")?;
            let session_id = parse_scope_identifier(identifiers.next(), b'$', "session")?;
            let window_id = parse_scope_identifier(identifiers.next(), b'@', "window")?;

            if policy.pane_ids.contains(&pane_id)
                || policy.session_ids.contains(&session_id)
                || policy.window_ids.contains(&window_id)
            {
                return Ok(true);
            }

            if !policy.session_names.is_empty() {
                let session_name = client.pane_format(pane, "#{session_name}")?;

                if policy
                    .session_names
                    .iter()
                    .any(|name| return name.as_bytes() == session_name)
                {
                    return Ok(true);
                }
            }

            if !policy.window_names.is_empty() {
                let window_name = client.pane_format(pane, "#{window_name}")?;

                if policy
                    .window_names
                    .iter()
                    .any(|name| return name.as_bytes() == window_name)
                {
                    return Ok(true);
                }
            }

            Ok(false)
        }
    };
}

pub(super) fn parse_scope_identifier(
    value: Option<&[u8]>,
    prefix: u8,
    kind: &str,
) -> Result<String> {
    let identifier = value
        .and_then(|value| return str::from_utf8(value).ok())
        .filter(|value| return valid_tmux_identifier(value, prefix))
        .ok_or_else(|| return miette::miette!("tmux returned an invalid {kind} ID"))?;

    return Ok(identifier.to_owned());
}

pub(super) fn pane_scope_decision(
    client: &TmuxClient,
    pane: &str,
    pid: u32,
    policy: &TmuxScopePolicy,
) -> Result<bool> {
    if pane_option_pid(client, pane, OPTION_SELECTED_PID)? == Some(pid) {
        return Ok(true);
    }

    if pane_option_pid(client, pane, OPTION_EXCLUDED_PID)? == Some(pid) {
        return Ok(false);
    }

    return pane_matches_scope(client, pane, policy);
}

pub(super) fn pane_option_pid(client: &TmuxClient, pane: &str, name: &str) -> Result<Option<u32>> {
    let bytes = client.pane_option(pane, name)?;

    return Ok(str::from_utf8(&bytes)
        .ok()
        .and_then(|value| return value.parse::<u32>().ok()));
}

pub(super) fn mark_pane_scope_decision(
    client: &TmuxClient,
    pane: &str,
    selected: bool,
) -> Result<()> {
    let pid = query_number::<u32>(client, pane, "#{pane_pid}")?;
    let (set, unset) = if selected {
        (OPTION_SELECTED_PID, OPTION_EXCLUDED_PID)
    } else {
        (OPTION_EXCLUDED_PID, OPTION_SELECTED_PID)
    };

    client.set_pane_option(pane, set, OsStr::new(&pid.to_string()))?;
    client.unset_pane_option(pane, unset)?;

    return Ok(());
}
