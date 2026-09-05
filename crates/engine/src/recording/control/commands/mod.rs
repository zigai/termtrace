//! Recording-actor control admission and command execution.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use uuid::Uuid;
use zstd::stream::encode_all;

use termtrace_evidence::event_log::EventLogWriter;
use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource, OperationStatus},
    protocol::control::{
        ActionInputResult, ActionPostcondition, ActionPostconditionResult,
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlErrorDetails, ControlRequest,
        ControlResponse, ControlResult, InputSequenceFailureDetails, PointerOperation,
        ProcessSignal, ScreenFrameFailureDetails, ScreenPostconditionQuality, StableCaptureQuality,
    },
    terminal::observation::{ActorBoundaries, CaptureQuality},
    terminal::screen::{
        ScreenFrame, ScreenFrameError, ScreenFrameLimitKind, ScreenFrameSource,
        ScreenProjectionRequest,
    },
    terminal::state::{FocusEvent, MouseAction, MouseButton},
    value::ByteString,
};
use termtrace_terminal::{
    encode_focus, project_screen_frame, semantic_state_hash, supported_geometry_cell_count,
    validate_screen_region,
};

use super::super::{
    actor::SessionActor,
    input::{
        InputSequenceExecution, apply_input, apply_resize, encode_paste, interpolate_cell,
        mouse_button_name, semantic_key_input, semantic_key_name,
    },
    model::TerminalGeometry,
    presentation::{LiveTerminal, find_text_regions, resize_semantic, semantic_viewport_text},
    runtime::HOST_SIGNAL_GRACE_PERIOD,
    support::{
        actor_boundaries, control_error_response, control_rejected, finish_with_cleanup,
        terminal_error, validate_control_input,
    },
};
use super::screen::{
    MAX_SCREEN_WAITERS, ScreenWaiter, evaluate_screen_predicate, presented_state_hash,
    valid_blake3_hash, validate_screen_predicate,
};
use crate::{Error, Result};

mod action;
mod admission;
mod mutations;
mod observation;
mod response;

pub(in crate::recording) use action::PendingInputSequence;
pub(in crate::recording) use mutations::apply_signal;
use observation::{evaluate_action_postcondition, validate_action_postcondition};
use response::{control_error_code, screen_frame_error_response};
pub(in crate::recording::control) use response::{
    screen_frame_error_code, screen_frame_failure_details,
};
