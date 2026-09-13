use crate::cli::output::ResponseWriter;
use miette::Result;
use std::process::ExitCode;

pub(super) mod args;

use args::{AnalyzeArgs, AnalyzeSubcommand};

mod replay;
mod reports;
pub(in crate::cli) mod reviewer;
mod streams;

pub(in crate::cli) use replay::{
    run_cache, run_index, run_inspect, run_markers, run_playback, run_playback_worker, run_review,
};
pub(in crate::cli) use reports::{
    run_diagnose, run_diagnose_last_action_facade, run_diff, run_explain, run_last_action_facade,
    run_recover, run_timeline, run_verify,
};
pub(in crate::cli) use streams::{run_events, run_output};

pub(super) fn dispatch(response: &mut ResponseWriter, arguments: AnalyzeArgs) -> Result<ExitCode> {
    if let Some(command) = arguments.command {
        return match command {
            AnalyzeSubcommand::LastAction(arguments) => run_last_action_facade(response, arguments),
            AnalyzeSubcommand::Index(arguments) => run_index(response, arguments),
            AnalyzeSubcommand::Inspect(arguments) => run_inspect(response, arguments),
            AnalyzeSubcommand::Timeline(arguments) => run_timeline(response, arguments),
            AnalyzeSubcommand::Diff(arguments) => run_diff(response, arguments),
            AnalyzeSubcommand::Explain(arguments) => run_explain(response, arguments),
            AnalyzeSubcommand::Diagnose(arguments) => {
                if arguments.last_action {
                    run_diagnose_last_action_facade(
                        response,
                        args::DiagnoseFacadeArgs {
                            session: arguments.session,
                            broad_change_percent: arguments.broad_change_percent,
                            json: arguments.json,
                        },
                    )
                } else {
                    run_diagnose(response, arguments)
                }
            }
            AnalyzeSubcommand::Review(arguments) => run_review(response, arguments),
            AnalyzeSubcommand::Output(arguments) => run_output(response, arguments),
            AnalyzeSubcommand::Events(arguments) => run_events(response, arguments),
            AnalyzeSubcommand::Markers(arguments) => run_markers(response, arguments),
            AnalyzeSubcommand::Cache(arguments) => run_cache(response, arguments),
            AnalyzeSubcommand::Verify(arguments) => run_verify(response, arguments),
            AnalyzeSubcommand::Recover(arguments) => run_recover(response, arguments),
        };
    }

    if let Some(session) = arguments.session {
        return run_last_action_facade(
            response,
            args::InspectFacadeArgs {
                session,
                broad_change_percent: 50,
                json: arguments.json,
            },
        );
    }
    return Err(miette::miette!(
        "session path or analyze subcommand is required"
    ));
}
