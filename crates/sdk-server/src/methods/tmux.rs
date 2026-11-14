use super::{method, millis, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use termtrace_model::execution::tmux::TmuxRecordConfiguration;
use termtrace_model::value::ExactPath;
use termtrace_sdk::contracts::diagnostics::{self as core, TmuxCatalog, TmuxEvents};
use termtrace_sdk::defaults;
use termtrace_sdk::tmux::{
    ServiceTracks, TmuxClient, TmuxRecordingManifest, TmuxRecordingStart, TmuxService, TmuxSocket,
    TmuxStatusReport, TmuxStreamingBundleManifest, Tracks as _,
};

/// The tmux server and aggregate recording root a call uses.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TmuxTarget {
    /// State root of aggregate recordings; the default registry root when absent.
    #[serde(default)]
    root: Option<ExactPath>,
    /// tmux executable; `tmux` on `PATH` when absent.
    #[serde(default)]
    tmux: Option<ExactPath>,
    /// How tmux reaches its server; the default socket when absent.
    #[serde(default)]
    socket: TmuxSocket,
}

impl TmuxTarget {
    fn client(&self) -> TmuxClient {
        return TmuxClient {
            executable: self.tmux.as_ref().map_or_else(
                || return PathBuf::from("tmux"),
                |tmux| return tmux.0.clone(),
            ),
            socket: self.socket.clone(),
        };
    }

    fn tracks(&self, call: &Call<'_>) -> Result<ServiceTracks, Failure> {
        let binary = call.terminal()?.binary().to_path_buf();

        return sdk(ServiceTracks::open(
            binary,
            self.root.as_ref().map(|root| return root.0.as_path()),
        ));
    }
}

/// Params of `tmux.status`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TmuxStatusParams {
    #[serde(flatten)]
    target: TmuxTarget,
}

/// Params of `tmux.record`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TmuxRecordParams {
    #[serde(flatten)]
    target: TmuxTarget,
    /// Recording name, coverage, and capture settings.
    options: TmuxRecordConfiguration,
}

/// Params of calls that address one aggregate recording by name.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TmuxNameParams {
    #[serde(flatten)]
    target: TmuxTarget,
    /// Aggregate recording name.
    name: String,
}

/// Params of `tmux.track`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TmuxTrackParams {
    /// Aggregate recording manifest.
    manifest: ExactPath,
    /// tmux pane identity, such as `%3`.
    pane: String,
    /// Session directory that records the pane.
    session: ExactPath,
}

/// Params of `tmux.stop`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TmuxStopParams {
    #[serde(flatten)]
    target: TmuxTarget,
    /// Aggregate recording name.
    name: String,
    /// Whether to wait for every track to complete; true when absent.
    #[serde(default)]
    wait: Option<bool>,
    /// Bound on waiting; the default wait timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Params of `tmux.events`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TmuxEventsParams {
    #[serde(flatten)]
    target: TmuxTarget,
    /// Aggregate recording name.
    name: String,
    /// First sequence to return; 1 when absent.
    #[serde(default)]
    from_sequence: Option<u64>,
    /// Maximum entries to return; 100 when absent.
    #[serde(default)]
    limit: Option<u16>,
}

/// Params of `tmux.catalog`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TmuxCatalogParams {
    #[serde(flatten)]
    target: TmuxTarget,
    /// Aggregate recording name.
    name: String,
    /// Whether to list coverage gaps instead of tracks.
    #[serde(default)]
    gaps: bool,
    /// First sequence to return; 1 when absent.
    #[serde(default)]
    from_sequence: Option<u64>,
    /// Maximum entries to return; 100 when absent.
    #[serde(default)]
    limit: Option<u16>,
    /// Whether to list only active tracks.
    #[serde(default)]
    active_only: bool,
}

/// Params of `tmux.bundle` and `tmux.verify_bundle`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TmuxBundleParams {
    /// Aggregate recording manifest.
    manifest: ExactPath,
    /// Bundle directory.
    bundle: ExactPath,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "tmux.status",
        "Report tmux server and integration status.",
        status,
    );
    method(
        registry,
        "tmux.record",
        "Start an aggregate tmux recording.",
        record,
    );
    method(
        registry,
        "tmux.recording_status",
        "Report the lifecycle of one aggregate recording.",
        recording_status,
    );
    method(
        registry,
        "tmux.track",
        "Register a pane session as a track of an aggregate recording.",
        track,
    );
    method(
        registry,
        "tmux.stop",
        "Stop the tracks of an aggregate recording.",
        stop,
    );
    method(
        registry,
        "tmux.events",
        "Read one page of aggregate history.",
        events,
    );
    method(
        registry,
        "tmux.catalog",
        "Read one page of tracks or coverage gaps.",
        catalog,
    );
    method(
        registry,
        "tmux.bundle",
        "Bundle an aggregate recording.",
        |_call, params: TmuxBundleParams| return bundle(&params, false),
    );
    method(
        registry,
        "tmux.verify_bundle",
        "Verify an aggregate bundle against its manifest.",
        |_call, params: TmuxBundleParams| return bundle(&params, true),
    );
}

fn sdk<T>(result: termtrace_sdk::Result<T>) -> Result<T, Failure> {
    return result.map_err(|error| return Failure::sdk(&error));
}

fn status(_call: &Call<'_>, params: TmuxStatusParams) -> Result<TmuxStatusReport, Failure> {
    return sdk(core::tmux_status(&params.target.client()));
}

fn record(call: &Call<'_>, params: TmuxRecordParams) -> Result<TmuxRecordingStart, Failure> {
    let tracks = params.target.tracks(call)?;

    return sdk(core::tmux_record(
        &tracks,
        params.target.client(),
        params.options,
    ));
}

fn recording_status(
    call: &Call<'_>,
    params: TmuxNameParams,
) -> Result<TmuxRecordingManifest, Failure> {
    let tracks = params.target.tracks(call)?;

    return sdk(core::tmux_recording(tracks.state_root(), &params.name));
}

fn track(_call: &Call<'_>, params: TmuxTrackParams) -> Result<(), Failure> {
    return sdk(TmuxService::track(
        &params.manifest.0,
        &params.pane,
        &params.session.0,
    ));
}

fn stop(call: &Call<'_>, params: TmuxStopParams) -> Result<TmuxRecordingManifest, Failure> {
    let tracks = params.target.tracks(call)?;

    return sdk(core::tmux_stop(
        &tracks,
        &params.name,
        params.wait.unwrap_or(true),
        millis(params.timeout_ms, defaults::WAIT_TIMEOUT),
    ));
}

fn events(call: &Call<'_>, params: TmuxEventsParams) -> Result<TmuxEvents, Failure> {
    let tracks = params.target.tracks(call)?;

    return sdk(core::tmux_events(
        tracks.state_root(),
        &params.name,
        params.from_sequence.unwrap_or(1),
        params.limit.unwrap_or(100),
    ));
}

fn catalog(call: &Call<'_>, params: TmuxCatalogParams) -> Result<TmuxCatalog, Failure> {
    let tracks = params.target.tracks(call)?;

    return sdk(core::tmux_catalog(
        tracks.state_root(),
        &params.name,
        params.gaps,
        params.from_sequence.unwrap_or(1),
        params.limit.unwrap_or(100),
        params.active_only,
    ));
}

fn bundle(params: &TmuxBundleParams, verify: bool) -> Result<TmuxStreamingBundleManifest, Failure> {
    return sdk(core::tmux_bundle(
        &params.manifest.0,
        &params.bundle.0,
        verify,
    ));
}
