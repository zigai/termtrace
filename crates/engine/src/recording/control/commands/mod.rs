//! Recording-actor control admission and command execution.

use super::super::{
    actor::SessionActor,
    input::{
        apply_input, apply_resize, interpolate_cell, mode_hash, mouse_button_name, prepare_bytes,
        prepare_key, prepare_paste, InputSequenceExecution,
    },
    model::TerminalGeometry,
    presentation::{find_text_regions, resize_semantic, LiveTerminal},
    support::{
        actor_boundaries, control_error_response, control_rejected, finish_with_cleanup,
        terminal_error, validate_control_input,
    },
    termination::Fact,
};
use super::screen::{
    compile_screen_predicate, evaluate_screen_predicate, presented_state_hash, valid_blake3_hash,
    CompiledScreenPredicate, ScreenWaiter, MAX_SCREEN_WAITERS,
};
use crate::{Error, Result};
use nix::{
    errno::Errno,
    sys::signal::{killpg, Signal},
    unistd::Pid,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use termtrace_evidence::event_log::EventLogWriter;
use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource, OperationStatus},
    protocol::control::{
        ActionInputResult, ActionPostcondition, ActionPostconditionResult, ControlCommand,
        ControlData, ControlErrorDetails, ControlRequest, ControlResponse, ControlResult,
        InputSequenceFailureDetails, PointerOperation, ProcessSignal, ScreenFrameFailureDetails,
        ScreenPostconditionQuality, StableCaptureQuality, CONTROL_PROTOCOL_VERSION,
    },
    protocol::mutation::MutationEffect,
    terminal::observation::{ActorBoundaries, CaptureQuality},
    terminal::screen::{
        ScreenFrame, ScreenFrameError, ScreenFrameLimitKind, ScreenFrameSource,
        ScreenProjectionRequest,
    },
    terminal::state::{FocusEvent, MouseAction, MouseButton},
    value::ByteString,
};
use termtrace_terminal::{
    encode_focus, project_screen_frame, semantic_viewport_text, supported_geometry_cell_count,
    validate_screen_region,
};
use uuid::Uuid;
use zstd::stream::encode_all;

mod action;
mod admission;
mod mutations;
mod observation;
mod response;

pub(in crate::recording) use action::PendingInputSequence;
pub(in crate::recording) use mutations::apply_signal;
use observation::{
    compile_action_postcondition, evaluate_action_postcondition, AdmittedAction,
    AdmittedPostcondition,
};
use response::{control_error_code, mutation_failure_effect};
pub(in crate::recording::control) use response::screen_frame_error_response;
