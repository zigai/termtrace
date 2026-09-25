//! Reusable application services and orchestration for Termtrace.

pub mod actions;
pub mod analysis;
pub mod artifacts;
pub mod assertions;
pub mod blocking;
pub mod client;
pub mod context;
pub mod error;
pub mod events;
pub mod integrity;
pub mod locator;
pub mod output;
pub mod protocol;
pub mod recording;
pub mod registry;
pub mod screen;
pub mod session;
pub mod system;
pub mod testing;
pub mod tmux;
pub mod types;

/// Completed semantic analysis and bounded canonical event reads.
pub mod inspection {
    pub use crate::client::{Events, Semantic};
    pub use crate::protocol::wait_for_screen;
    pub use termtrace_engine::inspection::{inspect_session, wait_for_completion, wait_for_output};
    pub use termtrace_engine::observation::state_text;
    pub use termtrace_model::terminal::inspection::{ScreenWaitOutcome, WaitOutcome};
}

pub use actions::{
    ActionBuilder, ActionCompletion, ActionCondition, ActionOptions, ActionResult, InputAtom,
};
pub use analysis::AnalysisService;
pub use artifacts::ArtifactService;
pub use assertions::{assert_screen_text, SnapshotProfile};
pub use blocking::BlockingContext;
pub use client::{
    observe_session, observe_session_with_options, semantic_state_for_session,
    semantic_state_with_hash_for_session, Events, Evidence, Keyboard, Native, Output, Pointer,
    Semantic, SessionBuilder, Terminal,
};
pub use context::OperationContext;
pub use error::{Error, Result, RetrySafety};
pub use events::{EventCursor, EventPage, EventsService};
pub use integrity::IntegrityService;
pub use locator::ScreenFrameQuery;
pub use output::{OutputCursor, OutputPage, OutputService};
pub use protocol::wait_for_screen;
pub use recording::{
    DetachedRecording, FinishPolicy, Recording, RecordingOptions, StopOptions, StopReceipt,
};
pub use registry::{RegistryService, RestartOverrides, ValueOverride};
pub use screen::{Screen, StableCaptureOptions, StableScreenCapture};
pub use session::{CompletedSession, LiveSession, ResolveOptions, Session};
pub use system::SystemService;
pub use testing::{run_terminal_test, EvidencePolicy, TerminalTest, TerminalTestConfig};
pub use tmux::TmuxService;
pub use types::{ArtifactRef, Operation, OperationStatus, ResolvedState, SessionId, StateSelector};
