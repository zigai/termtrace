//! Canonical Rust SDK for Termtrace: recording, live control, replay analysis, and evidence integrity.

#[cfg(feature = "local")]
pub use termtrace_service::{client, defaults};

pub mod instrumentation;
mod remote;

pub use remote::Client;
pub use termtrace_model::{execution::session::ResourceLimits, value::ByteString};
#[cfg(feature = "scenario")]
pub use termtrace_scenario as scenario;
#[cfg(feature = "local")]
pub use termtrace_service::actions;
#[cfg(feature = "local")]
pub use termtrace_service::assertions;
#[cfg(feature = "local")]
pub use termtrace_service::locator;
#[cfg(feature = "local")]
pub use termtrace_service::testing;

/// Launching, attaching, and stopping sessions.
#[cfg(feature = "local")]
pub mod session {
    pub use termtrace_model::artifact::original::{
        CellGrid, OriginalCaptureFacts, OriginalCaptureFailure, OriginalCaptureState,
        OriginalCaptureStatus, PixelCanvas, SurvivingKind, SurvivingPathRecord,
        WindowResizeDecision, WindowResizeRecord,
    };
    pub use termtrace_model::execution::visual_profile::{
        NativeProfileReport, NativeVisualProfile, PaddingPoints, RecordedVisualProfile,
        SemanticPreset, SemanticVisualProfile, VisualProfile, VisualProfileFailure,
        VisualProfileReport,
    };
    pub use termtrace_service::client::{
        await_ready, original_capture_status, send_step_request_id, CommandPostcondition,
        ControlAcknowledgement, LaunchSpec, LocatedSession, MarkOutcome, OriginalFps,
        OriginalGeometry, OriginalLaunchSpec, OriginalRecording, OriginalSession, Readiness,
        SendFailure, SendLastStep, SendOutcome, SendRequest, SendStep, Session, SessionStatus,
        SessionTarget, StopOutcome, StopRequest, Terminal,
    };
}

/// Typed key chords, pointer operations, and actor-ordered actions.
#[cfg(feature = "local")]
pub mod input {
    pub use termtrace_model::terminal::state::{KeyChord, KeyModifiers, MouseButton, NamedKey};
    pub use termtrace_service::actions::{
        ActOutcome, ActRequest, ActionCondition, InputAtom, MutationGuard, Reaction,
    };
    pub use termtrace_service::client::{Pointer, PointerOutcome};
}

/// Screen frames, compact observations, predicates, and queries.
#[cfg(feature = "local")]
pub mod screen {
    pub use termtrace_model::protocol::control::{
        ScreenMode, ScreenPredicate, StateHashComparison,
    };
    pub use termtrace_model::terminal::observation::{Observation, ObservationResolution};
    pub use termtrace_model::terminal::screen::{
        FrameDetail, ScreenFrame, ScreenProjectionRequest, ScreenRegion, ScreenSnapshotOptions,
    };
    pub use termtrace_service::screen::{
        frame_text, ScreenWaitResult, StableCaptureOptions, StableScreenCapture,
    };
}

/// Raw child PTY byte streaming and text waits.
#[cfg(feature = "local")]
pub mod output {
    pub use termtrace_model::execution::paging::{ByteOutputPage, OutputPageCursor};
    pub use termtrace_service::client::Output;
    pub use termtrace_service::output::{OutputCursor, OutputPage, OutputService};
}

/// Filtered canonical event streams and cursors.
#[cfg(feature = "local")]
pub mod events {
    pub use termtrace_model::execution::event::{
        BytePayloadSummary, Event, EventMetadata, EventProjection,
    };
    pub use termtrace_model::execution::paging::{EventPageCursor, PageSource, ProjectedEventPage};
    pub use termtrace_service::client::{Events, FollowPosition};
    pub use termtrace_service::events::{EventCursor, EventPage, EventsService};
}

/// Forensic timeline, semantic diff, and rendering diagnostics.
#[cfg(feature = "local")]
pub mod analysis {
    pub use termtrace_model::terminal::{
        analysis::{
            DiagnoseReport, DiagnosticCheck, ExplainReport, LastActionAnalysis, NativeCorrelation,
            NativeEvidenceLevel,
        },
        diff::SemanticDiff,
        observation::{ReplayCachePruneReport, ReplayCacheStatus},
        replay::SourceRange,
        reviewer::{ReviewExportManifest, ReviewSearchResult, ReviewSummary},
        timeline::{TimelineEntry, TimelinePage},
    };
    pub use termtrace_service::analysis::AnalysisService;
    pub use termtrace_service::client::Semantic;
    pub use termtrace_service::inspection::{
        inspect_session, state_text, wait_for_completion, wait_for_output, ScreenWaitOutcome,
        WaitOutcome,
    };
}

/// Artifact generation: screenshots, videos, bundles, and saved representations.
#[cfg(feature = "local")]
pub mod artifacts {
    #[cfg(feature = "in-process")]
    pub use termtrace_media::native_capture::{validate_replay_screenshot, NativeReplayWorker};
    pub use termtrace_model::artifact::bundle::{
        BundleManifest, BundleProfile, BundleVerificationPolicy,
    };
    pub use termtrace_model::artifact::derivative::{DerivativeFormat, GifLoop};
    pub use termtrace_model::artifact::presentation::{
        AnnotationAnchor, AnnotationPosition, AnnotationScope, AnnotationStyle, CaptionLayout,
        ChangeMode, ChangeSource, ChangeVisualization, EditClip, EditFailure, EditFailureCode,
        EditPlan, EditSelector, NormalizedEdit, OutputFrameSource, PixelTransform,
        PresentationDelivery, PresentationLayout, PresentationManifest, PresentationMapEntry,
        PresentationPlan, PresentationPreviewManifest, PresentationSource, PreviewOptions,
        RationalSpeed, SubtitleTrack, TextAnnotation, EDIT_PLAN_VERSION,
    };
    pub use termtrace_model::artifact::save::{
        SaveFormat, SaveManifest, SaveOutcome, SemanticRasterProfile,
    };
    pub use termtrace_service::artifacts::{
        output_frames_for_source_frame, source_for_output_frame, ArtifactService, CursorStyle,
        EditSource, VideoMode, VideoOptions, VideoOutcome, VideoPreset, VideoPreview, VideoRender,
        VideoRenderer, VideoRequest,
    };
    pub use termtrace_service::client::{Evidence, Native};
}

/// Offline integrity verification and cryptographic provenance.
#[cfg(feature = "local")]
pub mod integrity {
    pub use termtrace_model::execution::session::{
        SessionCompletion, VerificationLevel, VerificationReport,
    };
    pub use termtrace_service::integrity::IntegrityService;
}

/// Session registry discovery, status, and retention.
#[cfg(feature = "local")]
pub mod registry {
    pub use termtrace_model::execution::session::ResourceLimits;
    pub use termtrace_model::registry::{
        GarbageCollectionReport, RegistryEntry, RegistryListEntry, RegistrySessionResolution,
        RestartConfiguration, RestartReport,
    };
    pub use termtrace_service::registry::{RegistryService, RestartOverrides, ValueOverride};
}

/// Managed tmux pane recording and aggregate tracking.
#[cfg(feature = "local")]
pub mod tmux {
    pub use termtrace_model::execution::tmux::{
        TmuxCoverage, TmuxGhosttyProfile, TmuxRecordConfiguration,
    };
    pub use termtrace_model::protocol::control::ControlCommand;
    pub use termtrace_service::tmux::{
        CatalogPage, RecordRequest, ServiceTracks, StopRequest, TmuxCatalogSummary, TmuxClient,
        TmuxEventPage, TmuxGapCatalogEntry, TmuxRecordingManifest, TmuxRecordingStart, TmuxService,
        TmuxSocket, TmuxStatusReport, TmuxStreamingBundleManifest, TmuxTrackCatalogEntry, Tracks,
    };
}

/// System capability reports and component version metadata.
#[cfg(feature = "local")]
pub mod system {
    pub use termtrace_model::protocol::capability::CapabilityStatus;
    pub use termtrace_service::system::{SystemService, Versions};
}

/// Low-level raw protocol access.
pub mod advanced {
    pub use crate::remote::Client;
}

#[cfg(feature = "local")]
pub use actions::{ActOutcome, ActRequest, ActionCondition, InputAtom, MutationGuard, Reaction};
#[cfg(feature = "local")]
pub use client::{
    ControlAcknowledgement, Events, Evidence, LaunchSpec, MarkOutcome, Native, Output, Pointer,
    PointerOutcome, Semantic, Session, SessionStatus, SessionTarget, StopOutcome, StopRequest,
    Terminal,
};
#[cfg(feature = "local")]
pub use locator::ScreenFrameQuery;
#[cfg(feature = "local")]
pub use screen::{StableCaptureOptions, StableScreenCapture};
pub use termtrace_control::{
    control_endpoint_is_reachable, request, send_request, send_request_response,
};
#[cfg(not(feature = "local"))]
pub use termtrace_control::{wait_for_screen, Error, Result};
#[cfg(feature = "local")]
pub use termtrace_model::artifact::bundle::{
    BundleManifest, BundleProfile, BundleVerificationPolicy,
};
pub use termtrace_model::execution::environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec};
pub use termtrace_model::protocol::control::{
    ControlCommand, ControlPayload, ControlRequest, ControlResponse, ControlResult,
    PointerOperation, ProcessSignal, ScreenMode, ScreenPredicate, StateHashComparison,
};
pub use termtrace_model::protocol::mutation::{MutationEffect, Retry};
pub use termtrace_model::terminal::observation::StatePoint;
pub use termtrace_model::terminal::screen::{
    FrameDetail, ScreenFrame, ScreenProjectionRequest, ScreenRegion, ScreenSnapshotOptions,
};
pub use termtrace_model::terminal::state::{KeyChord, KeyModifiers, MouseButton, NamedKey};
#[cfg(feature = "local")]
pub use termtrace_service::inspection;
#[cfg(feature = "local")]
pub use termtrace_service::{error::ErrorKind, Error, Result};
pub use uuid::Uuid;

/// Portable SDK data contracts, independent of local runtime features.
pub mod contracts {
    pub use termtrace_model::artifact::sdk::*;
    #[cfg(feature = "local")]
    pub use termtrace_service::sdk_contract::control::{
        ActionDraft, AtomSpec, CommandOutcome, ConditionSpec, ControlOutcome, ErrorReport,
        LaunchOptions, OriginalLaunchOptions, PostconditionSummary, ProjectionSpec, ReactionSpec,
        SendSpec, SendStepSpec, StopReport,
    };
    #[cfg(feature = "local")]
    pub use termtrace_service::sdk_contract::{control, diagnostics, types};
    #[cfg(feature = "local")]
    pub use termtrace_service::sdk_contract::{
        decode_media_artifact, encode_media_artifact, timeout_from_ms, validate_region,
    };
}

/// Non-actor management progress and failure guidance.
pub use termtrace_model::protocol::management;
