//! Canonical-session effects the tmux library asks of the CLI.
//!
//! Every operation here is a session-layer operation (registry, detached record, stop,
//! completion, video, bundle); point them at service operations when those exist.

use super::args::{TmuxBundleArgs, TmuxVideoArgs};
use crate::cli::{
    artifacts::{
        args::{VideoCursorStyleArg, VideoFacadeArgs},
        run_video_facade,
    },
    control::{args::StopArgs, run_stop},
    output::ResponseWriter,
};
use miette::{IntoDiagnostic as _, Result};
use std::{
    env,
    ffi::OsString,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    time::Duration,
};
use termtrace_engine::{inspection::wait_for_completion, registry::SessionRegistry};
use termtrace_export::tmux::streaming;
use termtrace_model::value::ByteString;
use termtrace_tmux::{
    artifact::{TmuxSessionLink, TmuxStreamingBundleManifest},
    recording::{composite_session, Tracks},
    store::RecordingStore,
    Error as TmuxError, Result as TmuxResult,
};

/// Session effects backed by the managed registry and CLI commands.
pub(super) struct CliTracks {
    registry: SessionRegistry,
}

impl CliTracks {
    pub(super) fn open(output_root: Option<&Path>) -> Result<Self> {
        let registry = output_root
            .map_or_else(SessionRegistry::open_default, SessionRegistry::open)
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
        let executable = env::current_exe()
            .map_err(|error| return failed("composite tmux client track start", error))?;
        let output = Command::new(executable)
            .args(["record", "-d"])
            .arg(alias)
            .arg("--")
            .args(command)
            .env("TERMTRACE_STATE_DIR", self.registry.state_root())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|error| return failed("composite tmux client track start", error))?;

        if !output.status.success() {
            return Err(failed(
                "composite tmux client track start",
                format!("exited with {}", output.status),
            ));
        }

        let entry = self
            .registry
            .resolve(alias)
            .map_err(|error| return failed("composite tmux client track start", error))?;

        return Ok(TmuxSessionLink {
            session_id: entry.session_id,
            session: ByteString::from(entry.session_path().as_os_str().as_bytes()),
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
        let arguments = StopArgs {
            session: session.to_path_buf(),
            wait,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            expected_presented_state_hash: None,
            request_id: None,
            json: false,
        };

        let exit_code = run_stop(&mut ResponseWriter::quiet(), arguments)
            .map_err(|error| return failed("composite tmux client track stop", error))?;
        if exit_code != ExitCode::SUCCESS {
            return Err(failed(
                "composite tmux client track stop",
                "stop exited with failure",
            ));
        }
        return Ok(());
    }

    fn wait_completed(&self, session: &Path, timeout: Duration) -> TmuxResult<()> {
        return wait_for_completion(session, timeout)
            .map(drop)
            .map_err(|error| return failed("tmux track completion", error));
    }
}

/// Render an aggregate recording's composite client track.
pub(super) fn render_video(
    response: &mut ResponseWriter,
    tracks: &CliTracks,
    arguments: TmuxVideoArgs,
) -> Result<ExitCode> {
    let session = composite_session(tracks.state_root(), &arguments.name).into_diagnostic()?;

    return run_video_facade(
        response,
        VideoFacadeArgs {
            session,
            out: Some(arguments.out),
            dry_run: false,
            preview_out: None,
            edit: None,
            preset: arguments.preset,
            fps: arguments.fps,
            renderer: arguments.renderer,
            semantic_raster_profile: None,
            resolution: None,
            scale: None,
            start_marker: None,
            end_marker: None,
            trim_idle: false,
            hold_start: None,
            hold_end: None,
            cursor_style: VideoCursorStyleArg::Recorded,
            columns: 4,
            tile_width: 320,
            max_frames: 12,
            change_only: false,
            preview_width: 640,
            json: false,
        },
    );
}

/// Export a completed aggregate recording as a streaming evidence bundle.
pub(super) fn bundle(
    tracks: &CliTracks,
    arguments: &TmuxBundleArgs,
) -> Result<TmuxStreamingBundleManifest> {
    let store = RecordingStore::locate(tracks.state_root(), &arguments.name).into_diagnostic()?;

    return streaming::create(store.manifest_path(), &arguments.out).into_diagnostic();
}
