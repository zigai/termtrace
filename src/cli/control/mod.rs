use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::{
    control::args::{
        ActArgs, PointerArgs, PointerCommand, ResizeArgs, ScreenArgs, ScreenEncoding, SendArgs,
        StatusArgs, StopArgs, WaitArgs,
    },
    output::write_json_line,
    parsers::{parse_cursor_position, parse_region_predicate, parse_row_predicate},
    session_helpers::{resolve_session, write_session_lifecycle, ResolvedSessionRef},
};
use args::{ControlSubcommand, FocusDirection};
use miette::{IntoDiagnostic as _, Result};
use std::{
    io::{self, Read as _},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};
use termtrace_engine::{
    inspection::{wait_for_completion, wait_for_output},
    observation::observe_validated_prefix,
};
use termtrace_model::{
    execution::event::InputKind,
    protocol::control::{
        derive_input_operation_id, ActionPostcondition, ActionPostconditionResult, ControlCommand,
        ControlData, ControlErrorDetails, ControlRequest, ControlResult, InputSequenceAtom,
        InputSequenceAtomOutcome, PointerOperation, ScreenPostconditionQuality, ScreenPredicate,
        ScreenWaitQuality, StableCaptureQuality, StateHashComparison,
    },
    registry::RegistryLifecycle,
    terminal::observation::{ActorBoundaries, CaptureQuality, StatePoint},
    terminal::screen::{ScreenFrame, ScreenProjectionRequest},
    terminal::state::{KeyModifiers, NamedKey},
    value::ByteString,
};
use termtrace_sdk::{
    observe_session_with_options, request, send_request, send_request_response, wait_for_screen,
};

mod action;
mod input;
mod screen;
mod transport;
mod wait;

pub(super) use action::run_act;
use action::{input_failure_application, input_retry};
pub(super) use input::{run_keys, run_pointer, run_resize, run_send};
pub(super) use screen::run_screen;
pub(super) use transport::{run_control_guarded, run_status, run_stop};
use transport::{send_cli_control, CliControlOutcome};
use wait::compact_boundaries;
pub(super) use wait::run_wait;

pub(super) fn dispatch(
    response: &mut ResponseWriter,
    command: ControlSubcommand,
) -> Result<ExitCode> {
    return match command {
        ControlSubcommand::Keys => run_keys(response),
        ControlSubcommand::Focus(arguments) => run_control_guarded(
            response,
            arguments.session,
            ControlCommand::Focus {
                focused: matches!(arguments.direction, FocusDirection::In),
                strict: arguments.strict,
            },
            arguments.expected_presented_state_hash,
            arguments.request_id,
        ),
        ControlSubcommand::Pointer(arguments) => run_pointer(response, arguments),
        ControlSubcommand::Activate(arguments) => run_control_guarded(
            response,
            arguments.session,
            ControlCommand::Activate {
                text: arguments.text,
                unique: arguments.unique,
                strict: arguments.strict,
            },
            arguments.expected_presented_state_hash,
            arguments.request_id,
        ),
        ControlSubcommand::Resize(arguments) => run_resize(response, arguments),
        ControlSubcommand::Signal(arguments) => run_control_guarded(
            response,
            arguments.session,
            ControlCommand::Signal {
                signal: arguments.signal.into(),
            },
            arguments.expected_presented_state_hash,
            arguments.request_id,
        ),
        ControlSubcommand::Mark(arguments) => run_control_guarded(
            response,
            arguments.session,
            ControlCommand::Mark {
                label: arguments.label,
            },
            arguments.expected_presented_state_hash,
            arguments.request_id,
        ),
    };
}
