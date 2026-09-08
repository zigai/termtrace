//! Blocking Rust SDK for live Termtrace sessions.
//!
//! The default build uses the installed recorder through authenticated local IPC
//! and does not compile a terminal engine or native media tooling. Enable
//! `in-process` for completed replay, high-level test helpers, and media APIs.

#[cfg(feature = "in-process")]
mod client;
mod driver;
mod error;
pub mod instrumentation;
mod protocol;
mod remote;

pub use driver::Driver;
pub use remote::Client;
pub use termtrace_model::protocol::driver::{
    DriverData, DriverMethod, DriverRequest, DriverResponse, DriverResult,
};
pub use termtrace_model::{execution::session::ResourceLimits, value::ByteString};

#[cfg(feature = "in-process")]
pub mod actions;
#[cfg(feature = "in-process")]
pub mod assertions;
#[cfg(feature = "in-process")]
pub mod locator;
#[cfg(feature = "in-process")]
pub mod screen;
#[cfg(feature = "in-process")]
pub mod testing;

/// Process launch configuration and fresh-session construction.
#[cfg(feature = "in-process")]
pub mod terminal {
    pub use crate::client::{SessionBuilder, Terminal};
}

/// Canonical live or completed session ownership.
#[cfg(feature = "in-process")]
pub mod session {
    pub use crate::client::Session;
}

/// Typed keyboard and pointer action namespaces.
#[cfg(feature = "in-process")]
pub mod input {
    pub use crate::client::{Keyboard, Pointer};
}

/// Completed evidence, native media, and canonical output operations.
#[cfg(feature = "in-process")]
pub mod evidence {
    pub use crate::client::{Evidence, Native, Output};
    pub use termtrace_media::native_capture::{validate_replay_screenshot, NativeReplayWorker};
}

/// Completed semantic analysis and bounded canonical event reads.
#[cfg(feature = "in-process")]
pub mod inspection {
    pub use crate::client::{Events, Semantic};
    pub use crate::protocol::wait_for_screen;
    pub use termtrace_engine::inspection::{inspect_session, wait_for_completion, wait_for_output};
    pub use termtrace_engine::observation::state_text;
    pub use termtrace_model::terminal::inspection::{ScreenWaitOutcome, WaitOutcome};
}

#[cfg(feature = "in-process")]
pub use actions::{ActionCompletion, ActionCondition, ActionOptions, ActionResult, InputAtom};
#[cfg(feature = "in-process")]
pub use client::{
    observe_session, observe_session_with_options, semantic_state_for_session,
    semantic_state_with_hash_for_session, Events, Evidence, Keyboard, Native, Output, Pointer,
    Semantic, Session, SessionBuilder, Terminal,
};
pub use error::{Error, Result, RetrySafety};
#[cfg(feature = "in-process")]
pub use locator::ScreenFrameQuery;
pub use protocol::{
    control_endpoint_is_reachable, request, send_request, send_request_response, wait_for_screen,
};
#[cfg(feature = "in-process")]
pub use screen::{Screen, StableCaptureOptions, StableScreenCapture};

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
