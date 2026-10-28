//! Canonical-session effects the tmux library asks of the CLI.
//!
//! Track start, stop, completion, video, and bundle requests go through the service operations;
//! the adapter never calls CLI command functions.

use super::args::{TmuxBundleArgs, TmuxVideoArgs};
use miette::{IntoDiagnostic as _, Result};
use std::{env, path::Path};
use termtrace_service::{
    artifacts::{VideoMode, VideoRequest},
    tmux::ServiceTracks,
    tmux::TmuxService,
};
use termtrace_tmux::recording::Tracks as _;
use termtrace_tmux::{
    artifact::TmuxStreamingBundleManifest, recording::composite_session, store::RecordingStore,
};

pub(super) type CliTracks = ServiceTracks;

pub(super) fn open(output_root: Option<&Path>) -> Result<ServiceTracks> {
    return ServiceTracks::open(env::current_exe().into_diagnostic()?, output_root)
        .into_diagnostic();
}

/// Video request for an aggregate recording's composite client track.
pub(super) fn video_request(tracks: &CliTracks, arguments: TmuxVideoArgs) -> Result<VideoRequest> {
    let session = composite_session(tracks.state_root(), &arguments.name).into_diagnostic()?;

    return Ok(VideoRequest {
        preset: arguments.preset.into(),
        fps: arguments.fps,
        renderer: arguments.renderer.into(),
        overwrite: true,
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
