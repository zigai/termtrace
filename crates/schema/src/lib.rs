//! Deterministic JSON Schema catalog for Termtrace public contracts.

#![deny(unsafe_code)]

use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    result::Result as StdResult,
};

use schemars::{schema_for, JsonSchema};
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

macro_rules! registry {
    ($(($filename:literal, $($generator:tt)+)),+ $(,)?) => {
        /// Complete, ordered public schema registry.
        pub static SCHEMA_REGISTRY: &[SchemaEntry] = &[
            $(SchemaEntry {
                filename: $filename,
                generator: schema_generator!($($generator)+),
            }),+
        ];

        /// Complete, ordered list of public schema filenames.
        pub static PUBLIC_SCHEMA_FILENAMES: &[&str] = &[$($filename),+];
    };
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "SCHEMA_REGISTRY requires the public fallible SchemaGenerator function pointer signature"
)]
fn generate_acceptance_benchmark_schema(filename: &'static str) -> Result<GeneratedSchema> {
    return Ok(GeneratedSchema {
        filename,
        json: include_bytes!("../../../schemas/acceptance-benchmark.schema.json").to_vec(),
    });
}

registry!(
    ("event-log-header.schema.json",
        event::EventLogHeader
    ),
    ("event-record.schema.json",
        event::EventRecord
    ),
    ("event-projection.schema.json",
        event::EventProjection
    ),
    ("capability-report.schema.json",
        capability::CapabilityReport
    ),
    ("save-manifest.schema.json",
        save::SaveManifest
    ),
    ("screenshot-batch-manifest.schema.json",
        save::ScreenshotBatchManifest
    ),
    ("scenario.schema.json",
        scenario::Scenario
    ),
    ("scenario-report.schema.json",
        scenario::ScenarioReport
    ),
    ("scenario-frame-snapshot.schema.json",
        scenario::ScenarioFrameSnapshot
    ),
    ("scenario-suite-report.schema.json",
        scenario::ScenarioSuiteReport
    ),
    ("evidence-export-plan.schema.json",
        model_bundle::BundlePlan
    ),
    ("evidence-export-manifest.schema.json",
        model_bundle::BundleManifest
    ),
    ("control-request.schema.json",
        control::ControlRequest
    ),
    ("control-response.schema.json",
        control::ControlResponse
    ),
    ("driver-request.schema.json",
        driver::DriverRequest
    ),
    ("driver-response.schema.json",
        driver::DriverResponse
    ),
    ("environment-spec.schema.json",
        environment::EnvironmentSpec
    ),
    ("session.schema.json",
        session::SessionManifest
    ),
    ("completion.schema.json",
        session::SessionCompletion
    ),
    ("verification-report.schema.json",
        session::VerificationReport
    ),
    ("artifact-root-record.schema.json",
        artifact::ArtifactRootRecord
    ),
    ("mutation-journal-record.schema.json",
        request_journal::MutationJournalRecord
    ),
    ("provenance-policy.schema.json",
        provenance::ProvenancePolicy
    ),
    ("environment-provenance.schema.json",
        provenance::EnvironmentProvenance
    ),
    ("execution-provenance.schema.json",
        provenance::ExecutionProvenance
    ),
    ("evidence-receipt.schema.json",
        receipt::EvidenceReceipt
    ),
    ("registry-entry.schema.json",
        model_registry::RegistryEntry
    ),
    ("registry-list-entry.schema.json",
        model_registry::RegistryListEntry
    ),
    ("state-record.schema.json",
        replay::StateRecord
    ),
    ("state-index-entry.schema.json",
        replay::StateIndexEntry
    ),
    ("state-inspection.schema.json",
        replay::StateInspection
    ),
    ("replay-manifest.schema.json",
        replay::ReplayManifest
    ),
    ("replay-endpoint.schema.json",
        replay::ReplayEndpoint
    ),
    ("semantic-replay-checkpoint.schema.json",
        replay::SemanticReplayCheckpoint
    ),
    ("review-summary.schema.json",
        reviewer::ReviewSummary
    ),
    ("reviewer-capabilities.schema.json",
        reviewer::ReviewerCapabilities
    ),
    ("review-search-result.schema.json",
        reviewer::ReviewSearchResult
    ),
    ("review-export-manifest.schema.json",
        reviewer::ReviewExportManifest
    ),
    ("visual-manifest.schema.json",
        visual::VisualManifest
    ),
    ("geometry-solve-report.schema.json",
        visual::GeometrySolveReport
    ),
    ("video-map-entry.schema.json",
        visual::VideoMapEntry
    ),
    ("recovery-report.schema.json",
        event_log::RecoveryReport
    ),
    ("wait-outcome.schema.json",
        inspection::WaitOutcome
    ),
    ("screen-wait-outcome.schema.json",
        inspection::ScreenWaitOutcome
    ),
    ("observation.schema.json",
        observation::Observation
    ),
    ("frame-envelope.schema.json",
        observation::FrameEnvelope
    ),
    ("screen-frame.schema.json",
        screen::ScreenFrame
    ),
    ("screen-projection-request.schema.json",
        screen::ScreenProjectionRequest
    ),
    ("replay-checkpoint.schema.json",
        observation::ReplayCheckpoint
    ),
    ("output-projection-report.schema.json",
        output_projection::OutputProjectionReport
    ),
    ("timeline-entry.schema.json",
        timeline::TimelineEntry
    ),
    ("semantic-diff.schema.json",
        diff::SemanticDiff
    ),
    ("vt-token.schema.json",
        vt::VtToken
    ),
    ("explain-report.schema.json",
        analysis::ExplainReport
    ),
    ("diagnose-report.schema.json",
        analysis::DiagnoseReport
    ),
    ("playback-timing.schema.json",
        playback::PlaybackTiming
    ),
    ("playback-failure.schema.json",
        playback::PlaybackFailure
    ),
    ("edit-plan.schema.json",
        presentation::EditPlan
    ),
    ("presentation-plan.schema.json",
        presentation::PresentationPlan
    ),
    ("presentation-manifest.schema.json",
        presentation::PresentationManifest
    ),
    ("presentation-map-entry.schema.json",
        presentation::PresentationMapEntry
    ),
    ("presentation-preview-manifest.schema.json",
        presentation::PresentationPreviewManifest
    ),
    ("marker-record.schema.json",
        presentation::MarkerRecord
    ),
    ("semantic-video-source-plan.schema.json",
        semantic_video::SemanticVideoSourcePlan
    ),
    ("semantic-frame-span.schema.json",
        semantic_video::SemanticFrameSpan
    ),
    ("semantic-video-source-manifest.schema.json",
        semantic_video::SemanticVideoSourceManifest
    ),
    ("tmux-recording-manifest.schema.json",
        tmux::TmuxRecordingManifest
    ),
    ("tmux-bundle-manifest.schema.json",
        tmux::TmuxBundleManifest
    ),
    ("tmux-streaming-bundle-manifest.schema.json",
        tmux::TmuxStreamingBundleManifest
    ),
    ("tmux-track-catalog-entry.schema.json",
        tmux::TmuxTrackCatalogEntry
    ),
    ("tmux-gap-catalog-entry.schema.json",
        tmux::TmuxGapCatalogEntry
    ),
    ("acceptance-benchmark.schema.json",
        manual generate_acceptance_benchmark_schema
    ),
    ("cli-response.schema.json",
        cli_response::CliResponseEnvelope
    ),
);

/// Return the complete ordered catalog after checking registry invariants.
pub fn catalog() -> Result<&'static [SchemaEntry]> {
    validate_registry()?;

    return Ok(SCHEMA_REGISTRY);
}

/// Validate filename uniqueness for the complete registry.
pub fn validate_registry() -> Result<()> {
    let mut filenames: BTreeSet<&'static str> = BTreeSet::new();
    for entry in SCHEMA_REGISTRY {
        if !filenames.insert(entry.filename) {
            return Err(Error::DuplicateFilename {
                filename: entry.filename,
            });
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
