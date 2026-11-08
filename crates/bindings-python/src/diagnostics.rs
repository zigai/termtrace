//! Diagnostics, evidence, registry, tmux, instrumentation, and system bindings.
//!
//! Each method runs one shared Rust operation off the GIL and returns its model value.
//! Cancellation of a calling thread never rolls an admitted operation back.

use crate::{
    errors::to_py_err,
    host::{Arg, Model, PathArg},
    locator::PyCancelToken,
};
use pyo3::{exceptions::PyValueError, prelude::*};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::{
    collections::{BTreeMap, HashMap},
    os::unix::ffi::OsStringExt as _,
    path::PathBuf,
    sync::Arc,
};
use termtrace_model::{
    artifact::{
        bundle::{BundleManifest, BundleProfile},
        presentation::MarkerRecord,
        save::{SaveFormat, SaveOutcome},
    },
    execution::{
        application::ApplicationAcknowledgement,
        environment::EnvironmentSpec,
        event_log::RecoveryReport,
        session::{VerificationLevel, VerificationReport},
        tmux::TmuxRecordConfiguration,
    },
    protocol::capability::CapabilityReport,
    registry::{
        GarbageCollectionReport, RegistryEntry, RegistryListEntry, RegistrySessionResolution,
        RestartConfiguration, RestartReport,
    },
    terminal::{
        analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport, LastActionAnalysis},
        diff::SemanticDiff,
        observation::{ReplayCachePruneReport, ReplayCacheStatus, StatePoint},
        replay::SourceRange,
        reviewer::{ReviewExportManifest, ReviewSearchResult, ReviewSummary},
        timeline::TimelinePage,
    },
    value::ExactPath,
};
use termtrace_sdk::{
    contracts::{
        diagnostics::{self as core, TmuxCatalog, TmuxEvents},
        timeout_from_ms,
    },
    defaults,
    instrumentation::Instrumentation,
    locator::WaitCancellation,
    registry::RegistryService,
    system::{SystemService, Versions},
    tmux::{
        ServiceTracks, TmuxClient, TmuxRecordingManifest, TmuxRecordingStart, TmuxService,
        TmuxSocket, TmuxStatusReport, TmuxStreamingBundleManifest, Tracks as _,
    },
    ByteString, Uuid,
};

fn range(start: Option<u64>, end: Option<u64>) -> PyResult<Option<SourceRange>> {
    return match (start, end) {
        (None, None) => Ok(None),
        (Some(start_monotonic_ns), Some(end_monotonic_ns)) => Ok(Some(SourceRange {
            start_monotonic_ns,
            end_monotonic_ns,
        })),
        _ => Err(PyValueError::new_err("range requires both endpoints")),
    };
}

fn timeout(timeout_ms: Option<f64>, default: std::time::Duration) -> PyResult<std::time::Duration> {
    return timeout_ms.map_or(Ok(default), |ms| {
        return timeout_from_ms(ms).map_err(PyValueError::new_err);
    });
}

fn registry(root: Option<PathBuf>) -> PyResult<RegistryService> {
    return root
        .map_or_else(RegistryService::open_default, RegistryService::open)
        .map_err(to_py_err);
}

/// Replay analysis of one canonical session.
#[gen_stub_pyclass]
#[pyclass(name = "Diagnostics", frozen)]
pub struct PyDiagnostics {
    pub(crate) path: PathBuf,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyDiagnostics {
    /// Bind one canonical session directory.
    #[new]
    pub fn new(path: PathArg) -> Self {
        return Self { path: path.0 };
    }

    /// One bounded timeline page.
    /// # Errors
    /// Rejects a limit outside 1..=4096 or a partial or empty range.
    #[pyo3(signature = (offset = 0, limit = 100, include_hidden = false, controls = false, start_monotonic_ns = None, end_monotonic_ns = None))]
    pub fn timeline(
        &self,
        py: Python<'_>,
        offset: u64,
        limit: u16,
        include_hidden: bool,
        controls: bool,
        start_monotonic_ns: Option<u64>,
        end_monotonic_ns: Option<u64>,
    ) -> PyResult<Model<TimelinePage>> {
        let range = range(start_monotonic_ns, end_monotonic_ns)?;
        let path = self.path.clone();

        return py
            .detach(move || core::timeline(&path, offset, limit, include_hidden, controls, range))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Semantic difference between two states.
    /// # Errors
    /// Propagates endpoint validation and replay failures.
    #[pyo3(signature = (from_, to))]
    pub fn diff(
        &self,
        py: Python<'_>,
        from_: Arg<StatePoint>,
        to: Arg<StatePoint>,
    ) -> PyResult<Model<SemanticDiff>> {
        let path = self.path.clone();

        return py
            .detach(move || core::diff(&path, from_.0, to.0))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Source-bound VT explanation of one state.
    /// # Errors
    /// Propagates replay and validation failures.
    #[pyo3(signature = (target = None, context = 8))]
    pub fn explain(
        &self,
        py: Python<'_>,
        target: Option<Arg<StatePoint>>,
        context: u64,
    ) -> PyResult<Model<ExplainReport>> {
        let target = target.map_or(StatePoint::LatestPresented, |target| return target.0);
        let path = self.path.clone();

        return py
            .detach(move || core::explain(&path, target, context))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Conservative diagnostics, including native evidence limitations.
    /// # Errors
    /// Rejects a change percent above 100 and propagates replay failures.
    #[pyo3(signature = (checks = None, broad_change_percent = 50))]
    pub fn diagnose(
        &self,
        py: Python<'_>,
        checks: Option<Vec<Arg<DiagnosticCheck>>>,
        broad_change_percent: u8,
    ) -> PyResult<Model<DiagnoseReport>> {
        let checks = checks
            .unwrap_or_default()
            .into_iter()
            .map(|check| return check.0)
            .collect();
        let path = self.path.clone();

        return py
            .detach(move || core::diagnose(&path, checks, broad_change_percent))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Triage of the most recent action.
    /// # Errors
    /// Rejects a change percent above 100 and propagates absent-action and replay failures.
    #[pyo3(signature = (broad_change_percent = 50))]
    pub fn last_action(
        &self,
        py: Python<'_>,
        broad_change_percent: u8,
    ) -> PyResult<Model<LastActionAnalysis>> {
        let path = self.path.clone();

        return py
            .detach(move || core::last_action(&path, broad_change_percent))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Marker positions and nominal frames.
    /// # Errors
    /// Rejects zero fps and propagates canonical evidence failures.
    #[pyo3(signature = (fps = 30))]
    pub fn markers(&self, py: Python<'_>, fps: u16) -> PyResult<Vec<Model<MarkerRecord>>> {
        let path = self.path.clone();

        return py
            .detach(move || core::markers(&path, fps))
            .map(|markers| return markers.into_iter().map(Model).collect())
            .map_err(to_py_err);
    }

    /// Replay cache inventory.
    /// # Errors
    /// Propagates filesystem failures.
    pub fn cache_status(&self, py: Python<'_>) -> PyResult<Model<ReplayCacheStatus>> {
        let path = self.path.clone();

        return py
            .detach(move || core::cache_status(&path))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Prune disposable replay checkpoints.
    /// # Errors
    /// Propagates filesystem failures.
    pub fn cache_prune(&self, py: Python<'_>) -> PyResult<Model<ReplayCachePruneReport>> {
        let path = self.path.clone();

        return py
            .detach(move || core::cache_prune(&path))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Read-only forensic review summary.
    /// # Errors
    /// Propagates native-manifest and canonical validation failures.
    pub fn review_summary(&self, py: Python<'_>) -> PyResult<Model<ReviewSummary>> {
        let path = self.path.clone();

        return py
            .detach(move || core::review_summary(&path))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Semantic viewport search.
    /// # Errors
    /// Propagates replay and query failures.
    pub fn review_search(
        &self,
        py: Python<'_>,
        query: String,
    ) -> PyResult<Model<ReviewSearchResult>> {
        let path = self.path.clone();

        return py
            .detach(move || core::review_search(&path, &query))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Export one reviewed state without replacing an existing destination.
    /// # Errors
    /// Propagates replay and publication failures.
    pub fn review_export(
        &self,
        py: Python<'_>,
        state_id: u64,
        destination: PathArg,
    ) -> PyResult<Model<ReviewExportManifest>> {
        let path = self.path.clone();

        return py
            .detach(move || core::review_export(&path, state_id, &destination))
            .map(Model)
            .map_err(to_py_err);
    }
}

/// Evidence artifacts, verification, signing, and recovery for one canonical session.
#[gen_stub_pyclass]
#[pyclass(name = "Evidence", frozen)]
pub struct PyEvidence {
    pub(crate) path: PathBuf,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyEvidence {
    /// Bind one canonical session directory.
    #[new]
    pub fn new(path: PathArg) -> Self {
        return Self { path: path.0 };
    }

    /// Atomically save the requested representations of one state.
    /// # Errors
    /// Propagates state resolution, rendering, and publication failures.
    #[pyo3(signature = (destination, formats, point = None, include_hidden = false))]
    pub fn save(
        &self,
        py: Python<'_>,
        destination: PathArg,
        formats: Vec<Arg<SaveFormat>>,
        point: Option<Arg<StatePoint>>,
        include_hidden: bool,
    ) -> PyResult<Model<SaveOutcome>> {
        let formats = formats
            .into_iter()
            .map(|format| return format.0)
            .collect::<Vec<_>>();
        let point = point.map_or(StatePoint::LatestPresented, |point| return point.0);
        let path = self.path.clone();

        return py
            .detach(move || core::save(&path, &destination, point, &formats, include_hidden))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Create an evidence bundle with an explicit profile.
    /// # Errors
    /// Propagates bundle failures.
    pub fn bundle(
        &self,
        py: Python<'_>,
        destination: PathArg,
        profile: Arg<BundleProfile>,
    ) -> PyResult<Model<BundleManifest>> {
        let path = self.path.clone();

        return py
            .detach(move || core::bundle(&path, &destination, profile.0))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Verify a bundle against this session and an explicit profile.
    /// # Errors
    /// Propagates invalid, tampered, wrong-source, and wrong-profile failures.
    pub fn verify_bundle(
        &self,
        py: Python<'_>,
        directory: PathArg,
        profile: Arg<BundleProfile>,
    ) -> PyResult<Model<BundleManifest>> {
        let path = self.path.clone();

        return py
            .detach(move || core::verify_bundle(&directory, &path, profile.0))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Verify the session; validity, authenticity, and trust stay separate facts.
    #[pyo3(signature = (level = None, allowed_signers = None))]
    pub fn verify(
        &self,
        py: Python<'_>,
        level: Option<Arg<VerificationLevel>>,
        allowed_signers: Option<PathArg>,
    ) -> Model<VerificationReport> {
        let level = level.map_or(VerificationLevel::Content, |level| return level.0);
        let path = self.path.clone();

        return Model(py.detach(move || core::verify(&path, level, allowed_signers.as_deref())));
    }

    /// Sign an exact receipt; signing never implies trust.
    /// # Errors
    /// Propagates verification, key, signing, and publication failures.
    pub fn sign(&self, py: Python<'_>, key: PathArg, signer: String) -> PyResult<Model<ExactPath>> {
        let path = self.path.clone();

        return py
            .detach(move || core::sign(&path, &key, &signer))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Recover the valid prefix into a new session directory without changing the source.
    /// # Errors
    /// Propagates invalid prefix and destination failures.
    pub fn recover(&self, py: Python<'_>, destination: PathArg) -> PyResult<Model<RecoveryReport>> {
        let path = self.path.clone();

        return py
            .detach(move || core::recover(&path, &destination, false))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Recover the valid prefix of this event log into a new log.
    /// # Errors
    /// Propagates invalid prefix and destination failures.
    pub fn recover_log(
        &self,
        py: Python<'_>,
        destination: PathArg,
    ) -> PyResult<Model<RecoveryReport>> {
        let path = self.path.clone();

        return py
            .detach(move || core::recover(&path, &destination, true))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Export an asciicast and return its path.
    /// # Errors
    /// Propagates replay and publication failures.
    pub fn asciicast(&self, py: Python<'_>, destination: PathArg) -> PyResult<Model<ExactPath>> {
        let path = self.path.clone();

        return py
            .detach(move || {
                termtrace_sdk::artifacts::ArtifactService::export_asciicast(&path, &destination)
                    .map(|()| return ExactPath(destination.0))
            })
            .map(Model)
            .map_err(to_py_err);
    }
}

/// Session registry: aliases, lifecycle, retention, and restart.
#[gen_stub_pyclass]
#[pyclass(name = "Registry", frozen)]
pub struct PyRegistry {
    pub(crate) root: Option<PathBuf>,
    pub(crate) binary: PathBuf,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyRegistry {
    /// Open an explicit or the default registry lazily.
    #[new]
    #[pyo3(signature = (root = None, binary = None))]
    pub fn new(root: Option<PathArg>, binary: Option<PathArg>) -> Self {
        return Self {
            root: root.map(|root| return root.0),
            binary: binary.map_or_else(defaults::binary, |binary| return binary.0),
        };
    }

    /// Restart through shared readiness, lineage, and alias compare-and-swap.
    /// # Errors
    /// Propagates option, predecessor, successor, orphan, and retarget failures.
    #[pyo3(signature = (alias, options = None, environment = None, timeout_ms = None))]
    pub fn restart(
        &self,
        py: Python<'_>,
        alias: String,
        options: Option<Arg<RestartConfiguration>>,
        environment: Option<Arg<EnvironmentSpec>>,
        timeout_ms: Option<f64>,
    ) -> PyResult<Model<RestartReport>> {
        let options = options.map(|options| return options.0).unwrap_or_default();
        let environment = environment.map(|environment| return environment.0);
        let timeout = timeout(timeout_ms, termtrace_sdk::defaults::STARTUP_TIMEOUT)?;
        let root = self.root.clone();
        let binary = self.binary.clone();

        return py
            .detach(move || {
                let registry = registry(root)?;

                return core::restart(
                    &registry,
                    &binary,
                    &alias,
                    &options,
                    environment.as_ref(),
                    timeout,
                )
                .map_err(to_py_err);
            })
            .map(Model);
    }

    /// Registered sessions; unhealthy rows keep their diagnostics.
    /// # Errors
    /// Propagates state-root errors.
    #[pyo3(signature = (include_all = false))]
    pub fn list(
        &self,
        py: Python<'_>,
        include_all: bool,
    ) -> PyResult<Vec<Model<RegistryListEntry>>> {
        let root = self.root.clone();

        return py.detach(move || {
            let entries = core::registry_list(&registry(root)?, include_all).map_err(to_py_err)?;

            return Ok(entries.into_iter().map(Model).collect());
        });
    }

    /// Register a session under an alias.
    /// # Errors
    /// Propagates invalid aliases, collisions, security, and identity errors.
    pub fn register(
        &self,
        py: Python<'_>,
        alias: String,
        session: PathArg,
    ) -> PyResult<Model<RegistryEntry>> {
        let root = self.root.clone();

        return py
            .detach(move || {
                return core::registry_register(&registry(root)?, &alias, &session)
                    .map_err(to_py_err);
            })
            .map(Model);
    }

    /// Resolve an alias; `None` only when no alias record exists.
    /// # Errors
    /// Corruption, permissions, missing evidence, and invalid aliases propagate.
    pub fn resolve(&self, py: Python<'_>, alias: String) -> PyResult<Option<Model<RegistryEntry>>> {
        let root = self.root.clone();

        return py
            .detach(move || core::registry_resolve(&registry(root)?, &alias).map_err(to_py_err))
            .map(|entry| return entry.map(Model));
    }

    /// Derived lifecycle of an alias, including pending and incompatible states.
    /// # Errors
    /// Registry-record failures propagate.
    pub fn lifecycle(
        &self,
        py: Python<'_>,
        alias: String,
    ) -> PyResult<Model<RegistrySessionResolution>> {
        let root = self.root.clone();

        return py
            .detach(move || core::registry_lifecycle(&registry(root)?, &alias).map_err(to_py_err))
            .map(Model);
    }

    /// Remove an alias; canonical evidence is kept.
    /// # Errors
    /// Missing aliases, permissions, and invalid records propagate.
    pub fn remove(&self, py: Python<'_>, alias: String) -> PyResult<Model<RegistryEntry>> {
        let root = self.root.clone();

        return py
            .detach(move || core::registry_remove(&registry(root)?, &alias).map_err(to_py_err))
            .map(Model);
    }

    /// Retention planning, applied only when `apply` is true.
    /// # Errors
    /// Inspection and deletion failures propagate.
    #[pyo3(signature = (older_than_days = 30, keep_latest = 20, max_completed_bytes = None, apply = false))]
    pub fn gc(
        &self,
        py: Python<'_>,
        older_than_days: u64,
        keep_latest: u64,
        max_completed_bytes: Option<u64>,
        apply: bool,
    ) -> PyResult<Model<GarbageCollectionReport>> {
        let root = self.root.clone();

        return py
            .detach(move || {
                return core::registry_gc(
                    &registry(root)?,
                    older_than_days,
                    keep_latest,
                    max_completed_bytes,
                    apply,
                )
                .map_err(to_py_err);
            })
            .map(Model);
    }
}

/// Aggregate tmux recording through the shared tmux service.
#[gen_stub_pyclass]
#[pyclass(name = "Tmux", frozen)]
pub struct PyTmux {
    root: Option<PathBuf>,
    binary: PathBuf,
    client: TmuxClient,
}

impl PyTmux {
    fn tracks(&self) -> PyResult<ServiceTracks> {
        return ServiceTracks::open(self.binary.clone(), self.root.as_deref()).map_err(to_py_err);
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyTmux {
    /// Bind paths lazily; construction starts no tmux server.
    #[new]
    #[pyo3(signature = (root = None, binary = None, tmux = None, socket = None, socket_name = false))]
    pub fn new(
        root: Option<PathArg>,
        binary: Option<PathArg>,
        tmux: Option<PathArg>,
        socket: Option<PathArg>,
        socket_name: bool,
    ) -> Self {
        let socket = socket.map_or(TmuxSocket::Default, |socket| {
            let bytes = ByteString::new(socket.0.into_os_string().into_vec());
            if socket_name {
                return TmuxSocket::Name(bytes);
            }

            return TmuxSocket::Path(bytes);
        });

        return Self {
            root: root.map(|root| return root.0),
            binary: binary.map_or_else(defaults::binary, |binary| return binary.0),
            client: TmuxClient {
                executable: tmux.map_or_else(|| return PathBuf::from("tmux"), |tmux| return tmux.0),
                socket,
            },
        };
    }

    /// Start aggregate recording.
    /// # Errors
    /// Rejects invalid settings and propagates setup and track admission failures.
    pub fn record(
        &self,
        py: Python<'_>,
        options: Arg<TmuxRecordConfiguration>,
    ) -> PyResult<Model<TmuxRecordingStart>> {
        let tracks = self.tracks()?;
        let client = self.client.clone();

        return py
            .detach(move || core::tmux_record(&tracks, client, options.0))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Server and integration status.
    /// # Errors
    /// Propagates connection and malformed integration failures.
    pub fn status(&self, py: Python<'_>) -> PyResult<Model<TmuxStatusReport>> {
        let client = self.client.clone();

        return py
            .detach(move || core::tmux_status(&client))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Lifecycle of one aggregate recording.
    /// # Errors
    /// Propagates manifest, catalog, and identity errors.
    pub fn recording_status(
        &self,
        py: Python<'_>,
        name: String,
    ) -> PyResult<Model<TmuxRecordingManifest>> {
        let tracks = self.tracks()?;

        return py
            .detach(move || core::tmux_recording(tracks.state_root(), &name))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Register one pane session as a track of an aggregate recording.
    /// # Errors
    /// Propagates identity and track failures.
    pub fn track(
        &self,
        py: Python<'_>,
        manifest: PathArg,
        pane: String,
        session: PathArg,
    ) -> PyResult<()> {
        return py
            .detach(move || TmuxService::track(&manifest, &pane, &session))
            .map_err(to_py_err);
    }

    /// Stop the canonical tracks of one aggregate recording.
    /// # Errors
    /// Track and identity failures propagate; prior effects are not rolled back.
    #[pyo3(signature = (name, wait = true, timeout_ms = None))]
    pub fn stop(
        &self,
        py: Python<'_>,
        name: String,
        wait: bool,
        timeout_ms: Option<f64>,
    ) -> PyResult<Model<TmuxRecordingManifest>> {
        let timeout = timeout(timeout_ms, termtrace_sdk::defaults::WAIT_TIMEOUT)?;
        let tracks = self.tracks()?;

        return py
            .detach(move || core::tmux_stop(&tracks, &name, wait, timeout))
            .map(Model)
            .map_err(to_py_err);
    }

    /// One aggregate history page.
    /// # Errors
    /// Propagates manifest and journal errors and invalid paging bounds.
    #[pyo3(signature = (name, from_sequence = 1, limit = 100))]
    pub fn events(
        &self,
        py: Python<'_>,
        name: String,
        from_sequence: u64,
        limit: u16,
    ) -> PyResult<Model<TmuxEvents>> {
        let tracks = self.tracks()?;

        return py
            .detach(move || core::tmux_events(tracks.state_root(), &name, from_sequence, limit))
            .map(Model)
            .map_err(to_py_err);
    }

    /// One track or coverage-gap catalog page.
    /// # Errors
    /// Propagates catalog, manifest, and paging failures.
    #[pyo3(signature = (name, gaps = false, from_sequence = 1, limit = 100, active_only = false))]
    pub fn catalog(
        &self,
        py: Python<'_>,
        name: String,
        gaps: bool,
        from_sequence: u64,
        limit: u16,
        active_only: bool,
    ) -> PyResult<Model<TmuxCatalog>> {
        let tracks = self.tracks()?;

        return py
            .detach(move || {
                core::tmux_catalog(
                    tracks.state_root(),
                    &name,
                    gaps,
                    from_sequence,
                    limit,
                    active_only,
                )
            })
            .map(Model)
            .map_err(to_py_err);
    }

    /// Bundle an aggregate recording.
    /// # Errors
    /// Propagates mismatched or incomplete source and publication failures.
    pub fn bundle(
        &self,
        py: Python<'_>,
        manifest: PathArg,
        destination: PathArg,
    ) -> PyResult<Model<TmuxStreamingBundleManifest>> {
        return py
            .detach(move || core::tmux_bundle(&manifest, &destination, false))
            .map(Model)
            .map_err(to_py_err);
    }

    /// Verify an aggregate bundle against its manifest.
    /// # Errors
    /// Propagates mismatched source and verification failures.
    pub fn verify_bundle(
        &self,
        py: Python<'_>,
        manifest: PathArg,
        destination: PathArg,
    ) -> PyResult<Model<TmuxStreamingBundleManifest>> {
        return py
            .detach(move || core::tmux_bundle(&manifest, &destination, true))
            .map(Model)
            .map_err(to_py_err);
    }
}

/// Application instrumentation; spans end only when the caller ends them.
#[gen_stub_pyclass]
#[pyclass(name = "Instrumentation", frozen)]
pub struct PyInstrumentation {
    inner: Arc<Instrumentation>,
}

fn span_id(text: &str) -> PyResult<Uuid> {
    return Uuid::parse_str(text).map_err(|error| return PyValueError::new_err(error.to_string()));
}

type Attributes = HashMap<String, Arg<serde_json::Value>>;

#[gen_stub_pymethods]
#[pymethods]
impl PyInstrumentation {
    /// Create an emitter bound to one live session.
    #[new]
    pub fn new(source: PathArg) -> Self {
        return Self {
            inner: Arc::new(Instrumentation::new(termtrace_sdk::Client::new(source.0))),
        };
    }

    /// Process identity of every event and span of this emitter.
    #[getter]
    pub fn process_instance_id(&self) -> String {
        return self.inner.process_instance_id().to_string();
    }

    /// Emit one event and return the actor acknowledgement.
    /// # Errors
    /// Rejects unsupported attributes and propagates control failures, without retries.
    #[pyo3(signature = (name, attributes = None))]
    pub fn event(
        &self,
        py: Python<'_>,
        name: String,
        attributes: Option<Attributes>,
    ) -> PyResult<Model<ApplicationAcknowledgement>> {
        let attributes = attributes
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| return (name, value.0))
            .collect::<BTreeMap<_, _>>();
        let emitter = Arc::clone(&self.inner);

        return py
            .detach(move || emitter.event_ack(&name, attributes))
            .map(Model)
            .map_err(|error| return to_py_err(error.into()));
    }

    /// Start a span; dropping this object never ends it.
    /// # Errors
    /// Propagates validation and control failures.
    #[pyo3(signature = (name, parent_span_id = None, attributes = None))]
    pub fn start_span(
        &self,
        py: Python<'_>,
        name: String,
        parent_span_id: Option<String>,
        attributes: Option<Attributes>,
    ) -> PyResult<Model<ApplicationAcknowledgement>> {
        let attributes = attributes
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| return (name, value.0))
            .collect::<BTreeMap<_, _>>();
        let parent = parent_span_id.as_deref().map(span_id).transpose()?;
        let emitter = Arc::clone(&self.inner);

        return py
            .detach(move || emitter.start_span_ack(&name, parent, attributes))
            .map(Model)
            .map_err(|error| return to_py_err(error.into()));
    }

    /// End one span.
    /// # Errors
    /// Propagates validation and control failures.
    #[pyo3(signature = (name, span_id, attributes = None))]
    pub fn end_span(
        &self,
        py: Python<'_>,
        name: String,
        span_id: String,
        attributes: Option<Attributes>,
    ) -> PyResult<Model<ApplicationAcknowledgement>> {
        let attributes = attributes
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| return (name, value.0))
            .collect::<BTreeMap<_, _>>();
        let span = crate::diagnostics::span_id(&span_id)?;
        let emitter = Arc::clone(&self.inner);

        return py
            .detach(move || emitter.end_span_ack(&name, span, attributes))
            .map(Model)
            .map_err(|error| return to_py_err(error.into()));
    }
}

/// System capability probes, versions, and the public schema catalog.
#[gen_stub_pyclass]
#[pyclass(name = "System", frozen)]
#[derive(Default)]
pub struct PySystem;

#[gen_stub_pymethods]
#[pymethods]
impl PySystem {
    /// Bind the local system service without starting a probe.
    #[new]
    pub fn new() -> Self {
        return Self;
    }

    /// Probe capabilities; unavailable tools are reported as blocked, never skipped.
    ///
    /// Cancelling `cancel` stops the native capture probe early; it then reports blocked.
    #[pyo3(signature = (cancel = None))]
    pub fn capabilities(
        &self,
        py: Python<'_>,
        cancel: Option<PyCancelToken>,
    ) -> Model<CapabilityReport> {
        return Model(py.detach(|| {
            let token = cancel.map(|token| return token.inner);

            return WaitCancellation::forward(token.as_ref(), SystemService::probe_capabilities);
        }));
    }

    /// Build and component versions.
    pub fn versions(&self) -> Model<Versions> {
        return Model(SystemService::versions());
    }

    /// Names in the public schema catalog.
    /// # Errors
    /// Propagates catalog registration failures.
    pub fn schemas(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        return py.detach(|| SystemService::schemas().map_err(to_py_err));
    }

    /// One public JSON Schema.
    /// # Errors
    /// Unknown schemas and generation failures propagate.
    pub fn schema(&self, py: Python<'_>, name: String) -> PyResult<Model<serde_json::Value>> {
        return py
            .detach(|| SystemService::schema(&name))
            .map(Model)
            .map_err(to_py_err);
    }
}
