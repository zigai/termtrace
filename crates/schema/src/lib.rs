//! Deterministic JSON Schema catalog for Termtrace public contracts.

#![deny(unsafe_code)]

use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    result::Result as StdResult,
};

use schemars::{JsonSchema, schema_for};
use termtrace_model::{
    artifact::{
        bundle as model_bundle, metadata as artifact, playback, presentation, save, semantic_video,
        tmux, visual,
    },
    execution::{environment, event, event_log, provenance, receipt, request_journal, session},
    protocol::{capability, cli_response, control, driver, output_projection},
    registry as model_registry, scenario,
    terminal::{analysis, diff, inspection, observation, replay, reviewer, screen, timeline, vt},
};

/// Result type for schema catalog operations.
pub type Result<T, E = Error> = StdResult<T, E>;

/// Schema catalog or generation failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Two contracts resolve to the same stable filename.
    #[error("duplicate schema filename in registry: {filename}")]
    DuplicateFilename {
        /// Duplicated repository filename.
        filename: &'static str,
    },
    /// The stable public inventory names a contract absent from the registry.
    #[error("public schema inventory is missing registry entry: {filename}")]
    MissingRegistryEntry {
        /// Expected stable filename.
        filename: &'static str,
    },
    /// The registry contains a contract absent from the stable public inventory.
    #[error("schema registry contains an unrecognized entry: {filename}")]
    UnexpectedRegistryEntry {
        /// Unexpected stable filename.
        filename: &'static str,
    },
    /// A caller requested a filename absent from the complete registry.
    #[error("schema is not registered: {filename}")]
    UnknownSchema {
        /// Requested filename.
        filename: String,
    },
    /// Schemars produced a value that cannot be emitted as a schema object.
    #[error("generated schema root for {filename} is not an object")]
    GeneratedRootNotObject {
        /// Registered repository filename.
        filename: &'static str,
    },
    /// A generated schema could not be encoded as JSON.
    #[error("failed to serialize generated schema {filename}: {source}")]
    Serialize {
        /// Registered repository filename.
        filename: &'static str,
        /// JSON encoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// A catalog output path could not be created or written.
    #[error("failed to write schema path {path}: {source}")]
    Filesystem {
        /// Path being created or written.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },
}

/// Crate that owns the serialized contract represented by a schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractOwner {
    /// Shared data contract owned by `termtrace-model`.
    Model,
    /// Root CLI and repository-operated contracts.
    Root,
}

/// Stable family used to inventory and select related schemas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaCategory {
    /// Canonical event-log values.
    EventLog,
    /// Runtime capability reports.
    Capability,
    /// Session save and screenshot publication values.
    Save,
    /// Scenario language and report values.
    Scenario,
    /// Evidence export plans and manifests.
    EvidenceExport,
    /// Live control protocol values.
    ControlProtocol,
    /// SDK driver wire protocol values.
    DriverProtocol,
    /// Child environment contracts.
    Environment,
    /// Recording session lifecycle values.
    Session,
    /// Evidence identity and integrity values.
    Evidence,
    /// Execution provenance values.
    Provenance,
    /// Named-session registry values.
    Registry,
    /// Semantic replay values.
    Replay,
    /// Read-only review values.
    Review,
    /// Native visual evidence values.
    Visual,
    /// Inspection and observation values.
    Observation,
    /// Rich terminal projection values.
    Screen,
    /// Timeline and semantic diff values.
    Analysis,
    /// Native playback values.
    Playback,
    /// Edited presentation values.
    Presentation,
    /// Semantic-video source values.
    SemanticVideo,
    /// Aggregate tmux values.
    Tmux,
    /// Acceptance benchmark configuration.
    AcceptanceBenchmark,
    /// Compact CLI response protocol.
    CliResponse,
}

/// Meaning of a contract version attached to a registry entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionKind {
    /// Version of an on-disk or exported data format.
    Format,
    /// Version of a schema envelope.
    Schema,
    /// Version of a request/response protocol.
    Protocol,
}

/// Version metadata for a schema when its owning contract defines one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContractVersion {
    /// What the numeric version governs.
    pub kind: VersionKind,
    /// Stable format identifier, when the contract defines one.
    pub format: Option<&'static str>,
    /// Current owning-contract version.
    pub value: u32,
}

impl ContractVersion {
    const fn new(kind: VersionKind, format: Option<&'static str>, value: u32) -> Self {
        return Self {
            kind,
            format,
            value,
        };
    }
}
/// Origin of one public schema definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaSource {
    /// Generated from an owning Rust type.
    RustType,
    /// Handwritten contract with bytes embedded into the catalog.
    Manual,
}

/// One schema generated from its owning Rust contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedSchema {
    /// Stable repository filename.
    pub filename: &'static str,
    /// Deterministic pretty JSON with a trailing newline.
    pub json: Vec<u8>,
}

/// Function that generates one registered schema.
pub type SchemaGenerator = fn(&'static str) -> Result<GeneratedSchema>;

/// Explicit metadata and generator for one public schema contract.
#[derive(Clone, Copy)]
pub struct SchemaEntry {
    /// Stable repository filename.
    pub filename: &'static str,
    /// Crate or repository layer that owns the serialized contract.
    pub owner: ContractOwner,
    /// Contract family.
    pub category: SchemaCategory,
    /// Format, schema, or protocol version where one is defined.
    pub version: Option<ContractVersion>,
    /// Rust type that generates the schema, or `None` for a handwritten contract.
    pub rust_type: Option<&'static str>,
    /// Whether the schema is generated from a Rust type or maintained manually.
    pub source: SchemaSource,
    /// Generator for the canonical checked-in bytes.
    pub generator: SchemaGenerator,
}

impl SchemaEntry {
    /// Generate this registry entry without consulting repository files.
    pub fn generate(self) -> Result<GeneratedSchema> {
        return (self.generator)(self.filename);
    }
}

macro_rules! schema_generator {
    (manual $generator:path) => {
        $generator
    };
    ($contract:path) => {
        generate_typed::<$contract>
    };
}

macro_rules! schema_rust_type {
    (manual $generator:path) => {
        None
    };
    ($contract:path) => {
        Some(stringify!($contract))
    };
}

macro_rules! schema_source {
    (manual $generator:path) => {
        SchemaSource::Manual
    };
    ($contract:path) => {
        SchemaSource::RustType
    };
}

macro_rules! registry {
    ($(($filename:literal, $owner:ident, $category:ident, $version:expr, $($generator:tt)+)),+ $(,)?) => {
        /// Complete, ordered public schema registry.
        pub static SCHEMA_REGISTRY: &[SchemaEntry] = &[
            $(SchemaEntry {
                filename: $filename,
                owner: ContractOwner::$owner,
                category: SchemaCategory::$category,
                version: $version,
                rust_type: schema_rust_type!($($generator)+),
                source: schema_source!($($generator)+),
                generator: schema_generator!($($generator)+),
            }),+
        ];
    };
}

const fn format_version(format: &'static str, value: u32) -> Option<ContractVersion> {
    return Some(ContractVersion::new(
        VersionKind::Format,
        Some(format),
        value,
    ));
}

const fn unnamed_format_version(value: u32) -> Option<ContractVersion> {
    return Some(ContractVersion::new(VersionKind::Format, None, value));
}
const fn schema_version(format: Option<&'static str>, value: u32) -> Option<ContractVersion> {
    return Some(ContractVersion::new(VersionKind::Schema, format, value));
}

const fn protocol_version(value: u32) -> Option<ContractVersion> {
    return Some(ContractVersion::new(VersionKind::Protocol, None, value));
}
fn generate_embedded_schema(
    filename: &'static str,
    json: &'static [u8],
) -> Result<GeneratedSchema> {
    return Ok(GeneratedSchema {
        filename,
        json: json.to_vec(),
    });
}

fn generate_acceptance_benchmark_schema(filename: &'static str) -> Result<GeneratedSchema> {
    return generate_embedded_schema(
        filename,
        include_bytes!("../../../schemas/acceptance-benchmark.schema.json"),
    );
}

registry!(
    (
        "event-log-header.schema.json",
        Model,
        EventLog,
        format_version(event::EVENT_LOG_FORMAT, event::EVENT_LOG_VERSION),
        event::EventLogHeader
    ),
    (
        "event-record.schema.json",
        Model,
        EventLog,
        format_version(event::EVENT_LOG_FORMAT, event::EVENT_LOG_VERSION),
        event::EventRecord
    ),
    (
        "event-projection.schema.json",
        Model,
        EventLog,
        format_version(event::EVENT_LOG_FORMAT, event::EVENT_LOG_VERSION),
        event::EventProjection
    ),
    (
        "capability-report.schema.json",
        Model,
        Capability,
        None,
        capability::CapabilityReport
    ),
    (
        "save-manifest.schema.json",
        Model,
        Save,
        None,
        save::SaveManifest
    ),
    (
        "screenshot-batch-manifest.schema.json",
        Model,
        Save,
        None,
        save::ScreenshotBatchManifest
    ),
    (
        "scenario.schema.json",
        Model,
        Scenario,
        schema_version(None, scenario::SCENARIO_VERSION),
        scenario::Scenario
    ),
    (
        "scenario-report.schema.json",
        Model,
        Scenario,
        schema_version(None, scenario::SCENARIO_VERSION),
        scenario::ScenarioReport
    ),
    (
        "scenario-frame-snapshot.schema.json",
        Model,
        Scenario,
        schema_version(None, scenario::SCENARIO_VERSION),
        scenario::ScenarioFrameSnapshot
    ),
    (
        "scenario-suite-report.schema.json",
        Model,
        Scenario,
        schema_version(None, scenario::SCENARIO_VERSION),
        scenario::ScenarioSuiteReport
    ),
    (
        "evidence-export-plan.schema.json",
        Model,
        EvidenceExport,
        format_version(model_bundle::BUNDLE_FORMAT, model_bundle::BUNDLE_VERSION),
        model_bundle::BundlePlan
    ),
    (
        "evidence-export-manifest.schema.json",
        Model,
        EvidenceExport,
        format_version(model_bundle::BUNDLE_FORMAT, model_bundle::BUNDLE_VERSION),
        model_bundle::BundleManifest
    ),
    (
        "control-request.schema.json",
        Model,
        ControlProtocol,
        protocol_version(control::CONTROL_PROTOCOL_VERSION),
        control::ControlRequest
    ),
    (
        "control-response.schema.json",
        Model,
        ControlProtocol,
        protocol_version(control::CONTROL_PROTOCOL_VERSION),
        control::ControlResponse
    ),
    (
        "driver-request.schema.json",
        Model,
        DriverProtocol,
        protocol_version(driver::DRIVER_PROTOCOL_VERSION),
        driver::DriverRequest
    ),
    (
        "driver-response.schema.json",
        Model,
        DriverProtocol,
        protocol_version(driver::DRIVER_PROTOCOL_VERSION),
        driver::DriverResponse
    ),
    (
        "environment-spec.schema.json",
        Model,
        Environment,
        None,
        environment::EnvironmentSpec
    ),
    (
        "session.schema.json",
        Model,
        Session,
        schema_version(
            Some(session::SESSION_MANIFEST_FORMAT),
            session::SESSION_MANIFEST_VERSION
        ),
        session::SessionManifest
    ),
    (
        "completion.schema.json",
        Model,
        Session,
        schema_version(
            Some(session::SESSION_COMPLETION_FORMAT),
            session::SESSION_COMPLETION_VERSION
        ),
        session::SessionCompletion
    ),
    (
        "verification-report.schema.json",
        Model,
        Session,
        schema_version(
            Some(session::VERIFICATION_REPORT_FORMAT),
            session::VERIFICATION_REPORT_VERSION
        ),
        session::VerificationReport
    ),
    (
        "artifact-root-record.schema.json",
        Model,
        Evidence,
        None,
        artifact::ArtifactRootRecord
    ),
    (
        "mutation-journal-record.schema.json",
        Model,
        Evidence,
        schema_version(
            Some(request_journal::MUTATION_JOURNAL_FORMAT),
            request_journal::MUTATION_JOURNAL_VERSION
        ),
        request_journal::MutationJournalRecord
    ),
    (
        "provenance-policy.schema.json",
        Model,
        Provenance,
        None,
        provenance::ProvenancePolicy
    ),
    (
        "environment-provenance.schema.json",
        Model,
        Provenance,
        None,
        provenance::EnvironmentProvenance
    ),
    (
        "execution-provenance.schema.json",
        Model,
        Provenance,
        None,
        provenance::ExecutionProvenance
    ),
    (
        "evidence-receipt.schema.json",
        Model,
        Evidence,
        schema_version(
            Some(receipt::EVIDENCE_RECEIPT_FORMAT),
            receipt::EVIDENCE_RECEIPT_VERSION
        ),
        receipt::EvidenceReceipt
    ),
    (
        "registry-entry.schema.json",
        Model,
        Registry,
        schema_version(
            Some(model_registry::REGISTRY_ENTRY_FORMAT),
            model_registry::REGISTRY_ENTRY_VERSION
        ),
        model_registry::RegistryEntry
    ),
    (
        "registry-list-entry.schema.json",
        Model,
        Registry,
        schema_version(
            Some(model_registry::REGISTRY_ENTRY_FORMAT),
            model_registry::REGISTRY_ENTRY_VERSION
        ),
        model_registry::RegistryListEntry
    ),
    (
        "state-record.schema.json",
        Model,
        Replay,
        unnamed_format_version(replay::STATE_FORMAT_VERSION),
        replay::StateRecord
    ),
    (
        "state-index-entry.schema.json",
        Model,
        Replay,
        unnamed_format_version(replay::STATE_FORMAT_VERSION),
        replay::StateIndexEntry
    ),
    (
        "state-inspection.schema.json",
        Model,
        Replay,
        unnamed_format_version(replay::STATE_FORMAT_VERSION),
        replay::StateInspection
    ),
    (
        "replay-manifest.schema.json",
        Model,
        Replay,
        unnamed_format_version(replay::STATE_FORMAT_VERSION),
        replay::ReplayManifest
    ),
    (
        "semantic-replay-checkpoint.schema.json",
        Model,
        Replay,
        unnamed_format_version(replay::STATE_FORMAT_VERSION),
        replay::SemanticReplayCheckpoint
    ),
    (
        "review-summary.schema.json",
        Model,
        Review,
        None,
        reviewer::ReviewSummary
    ),
    (
        "reviewer-capabilities.schema.json",
        Model,
        Review,
        None,
        reviewer::ReviewerCapabilities
    ),
    (
        "review-search-result.schema.json",
        Model,
        Review,
        None,
        reviewer::ReviewSearchResult
    ),
    (
        "review-export-manifest.schema.json",
        Model,
        Review,
        None,
        reviewer::ReviewExportManifest
    ),
    (
        "visual-manifest.schema.json",
        Model,
        Visual,
        schema_version(None, visual::VISUAL_MANIFEST_VERSION),
        visual::VisualManifest
    ),
    (
        "geometry-solve-report.schema.json",
        Model,
        Visual,
        None,
        visual::GeometrySolveReport
    ),
    (
        "video-map-entry.schema.json",
        Model,
        Visual,
        None,
        visual::VideoMapEntry
    ),
    (
        "recovery-report.schema.json",
        Model,
        EventLog,
        None,
        event_log::RecoveryReport
    ),
    (
        "wait-outcome.schema.json",
        Model,
        Observation,
        None,
        inspection::WaitOutcome
    ),
    (
        "screen-wait-outcome.schema.json",
        Model,
        Observation,
        None,
        inspection::ScreenWaitOutcome
    ),
    (
        "observation.schema.json",
        Model,
        Observation,
        None,
        observation::Observation
    ),
    (
        "frame-envelope.schema.json",
        Model,
        Observation,
        None,
        observation::FrameEnvelope
    ),
    (
        "screen-frame.schema.json",
        Model,
        Screen,
        schema_version(None, screen::SCREEN_FRAME_VERSION),
        screen::ScreenFrame
    ),
    (
        "screen-projection-request.schema.json",
        Model,
        Screen,
        schema_version(None, screen::SCREEN_FRAME_VERSION),
        screen::ScreenProjectionRequest
    ),
    (
        "replay-checkpoint.schema.json",
        Model,
        Observation,
        None,
        observation::ReplayCheckpoint
    ),
    (
        "output-projection-report.schema.json",
        Model,
        Observation,
        None,
        output_projection::OutputProjectionReport
    ),
    (
        "timeline-entry.schema.json",
        Model,
        Analysis,
        None,
        timeline::TimelineEntry
    ),
    (
        "semantic-diff.schema.json",
        Model,
        Analysis,
        None,
        diff::SemanticDiff
    ),
    ("vt-token.schema.json", Model, EventLog, None, vt::VtToken),
    (
        "explain-report.schema.json",
        Model,
        Analysis,
        None,
        analysis::ExplainReport
    ),
    (
        "diagnose-report.schema.json",
        Model,
        Analysis,
        None,
        analysis::DiagnoseReport
    ),
    (
        "playback-timing.schema.json",
        Model,
        Playback,
        None,
        playback::PlaybackTiming
    ),
    (
        "playback-failure.schema.json",
        Model,
        Playback,
        None,
        playback::PlaybackFailure
    ),
    (
        "edit-plan.schema.json",
        Model,
        Presentation,
        schema_version(None, presentation::EDIT_PLAN_VERSION),
        presentation::EditPlan
    ),
    (
        "presentation-plan.schema.json",
        Model,
        Presentation,
        unnamed_format_version(presentation::PRESENTATION_FORMAT_VERSION),
        presentation::PresentationPlan
    ),
    (
        "presentation-manifest.schema.json",
        Model,
        Presentation,
        unnamed_format_version(presentation::PRESENTATION_FORMAT_VERSION),
        presentation::PresentationManifest
    ),
    (
        "presentation-map-entry.schema.json",
        Model,
        Presentation,
        unnamed_format_version(presentation::PRESENTATION_FORMAT_VERSION),
        presentation::PresentationMapEntry
    ),
    (
        "presentation-preview-manifest.schema.json",
        Model,
        Presentation,
        unnamed_format_version(presentation::PRESENTATION_FORMAT_VERSION),
        presentation::PresentationPreviewManifest
    ),
    (
        "marker-record.schema.json",
        Model,
        Presentation,
        unnamed_format_version(presentation::PRESENTATION_FORMAT_VERSION),
        presentation::MarkerRecord
    ),
    (
        "semantic-video-source-plan.schema.json",
        Model,
        SemanticVideo,
        unnamed_format_version(semantic_video::SEMANTIC_VIDEO_VERSION),
        semantic_video::SemanticVideoSourcePlan
    ),
    (
        "semantic-frame-span.schema.json",
        Model,
        SemanticVideo,
        unnamed_format_version(semantic_video::SEMANTIC_VIDEO_VERSION),
        semantic_video::SemanticFrameSpan
    ),
    (
        "semantic-video-source-manifest.schema.json",
        Model,
        SemanticVideo,
        unnamed_format_version(semantic_video::SEMANTIC_VIDEO_VERSION),
        semantic_video::SemanticVideoSourceManifest
    ),
    (
        "tmux-recording-manifest.schema.json",
        Model,
        Tmux,
        format_version(tmux::TMUX_RECORDING_FORMAT, tmux::TMUX_RECORDING_VERSION),
        tmux::TmuxRecordingManifest
    ),
    (
        "tmux-bundle-manifest.schema.json",
        Model,
        Tmux,
        format_version(tmux::TMUX_BUNDLE_FORMAT, tmux::TMUX_BUNDLE_VERSION),
        tmux::TmuxBundleManifest
    ),
    (
        "acceptance-benchmark.schema.json",
        Root,
        AcceptanceBenchmark,
        format_version("termtrace-acceptance-benchmark", 1),
        manual generate_acceptance_benchmark_schema
    ),
    (
        "cli-response.schema.json",
        Model,
        CliResponse,
        protocol_version(cli_response::CLI_RESPONSE_VERSION),
        cli_response::CliResponseEnvelope
    ),
);
/// Stable filenames that constitute the complete public generated-schema inventory.
pub static PUBLIC_SCHEMA_FILENAMES: &[&str] = &[
    "event-log-header.schema.json",
    "event-record.schema.json",
    "event-projection.schema.json",
    "capability-report.schema.json",
    "save-manifest.schema.json",
    "screenshot-batch-manifest.schema.json",
    "scenario.schema.json",
    "scenario-report.schema.json",
    "scenario-frame-snapshot.schema.json",
    "scenario-suite-report.schema.json",
    "evidence-export-plan.schema.json",
    "evidence-export-manifest.schema.json",
    "control-request.schema.json",
    "control-response.schema.json",
    "driver-request.schema.json",
    "driver-response.schema.json",
    "environment-spec.schema.json",
    "session.schema.json",
    "completion.schema.json",
    "verification-report.schema.json",
    "artifact-root-record.schema.json",
    "mutation-journal-record.schema.json",
    "provenance-policy.schema.json",
    "environment-provenance.schema.json",
    "execution-provenance.schema.json",
    "evidence-receipt.schema.json",
    "registry-entry.schema.json",
    "registry-list-entry.schema.json",
    "state-record.schema.json",
    "state-index-entry.schema.json",
    "state-inspection.schema.json",
    "replay-manifest.schema.json",
    "semantic-replay-checkpoint.schema.json",
    "review-summary.schema.json",
    "reviewer-capabilities.schema.json",
    "review-search-result.schema.json",
    "review-export-manifest.schema.json",
    "visual-manifest.schema.json",
    "geometry-solve-report.schema.json",
    "video-map-entry.schema.json",
    "recovery-report.schema.json",
    "wait-outcome.schema.json",
    "screen-wait-outcome.schema.json",
    "observation.schema.json",
    "frame-envelope.schema.json",
    "screen-frame.schema.json",
    "screen-projection-request.schema.json",
    "replay-checkpoint.schema.json",
    "output-projection-report.schema.json",
    "timeline-entry.schema.json",
    "semantic-diff.schema.json",
    "vt-token.schema.json",
    "explain-report.schema.json",
    "diagnose-report.schema.json",
    "playback-timing.schema.json",
    "playback-failure.schema.json",
    "edit-plan.schema.json",
    "presentation-plan.schema.json",
    "presentation-manifest.schema.json",
    "presentation-map-entry.schema.json",
    "presentation-preview-manifest.schema.json",
    "marker-record.schema.json",
    "semantic-video-source-plan.schema.json",
    "semantic-frame-span.schema.json",
    "semantic-video-source-manifest.schema.json",
    "tmux-recording-manifest.schema.json",
    "tmux-bundle-manifest.schema.json",
    "acceptance-benchmark.schema.json",
    "cli-response.schema.json",
];

/// Return the complete ordered catalog after checking registry invariants.
pub fn catalog() -> Result<&'static [SchemaEntry]> {
    validate_registry()?;
    return Ok(SCHEMA_REGISTRY);
}

/// Validate uniqueness and filename stability for the complete registry.
pub fn validate_registry() -> Result<()> {
    let mut filenames: BTreeSet<&'static str> = BTreeSet::new();
    for entry in SCHEMA_REGISTRY {
        if !filenames.insert(entry.filename) {
            return Err(Error::DuplicateFilename {
                filename: entry.filename,
            });
        }
    }
    for &filename in PUBLIC_SCHEMA_FILENAMES {
        if !filenames.contains(filename) {
            return Err(Error::MissingRegistryEntry { filename });
        }
    }
    let expected: BTreeSet<&'static str> = PUBLIC_SCHEMA_FILENAMES.iter().copied().collect();
    for filename in filenames {
        if !expected.contains(filename) {
            return Err(Error::UnexpectedRegistryEntry { filename });
        }
    }
    return Ok(());
}

/// Generate every public schema in stable registry order.
pub fn generate() -> Result<Vec<GeneratedSchema>> {
    return catalog()?
        .iter()
        .copied()
        .map(SchemaEntry::generate)
        .collect();
}

/// Generate one public schema by its stable filename.
pub fn generate_named(filename: &str) -> Result<GeneratedSchema> {
    let entry = catalog()?
        .iter()
        .find(|entry| return entry.filename == filename)
        .copied();
    let Some(entry) = entry else {
        return Err(Error::UnknownSchema {
            filename: filename.to_owned(),
        });
    };
    return entry.generate();
}

/// Write all generated schemas to a directory in stable registry order.
pub fn write_all(directory: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let directory = directory.as_ref();
    fs_err::create_dir_all(directory).map_err(|source| {
        return Error::Filesystem {
            path: directory.to_path_buf(),
            source,
        };
    })?;
    return generate()?
        .into_iter()
        .map(|schema| {
            let path = directory.join(schema.filename);
            fs_err::write(&path, schema.json).map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;
            return Ok(path);
        })
        .collect();
}

fn generate_typed<T: JsonSchema>(filename: &'static str) -> Result<GeneratedSchema> {
    let mut value = serde_json::to_value(schema_for!(T))
        .map_err(|source| return Error::Serialize { filename, source })?;
    let Some(root) = value.as_object_mut() else {
        return Err(Error::GeneratedRootNotObject { filename });
    };
    root.insert(
        "$id".to_owned(),
        serde_json::Value::String(format!(
            "https://github.com/zigai/termtrace/schemas/{filename}"
        )),
    );
    let mut json = serde_json::to_vec_pretty(&value)
        .map_err(|source| return Error::Serialize { filename, source })?;
    json.push(b'\n');
    return Ok(GeneratedSchema { filename, json });
}

#[cfg(test)]
mod tests;
