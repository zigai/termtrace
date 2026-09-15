//! Canonical Rust SDK for Termtrace: recording, live control, replay analysis, and evidence integrity.

#[cfg(feature = "local")]
pub use termtrace_service::client;

mod driver;
#[cfg(not(feature = "local"))]
mod error;
#[cfg(feature = "local")]
pub(crate) use termtrace_service::error;

pub mod instrumentation;
#[cfg(not(feature = "local"))]
mod protocol;
#[cfg(feature = "local")]
pub(crate) use termtrace_service::protocol;

mod remote;

pub use driver::Driver;
pub use remote::Client;
pub use termtrace_model::protocol::driver::{
    DriverData, DriverMethod, DriverRequest, DriverResponse, DriverResult,
};
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

/// Canonical entry point alias for Termtrace programmatic operations.
#[cfg(feature = "local")]
pub type Termtrace = Terminal;

/// Process launch configuration, recording options, and owned leases.
#[cfg(feature = "local")]
pub mod recording {
    pub use termtrace_service::client::{SessionBuilder, Terminal};
    pub use termtrace_service::recording::{
        DetachedRecording, FinishPolicy, Recording, RecordingOptions, StopOptions, StopReceipt,
    };
}

/// Canonical live or completed session ownership and state resolution.
#[cfg(feature = "local")]
pub mod session {
    pub use termtrace_service::client::Session as ClientSession;
    pub use termtrace_service::session::{CompletedSession, LiveSession, ResolveOptions, Session};
    pub use termtrace_service::types::{ResolvedState, SessionId, StateSelector};
}

/// Typed keyboard, pointer, and composite action operations.
#[cfg(feature = "local")]
pub mod input {
    pub use termtrace_model::terminal::state::{KeyModifiers, MouseButton, NamedKey};
    pub use termtrace_service::actions::{
        ActionBuilder, ActionCompletion, ActionCondition, ActionOptions, ActionResult, InputAtom,
    };
    pub use termtrace_service::client::{Keyboard, Pointer};
}

/// Screen frames, compact observations, predicates, and queries.
#[cfg(feature = "local")]
pub mod screen {
    pub use termtrace_model::protocol::control::{
        ScreenMode, ScreenPredicate, StateHashComparison,
    };
    pub use termtrace_model::terminal::screen::{
        FrameDetail, ScreenFrame, ScreenProjectionRequest, ScreenRegion, ScreenSnapshotOptions,
    };
    pub use termtrace_service::screen::{
        frame_text, Screen, StableCaptureOptions, StableScreenCapture,
    };
}

/// Raw child PTY byte streaming and text waits.
#[cfg(feature = "local")]
pub mod output {
    pub use termtrace_service::client::Output;
    pub use termtrace_service::output::{OutputCursor, OutputPage, OutputService};
}

/// Filtered canonical event streams and cursors.
#[cfg(feature = "local")]
pub mod events {
    pub use termtrace_service::client::Events;
    pub use termtrace_service::events::{EventCursor, EventPage, EventsService};
}

/// Forensic timeline, semantic diff, and rendering diagnostics.
#[cfg(feature = "local")]
pub mod analysis {
    pub use termtrace_service::analysis::AnalysisService;
    pub use termtrace_service::client::Semantic;
    pub use termtrace_service::inspection::{
        inspect_session, state_text, wait_for_completion, wait_for_output, wait_for_screen,
        ScreenWaitOutcome, WaitOutcome,
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
    pub use termtrace_service::artifacts::ArtifactService;
    pub use termtrace_service::client::{Evidence, Native};
    pub use termtrace_service::types::ArtifactRef;
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
    pub use termtrace_service::registry::{RegistryService, RestartOverrides, ValueOverride};
}

/// Managed tmux pane recording and aggregate tracking.
#[cfg(feature = "local")]
pub mod tmux {
    pub use termtrace_model::protocol::control::ControlCommand;
    pub use termtrace_service::tmux::TmuxService;
}

/// System capability reports and component version metadata.
#[cfg(feature = "local")]
pub mod system {
    pub use termtrace_model::protocol::capability::CapabilityStatus;
    pub use termtrace_service::system::SystemService;
}

/// Synchronous blocking facade over shared application services.
#[cfg(feature = "local")]
pub mod blocking {
    pub use termtrace_service::client::{
        Events, Evidence, Keyboard, Native, Output, Pointer, Semantic, Session, SessionBuilder,
        Terminal,
    };
}

/// Low-level process driver and raw protocol access.
pub mod advanced {
    pub use crate::driver::Driver;
    pub use crate::remote::Client;
}

#[cfg(feature = "local")]
pub use actions::{
    ActionBuilder, ActionCompletion, ActionCondition, ActionOptions, ActionResult, InputAtom,
};
#[cfg(feature = "local")]
pub use client::{
    observe_session, observe_session_with_options, semantic_state_for_session,
    semantic_state_with_hash_for_session, Events, Evidence, Keyboard, Native, Output, Pointer,
    Semantic, Session, SessionBuilder, Terminal,
};
pub use error::{Error, ErrorKind, Result, RetrySafety};
#[cfg(feature = "local")]
pub use locator::ScreenFrameQuery;
pub use protocol::{
    control_endpoint_is_reachable, request, send_request, send_request_response, wait_for_screen,
};
#[cfg(feature = "local")]
pub use recording::{
    DetachedRecording, FinishPolicy, Recording, RecordingOptions, StopOptions, StopReceipt,
};
#[cfg(feature = "local")]
pub use screen::{Screen, StableCaptureOptions, StableScreenCapture};
#[cfg(feature = "local")]
pub use session::{CompletedSession, LiveSession, ResolveOptions};
#[cfg(feature = "local")]
pub use termtrace_model::artifact::bundle::{
    BundleManifest, BundleProfile, BundleVerificationPolicy,
};
pub use termtrace_model::execution::environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec};
pub use termtrace_model::protocol::control::{
    ControlCommand, ControlData, ControlRequest, ControlResponse, ControlResult, PointerOperation,
    ProcessSignal, ScreenMode, ScreenPredicate, StateHashComparison,
};
pub use termtrace_model::terminal::observation::StatePoint;
pub use termtrace_model::terminal::screen::{
    FrameDetail, ScreenFrame, ScreenProjectionRequest, ScreenRegion, ScreenSnapshotOptions,
};
pub use termtrace_model::terminal::state::{KeyModifiers, MouseButton, NamedKey};
#[cfg(feature = "local")]
pub use termtrace_service::context::OperationContext;
#[cfg(feature = "local")]
pub use termtrace_service::inspection;
#[cfg(feature = "local")]
pub use termtrace_service::types::{
    ArtifactRef, Operation, OperationStatus, ResolvedState, SessionId, StateSelector,
};
