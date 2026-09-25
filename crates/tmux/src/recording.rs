//! Aggregate recording operations: record, observe, stop, and history pages.
//!
//! Track-level effects (starting, stopping, and awaiting canonical sessions) are supplied
//! by the caller through [`Tracks`]; everything else is owned here.

use crate::{
    artifact::{
        TmuxCatalogSummary, TmuxCompositeTrack, TmuxEvent, TmuxGapCatalogEntry,
        TmuxJournalBoundary, TmuxRecordingLifecycle, TmuxRecordingManifest, TmuxRecordingState,
        TmuxSessionLink, TmuxSocket, TmuxTrackCatalogEntry, TMUX_RECORDING_FORMAT,
        TMUX_RECORDING_VERSION,
    },
    catalog::TmuxCatalog,
    client::{same_executable, PaneRow, TmuxClient},
    error::{Error, Result},
    reducer::{check_identity, permanent_gaps, unlinked_recordings, Input, Observation},
    setup::{self, PreparedTmuxSetup, SetupRequest},
    status::{self, Integration, PaneCoverage},
    store::{path_from_bytes, EventPage, LockedRecording, RecordingStore, CONFIG_NAME},
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use termtrace_control::{is_completed, read_completed};
use termtrace_model::value::ByteString;
use uuid::Uuid;

const TRACK_PAGE: u16 = 128;

/// Track effects an aggregate recording needs from the Termtrace session layer.
pub trait Tracks {
    /// Managed state root holding sessions, the registry, and aggregate recordings.
    fn state_root(&self) -> &Path;
    /// Fail unless `alias` is free in the session registry.
    ///
    /// # Errors
    /// Returns a taken alias or registry failures.
    fn reserve(&self, alias: &str) -> Result<()>;
    /// Start a detached recording of `command` registered as `alias`.
    ///
    /// # Errors
    /// Returns a failure to start the recording.
    fn start(&self, alias: &str, command: &[OsString]) -> Result<TmuxSessionLink>;
    /// Resolve a registered alias to its session directory.
    ///
    /// # Errors
    /// Returns an unknown alias or registry failures.
    fn resolve(&self, alias: &str) -> Result<PathBuf>;
    /// Request that a session stop, optionally awaiting durable completion.
    ///
    /// Stopping a completed session succeeds.
    ///
    /// # Errors
    /// Returns a rejected stop or a completion timeout.
    fn stop(&self, session: &Path, wait: bool, timeout: Duration) -> Result<()>;
    /// Await validated completion of a session.
    ///
    /// # Errors
    /// Returns a timeout or invalid completion evidence.
    fn wait_completed(&self, session: &Path, timeout: Duration) -> Result<()>;
}

/// Request to start an aggregate recording.
#[derive(Clone, Debug)]
pub struct RecordRequest {
    /// Recording name.
    pub name: String,
    /// Start a new tmux server from a generated configuration.
    pub new_server: bool,
    /// Client of the recorded server.
    pub client: TmuxClient,
    /// Integration settings.
    pub setup: SetupRequest,
    /// tmux client command recorded as the composite track.
    pub command: Vec<OsString>,
}

/// Request to stop an aggregate recording.
#[derive(Clone, Copy, Debug)]
pub struct StopRequest<'name> {
    /// Recording name.
    pub name: &'name str,
    /// Await completion of every track.
    pub wait: bool,
    /// Completion wait budget.
    pub timeout: Duration,
}

/// Start a composite client track and supervise its server as one aggregate recording.
///
/// The name is claimed first. If the composite track cannot start, the integration
/// options this call installed are restored and the name is released.
///
/// # Errors
/// Returns a taken name or alias, invalid settings, or a composite start failure.
pub fn record(
    tracks: &impl Tracks,
    mut request: RecordRequest,
) -> Result<(RecordingStore, TmuxRecordingManifest)> {
    let recording_id = Uuid::now_v7();

    if request.new_server && request.client.socket == TmuxSocket::Default {
        request.client.socket = TmuxSocket::Name(ByteString::from(
            format!("termtrace-recording-{recording_id}").into_bytes(),
        ));
    }

    let store = RecordingStore::create(tracks.state_root(), &request.name)?;

    return match start(tracks, &store, request, recording_id) {
        Ok(manifest) => Ok((store, manifest)),
        Err(error) => {
            if let Err(release) = store.release() {
                tracing::warn!(%release, "could not release a tmux recording that failed to start");
            }

            Err(error)
        }
    };
}

fn start(
    tracks: &impl Tracks,
    store: &RecordingStore,
    request: RecordRequest,
    recording_id: Uuid,
) -> Result<TmuxRecordingManifest> {
    let RecordRequest {
        name,
        new_server,
        client,
        mut setup,
        command,
    } = request;
    setup.aggregate_manifest = Some(store.manifest_path());
    let setup = PreparedTmuxSetup::prepare(setup, client.executable.clone(), tracks.state_root())?;

    let alias = format!("tmux-{name}-composite");
    tracks.reserve(&alias)?;

    let now = unix_time_ns();
    let gaps = permanent_gaps(now);

    let mut events = vec![TmuxEvent::RecordingStarted {
        coverage: setup.coverage,
    }];
    events.extend(
        gaps.iter()
            .map(|gap| return TmuxEvent::CoverageGapOpened { gap: gap.clone() }),
    );
    store.initialize(
        TmuxRecordingManifest {
            format: TMUX_RECORDING_FORMAT.to_owned(),
            version: TMUX_RECORDING_VERSION,
            recording_id,
            name,
            created_unix_ns: now,
            coverage: setup.coverage,
            tmux_executable: ByteString::from(client.executable.as_os_str().as_bytes()),
            socket: client.socket.clone(),
            state: TmuxRecordingState {
                lifecycle: TmuxRecordingLifecycle::Starting,
                composite_track: TmuxCompositeTrack {
                    alias: alias.clone(),
                    link: None,
                },
                server: None,
                panes: Vec::new(),
                pane_tracks: BTreeMap::new(),
                coverage_gaps: gaps,
                updated_unix_ns: now,
            },
            lifecycle_journal: TmuxJournalBoundary::empty(),
            catalog: TmuxCatalogSummary::default(),
        },
        events,
        now,
    )?;

    let (config, enabled) = if new_server {
        (
            Some(store.write_private(CONFIG_NAME, &setup.config_fragment()?)?),
            None,
        )
    } else {
        (None, Some(setup.enable(&client)?))
    };

    let link = composite_command(&client, config.as_deref(), &command)
        .and_then(|command| return tracks.start(&alias, &command));

    let link = match link {
        Ok(link) => link,
        Err(error) => {
            if let Some(enabled) = enabled
                && let Err(rollback) = enabled.roll_back(&client)
            {
                tracing::warn!(%rollback, "could not restore tmux options after a failed start");
            }

            return Err(error);
        }
    };

    store
        .lock()?
        .apply(Input::CompositeLinked(link), unix_time_ns())?;

    if let Err(error) = refresh(store, &client) {
        tracing::warn!(%error, "aggregate tmux recording started before its first server snapshot");
    }

    return store.read();
}

fn composite_command(
    client: &TmuxClient,
    config: Option<&Path>,
    requested: &[OsString],
) -> Result<Vec<OsString>> {
    let requested_executable = requested
        .first()
        .map(PathBuf::from)
        .ok_or_else(|| return Error::invalid("tmux record command is empty"))?;

    if !same_executable(&requested_executable, &client.executable)
        && requested_executable.file_name() != client.executable.file_name()
    {
        return Err(Error::invalid(format!(
            "tmux record command executable {} does not match configured tmux executable {}",
            requested_executable.display(),
            client.executable.display()
        )));
    }

    let mut command = vec![client.executable.as_os_str().to_owned()];
    command.extend(client.socket_arguments());

    if let Some(config) = config {
        command.push(OsString::from("-f"));
        command.push(config.as_os_str().to_owned());
    }

    command.extend(requested.iter().skip(1).cloned());

    return Ok(command);
}

/// Client of the server a recording bound.
pub fn client_of(manifest: &TmuxRecordingManifest) -> TmuxClient {
    return TmuxClient::new(
        path_from_bytes(&manifest.tmux_executable),
        manifest.socket.clone(),
    );
}

/// Refresh a recording from its server, then return its snapshot.
///
/// An unreachable server leaves the snapshot unchanged.
///
/// # Errors
/// Returns [`Error::ServerIdentityChanged`] when another server answers, or read failures.
pub fn status(state_root: &Path, name: &str) -> Result<TmuxRecordingManifest> {
    let store = RecordingStore::locate(state_root, name)?;
    let client = client_of(&store.read()?);

    match refresh(&store, &client) {
        Ok(()) => {}
        Err(error @ Error::ServerIdentityChanged { .. }) => return Err(error),
        Err(error) => {
            tracing::debug!(%error, "tmux server is unavailable while reading aggregate status")
        }
    }

    return store.read();
}

fn refresh(store: &RecordingStore, client: &TmuxClient) -> Result<()> {
    let integration = Integration::read(client)?;
    let rows = client.list_panes()?;
    let panes = status::classify(integration.as_ref(), &rows, |pid| {
        return integration.as_ref().is_some_and(|integration| {
            return status::is_pane_wrapper(pid, &integration.termtrace);
        });
    });

    return observe(store, client, &rows, &panes);
}

/// Refresh the aggregate at `manifest` from one supervisor snapshot.
///
/// # Errors
/// Returns a changed server identity, track resolution, or commit failures.
pub fn refresh_from(
    manifest: &Path,
    client: &TmuxClient,
    rows: &[PaneRow],
    panes: &[PaneCoverage],
) -> Result<()> {
    return observe(&RecordingStore::at(manifest)?, client, rows, panes);
}

fn observe(
    store: &RecordingStore,
    client: &TmuxClient,
    rows: &[PaneRow],
    panes: &[PaneCoverage],
) -> Result<()> {
    let identity = client.server_identity()?;
    let mut locked = store.lock()?;
    let now = unix_time_ns();
    settle(store, &mut locked, now)?;
    let manifest = locked.manifest();

    if !manifest.state.lifecycle.is_open() {
        return Ok(());
    }

    let snapshots = rows
        .iter()
        .map(|row| return row.snapshot.clone())
        .collect::<Vec<_>>();

    let mut links = BTreeMap::new();
    for (pane_id, session) in unlinked_recordings(&manifest.state, &snapshots) {
        links.insert(
            pane_id.clone(),
            store.resolve_track(manifest, &pane_id, &session, now)?,
        );
    }

    return locked.apply(
        Input::Observe(Observation {
            identity: Some(identity),
            panes: snapshots,
            uncovered: status::uncovered(panes),
            links,
        }),
        now,
    );
}

/// Complete a stopping recording whose composite and pane tracks all published completion.
fn settle(store: &RecordingStore, locked: &mut LockedRecording<'_>, now: u64) -> Result<()> {
    let manifest = locked.manifest();
    if manifest.state.lifecycle != TmuxRecordingLifecycle::Stopping {
        return Ok(());
    }

    let composite_completed = manifest
        .state
        .composite_track
        .link
        .as_ref()
        .is_some_and(|link| return is_completed(&path_from_bytes(&link.session)));

    if !composite_completed {
        return Ok(());
    }

    let catalog = store.catalog(manifest)?;
    let mut remaining = catalog.summary().unfinished_tracks;
    let mut completed = Vec::new();

    for entry in unfinished_tracks(&catalog)? {
        if let Some(completion) = read_completed(&path_from_bytes(&entry.track.session))? {
            if completion.session_id != entry.track.session_id {
                return Err(Error::invalid("tmux track completion identity changed"));
            }

            remaining = remaining
                .checked_sub(1)
                .ok_or_else(|| return Error::invalid("tmux unfinished track count underflow"))?;

            completed.push((entry.track.pane_id, entry.track.session_id));
        }
    }

    drop(catalog);

    return locked.apply(
        Input::TracksSettled {
            completed,
            remaining,
            composite_completed,
        },
        now,
    );
}

fn unfinished_tracks(catalog: &TmuxCatalog) -> Result<Vec<TmuxTrackCatalogEntry>> {
    let mut entries = Vec::new();
    let mut next = 0;

    loop {
        let page = catalog.tracks(next, TRACK_PAGE, true)?;
        let Some(last) = page.last() else {
            return Ok(entries);
        };
        next = last
            .sequence
            .checked_add(1)
            .ok_or_else(|| return Error::invalid("tmux track sequence overflow"))?;

        entries.extend(page);
    }
}

/// Record that a supervisor started observing the aggregate at `manifest`.
///
/// # Errors
/// Returns lock or commit failures.
pub fn observer_started(manifest: &Path, pid: u32) -> Result<()> {
    let store = RecordingStore::at(manifest)?;

    return store
        .lock()?
        .apply(Input::ObserverStarted { pid }, unix_time_ns());
}

/// Link a pane recording published by its wrapper.
///
/// # Errors
/// Returns [`Error::Sealed`] for completed recordings, or a rebound identity.
pub fn register_pane_track(manifest: &Path, pane: &str, session: &Path) -> Result<()> {
    let store = RecordingStore::at(manifest)?;
    let mut locked = store.lock()?;
    let now = unix_time_ns();
    let track = store.resolve_track(
        locked.manifest(),
        pane,
        &ByteString::from(session.as_os_str().as_bytes()),
        now,
    )?;

    return locked.apply(Input::TrackLinked(track), now);
}

/// Stop the composite track and every pane track, then disable the integration.
///
/// Pane tracks are only killed and the integration only disabled when the bound server
/// still answers; a replacement server fails the stop.
///
/// # Errors
/// Returns identity mismatches, track stop failures, or a completion timeout.
pub fn stop(tracks: &impl Tracks, request: StopRequest<'_>) -> Result<TmuxRecordingManifest> {
    let store = RecordingStore::locate(tracks.state_root(), request.name)?;
    let (manifest, server_validated) = {
        let mut locked = store.lock()?;
        if !locked.manifest().state.lifecycle.is_open() {
            return Ok(locked.into_manifest());
        }

        let manifest = locked.manifest();
        let client = client_of(manifest);
        let server_validated = match client.server_identity() {
            Err(_) => false,
            Ok(observed) => {
                if manifest.state.server.is_none() {
                    return Err(Error::ServerIdentityChanged {
                        name: manifest.name.clone(),
                        detail: "manifest has no recorded server identity".to_owned(),
                    });
                }

                check_identity(&manifest.name, manifest.state.server.as_ref(), &observed)?;

                true
            }
        };

        locked.apply(Input::StopRequested, unix_time_ns())?;

        (locked.into_manifest(), server_validated)
    };

    let composite = match &manifest.state.composite_track.link {
        Some(link) => path_from_bytes(&link.session),
        None => tracks.resolve(&manifest.state.composite_track.alias)?,
    };

    tracks.stop(&composite, request.wait, request.timeout)?;

    let client = client_of(&manifest);

    // A wrapper can be running before its asynchronous track registration is
    // published. Include those panes in shutdown as well as the saved links.
    if server_validated && let Ok(report) = status::observe(&client) {
        for pane in report.panes {
            if pane.selected && pane.recorded {
                let _ = client.run(["kill-pane", "-t", pane.pane_id.as_str()]);
            }
        }
    }

    for (pane_id, session) in &manifest.state.pane_tracks {
        if server_validated {
            let _ = client.run(["kill-pane", "-t", pane_id.as_str()]);
        }

        if request.wait {
            tracks
                .wait_completed(&path_from_bytes(session), request.timeout)
                .map_err(|error| {
                    return Error::invalid(format!(
                        "could not complete pane track {pane_id}: {error}"
                    ));
                })?;
        }
    }

    if request.wait {
        let deadline = Instant::now() + request.timeout;

        for entry in unfinished_tracks(&store.catalog(&manifest)?)? {
            tracks.wait_completed(
                &path_from_bytes(&entry.track.session),
                deadline.saturating_duration_since(Instant::now()),
            )?;
        }
    }

    if server_validated && let Err(error) = setup::disable(&client) {
        tracing::debug!(%error, "could not disable tmux recording after stop");
    }

    let deadline = Instant::now() + request.timeout;

    loop {
        let mut locked = store.lock()?;
        settle(&store, &mut locked, unix_time_ns())?;
        let manifest = locked.into_manifest();

        if !request.wait || manifest.state.lifecycle == TmuxRecordingLifecycle::Completed {
            return Ok(manifest);
        }

        if Instant::now() >= deadline {
            return Err(Error::invalid(format!(
                "tmux recording has unfinished tracks: composite={:?}, catalog={:?}",
                manifest.state.composite_track.link, manifest.catalog,
            )));
        }

        thread::sleep(Duration::from_millis(20));
    }
}

/// Session directory of a recording's composite client track.
///
/// # Errors
/// Returns an unknown recording or one whose composite track never started.
pub fn composite_session(state_root: &Path, name: &str) -> Result<PathBuf> {
    let manifest = RecordingStore::locate(state_root, name)?.read()?;

    return manifest
        .state
        .composite_track
        .link
        .map(|link| return path_from_bytes(&link.session))
        .ok_or_else(|| {
            return Error::invalid("aggregate tmux recording has no linked composite client track");
        });
}

/// A bounded page of lifecycle history.
///
/// # Errors
/// Returns an unknown recording or history failures.
pub fn events(
    state_root: &Path,
    name: &str,
    from_sequence: u64,
    limit: u16,
) -> Result<(TmuxRecordingManifest, EventPage)> {
    let store = RecordingStore::locate(state_root, name)?;
    let manifest = store.read()?;
    let page = store.events(&manifest, from_sequence, limit)?;

    return Ok((manifest, page));
}

/// A bounded page of track incarnations or coverage intervals.
#[derive(Clone, Debug)]
pub enum CatalogPage {
    /// Track incarnations.
    Tracks(Vec<TmuxTrackCatalogEntry>),
    /// Coverage intervals.
    Gaps(Vec<TmuxGapCatalogEntry>),
}

/// Read a catalog page.
///
/// # Errors
/// Returns an unknown recording or catalog failures.
pub fn catalog_page(
    state_root: &Path,
    name: &str,
    gaps: bool,
    from_sequence: u64,
    limit: u16,
    active_only: bool,
) -> Result<(TmuxRecordingManifest, TmuxCatalogSummary, CatalogPage)> {
    let store = RecordingStore::locate(state_root, name)?;
    let manifest = store.read()?;
    let catalog = store.catalog(&manifest)?;
    let page = if gaps {
        CatalogPage::Gaps(catalog.gaps(from_sequence, limit, active_only)?)
    } else {
        CatalogPage::Tracks(catalog.tracks(from_sequence, limit, active_only)?)
    };

    let summary = catalog.summary().clone();

    return Ok((manifest, summary, page));
}

fn unix_time_ns() -> u64 {
    return SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| return u64::try_from(duration.as_nanos()).ok())
        .unwrap_or(u64::MAX);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{artifact::TmuxRecordingCoverage, scope::ScopeTargets, setup::GhosttyProfileMode};
    use std::cell::Cell;
    use tempfile::TempDir;

    struct FailingTracks {
        root: PathBuf,
        starts: Cell<u32>,
    }

    impl Tracks for FailingTracks {
        fn state_root(&self) -> &Path {
            return &self.root;
        }

        fn reserve(&self, _alias: &str) -> Result<()> {
            return Ok(());
        }

        fn start(&self, _alias: &str, _command: &[OsString]) -> Result<TmuxSessionLink> {
            self.starts.set(self.starts.get() + 1);

            return Err(Error::Track {
                operation: "start",
                message: "spawn failed".to_owned(),
            });
        }

        fn resolve(&self, alias: &str) -> Result<PathBuf> {
            return Err(Error::NotFound(alias.to_owned()));
        }

        fn stop(&self, _session: &Path, _wait: bool, _timeout: Duration) -> Result<()> {
            return Ok(());
        }

        fn wait_completed(&self, _session: &Path, _timeout: Duration) -> Result<()> {
            return Ok(());
        }
    }

    fn request() -> RecordRequest {
        return RecordRequest {
            name: "failed".to_owned(),
            new_server: true,
            client: TmuxClient::new(PathBuf::from("tmux"), TmuxSocket::Default),
            setup: SetupRequest {
                launch_hook: false,
                profile_mode: GhosttyProfileMode::Off,
                ghostty_config: None,
                coverage: TmuxRecordingCoverage::Warn,
                aggregate_manifest: None,
                explicit_only: false,
                targets: ScopeTargets::default(),
                cell_width: 8,
                cell_height: 16,
                term: None,
            },
            command: vec![OsString::from("tmux"), OsString::from("new-session")],
        };
    }

    #[test]
    fn a_failed_composite_start_releases_the_recording_name() {
        let temporary = TempDir::new().expect("temporary state root");
        let tracks = FailingTracks {
            root: temporary.path().to_path_buf(),
            starts: Cell::new(0),
        };

        for attempt in 1..=2 {
            assert!(matches!(
                record(&tracks, request()),
                Err(Error::Track { .. })
            ));
            assert_eq!(tracks.starts.get(), attempt, "the name was free again");
            assert!(!RecordingStore::locate(temporary.path(), "failed")
                .expect("name")
                .directory()
                .exists());
        }
    }
}
