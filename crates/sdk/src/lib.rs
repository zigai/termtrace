//! Blocking Rust SDK for live and completed Termtrace sessions.

mod client;
mod error;
mod protocol;

pub mod actions;
pub mod assertions;
pub mod locator;
pub mod screen;
pub mod testing;

/// Process launch configuration and fresh-session construction.
pub mod terminal {
    pub use crate::client::{SessionBuilder, Terminal};
}

/// Canonical live or completed session ownership.
pub mod session {
    pub use crate::client::Session;
}

/// Typed keyboard and pointer action namespaces.
pub mod input {
    pub use crate::client::{Keyboard, Pointer};
}

/// Completed evidence, native media, and canonical output operations.
pub mod evidence {
    pub use crate::client::{Evidence, Native, Output};
}

/// Completed semantic analysis and bounded canonical event reads.
pub mod inspection {
    pub use crate::client::{Events, Semantic};
    pub use crate::protocol::wait_for_screen;
    pub use termtrace_engine::inspection::{
        inspect_session, inspect_state_text, wait_for_completion, wait_for_output,
    };
    pub use termtrace_model::terminal::inspection::{ScreenWaitOutcome, WaitOutcome};
}

pub use actions::{ActionCompletion, ActionCondition, ActionOptions, ActionResult, InputAtom};
pub use client::{
    Events, Evidence, Keyboard, Native, Output, Pointer, Semantic, Session, SessionBuilder,
    Terminal, observe_session, observe_session_with_options, semantic_state_for_session,
    semantic_state_with_hash_for_session,
};
pub use error::{Error, Result, RetrySafety};
pub use locator::ScreenFrameQuery;
pub use protocol::{
    control_endpoint_is_reachable, request, send_request, send_request_response, wait_for_screen,
};
pub use screen::{Screen, StableCaptureOptions, StableScreenCapture};

pub use termtrace_model::execution::environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec};
pub use termtrace_model::protocol::control::{
    ControlCommand, ControlData, ControlRequest, PointerOperation, ProcessSignal, ScreenMode,
    ScreenPredicate, StateHashComparison,
};
pub use termtrace_model::terminal::observation::StatePoint;
pub use termtrace_model::terminal::screen::{
    FrameDetail, ScreenFrame, ScreenProjectionRequest, ScreenRegion, ScreenSnapshotOptions,
};
pub use termtrace_model::terminal::state::{KeyModifiers, MouseButton, NamedKey};
