use std::process::ExitCode;

use miette::Result;

use crate::cli::output::ResponseWriter;

pub(super) mod args;
use args::AnalyzeSubcommand;

mod replay;
mod reports;
pub(in crate::cli) mod reviewer;
mod show;
mod streams;

pub(in crate::cli) use replay::{
    run_cache, run_index, run_inspect, run_markers, run_playback, run_playback_worker, run_review,
};
pub(in crate::cli) use reports::{
    run_diagnose, run_diagnose_facade, run_diff, run_explain, run_inspect_facade, run_timeline,
};
pub(in crate::cli) use show::run_show;
pub(in crate::cli) use streams::{run_events, run_output};

pub(super) fn dispatch(
    response: &mut ResponseWriter,
    command: AnalyzeSubcommand,
) -> Result<ExitCode> {
    return match command {
        AnalyzeSubcommand::Index(arguments) => run_index(response, arguments),
        AnalyzeSubcommand::Inspect(arguments) => run_inspect(response, arguments),
        AnalyzeSubcommand::Timeline(arguments) => run_timeline(response, arguments),
        AnalyzeSubcommand::Diff(arguments) => run_diff(response, arguments),
        AnalyzeSubcommand::Explain(arguments) => run_explain(response, arguments),
        AnalyzeSubcommand::Diagnose(arguments) => run_diagnose(response, arguments),
        AnalyzeSubcommand::Review(arguments) => run_review(response, arguments),
        AnalyzeSubcommand::Output(arguments) => run_output(response, arguments),
        AnalyzeSubcommand::Events(arguments) => run_events(response, arguments),
        AnalyzeSubcommand::Markers(arguments) => run_markers(response, arguments),
        AnalyzeSubcommand::Cache(arguments) => run_cache(response, arguments),
    };
}
