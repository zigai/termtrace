//! Catalog of every model type the native SDKs return or accept.
//!
//! The catalog is one JSON Schema document whose `$defs` hold each listed type under its
//! schema name, plus every type those reference, as accepted on input. `serialized` holds the
//! same definitions as produced on output, where every optional field is present. Python and
//! TypeScript declarations are generated from it, so a binding method can name any listed type.

pub mod declarations;

use super::{
    control::{
        CommandOutcome, ErrorReport, LaunchOptions, OriginalLaunchOptions, PostconditionSummary,
        SendSpec, StopReport,
    },
    diagnostics::{TmuxCatalog, TmuxEvents},
};
use crate::{
    actions::ActOutcome,
    artifacts::{EditSource, VideoOptions, VideoPreview, VideoRender},
    assertions::{ExpectationMismatchError, MaskedSnapshotOptions},
    client::{
        ControlAcknowledgement, MarkOutcome, OriginalRecording, PointerOutcome, SessionStatus,
    },
    locator::{
        LocatorCondition, LocatorFailure, MatchRange, MatchedCell, QueryDescription, QueryError,
        StyleFilter,
    },
    screen::{ScreenWaitResult, StableScreenCapture},
    system::Versions,
    testing::{EvidencePolicy, FinalizationReport, TestOutcome},
};
use schemars::{generate::SchemaSettings, JsonSchema, SchemaGenerator};
use serde_json::{Map, Value};
use termtrace_model::{
    artifact::{
        bundle::{BundleManifest, BundleProfile},
        original::OriginalCaptureStatus,
        presentation::{
            EditFailure, EditPlan, MarkerRecord, NormalizedEdit, OutputFrameSource,
            PresentationPlan,
        },
        save::{SaveFormat, SaveOutcome},
        sdk::MediaArtifactResult,
        visual::CaptureOutcome,
    },
    execution::{
        application::ApplicationAcknowledgement,
        environment::EnvironmentSpec,
        event_log::RecoveryReport,
        paging::{
            ByteOutputPage, EventFollowPosition, EventPageCursor, OutputPageCursor,
            ProjectedEventPage,
        },
        session::{SessionCompletion, VerificationLevel, VerificationReport},
        tmux::TmuxRecordConfiguration,
        visual_profile::{VisualProfile, VisualProfileFailure, VisualProfileReport},
    },
    protocol::{
        capability::CapabilityReport,
        control::{ActionInputResult, ControlErrorDetails, ProcessSignal, ScreenPredicate},
        management::ManagementFailure,
    },
    registry::{
        GarbageCollectionReport, RegistryEntry, RegistryListEntry, RegistrySessionResolution,
        RestartConfiguration, RestartReport,
    },
    scenario::ScenarioSuiteReport,
    terminal::{
        analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport, LastActionAnalysis},
        diff::{ScreenChanges, SemanticDiff},
        inspection::WaitOutcome,
        observation::{
            ActorBoundaries, CaptureQuality, Observation, ReplayCachePruneReport,
            ReplayCacheStatus, StatePoint,
        },
        reviewer::{ReviewExportManifest, ReviewSearchResult, ReviewSummary},
        screen::{
            FrameDetail, ScreenFrameContent, ScreenFrameSource, ScreenGeometry, ScreenRegion,
        },
        state::{ColorState, CursorState, ModeState},
        timeline::TimelinePage,
    },
    value::ExactPath,
};
use termtrace_tmux::{
    artifact::{TmuxRecordingManifest, TmuxRecordingStart, TmuxStreamingBundleManifest},
    status::TmuxStatusReport,
};

/// Schema name of the catalog document.
pub const CATALOG_TITLE: &str = "TermtraceSdkTypes";

struct Catalog {
    generator: SchemaGenerator,
    roots: Vec<String>,
}

impl Catalog {
    fn add<T: JsonSchema>(&mut self) {
        let name = T::schema_name().into_owned();

        // Inlined types never get a definition from `subschema_for`; a root needs one.
        if T::inline_schema() {
            let schema = T::json_schema(&mut self.generator);
            self.generator
                .definitions_mut()
                .insert(name.clone(), schema.to_value());
        } else {
            let _reference = self.generator.subschema_for::<T>();
        }

        self.roots.push(name);
    }
}

macro_rules! catalog {
    ($($root:ty),* $(,)?) => {
        fn collect(settings: SchemaSettings) -> Catalog {
            let mut catalog = Catalog {
                generator: SchemaGenerator::new(settings),
                roots: Vec::new(),
            };
            $(catalog.add::<$root>();)*

            return catalog;
        }
    };
}

catalog!(
    ActOutcome,
    ActionInputResult,
    ActorBoundaries,
    ApplicationAcknowledgement,
    BundleManifest,
    BundleProfile,
    ByteOutputPage,
    CapabilityReport,
    CaptureOutcome,
    CaptureQuality,
    ColorState,
    CommandOutcome,
    ControlAcknowledgement,
    ControlErrorDetails,
    CursorState,
    DiagnoseReport,
    DiagnosticCheck,
    EditFailure,
    EditPlan,
    EditSource,
    EnvironmentSpec,
    ErrorReport,
    EventFollowPosition,
    EventPageCursor,
    ExactPath,
    ExplainReport,
    FrameDetail,
    GarbageCollectionReport,
    LastActionAnalysis,
    LaunchOptions,
    MaskedSnapshotOptions,
    FinalizationReport,
    TestOutcome,
    EvidencePolicy,
    ExpectationMismatchError,
    LocatorCondition,
    LocatorFailure,
    ManagementFailure,
    MarkOutcome,
    MarkerRecord,
    MatchRange,
    MatchedCell,
    MediaArtifactResult,
    ModeState,
    NormalizedEdit,
    Observation,
    OriginalCaptureStatus,
    OriginalLaunchOptions,
    OriginalRecording,
    OutputFrameSource,
    OutputPageCursor,
    PointerOutcome,
    PresentationPlan,
    PostconditionSummary,
    ProcessSignal,
    ProjectedEventPage,
    QueryDescription,
    QueryError,
    RecoveryReport,
    RegistryEntry,
    RegistryListEntry,
    RegistrySessionResolution,
    ReplayCachePruneReport,
    ReplayCacheStatus,
    RestartConfiguration,
    RestartReport,
    ReviewExportManifest,
    ReviewSearchResult,
    ReviewSummary,
    SaveFormat,
    SaveOutcome,
    ScenarioSuiteReport,
    ScreenChanges,
    ScreenFrameContent,
    ScreenFrameSource,
    ScreenGeometry,
    ScreenPredicate,
    ScreenRegion,
    ScreenWaitResult,
    SemanticDiff,
    SendSpec,
    SessionCompletion,
    SessionStatus,
    StableScreenCapture,
    StatePoint,
    StopReport,
    StyleFilter,
    TimelinePage,
    TmuxCatalog,
    TmuxEvents,
    TmuxRecordConfiguration,
    TmuxRecordingManifest,
    TmuxRecordingStart,
    TmuxStatusReport,
    TmuxStreamingBundleManifest,
    Value,
    VerificationLevel,
    VerificationReport,
    Versions,
    VideoOptions,
    VideoPreview,
    VideoRender,
    VisualProfile,
    VisualProfileFailure,
    VisualProfileReport,
    WaitOutcome,
);

/// Error raised when two catalog types would share one generated name.
#[derive(Debug, thiserror::Error)]
#[error("SDK type `{name}` is not a catalog definition; another type already uses its name")]
pub struct CatalogError {
    /// Root schema name without its own definition.
    pub name: String,
}

fn definitions(settings: SchemaSettings) -> Result<Map<String, Value>, CatalogError> {
    let Catalog {
        mut generator,
        roots,
    } = collect(settings);
    let mut sorted = generator
        .take_definitions(true)
        .into_iter()
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| return left.0.cmp(&right.0));
    let definitions = sorted.into_iter().collect::<Map<_, _>>();
    if let Some(name) = roots
        .into_iter()
        .find(|name| return !definitions.contains_key(name))
    {
        return Err(CatalogError { name });
    }
    return Ok(definitions);
}

/// Build the catalog document.
///
/// # Errors
///
/// Fails when a listed type's schema name does not resolve to its own definition, which
/// happens when two different types share a name.
pub fn catalog() -> Result<Value, CatalogError> {
    let mut serialized = Map::new();
    serialized.insert(
        "$defs".into(),
        Value::Object(definitions(SchemaSettings::draft2020_12().for_serialize())?),
    );

    let mut document = Map::new();
    document.insert(
        "$schema".into(),
        Value::from("https://json-schema.org/draft/2020-12/schema"),
    );
    document.insert("title".into(), Value::from(CATALOG_TITLE));
    document.insert(
        "$defs".into(),
        Value::Object(definitions(
            SchemaSettings::draft2020_12().for_deserialize(),
        )?),
    );
    document.insert("serialized".into(), Value::Object(serialized));

    return Ok(Value::Object(document));
}

/// A generator that already holds every catalog type as accepted on input, so a caller can add
/// its own types and take one set of definitions with consistent names.
#[must_use]
pub fn input_generator() -> SchemaGenerator {
    return collect(SchemaSettings::draft2020_12().for_deserialize()).generator;
}

fn closure<T: JsonSchema>(settings: SchemaSettings) -> (Value, Map<String, Value>) {
    let mut generator = SchemaGenerator::new(settings);
    let root = T::json_schema(&mut generator).to_value();

    return (root, generator.take_definitions(true));
}

/// Generated name of `T` as accepted on input: its schema name, or that name with an `Input`
/// suffix when the input and output shapes of `T` or a type it references differ.
#[must_use]
pub fn input_name<T: JsonSchema>() -> String {
    let name = T::schema_name().into_owned();
    let input = closure::<T>(SchemaSettings::draft2020_12().for_deserialize());
    let output = closure::<T>(SchemaSettings::draft2020_12().for_serialize());

    return if input == output { name } else { format!("{name}Input") };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_type_has_a_definition_under_its_own_name() {
        let document = catalog().expect("catalog");
        let definitions = document["$defs"].as_object().expect("definitions");

        for name in collect(SchemaSettings::draft2020_12()).roots {
            assert!(definitions.contains_key(&name), "{name}");
        }
    }
}
