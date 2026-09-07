use std::{
    fmt::{Display, Write as _},
    io::{self, IsTerminal as _, Write as _},
    process::ExitCode,
};

use miette::{IntoDiagnostic as _, Result};
use termtrace_model::{
    protocol::{
        cli_response::{Applied, CLI_RESPONSE_VERSION, CliResponseEnvelope, Retry, SessionState},
        output_projection::OutputProjectionQuality,
    },
    terminal::state::RowState,
    value::pagination::OutputPage,
};

use crate::cli::analysis::args::PageArgs;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::{ValueEnum, error::ErrorKind as ClapErrorKind};

const MAX_AGENT_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_CAPTURE_BYTES: usize = 256 * 1024;
const START_RESPONSE_BYTES: usize = 300;
const ACTION_RESPONSE_BYTES: usize = 250;
const WAIT_STATUS_RESPONSE_BYTES: usize = 300;
const MAX_COMPACT_SCREEN_BYTES: usize = 16 * 1024;
const STOP_RESPONSE_BYTES: usize = 350;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum ResponseMode {
    Auto,
    Human,
    Agent,
    Full,
    Quiet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResolvedResponseMode {
    Human,
    Agent,
    Full,
    Quiet,
}

impl ResponseMode {
    pub(super) fn resolve(self) -> ResolvedResponseMode {
        match self {
            Self::Full => return ResolvedResponseMode::Full,
            Self::Quiet => return ResolvedResponseMode::Quiet,
            Self::Human => return ResolvedResponseMode::Human,
            Self::Agent => return ResolvedResponseMode::Agent,
            Self::Auto => {
                if io::stdout().is_terminal() {
                    return ResolvedResponseMode::Human;
                } else {
                    return ResolvedResponseMode::Agent;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum ResponseField {
    Cursor,
    Modes,
    Boundary,
    Timing,
}

#[derive(Debug)]
struct ResponseState {
    mode: ResolvedResponseMode,
    fields: Vec<ResponseField>,
    command_type: String,
    mutating: bool,
    values: Vec<serde_json::Value>,
    raw: Vec<u8>,
    captured_bytes: usize,
    omitted_items: usize,
}

/// Explicit owner of all response capture and rendering state for one CLI invocation.
#[derive(Debug)]
pub(super) struct ResponseWriter {
    state: ResponseState,
}

impl ResponseWriter {
    pub(super) fn new(
        mode: ResolvedResponseMode,
        fields: Vec<ResponseField>,
        command_type: String,
        mutating: bool,
    ) -> Self {
        return Self {
            state: ResponseState {
                mode,
                fields,
                command_type,
                mutating,
                values: Vec::new(),
                raw: Vec::new(),
                captured_bytes: 0,
                omitted_items: 0,
            },
        };
    }

    pub(in crate::cli) fn mode(&self) -> ResolvedResponseMode {
        return self.state.mode;
    }

    pub(in crate::cli) fn quiet() -> Self {
        return Self::new(
            ResolvedResponseMode::Quiet,
            Vec::new(),
            String::new(),
            false,
        );
    }
}

mod envelope;
mod helpers;
mod human;
mod io_core;
mod pagination;
mod projection;
mod sink;

use envelope::{
    applied_from_str, compact_failure, compact_success, response_encoding_failure,
    response_string_value, retry_for_applied, retry_from_str, session_state_from_str,
    typed_envelope,
};
use human::{human_failure_summary, human_summary};
use io_core::{capture_json, capture_raw, direct_stderr, direct_stdout, report_message};
use pagination::{pagination_projection, raw_pagination_projection};
use projection::{
    bounded_agent_response, bounded_capture_projection, compact_task_value, copy_value,
    enforce_success_budget, failure_string, nested_string, response_type, session_state,
    value_string,
};

pub(super) use helpers::{
    id_list, optional_display, projection_warning, quoted_text, row_text, stable_enum_label,
    write_projection_warning,
};
pub(super) use io_core::write_progress;
pub(super) use pagination::paginate;
pub(super) use sink::{
    finish_response, response_output, write_clap_error, write_follow_json_line,
    write_follow_stdout, write_json_line, write_stdout,
};

#[cfg(test)]
mod tests;
