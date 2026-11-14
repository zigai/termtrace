use super::recording::Recording;
use super::{method, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use termtrace_model::value::ExactPath;
use termtrace_sdk::client::{Session, Terminal};
use termtrace_sdk::contracts::LaunchOptions;
use termtrace_sdk::defaults;
use termtrace_sdk::session::SessionTarget;

/// Params of `initialize`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InitializeParams {
    /// Termtrace binary that records sessions; `$TERMTRACE_BIN`, then this server's own
    /// executable, by default.
    #[serde(default)]
    binary: Option<ExactPath>,
    /// Directory that holds new sessions; `$TERMTRACE_ROOT`, then `termtrace` in the temporary
    /// directory, by default.
    #[serde(default)]
    evidence_root: Option<ExactPath>,
}

impl InitializeParams {
    pub(crate) fn terminal(self) -> Terminal {
        let binary = self
            .binary
            .map_or_else(defaults::binary, |path| return path.0);

        let root = self
            .evidence_root
            .map_or_else(evidence_root, |path| return path.0);

        return Terminal::new(binary, root);
    }
}

fn evidence_root() -> PathBuf {
    return env::var_os("TERMTRACE_ROOT")
        .map_or_else(|| return env::temp_dir().join("termtrace"), PathBuf::from);
}

/// Result of `initialize`.
#[derive(Serialize, JsonSchema)]
pub(crate) struct InitializeResult {
    /// Termtrace release version; a client requires its own version to match exactly.
    version: String,
}

impl InitializeResult {
    pub(crate) fn current() -> Self {
        return Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        };
    }
}

/// Params of `termtrace.launch`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LaunchParams {
    /// Program and arguments.
    command: Vec<String>,
    /// Geometry, environment, limits, and visual profile.
    #[serde(default)]
    options: LaunchOptions,
}

/// A recording this client owns.
#[derive(Serialize, JsonSchema)]
struct RecordingInfo {
    /// Handle that addresses the recording in `recording.*` calls.
    recording: String,
    /// Canonical session identity.
    session_id: String,
    /// Session directory.
    path: ExactPath,
}

/// Params of `termtrace.attach`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AttachParams {
    /// Session directory.
    path: ExactPath,
}

/// Params of `termtrace.attach_target`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AttachTargetParams {
    /// Registered name, session id, or path, as accepted by `termtrace screen`.
    target: String,
}

/// An existing session.
#[derive(Serialize, JsonSchema)]
struct SessionInfo {
    /// Canonical session identity.
    session_id: String,
    /// Session directory.
    path: ExactPath,
}

impl SessionInfo {
    fn of(session: &Session) -> Self {
        return Self {
            session_id: session.session_id().to_string(),
            path: ExactPath(session.path().to_path_buf()),
        };
    }
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "termtrace.launch",
        "Launch a command in a new recorded terminal and own its recording.",
        launch,
    );
    method(
        registry,
        "termtrace.attach",
        "Attach to an existing session directory.",
        attach,
    );
    method(
        registry,
        "termtrace.attach_target",
        "Attach to a session by registered name, id, or path.",
        attach_target,
    );
}

fn launch(call: &Call<'_>, params: LaunchParams) -> Result<RecordingInfo, Failure> {
    let terminal = call.terminal()?;
    let command = params.command.into_iter().map(OsString::from).collect();
    let spec = params
        .options
        .to_spec(command)
        .map_err(|error| return Failure::sdk(&error))?;
    let session = terminal
        .start(spec)
        .map_err(|error| return Failure::sdk(&error))?;
    let info = SessionInfo::of(&session);
    let recording = call.server.recordings.insert(Recording::new(session));

    return Ok(RecordingInfo {
        recording,
        session_id: info.session_id,
        path: info.path,
    });
}

fn attach(call: &Call<'_>, params: AttachParams) -> Result<SessionInfo, Failure> {
    return call
        .terminal()?
        .attach(&params.path.0)
        .map(|session| return SessionInfo::of(&session))
        .map_err(|error| return Failure::sdk(&error));
}

fn attach_target(call: &Call<'_>, params: AttachTargetParams) -> Result<SessionInfo, Failure> {
    let target =
        SessionTarget::parse(OsStr::new(&params.target)).map_err(|e| return Failure::sdk(&e))?;

    return call
        .terminal()?
        .attach_target(&target)
        .map(|session| return SessionInfo::of(&session))
        .map_err(|error| return Failure::sdk(&error));
}
