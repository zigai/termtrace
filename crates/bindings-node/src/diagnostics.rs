//! Diagnostics, evidence, registry, tmux, instrumentation, and system bindings.
//!
//! Each method runs one shared Rust operation on the blocking pool and resolves to its model
//! value. Abandoning a promise never rolls an admitted operation back.

use crate::{
    blocking_failure,
    host::{Arg, Model},
    to_napi_err,
};
use napi_derive::napi;
use std::{
    collections::BTreeMap, os::unix::ffi::OsStringExt as _, path::PathBuf, sync::Arc,
    time::Duration,
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
    instrumentation::Instrumentation,
    registry::RegistryService,
    system::{SystemService, Versions},
    tmux::{
        ServiceTracks, TmuxClient, TmuxRecordingManifest, TmuxRecordingStart, TmuxService,
        TmuxSocket, TmuxStatusReport, TmuxStreamingBundleManifest, Tracks as _,
    },
    ByteString, Uuid,
};

async fn blocking<T, F>(operation: F) -> napi::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> termtrace_sdk::Result<T> + Send + 'static,
{
    return tokio::task::spawn_blocking(operation)
        .await
        .map_err(blocking_failure)?
        .map_err(to_napi_err);
}

fn argument(message: impl Into<String>) -> napi::Error {
    return crate::host::type_error(message.into());
}

fn path(value: Arg<ExactPath>) -> PathBuf {
    return value.0 .0;
}

fn range(start: Option<Arg<u64>>, end: Option<Arg<u64>>) -> napi::Result<Option<SourceRange>> {
    return match (start, end) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) => Ok(Some(SourceRange {
            start_monotonic_ns: start.0,
            end_monotonic_ns: end.0,
        })),
        _ => Err(argument("range requires both endpoints")),
    };
}

fn timeout(timeout_ms: Option<f64>, default: Duration) -> napi::Result<Duration> {
    return timeout_ms.map_or(Ok(default), |ms| {
        return timeout_from_ms(ms).map_err(argument);
    });
}

fn value<T>(argument: Option<Arg<T>>, default: T) -> T {
    return argument.map_or(default, |argument| return argument.0);
}

fn default_binary() -> PathBuf {
    return std::env::var_os("TERMTRACE_BIN")
        .map_or_else(|| PathBuf::from("termtrace"), PathBuf::from);
}

fn registry(root: Option<PathBuf>) -> termtrace_sdk::Result<RegistryService> {
    return root.map_or_else(RegistryService::open_default, RegistryService::open);
}

/// Replay analysis of one canonical session.
#[napi(js_name = "Diagnostics")]
pub struct JsDiagnostics {
    pub(crate) path: PathBuf,
}

#[napi]
impl JsDiagnostics {
    /// Bind one canonical session directory.
    #[napi(constructor)]
    pub fn new(path: Arg<ExactPath>) -> Self {
        return Self {
            path: self::path(path),
        };
    }

    /// One bounded timeline page.
    #[napi]
    pub async fn timeline(
        &self,
        offset: Option<Arg<u64>>,
        limit: Option<Arg<u16>>,
        include_hidden: Option<bool>,
        controls: Option<bool>,
        start_monotonic_ns: Option<Arg<u64>>,
        end_monotonic_ns: Option<Arg<u64>>,
    ) -> napi::Result<Model<TimelinePage>> {
        let range = range(start_monotonic_ns, end_monotonic_ns)?;
        let path = self.path.clone();

        return blocking(move || {
            core::timeline(
                &path,
                value(offset, 0),
                value(limit, 100),
                include_hidden.unwrap_or(false),
                controls.unwrap_or(false),
                range,
            )
        })
        .await
        .map(Model);
    }

    /// Semantic difference between two states.
    #[napi]
    pub async fn diff(
        &self,
        from: Arg<StatePoint>,
        to: Arg<StatePoint>,
    ) -> napi::Result<Model<SemanticDiff>> {
        let path = self.path.clone();

        return blocking(move || core::diff(&path, from.0, to.0))
            .await
            .map(Model);
    }

    /// Source-bound VT explanation of one state.
    #[napi]
    pub async fn explain(
        &self,
        target: Option<Arg<StatePoint>>,
        context: Option<Arg<u64>>,
    ) -> napi::Result<Model<ExplainReport>> {
        let path = self.path.clone();

        return blocking(move || {
            core::explain(
                &path,
                value(target, StatePoint::LatestPresented),
                value(context, 8),
            )
        })
        .await
        .map(Model);
    }

    /// Conservative diagnostics, including native evidence limitations.
    #[napi]
    pub async fn diagnose(
        &self,
        checks: Option<Vec<Arg<DiagnosticCheck>>>,
        broad_change_percent: Option<Arg<u8>>,
    ) -> napi::Result<Model<DiagnoseReport>> {
        let checks = checks
            .unwrap_or_default()
            .into_iter()
            .map(|check| return check.0)
            .collect();
        let path = self.path.clone();

        return blocking(move || core::diagnose(&path, checks, value(broad_change_percent, 50)))
            .await
            .map(Model);
    }

    /// Triage of the most recent action.
    #[napi]
    pub async fn last_action(
        &self,
        broad_change_percent: Option<Arg<u8>>,
    ) -> napi::Result<Model<LastActionAnalysis>> {
        let path = self.path.clone();

        return blocking(move || core::last_action(&path, value(broad_change_percent, 50)))
            .await
            .map(Model);
    }

    /// Marker positions and nominal frames.
    #[napi]
    pub async fn markers(&self, fps: Option<Arg<u16>>) -> napi::Result<Vec<Model<MarkerRecord>>> {
        let path = self.path.clone();

        return blocking(move || core::markers(&path, value(fps, 30)))
            .await
            .map(|markers| return markers.into_iter().map(Model).collect());
    }

    /// Replay cache inventory.
    #[napi]
    pub async fn cache_status(&self) -> napi::Result<Model<ReplayCacheStatus>> {
        let path = self.path.clone();

        return blocking(move || core::cache_status(&path)).await.map(Model);
    }

    /// Prune disposable replay checkpoints.
    #[napi]
    pub async fn cache_prune(&self) -> napi::Result<Model<ReplayCachePruneReport>> {
        let path = self.path.clone();

        return blocking(move || core::cache_prune(&path)).await.map(Model);
    }

    /// Read-only forensic review summary.
    #[napi]
    pub async fn review_summary(&self) -> napi::Result<Model<ReviewSummary>> {
        let path = self.path.clone();

        return blocking(move || core::review_summary(&path))
            .await
            .map(Model);
    }

    /// Semantic viewport search.
    #[napi]
    pub async fn review_search(&self, query: String) -> napi::Result<Model<ReviewSearchResult>> {
        let path = self.path.clone();

        return blocking(move || core::review_search(&path, &query))
            .await
            .map(Model);
    }

    /// Export one reviewed state without replacing an existing destination.
    #[napi]
    pub async fn review_export(
        &self,
        state_id: Arg<u64>,
        destination: Arg<ExactPath>,
    ) -> napi::Result<Model<ReviewExportManifest>> {
        let path = self.path.clone();
        let destination = self::path(destination);

        return blocking(move || core::review_export(&path, state_id.0, &destination))
            .await
            .map(Model);
    }
}

/// Evidence artifacts, verification, signing, and recovery for one canonical session.
#[napi(js_name = "Evidence")]
pub struct JsEvidence {
    pub(crate) path: PathBuf,
}

#[napi]
impl JsEvidence {
    /// Bind one canonical session directory.
    #[napi(constructor)]
    pub fn new(path: Arg<ExactPath>) -> Self {
        return Self {
            path: self::path(path),
        };
    }

    /// Atomically save the requested representations of one state.
    #[napi]
    pub async fn save(
        &self,
        destination: Arg<ExactPath>,
        formats: Vec<Arg<SaveFormat>>,
        point: Option<Arg<StatePoint>>,
        include_hidden: Option<bool>,
    ) -> napi::Result<Model<SaveOutcome>> {
        let formats = formats
            .into_iter()
            .map(|format| return format.0)
            .collect::<Vec<_>>();
        let point = value(point, StatePoint::LatestPresented);
        let path = self.path.clone();
        let destination = self::path(destination);

        return blocking(move || {
            core::save(
                &path,
                &destination,
                point,
                &formats,
                include_hidden.unwrap_or(false),
            )
        })
        .await
        .map(Model);
    }

    /// Create an evidence bundle with an explicit profile.
    #[napi]
    pub async fn bundle(
        &self,
        destination: Arg<ExactPath>,
        profile: Arg<BundleProfile>,
    ) -> napi::Result<Model<BundleManifest>> {
        let path = self.path.clone();
        let destination = self::path(destination);

        return blocking(move || core::bundle(&path, &destination, profile.0))
            .await
            .map(Model);
    }

    /// Verify a bundle against this session and an explicit profile.
    #[napi]
    pub async fn verify_bundle(
        &self,
        directory: Arg<ExactPath>,
        profile: Arg<BundleProfile>,
    ) -> napi::Result<Model<BundleManifest>> {
        let path = self.path.clone();
        let directory = self::path(directory);

        return blocking(move || core::verify_bundle(&directory, &path, profile.0))
            .await
            .map(Model);
    }

    /// Verify the session; validity, authenticity, and trust stay separate facts.
    #[napi]
    pub async fn verify(
        &self,
        level: Option<Arg<VerificationLevel>>,
        allowed_signers: Option<Arg<ExactPath>>,
    ) -> napi::Result<Model<VerificationReport>> {
        let level = value(level, VerificationLevel::Content);
        let trust = allowed_signers.map(path);
        let path = self.path.clone();

        return blocking(move || Ok(core::verify(&path, level, trust.as_deref())))
            .await
            .map(Model);
    }

    /// Sign an exact receipt; signing never implies trust.
    #[napi]
    pub async fn sign(
        &self,
        key: Arg<ExactPath>,
        signer: String,
    ) -> napi::Result<Model<ExactPath>> {
        let path = self.path.clone();
        let key = self::path(key);

        return blocking(move || core::sign(&path, &key, &signer))
            .await
            .map(Model);
    }

    /// Recover the valid prefix into a new session directory without changing the source.
    #[napi]
    pub async fn recover(
        &self,
        destination: Arg<ExactPath>,
    ) -> napi::Result<Model<RecoveryReport>> {
        let path = self.path.clone();
        let destination = self::path(destination);

        return blocking(move || core::recover(&path, &destination, false))
            .await
            .map(Model);
    }

    /// Recover the valid prefix of this event log into a new log.
    #[napi]
    pub async fn recover_log(
        &self,
        destination: Arg<ExactPath>,
    ) -> napi::Result<Model<RecoveryReport>> {
        let path = self.path.clone();
        let destination = self::path(destination);

        return blocking(move || core::recover(&path, &destination, true))
            .await
            .map(Model);
    }

    /// Export an asciicast and return its path.
    #[napi]
    pub async fn asciicast(&self, destination: Arg<ExactPath>) -> napi::Result<Model<ExactPath>> {
        let path = self.path.clone();
        let destination = self::path(destination);

        return blocking(move || {
            termtrace_sdk::artifacts::ArtifactService::export_asciicast(&path, &destination)
                .map(|()| return ExactPath(destination))
        })
        .await
        .map(Model);
    }
}

/// Session registry: aliases, lifecycle, retention, and restart.
#[napi(js_name = "Registry")]
pub struct JsRegistry {
    pub(crate) root: Option<PathBuf>,
    pub(crate) binary: PathBuf,
}

#[napi]
impl JsRegistry {
    /// Open an explicit or the default registry lazily.
    #[napi(constructor)]
    pub fn new(root: Option<Arg<ExactPath>>, binary: Option<Arg<ExactPath>>) -> Self {
        return Self {
            root: root.map(path),
            binary: binary.map_or_else(default_binary, path),
        };
    }

    /// Restart through shared readiness, lineage, and alias compare-and-swap.
    #[napi]
    pub async fn restart(
        &self,
        alias: String,
        options: Option<Arg<RestartConfiguration>>,
        environment: Option<Arg<EnvironmentSpec>>,
        timeout_ms: Option<f64>,
    ) -> napi::Result<Model<RestartReport>> {
        let options = options.map(|options| return options.0).unwrap_or_default();
        let environment = environment.map(|environment| return environment.0);
        let timeout = timeout(timeout_ms, termtrace_sdk::defaults::STARTUP_TIMEOUT)?;
        let root = self.root.clone();
        let binary = self.binary.clone();

        return blocking(move || {
            core::restart(
                &registry(root)?,
                &binary,
                &alias,
                &options,
                environment.as_ref(),
                timeout,
            )
        })
        .await
        .map(Model);
    }

    /// Registered sessions; unhealthy rows keep their diagnostics.
    #[napi]
    pub async fn list(
        &self,
        include_all: Option<bool>,
    ) -> napi::Result<Vec<Model<RegistryListEntry>>> {
        let root = self.root.clone();

        return blocking(move || {
            core::registry_list(&registry(root)?, include_all.unwrap_or(false))
        })
        .await
        .map(|entries| return entries.into_iter().map(Model).collect());
    }

    /// Register a session under an alias.
    #[napi]
    pub async fn register(
        &self,
        alias: String,
        session: Arg<ExactPath>,
    ) -> napi::Result<Model<RegistryEntry>> {
        let root = self.root.clone();
        let session = path(session);

        return blocking(move || core::registry_register(&registry(root)?, &alias, &session))
            .await
            .map(Model);
    }

    /// Resolve an alias; `null` only when no alias record exists.
    #[napi]
    pub async fn resolve(&self, alias: String) -> napi::Result<Option<Model<RegistryEntry>>> {
        let root = self.root.clone();

        return blocking(move || core::registry_resolve(&registry(root)?, &alias))
            .await
            .map(|entry| return entry.map(Model));
    }

    /// Derived lifecycle of an alias, including pending and incompatible states.
    #[napi]
    pub async fn lifecycle(&self, alias: String) -> napi::Result<Model<RegistrySessionResolution>> {
        let root = self.root.clone();

        return blocking(move || core::registry_lifecycle(&registry(root)?, &alias))
            .await
            .map(Model);
    }

    /// Remove an alias; canonical evidence is kept.
    #[napi]
    pub async fn remove(&self, alias: String) -> napi::Result<Model<RegistryEntry>> {
        let root = self.root.clone();

        return blocking(move || core::registry_remove(&registry(root)?, &alias))
            .await
            .map(Model);
    }

    /// Retention planning, applied only when `apply` is true.
    #[napi]
    pub async fn gc(
        &self,
        older_than_days: Option<Arg<u64>>,
        keep_latest: Option<Arg<u64>>,
        max_completed_bytes: Option<Arg<u64>>,
        apply: Option<bool>,
    ) -> napi::Result<Model<GarbageCollectionReport>> {
        let root = self.root.clone();

        return blocking(move || {
            core::registry_gc(
                &registry(root)?,
                value(older_than_days, 30),
                value(keep_latest, 20),
                max_completed_bytes.map(|bytes| return bytes.0),
                apply.unwrap_or(false),
            )
        })
        .await
        .map(Model);
    }
}

/// Aggregate tmux recording through the shared tmux service.
#[napi(js_name = "Tmux")]
pub struct JsTmux {
    root: Option<PathBuf>,
    binary: PathBuf,
    client: TmuxClient,
}

impl JsTmux {
    fn tracks(&self) -> napi::Result<ServiceTracks> {
        return ServiceTracks::open(self.binary.clone(), self.root.as_deref()).map_err(to_napi_err);
    }
}

#[napi]
impl JsTmux {
    /// Bind paths lazily; construction starts no tmux server.
    #[napi(constructor)]
    pub fn new(
        root: Option<Arg<ExactPath>>,
        binary: Option<Arg<ExactPath>>,
        tmux: Option<Arg<ExactPath>>,
        socket: Option<Arg<ExactPath>>,
        socket_name: Option<bool>,
    ) -> Self {
        let socket = socket.map_or(TmuxSocket::Default, |socket| {
            let bytes = ByteString::new(path(socket).into_os_string().into_vec());
            if socket_name.unwrap_or(false) {
                return TmuxSocket::Name(bytes);
            }

            return TmuxSocket::Path(bytes);
        });

        return Self {
            root: root.map(path),
            binary: binary.map_or_else(default_binary, path),
            client: TmuxClient {
                executable: tmux.map_or_else(|| return PathBuf::from("tmux"), path),
                socket,
            },
        };
    }

    /// Start aggregate recording.
    #[napi]
    pub async fn record(
        &self,
        options: Arg<TmuxRecordConfiguration>,
    ) -> napi::Result<Model<TmuxRecordingStart>> {
        let tracks = self.tracks()?;
        let client = self.client.clone();

        return blocking(move || core::tmux_record(&tracks, client, options.0))
            .await
            .map(Model);
    }

    /// Server and integration status.
    #[napi]
    pub async fn status(&self) -> napi::Result<Model<TmuxStatusReport>> {
        let client = self.client.clone();

        return blocking(move || core::tmux_status(&client))
            .await
            .map(Model);
    }

    /// Lifecycle of one aggregate recording.
    #[napi]
    pub async fn recording_status(
        &self,
        name: String,
    ) -> napi::Result<Model<TmuxRecordingManifest>> {
        let tracks = self.tracks()?;

        return blocking(move || core::tmux_recording(tracks.state_root(), &name))
            .await
            .map(Model);
    }

    /// Register one pane session as a track of an aggregate recording.
    #[napi]
    pub async fn track(
        &self,
        manifest: Arg<ExactPath>,
        pane: String,
        session: Arg<ExactPath>,
    ) -> napi::Result<()> {
        let manifest = path(manifest);
        let session = path(session);

        return blocking(move || TmuxService::track(&manifest, &pane, &session)).await;
    }

    /// Stop the canonical tracks of one aggregate recording.
    #[napi]
    pub async fn stop(
        &self,
        name: String,
        wait: Option<bool>,
        timeout_ms: Option<f64>,
    ) -> napi::Result<Model<TmuxRecordingManifest>> {
        let timeout = timeout(timeout_ms, termtrace_sdk::defaults::WAIT_TIMEOUT)?;
        let tracks = self.tracks()?;

        return blocking(move || core::tmux_stop(&tracks, &name, wait.unwrap_or(true), timeout))
            .await
            .map(Model);
    }

    /// One aggregate history page.
    #[napi]
    pub async fn events(
        &self,
        name: String,
        from_sequence: Option<Arg<u64>>,
        limit: Option<Arg<u16>>,
    ) -> napi::Result<Model<TmuxEvents>> {
        let tracks = self.tracks()?;

        return blocking(move || {
            core::tmux_events(
                tracks.state_root(),
                &name,
                value(from_sequence, 1),
                value(limit, 100),
            )
        })
        .await
        .map(Model);
    }

    /// One track or coverage-gap catalog page.
    #[napi]
    pub async fn catalog(
        &self,
        name: String,
        gaps: Option<bool>,
        from_sequence: Option<Arg<u64>>,
        limit: Option<Arg<u16>>,
        active_only: Option<bool>,
    ) -> napi::Result<Model<TmuxCatalog>> {
        let tracks = self.tracks()?;

        return blocking(move || {
            core::tmux_catalog(
                tracks.state_root(),
                &name,
                gaps.unwrap_or(false),
                value(from_sequence, 1),
                value(limit, 100),
                active_only.unwrap_or(false),
            )
        })
        .await
        .map(Model);
    }

    /// Bundle an aggregate recording.
    #[napi]
    pub async fn bundle(
        &self,
        manifest: Arg<ExactPath>,
        destination: Arg<ExactPath>,
    ) -> napi::Result<Model<TmuxStreamingBundleManifest>> {
        let manifest = path(manifest);
        let destination = path(destination);

        return blocking(move || core::tmux_bundle(&manifest, &destination, false))
            .await
            .map(Model);
    }

    /// Verify an aggregate bundle against its manifest.
    #[napi]
    pub async fn verify_bundle(
        &self,
        manifest: Arg<ExactPath>,
        destination: Arg<ExactPath>,
    ) -> napi::Result<Model<TmuxStreamingBundleManifest>> {
        let manifest = path(manifest);
        let destination = path(destination);

        return blocking(move || core::tmux_bundle(&manifest, &destination, true))
            .await
            .map(Model);
    }
}

type Attributes = BTreeMap<String, serde_json::Value>;

fn span_id(text: &str) -> napi::Result<Uuid> {
    return Uuid::parse_str(text).map_err(|error| return argument(error.to_string()));
}

/// Application instrumentation; spans end only when the caller ends them.
#[napi(js_name = "Instrumentation")]
pub struct JsInstrumentation {
    inner: Arc<Instrumentation>,
}

#[napi]
impl JsInstrumentation {
    /// Create an emitter bound to one live session.
    #[napi(constructor)]
    pub fn new(source: Arg<ExactPath>) -> Self {
        return Self {
            inner: Arc::new(Instrumentation::new(termtrace_sdk::Client::new(path(
                source,
            )))),
        };
    }

    /// Process identity of every event and span of this emitter.
    #[napi(getter)]
    pub fn process_instance_id(&self) -> String {
        return self.inner.process_instance_id().to_string();
    }

    /// Emit one event and resolve to the actor acknowledgement.
    #[napi]
    pub async fn event(
        &self,
        name: String,
        attributes: Option<Arg<Attributes>>,
    ) -> napi::Result<Model<ApplicationAcknowledgement>> {
        let attributes = value(attributes, Attributes::new());
        let emitter = Arc::clone(&self.inner);

        return blocking(move || emitter.event_ack(&name, attributes).map_err(Into::into))
            .await
            .map(Model);
    }

    /// Start a span; dropping this object never ends it.
    #[napi]
    pub async fn start_span(
        &self,
        name: String,
        parent_span_id: Option<String>,
        attributes: Option<Arg<Attributes>>,
    ) -> napi::Result<Model<ApplicationAcknowledgement>> {
        let attributes = value(attributes, Attributes::new());
        let parent = parent_span_id.as_deref().map(span_id).transpose()?;
        let emitter = Arc::clone(&self.inner);

        return blocking(move || {
            emitter
                .start_span_ack(&name, parent, attributes)
                .map_err(Into::into)
        })
        .await
        .map(Model);
    }

    /// End one span.
    #[napi]
    pub async fn end_span(
        &self,
        name: String,
        span_id: String,
        attributes: Option<Arg<Attributes>>,
    ) -> napi::Result<Model<ApplicationAcknowledgement>> {
        let attributes = value(attributes, Attributes::new());
        let span = self::span_id(&span_id)?;
        let emitter = Arc::clone(&self.inner);

        return blocking(move || {
            emitter
                .end_span_ack(&name, span, attributes)
                .map_err(Into::into)
        })
        .await
        .map(Model);
    }
}

/// System capability probes, versions, and the public schema catalog.
#[napi(js_name = "System")]
#[derive(Default)]
pub struct JsSystem;

#[napi]
impl JsSystem {
    /// Bind the local system service without starting a probe.
    #[napi(constructor)]
    pub fn new() -> Self {
        return Self;
    }

    /// Probe capabilities; unavailable tools are reported as blocked, never skipped.
    #[napi]
    pub async fn capabilities(&self) -> napi::Result<Model<CapabilityReport>> {
        return blocking(|| {
            Ok(SystemService::probe_capabilities(
                &std::sync::atomic::AtomicBool::new(false),
            ))
        })
        .await
        .map(Model);
    }

    /// Build and component versions.
    #[napi]
    pub fn versions(&self) -> Model<Versions> {
        return Model(SystemService::versions());
    }

    /// Names in the public schema catalog.
    #[napi]
    pub async fn schemas(&self) -> napi::Result<Vec<String>> {
        return blocking(SystemService::schemas).await;
    }

    /// One public JSON Schema.
    #[napi]
    pub async fn schema(&self, name: String) -> napi::Result<Model<serde_json::Value>> {
        return blocking(move || SystemService::schema(&name))
            .await
            .map(Model);
    }
}
