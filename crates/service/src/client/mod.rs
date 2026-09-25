//! High-level blocking Rust facade over canonical sessions and actor controls.

use crate::{
    actions::{ActionCompletion, ActionCondition, ActionOptions, ActionResult, InputAtom},
    protocol::wait_for_screen,
    screen::StableCaptureOptions,
    Error, Result,
};
use nix::{
    errno::Errno,
    sys::signal::{killpg, Signal},
    unistd::Pid,
};
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{Error as IoError, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use termtrace_control::{
    control_endpoint_is_reachable, exchange, read_completed, send_request, send_request_response,
    ExchangeTiming,
};
use termtrace_engine::observation::{project_screen_frame, resolved_screen_state_for_session};
use termtrace_engine::{
    analysis::diagnostics::{diagnose_session, explain_session},
    diff::diff_session_states_with_semantic_states,
    inspection::{wait_for_completion, wait_for_output},
    observation::{
        observation_from_live_snapshot,
        observe_session_with_options as observe_completed_session_with_options,
        semantic_state_from_live_snapshot,
        semantic_state_with_hash_for_session as completed_semantic_state_with_hash_for_session,
    },
    timeline::visit_session_timeline_while,
};
use termtrace_evidence::{
    artifact::sync_directory,
    event_log::EventLog,
    session::{read_manifest, validate_completed_session, verify_session},
};
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
    execution::environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec},
    execution::event::{EventProjection, InputKind},
    execution::session::{SessionCompletion, VerificationLevel, VerificationReport},
    protocol::control::{
        derive_input_operation_id, ActionPostcondition, ActionPostconditionResult, ControlCommand,
        ControlData, ControlErrorDetails, ControlRequest, ControlResult, InputSequenceAtom,
        PointerOperation, ScreenPredicate, StableCaptureQuality,
    },
    protocol::mutation::MutationEffect,
    terminal::analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport},
    terminal::diff::SemanticDiff,
    terminal::inspection::{ScreenWaitOutcome, WaitOutcome},
    terminal::observation::{CaptureQuality, Observation, StatePoint},
    terminal::screen::{
        FrameDetail, InvalidRegionReason, ScreenFrame, ScreenFrameError, ScreenFrameLimitKind,
        ScreenProjectionRequest, ScreenRegion,
    },
    terminal::state::{KeyModifiers, MouseButton, NamedKey},
    terminal::timeline::TimelineEntry,
    value::ByteString,
};
use uuid::Uuid;

mod builder;
mod capabilities;
mod control;
mod observation;
mod session;
mod terminal;

pub use builder::SessionBuilder;
pub use capabilities::{
    Events, Evidence, Keyboard, Native, Output, Pointer, Screen, Semantic, StableScreenCapture,
};
pub use observation::{
    observe_session, observe_session_with_options, semantic_state_for_session,
    semantic_state_with_hash_for_session,
};
pub use session::Session;
pub use terminal::Terminal;
