use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::{
    control::args::{ScreenArgs, ScreenEncoding, StatusArgs, StopArgs, WaitArgs},
    output::{
        human_action_summary, human_screen_summary, write_json_line, write_json_pretty,
        write_stdout,
    },
    parsers::{parse_cursor_position, parse_region_predicate, parse_row_predicate},
    session_helpers::{
        resolve_session, resolve_session_path, resolve_state_point, write_session_lifecycle,
        ResolvedSessionRef,
    },
};
use miette::{IntoDiagnostic as _, Result};
use std::{
    io::{self, Read as _},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};
use termtrace_control::{
    is_completed, request, request_or_completed, send_request, send_request_response,
    Error as ControlError, Reached,
};
use termtrace_engine::{
    inspection::{wait_for_completion, wait_for_output},
    observation::observe_validated_prefix,
};
use termtrace_model::{
    execution::event::InputKind,
    execution::session::SessionPhase,
    protocol::control::{
        derive_input_operation_id, ActionPostcondition, ActionPostconditionResult, ControlCommand,
        ControlData, ControlErrorDetails, ControlRequest, ControlResult, InputSequenceAtom,
        InputSequenceAtomOutcome, ScreenPostconditionQuality, ScreenPredicate, ScreenWaitQuality,
        StableCaptureQuality, StateHashComparison,
    },
    terminal::observation::{ActorBoundaries, CaptureQuality, StatePoint},
    terminal::screen::{ScreenFrame, ScreenProjectionRequest},
    terminal::state::{KeyModifiers, NamedKey},
    value::ByteString,
};
use termtrace_sdk::{observe_session_with_options, wait_for_screen};

mod action;
mod input;
mod screen;
mod transport;
mod wait;

pub(in crate::cli) use action::run_send;
pub(in crate::cli) use screen::run_screen;
pub(in crate::cli) use transport::{run_status, run_stop};
#[expect(unused_imports, reason = "Control transport types preserved for tests")]
use transport::{send_cli_control, CliControlOutcome};
use wait::compact_boundaries;
pub(in crate::cli) use wait::run_wait;
