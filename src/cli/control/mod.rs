use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::{
    control::args::{millis_or, ScreenArgs, ScreenEncoding, StatusArgs, StopArgs, WaitArgs},
    output::{
        human_action_summary, human_screen_summary, stable_enum_label, write_json_line,
        write_json_pretty, write_stdout,
    },
    parsers::{parse_cursor_position, parse_region_predicate, parse_row_predicate},
    session_helpers::{
        attach_session, resolve_session, resolve_session_path, resolve_state_point,
        write_session_lifecycle, ResolvedSessionRef,
    },
};
use miette::{IntoDiagnostic as _, Result};
use std::{
    io::{self, Read as _},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};
use termtrace_control::{is_completed, request};
use termtrace_engine::{
    inspection::{wait_for_completion, wait_for_output},
    observation::observe_validated_prefix,
};
use termtrace_model::{
    execution::session::SessionPhase,
    protocol::control::{
        ActionPostconditionResult, ControlCommand, ControlData, ControlRequest,
        InputSequenceAtomOutcome, ScreenPostconditionQuality, ScreenPredicate,
        StableCaptureQuality, StateHashComparison,
    },
    terminal::observation::{ActorBoundaries, CaptureQuality, StatePoint},
    terminal::screen::{ScreenFrame, ScreenProjectionRequest},
    terminal::state::KeyChord,
};
use termtrace_sdk::{
    actions::hash_only_projection, defaults, session::Session, ActOutcome, ActRequest,
    ActionCondition, Error as SessionError, InputAtom, MutationGuard, StableCaptureOptions,
    StopOutcome, StopPolicy, StopRequest,
};

mod action;
mod input;
mod screen;
mod transport;
mod wait;

pub(in crate::cli) use action::run_send;
pub(in crate::cli) use screen::run_screen;
pub(in crate::cli) use transport::{run_status, run_stop};
use wait::compact_boundaries;
pub(in crate::cli) use wait::run_wait;
