//! Reusable application services for Termtrace: one session type and its operations.
//!
//! The CLI control commands, the JSONL agent driver, the Python and Node bindings, and the
//! scenario runner all drive sessions through [`Session`]; they only parse input and render
//! output.

pub mod actions;
pub mod analysis;
pub mod artifacts;
pub mod assertions;
pub mod client;
pub mod defaults;
pub mod error;
pub mod events;
pub mod integrity;
pub mod locator;
pub mod output;
pub mod registry;
pub mod screen;
pub mod system;
pub mod testing;
pub mod tmux;

/// Completed semantic analysis and bounded canonical event reads.
pub mod inspection {
    pub use termtrace_engine::inspection::{inspect_session, wait_for_completion, wait_for_output};
    pub use termtrace_engine::observation::state_text;
    pub use termtrace_model::terminal::inspection::{ScreenWaitOutcome, WaitOutcome};
}

pub use actions::{ActOutcome, ActRequest, ActionCondition, InputAtom, MutationGuard, Reaction};
pub use analysis::AnalysisService;
pub use artifacts::ArtifactService;
pub use assertions::{assert_screen_text, SnapshotProfile};
pub use client::{
    await_ready, CommandPostcondition, Events, Evidence, FollowPosition, LaunchSpec,
    LocatedSession, Native, Output, Pointer, Readiness, Semantic, SendFailure, SendLastStep,
    SendOutcome, SendRequest, SendStep, Session, SessionStatus, SessionTarget, StopOutcome,
    StopRequest, Terminal,
};
pub use error::{ControlError, Error, ErrorKind, Result};
pub use events::{EventCursor, EventPage, EventsService};
pub use integrity::IntegrityService;
pub use locator::ScreenFrameQuery;
pub use output::{OutputCursor, OutputPage, OutputService};
pub use registry::{RegistryService, RestartOverrides, ValueOverride};
pub use screen::{frame_text, StableCaptureOptions, StableScreenCapture};
pub use system::SystemService;
pub use testing::{run_terminal_test, EvidencePolicy, TerminalTest, TerminalTestConfig};
pub use tmux::TmuxService;
