use crate::cli::analysis::args::PageArgs;
use clap::error::ErrorKind as ClapErrorKind;
use miette::{IntoDiagnostic as _, Result};
use std::{
    fmt::{self, Display, Write as _},
    io::{self, Write as _},
    process::ExitCode,
};
use termtrace_model::{
    protocol::output_projection::OutputProjectionQuality,
    terminal::{
        analysis::{LastActionDiagnosticsReport, LastActionReport},
        diff::{RowMove, ScreenChanges},
        screen::ScreenFrame,
        state::RowState,
    },
    value::pagination::OutputPage,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::cli) enum ResponseMode {
    Human,
    Quiet,
    Capture,
}

/// Output mode owner for one CLI invocation.
#[derive(Debug)]
pub(super) struct ResponseWriter {
    mode: ResponseMode,
    captured: Vec<u8>,
}

impl ResponseWriter {
    pub(super) fn new(mode: ResponseMode) -> Self {
        return Self {
            mode,
            captured: Vec::new(),
        };
    }

    pub(in crate::cli) fn mode(&self) -> ResponseMode {
        return self.mode;
    }

    pub(in crate::cli) fn into_captured(self) -> Vec<u8> {
        return self.captured;
    }
}

mod formatting;
mod human;
mod io_core;
mod pagination;
mod sink;

pub(super) use formatting::{
    id_list, optional_display, projection_warning, quoted_text, row_range, row_text,
    stable_enum_label, write_projection_warning,
};
pub(in crate::cli) use human::{
    human_action_summary, human_diagnostics_summary, human_last_action_summary,
    human_screen_changes, human_screen_summary,
};
use io_core::{direct_stderr, direct_stdout};
pub(in crate::cli) use io_core::{report_message, write_progress};
pub(super) use pagination::paginate;
pub(super) use sink::{
    finish_response, response_output, write_clap_error, write_follow_json_line,
    write_follow_stdout, write_json_line, write_json_pretty, write_stdout,
};
