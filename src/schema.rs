//! Deterministic JSON Schema generation for every public artifact envelope.

use std::{
    io,
    path::{Path, PathBuf},
};

use schemars::{JsonSchema, schema_for};

#[cfg(test)]
use jsonschema::meta::is_valid;

use crate::{
    Error, Result,
    analysis::{DiagnoseReport, ExplainReport},
    artifact::ArtifactRootRecord,
    bundle::{BundleManifest, BundlePlan},
    capability::CapabilityReport,
    control::{ControlRequest, ControlResponse},
    diff::SemanticDiff,
    driver::{DriverRequest, DriverResponse},
    environment::EnvironmentSpec,
    event::{EventLogHeader, EventProjection, EventRecord},
    event_log::RecoveryReport,
    inspection::{ScreenWaitOutcome, WaitOutcome},
    observation::{FrameEnvelope, Observation, ReplayCheckpoint},
    output_projection::OutputProjectionReport,
    playback::{PlaybackFailure, PlaybackTiming},
    presentation::{
        EditPlan, MarkerRecord, PresentationManifest, PresentationMapEntry, PresentationPlan,
        PresentationPreviewManifest,
    },
    provenance::{EnvironmentProvenance, ExecutionProvenance, ProvenancePolicy},
    receipt::EvidenceReceipt,
    replay::{ReplayManifest, StateIndexEntry, StateInspection, StateRecord},
    request_journal::MutationJournalRecord,
    reviewer::{ReviewExportManifest, ReviewSearchResult, ReviewSummary, ReviewerCapabilities},
    save::{SaveManifest, ScreenshotBatchManifest},
    scenario::{Scenario, ScenarioFrameSnapshot, ScenarioReport, ScenarioSuiteReport},
    semantic_video::{SemanticFrameSpan, SemanticVideoSourceManifest, SemanticVideoSourcePlan},
    session::{SessionCompletion, SessionManifest, VerificationReport},
    session_registry::{RegistryEntry, RegistryListEntry},
    timeline::TimelineEntry,
    visual::{GeometrySolveReport, VideoMapEntry, VisualManifest},
    vt_framer::VtToken,
};

/// One generated schema file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedSchema {
    /// Repository-relative filename.
    pub filename: &'static str,
    /// Pretty JSON with a trailing newline.
    pub json: Vec<u8>,
}

/// Generate all stable public JSON schemas.
///
/// # Errors
///
/// Returns an error only if an in-memory schema cannot serialize as JSON.
pub fn generate() -> Result<Vec<GeneratedSchema>> {
    return Ok(vec![
        schema::<EventLogHeader>("event-log-header.schema.json")?,
        schema::<EventRecord>("event-record.schema.json")?,
        schema::<EventProjection>("event-projection.schema.json")?,
        schema::<CapabilityReport>("capability-report.schema.json")?,
        schema::<SaveManifest>("save-manifest.schema.json")?,
        schema::<ScreenshotBatchManifest>("screenshot-batch-manifest.schema.json")?,
        schema::<Scenario>("scenario.schema.json")?,
        schema::<ScenarioReport>("scenario-report.schema.json")?,
        schema::<ScenarioFrameSnapshot>("scenario-frame-snapshot.schema.json")?,
        schema::<ScenarioSuiteReport>("scenario-suite-report.schema.json")?,
        schema::<BundlePlan>("bundle-plan.schema.json")?,
        schema::<BundleManifest>("bundle-manifest.schema.json")?,
        schema::<ControlRequest>("control-request.schema.json")?,
        schema::<ControlResponse>("control-response.schema.json")?,
        schema::<DriverRequest>("driver-request.schema.json")?,
        schema::<DriverResponse>("driver-response.schema.json")?,
        schema::<EnvironmentSpec>("environment-spec.schema.json")?,
        schema::<SessionManifest>("session.schema.json")?,
        schema::<SessionCompletion>("completion.schema.json")?,
        schema::<VerificationReport>("verification-report.schema.json")?,
        schema::<ArtifactRootRecord>("artifact-root-record.schema.json")?,
        schema::<MutationJournalRecord>("mutation-journal-record.schema.json")?,
        schema::<ProvenancePolicy>("provenance-policy.schema.json")?,
        schema::<EnvironmentProvenance>("environment-provenance.schema.json")?,
        schema::<ExecutionProvenance>("execution-provenance.schema.json")?,
        schema::<EvidenceReceipt>("evidence-receipt.schema.json")?,
        schema::<RegistryEntry>("registry-entry.schema.json")?,
        schema::<RegistryListEntry>("registry-list-entry.schema.json")?,
        schema::<StateRecord>("state-record.schema.json")?,
        schema::<StateIndexEntry>("state-index-entry.schema.json")?,
        schema::<StateInspection>("state-inspection.schema.json")?,
        schema::<ReplayManifest>("replay-manifest.schema.json")?,
        schema::<ReviewSummary>("review-summary.schema.json")?,
        schema::<ReviewerCapabilities>("reviewer-capabilities.schema.json")?,
        schema::<ReviewSearchResult>("review-search-result.schema.json")?,
        schema::<ReviewExportManifest>("review-export-manifest.schema.json")?,
        schema::<VisualManifest>("visual-manifest.schema.json")?,
        schema::<GeometrySolveReport>("geometry-solve-report.schema.json")?,
        schema::<VideoMapEntry>("video-map-entry.schema.json")?,
        schema::<RecoveryReport>("recovery-report.schema.json")?,
        schema::<WaitOutcome>("wait-outcome.schema.json")?,
        schema::<ScreenWaitOutcome>("screen-wait-outcome.schema.json")?,
        schema::<Observation>("observation.schema.json")?,
        schema::<FrameEnvelope>("frame-envelope.schema.json")?,
        schema::<ReplayCheckpoint>("replay-checkpoint.schema.json")?,
        schema::<OutputProjectionReport>("output-projection-report.schema.json")?,
        schema::<TimelineEntry>("timeline-entry.schema.json")?,
        schema::<SemanticDiff>("semantic-diff.schema.json")?,
        schema::<VtToken>("vt-token.schema.json")?,
        schema::<ExplainReport>("explain-report.schema.json")?,
        schema::<DiagnoseReport>("diagnose-report.schema.json")?,
        schema::<PlaybackTiming>("playback-timing.schema.json")?,
        schema::<PlaybackFailure>("playback-failure.schema.json")?,
        schema::<EditPlan>("edit-plan.schema.json")?,
        schema::<PresentationPlan>("presentation-plan.schema.json")?,
        schema::<PresentationManifest>("presentation-manifest.schema.json")?,
        schema::<PresentationMapEntry>("presentation-map-entry.schema.json")?,
        schema::<PresentationPreviewManifest>("presentation-preview-manifest.schema.json")?,
        schema::<MarkerRecord>("marker-record.schema.json")?,
        schema::<SemanticVideoSourcePlan>("semantic-video-source-plan.schema.json")?,
        schema::<SemanticFrameSpan>("semantic-frame-span.schema.json")?,
        schema::<SemanticVideoSourceManifest>("semantic-video-source-manifest.schema.json")?,
    ]);
}

/// Write all schemas to a directory, replacing generated files by name.
///
/// # Errors
///
/// Returns an error when the directory or any schema file cannot be written.
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

fn schema<T: JsonSchema>(filename: &'static str) -> Result<GeneratedSchema> {
    let mut value = serde_json::to_value(schema_for!(T)).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from(filename),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    let Some(root) = value.as_object_mut() else {
        return Err(Error::InvalidJson {
            path: PathBuf::from(filename),
            json_path: "$".to_owned(),
            source: serde_json::Error::io(io::Error::other(
                "generated schema root is not an object",
            )),
        });
    };
    root.insert(
        "$id".to_owned(),
        serde_json::Value::String(format!(
            "https://github.com/zigai/termtrace/schemas/{filename}"
        )),
    );
    let mut json = serde_json::to_vec_pretty(&value).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from(filename),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    json.push(b'\n');
    return Ok(GeneratedSchema { filename, json });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_schemas_match_generated_types() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas");
        for generated in generate().expect("generate schemas") {
            let path = directory.join(generated.filename);
            let schema: serde_json::Value =
                serde_json::from_slice(&generated.json).expect("schema JSON");
            assert!(
                is_valid(&schema),
                "invalid JSON Schema {}",
                generated.filename
            );
            assert_eq!(
                fs_err::read(&path).unwrap_or_else(|error| {
                    panic!("read checked-in schema {}: {error}", path.display())
                }),
                generated.json,
                "regenerate schemas with `termtrace tooling schemas --out schemas`"
            );
        }
    }

    #[test]
    fn shared_driver_response_fixtures_match_the_rust_contract() {
        let schema = serde_json::to_value(schema_for!(DriverResponse)).expect("driver schema");
        let validator = jsonschema::validator_for(&schema).expect("driver validator");
        let fixtures: serde_json::Value =
            serde_json::from_slice(include_bytes!("../fixtures/protocol/driver-responses.json"))
                .expect("shared protocol fixtures");
        let valid = fixtures
            .get("valid")
            .and_then(serde_json::Value::as_array)
            .expect("valid fixture array");
        let invalid = fixtures
            .get("invalid")
            .and_then(serde_json::Value::as_array)
            .expect("invalid fixture array");
        for case in valid {
            let response = case.get("response").expect("valid fixture response");
            assert!(
                validator.is_valid(response),
                "shared valid response fixture failed: {}",
                case.get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unnamed")
            );
        }
        for case in invalid {
            let response = case.get("response").expect("invalid fixture response");
            assert!(
                !validator.is_valid(response),
                "shared invalid response fixture passed: {}",
                case.get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unnamed")
            );
        }
    }
}
