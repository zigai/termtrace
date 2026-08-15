//! High-level blocking Rust facade over canonical sessions and actor controls.

use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{BufRead as _, BufReader, Error as IoError, ErrorKind, Read as _, Write as _},
    net::Shutdown,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use uuid::Uuid;

use termtrace_engine::observation::project_screen_frame;
use termtrace_engine::{
    analysis::diagnostics::{diagnose, explain},
    diff::diff_states,
    inspection::{wait_for_completion, wait_for_output},
    observation::{
        observation_from_live_snapshot,
        observe_session_with_options as observe_completed_session_with_options,
        semantic_state_from_live_snapshot,
        semantic_state_with_hash_for_session as completed_semantic_state_with_hash_for_session,
    },
    timeline::build_timeline,
};
use termtrace_evidence::{
    artifact::sync_directory,
    event_log::EventLog,
    session::{read_manifest, validate_completed_log, verify_session},
};
use termtrace_export::bundle::{create as create_bundle, verify_profile as verify_bundle};
use termtrace_media::{
    native_capture::{
        CaptureOptions, CaptureOutcome, capture as capture_native, validated_visual_manifests,
    },
    playback::{PlaybackOptions, playback},
    presentation::{export as export_presentation, preview as preview_presentation},
    save::{save, verify_saved_artifact},
};
use termtrace_model::{
    artifact::bundle::{BundleManifest, BundleProfile, BundleVerificationPolicy},
    artifact::presentation::{
        PresentationOutcome, PresentationPlan, PresentationPreviewOutcome, PreviewOptions,
    },
    artifact::save::{SaveFormat, SaveOutcome},
    execution::environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec},
    execution::event::{EventProjection, InputKind},
    execution::session::{SessionCompletion, VerificationLevel, VerificationReport},
    protocol::control::{
        ActionPostcondition, ActionPostconditionResult, CONTROL_PROTOCOL_VERSION, ControlCommand,
        ControlData, ControlErrorDetails, ControlRequest, ControlResponse, ControlResult,
        InputSequenceAtom, MAX_CONTROL_MESSAGE_BYTES, PointerOperation, ScreenPredicate,
        StableCaptureQuality,
    },
    terminal::analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport},
    terminal::diff::SemanticDiff,
    terminal::inspection::{ScreenWaitOutcome, WaitOutcome},
    terminal::observation::{CaptureQuality, Observation, StatePoint},
    terminal::screen::{
        FrameDetail, InvalidRegionReason, ScreenFrame, ScreenFrameError, ScreenFrameLimitKind,
        ScreenFrameSource, ScreenProjectionRequest, ScreenRegion,
    },
    terminal::state::{KeyModifiers, MouseButton, NamedKey},
    terminal::timeline::TimelineEntry,
    value::ByteString,
};

use crate::{
    Error, Result,
    actions::{ActionCompletion, ActionCondition, ActionOptions, ActionResult, InputAtom},
    error::RetrySafety,
    protocol::{
        connect_session_control, control_endpoint_is_reachable, send_request,
        send_request_response, validate_frame, wait_for_screen,
    },
    screen::StableCaptureOptions,
};

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
