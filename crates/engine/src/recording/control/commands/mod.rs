//! Recording-actor control admission and command execution.

use super::super::{
    actor::SessionActor,
    input::{
        apply, apply_resize, interpolate_cell, mode_hash, mouse_button_name, prepare_bytes,
        prepare_key, prepare_paste, InputSequenceExecution,
    },
    model::TerminalGeometry,
    presentation::{find_text_regions, resize_semantic, LiveTerminal},
    support::{
        actor_boundaries, control_error_response, control_rejected, deliver_response,
        finish_with_cleanup, terminal_error, validate_control_input,
    },
    termination::Fact,
};
use super::screen::{
    presented_state_hash, reaction_window, ScreenWaiter, WaiterReaction, MAX_SCREEN_WAITERS,
};
use crate::{Error, Result};
use nix::errno::Errno;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use termtrace_evidence::event_log::EventLogWriter;
use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource, OperationStatus},
    protocol::control::{
        is_lowercase_blake3_hex, ActionInputResult, ActionPostcondition, ActionPostconditionResult,
        CompiledScreenPredicate, ControlCommand, ControlErrorDetails, ControlPayload,
        ControlRequest, ControlResponse, ControlResult, InputSequenceFailureDetails,
        PointerOperation, ProcessSignal, ScreenFrameFailureDetails, ScreenPostconditionQuality,
        StableCaptureQuality, ValidatedCommand, CONTROL_PROTOCOL_VERSION,
    },
    protocol::mutation::MutationEffect,
    terminal::observation::{ActorBoundaries, CaptureQuality},
    terminal::screen::{
        ScreenFrame, ScreenFrameError, ScreenFrameLimitKind, ScreenFrameSource,
        ScreenProjectionRequest,
    },
    terminal::state::{supported_geometry_cell_count, FocusEvent, MouseAction, MouseButton},
    value::ByteString,
};
use termtrace_terminal::{
    encode_focus, project_screen_frame, semantic_viewport_text, validate_screen_region,
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
    admitted_postcondition, evaluate_action_postcondition, AdmittedAction, AdmittedPostcondition,
};
pub(in crate::recording::control) use response::screen_frame_error;
use response::{control_error_code, mutation_failure_effect};
