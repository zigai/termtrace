//! The one session handle every interface drives: launch, attach, act, wait, observe, stop.

use crate::{Error, Result};
use std::{path::Path, time::Duration};
use termtrace_engine::{
    analysis::diagnostics::{diagnose_session, explain_session},
    diff::diff_session_states_with_semantic_states,
    inspection::wait_for_output,
    timeline::visit_session_timeline_while,
};
use termtrace_evidence::{event_log::EventLog, session::verify_session};
use termtrace_export::bundle::{create as create_bundle, verify_profile as verify_bundle};
use termtrace_media::{
    native_capture::{
        capture as capture_native, validated_visual_manifests, CaptureOptions, CaptureOutcome,
    },
    playback::{playback, PlaybackOptions},
    save::{save, verify_saved_artifact},
};
use termtrace_model::{
    artifact::bundle::{BundleManifest, BundleProfile, BundleVerificationPolicy},
    artifact::save::{SaveFormat, SaveOutcome},
    execution::event::EventProjection,
    execution::session::{VerificationLevel, VerificationReport},
    protocol::control::{ControlCommand, ControlData, ControlRequest, PointerOperation},
    terminal::analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport},
    terminal::diff::SemanticDiff,
    terminal::inspection::WaitOutcome,
    terminal::observation::StatePoint,
    terminal::state::MouseButton,
    terminal::timeline::TimelineEntry,
};

mod capabilities;
mod launch;
mod session;
mod target;
mod terminal;

pub use capabilities::{Events, Evidence, Native, Output, Pointer, Semantic};
pub use launch::{await_ready, LaunchSpec, Readiness};
pub use session::{Session, StopOutcome, StopPolicy, StopRequest};
pub use target::{LocatedSession, SessionTarget};
pub use terminal::Terminal;

fn client_error(reason: impl Into<String>) -> Error {
    return Error::InvalidOperation {
        reason: reason.into(),
    };
}
