use crate::cli::analysis::args::PageArgs;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::error::ErrorKind as ClapErrorKind;
use miette::{IntoDiagnostic as _, Result};
use std::{
    fmt::{Display, Write as _},
    io::{self, Write as _},
    process::ExitCode,
};
use termtrace_model::{
    protocol::{
        cli_response::{Applied, CliResponseEnvelope, Retry, SessionState, CLI_RESPONSE_VERSION},
        output_projection::OutputProjectionQuality,
    },
    terminal::state::RowState,
    value::pagination::OutputPage,
};

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Legacy envelope constants used in test suite")
)]
const MAX_AGENT_RESPONSE_BYTES: usize = 64 * 1024;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Legacy envelope constants used in test suite")
)]
const MAX_CAPTURE_BYTES: usize = 256 * 1024;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Legacy envelope constants used in test suite")
)]
const START_RESPONSE_BYTES: usize = 300;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Legacy envelope constants used in test suite")
)]
const ACTION_RESPONSE_BYTES: usize = 250;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Legacy envelope constants used in test suite")
)]
const WAIT_STATUS_RESPONSE_BYTES: usize = 300;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Legacy envelope constants used in test suite")
)]
const MAX_COMPACT_SCREEN_BYTES: usize = 16 * 1024;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Legacy envelope constants used in test suite")
)]
const STOP_RESPONSE_BYTES: usize = 350;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::cli) enum ResponseMode {
    Human,
    Quiet,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Response fields used in test suite")
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResponseField {
    Cursor,
    Modes,
    Boundary,
    Timing,
}

#[derive(Debug)]
struct ResponseState {
    mode: ResponseMode,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Response fields used in test suite")
    )]
    fields: Vec<ResponseField>,
    command_type: String,
    mutating: bool,
    values: Vec<serde_json::Value>,
    raw: Vec<u8>,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Capture tracking used in test suite")
    )]
    captured_bytes: usize,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Omitted tracking used in test suite")
    )]
    omitted_items: usize,
}

/// Explicit owner of all response capture and rendering state for one CLI invocation.
#[derive(Debug)]
pub(super) struct ResponseWriter {
    state: ResponseState,
}

impl ResponseWriter {
    pub(super) fn new(mode: ResponseMode, command_type: String, mutating: bool) -> Self {
        return Self {
            state: ResponseState {
                mode,
                fields: Vec::new(),
                command_type,
                mutating,
                values: Vec::new(),
                raw: Vec::new(),
                captured_bytes: 0,
                omitted_items: 0,
            },
        };
    }

    pub(in crate::cli) fn mode(&self) -> ResponseMode {
        return self.state.mode;
    }

    pub(in crate::cli) fn quiet() -> Self {
        return Self::new(ResponseMode::Quiet, String::new(), false);
    }
}

mod envelope;
mod helpers;
mod human;
mod io_core;
mod pagination;
mod projection;
mod sink;

#[cfg(test)]
use envelope::compact_success;
use envelope::{
    applied_from_str, response_encoding_failure, response_string_value, retry_for_applied,
    retry_from_str, session_state_from_str, typed_envelope,
};
pub(super) use helpers::{
    id_list, optional_display, projection_warning, quoted_text, row_text, stable_enum_label,
    write_projection_warning,
};
use human::human_failure_summary;
pub(in crate::cli) use human::{human_action_summary, human_screen_summary, human_summary};
#[cfg(test)]
use io_core::capture_raw;
pub(in crate::cli) use io_core::write_progress;
use io_core::{direct_stderr, direct_stdout, report_message};
pub(super) use pagination::paginate;
use pagination::{pagination_projection, raw_pagination_projection};
#[cfg(test)]
use projection::{bounded_agent_response, enforce_success_budget};
use projection::{
    bounded_capture_projection, compact_task_value, copy_value, failure_string, nested_string,
    response_type, session_state, value_string,
};
pub(super) use sink::{
    finish_response, response_output, write_clap_error, write_follow_json_line,
    write_follow_stdout, write_json_line, write_json_pretty, write_stdout,
};

#[cfg(test)]
mod tests;
