//! Shared canonical-track effects and evidence exports for aggregate tmux recordings.
//!
//! The existing `termtrace-tmux` recorder retains state-machine, scope, coverage, and rollback authority.

use crate::error::{Error, Result};
use std::os::unix::ffi::OsStrExt as _;
use termtrace_model::{
    protocol::management::{FailureContext, ManagementFailure, TmuxFailureFacts},
    value::ByteString,
};

fn management(error: impl Into<Error>, action: &str, manifest: Option<&Path>) -> Error {
    let error = error.into();
    let mut facts = match error.failure_context() {
        FailureContext::Management { management, .. } => *management,
        FailureContext::Actor { .. } => ManagementFailure::Tmux(TmuxFailureFacts::inspect(action)),
    };

    if let ManagementFailure::Tmux(facts) = &mut facts {
        facts.action = action.into();

        if facts.manifest.is_none() {
            facts.manifest = manifest.map(|p| return ByteString::from(p.as_os_str().as_bytes()));
        }
    }

    return error.with_management(facts);
}

use std::path::Path;
use termtrace_export::tmux::streaming::{
    create as create_tmux_streaming_bundle, verify as verify_tmux_streaming_bundle,
};
use termtrace_model::execution::tmux::TmuxRecordConfiguration;
use termtrace_tmux::artifact::TmuxCatalogSummary as CoreTmuxTmuxCatalogSummary;
use termtrace_tmux::artifact::TmuxRecordingManifest as CoreTmuxTmuxRecordingManifest;
pub use termtrace_tmux::artifact::{
    TmuxCatalogSummary, TmuxGapCatalogEntry, TmuxRecordingManifest, TmuxStreamingBundleManifest,
    TmuxTrackCatalogEntry,
};
use termtrace_tmux::recording::catalog_page as coretmux_catalog_page;
use termtrace_tmux::recording::events as coretmux_events;
use termtrace_tmux::recording::record_with_executable as coretmux_record;
use termtrace_tmux::recording::register_pane_track as coretmux_register_pane_track;
use termtrace_tmux::recording::status as coretmux_status;
use termtrace_tmux::recording::stop as coretmux_stop;
use termtrace_tmux::status::observe as coretmux_observe;
pub use termtrace_tmux::status::TmuxStatusReport;
use termtrace_tmux::status::TmuxStatusReport as CoreTmuxTmuxStatusReport;
pub use termtrace_tmux::store::EventPage as TmuxEventPage;
use termtrace_tmux::store::EventPage as CoreTmuxEventPage;
use termtrace_tmux::store::RecordingStore as CoreTmuxRecordingStore;

/// Streaming evidence bundles of completed aggregate tmux recordings.
pub struct TmuxService;

impl TmuxService {
    /// Bundle a completed aggregate tmux recording into a streaming evidence bundle.
    ///
    /// # Errors
    /// Returns an error if bundling fails or evidence is incomplete.
    pub fn bundle(
        aggregate_manifest: &Path,
        destination: &Path,
    ) -> Result<TmuxStreamingBundleManifest> {
        return create_tmux_streaming_bundle(aggregate_manifest, destination)
            .map_err(|e| return management(e, "bundle", Some(aggregate_manifest)));
    }

    /// Verify an aggregate tmux streaming bundle against its manifest.
    ///
    /// # Errors
    /// Returns an error if verification fails or members do not match.
    pub fn verify_bundle(
        directory: &Path,
        aggregate_manifest: &Path,
    ) -> Result<TmuxStreamingBundleManifest> {
        return verify_tmux_streaming_bundle(directory, aggregate_manifest)
            .map_err(|e| return management(e, "verify_bundle", Some(aggregate_manifest)));
    }
}

mod tracks;

pub use termtrace_tmux::artifact::TmuxRecordingStart;
pub use termtrace_tmux::{
    artifact::TmuxSocket,
    client::TmuxClient,
    recording::{CatalogPage, RecordRequest, StopRequest, Tracks},
};
pub use tracks::ServiceTracks;

impl TmuxService {
    /// Observe the underlying tmux server, including coverage gaps and live observers.
    /// # Errors
    /// Propagates tmux connection, option and identity failures.
    pub fn status(client: &TmuxClient) -> Result<CoreTmuxTmuxStatusReport> {
        return coretmux_observe(client).map_err(|e| return management(e, "status", None));
    }

    /// Read and refresh an aggregate's lifecycle.
    /// # Errors
    /// Propagates manifest, catalog and server-identity failures.
    pub fn recording_status(root: &Path, name: &str) -> Result<CoreTmuxTmuxRecordingManifest> {
        return coretmux_status(root, name)
            .map_err(|e| return management(e, "recording_status", None));
    }

    /// Start an aggregate using reusable canonical-session effects.
    /// # Errors
    /// Propagates setup/track failures with the underlying rollback policy.
    pub fn record(
        tracks: &ServiceTracks,
        request: RecordRequest,
    ) -> Result<(CoreTmuxRecordingStore, CoreTmuxTmuxRecordingManifest)> {
        return coretmux_record(
            tracks,
            request,
            &tracks
                .recorder_executable()
                .map_err(|e| return management(e, "record", None))?,
        )
        .map_err(|e| return management(e, "record", None));
    }

    /// Register one pane incarnation through the existing aggregate service.
    /// # Errors
    /// Propagates manifest/source identity and track errors.
    pub fn track(manifest: &Path, pane: &str, session: &Path) -> Result<()> {
        return coretmux_register_pane_track(manifest, pane, session)
            .map_err(|e| return management(e, "track", Some(manifest)));
    }

    /// Stop tracks; already closed aggregates retain their repeated-stop behavior.
    /// # Errors
    /// Propagates identity/track/completion failures; prior effects are not rolled back.
    pub fn stop(
        tracks: &ServiceTracks,
        request: StopRequest<'_>,
    ) -> Result<CoreTmuxTmuxRecordingManifest> {
        return coretmux_stop(tracks, request).map_err(|e| return management(e, "stop", None));
    }

    /// Read bounded aggregate lifecycle events.
    /// # Errors
    /// Propagates malformed/changed manifest or journal failures.
    pub fn events(
        root: &Path,
        name: &str,
        from: u64,
        limit: u16,
    ) -> Result<(CoreTmuxTmuxRecordingManifest, CoreTmuxEventPage)> {
        return coretmux_events(root, name, from, limit)
            .map_err(|e| return management(e, "events", None));
    }

    /// Read typed track or coverage-gap catalog pages.
    /// # Errors
    /// Propagates malformed/changed catalog and manifest failures.
    pub fn catalog(
        root: &Path,
        name: &str,
        gaps: bool,
        from: u64,
        limit: u16,
        active: bool,
    ) -> Result<(
        CoreTmuxTmuxRecordingManifest,
        CoreTmuxTmuxCatalogSummary,
        CatalogPage,
    )> {
        return coretmux_catalog_page(root, name, gaps, from, limit, active)
            .map_err(|e| return management(e, "catalog", None));
    }
}

impl TmuxService {
    /// Convert typed SDK settings and delegate to the existing aggregate recorder.
    /// No SDK-specific track engine, rollback, retention, or terminal policy is introduced.
    /// # Errors
    /// Propagates the underlying recorder's validation, setup and track errors.
    pub fn record_configured(
        tracks: &ServiceTracks,
        client: TmuxClient,
        options: TmuxRecordConfiguration,
    ) -> Result<TmuxRecordingStart> {
        use std::{
            ffi::OsString,
            os::unix::ffi::{OsStrExt as _, OsStringExt as _},
        };
        use termtrace_model::{
            execution::tmux::{TmuxCoverage, TmuxGhosttyProfile},
            value::ByteString,
        };
        use termtrace_tmux::{
            artifact::TmuxRecordingCoverage,
            scope::ScopeTargets,
            setup::{GhosttyProfileMode, PaneRetention, SetupRequest},
        };
        let os = |bytes: ByteString| return OsString::from_vec(bytes.into_bytes());
        let request = RecordRequest {
            name: options.name,
            new_server: options.new_server,
            client,
            command: options.command.into_iter().map(os).collect(),
            setup: SetupRequest {
                launch_hook: options.launch_hook,
                profile_mode: match options.ghostty_profile {
                    TmuxGhosttyProfile::Auto => GhosttyProfileMode::Auto,
                    TmuxGhosttyProfile::Required => GhosttyProfileMode::Required,
                    TmuxGhosttyProfile::Off => GhosttyProfileMode::Off,
                },
                ghostty_config: options.ghostty_config.map(|value| return os(value).into()),
                coverage: match options.coverage {
                    TmuxCoverage::Warn => TmuxRecordingCoverage::Warn,
                    TmuxCoverage::Strict => TmuxRecordingCoverage::Strict,
                },
                aggregate_manifest: None,
                explicit_only: options.explicit_only,
                targets: ScopeTargets {
                    sessions: options.sessions.into_iter().map(os).collect(),
                    windows: options.windows.into_iter().map(os).collect(),
                    panes: options.panes.into_iter().map(os).collect(),
                },
                cell_width: options.cell_width,
                cell_height: options.cell_height,
                term: options.term,
                retention: PaneRetention {
                    older_than_days: options.retain_days,
                    max_bytes: options.retain_bytes,
                },
            },
        };
        let (store, recording) = Self::record(tracks, request)?;
        let manifest = store.manifest_path();

        return Ok(TmuxRecordingStart {
            manifest: ByteString::from(manifest.as_os_str().as_bytes()),
            manifest_display: manifest.display().to_string(),
            recording,
        });
    }
}
