//! Canonical-session effects the tmux library asks of the CLI.
//!
//! Track start, stop, completion, video, and bundle requests go through the service operations;
//! the adapter never calls CLI command functions.

use super::args::{TmuxBundleArgs, TmuxVideoArgs};
use crate::cli::{
    recording::{args::RecordingArgs, detached_record_arguments},
    session_helpers::attach_session,
};
use miette::{IntoDiagnostic as _, Result};
use std::{
    env,
    ffi::OsString,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_model::value::ByteString;
use termtrace_sdk::{defaults, session::StopRequest};
use termtrace_service::{
    artifacts::{VideoMode, VideoRequest},
    registry::{DetachedStart, RegistryService},
    tmux::TmuxService,
};
use termtrace_tmux::{
    artifact::{TmuxSessionLink, TmuxStreamingBundleManifest},
    recording::{composite_session, Tracks},
    store::RecordingStore,
    Error as TmuxError, Result as TmuxResult,
};

/// Session effects backed by the managed registry and the service session operations.
pub(super) struct CliTracks {
    registry: RegistryService,
}

impl CliTracks {
    pub(super) fn open(output_root: Option<&Path>) -> Result<Self> {
        let registry = output_root
            .map_or_else(RegistryService::open_default, RegistryService::open)
            .into_diagnostic()?;

        return Ok(Self { registry });
    }
}

fn failed(operation: &'static str, error: impl ToString) -> TmuxError {
    return TmuxError::Track {
        operation,
        message: error.to_string(),
    };
}

impl Tracks for CliTracks {
    fn state_root(&self) -> &Path {
        return self.registry.state_root();
    }

    fn reserve(&self, alias: &str) -> TmuxResult<()> {
        return self
            .registry
            .ensure_available(alias)
            .map_err(|error| return failed("composite alias reservation", error));
    }

    fn start(&self, alias: &str, command: &[OsString]) -> TmuxResult<TmuxSessionLink> {
        let operation = "composite tmux client track start";
        let recording = RecordingArgs::defaults_for(command)
            .map_err(|error| return failed(operation, error))?;
        let executable = env::current_exe().map_err(|error| return failed(operation, error))?;
        let started = self
            .registry
            .start_detached(
                &executable,
                DetachedStart {
                    alias: alias.to_owned(),
                    session: None,
                    startup_timeout: defaults::STARTUP_TIMEOUT,
                    stdin: None,
                },
                |session| return detached_record_arguments(alias, session, &recording),
            )
            .map_err(|error| return failed(operation, error))?;

        return Ok(TmuxSessionLink {
            session_id: started.entry.session_id,
            session: ByteString::from(started.entry.session_path().as_os_str().as_bytes()),
        });
    }

    fn resolve(&self, alias: &str) -> TmuxResult<PathBuf> {
        return self
            .registry
            .resolve(alias)
            .map(|entry| return entry.session_path())
            .map_err(|error| return failed("composite alias resolution", error));
    }

    fn stop(&self, session: &Path, wait: bool, timeout: Duration) -> TmuxResult<()> {
        return attach_session(session)
            .and_then(|mut session| {
                return session
                    .stop(StopRequest::default(), wait.then_some(timeout))
                    .into_diagnostic();
            })
            .map(drop)
            .map_err(|error| return failed("composite tmux client track stop", error));
    }

    fn wait_completed(&self, session: &Path, timeout: Duration) -> TmuxResult<()> {
        return attach_session(session)
            .and_then(|mut session| return session.complete(timeout).into_diagnostic())
            .map(drop)
            .map_err(|error| return failed("tmux track completion", error));
    }
}

/// Video request for an aggregate recording's composite client track.
pub(super) fn video_request(tracks: &CliTracks, arguments: TmuxVideoArgs) -> Result<VideoRequest> {
    let session = composite_session(tracks.state_root(), &arguments.name).into_diagnostic()?;

    return Ok(VideoRequest {
        preset: arguments.preset.into(),
        fps: arguments.fps,
        renderer: arguments.renderer.into(),
        ..VideoRequest::new(session, VideoMode::Render { out: arguments.out })
    });
}

/// Export a completed aggregate recording as a streaming evidence bundle.
pub(super) fn bundle(
    tracks: &CliTracks,
    arguments: &TmuxBundleArgs,
) -> Result<TmuxStreamingBundleManifest> {
    let store = RecordingStore::locate(tracks.state_root(), &arguments.name).into_diagnostic()?;

    return TmuxService::bundle(&store.manifest_path(), &arguments.out).into_diagnostic();
}
