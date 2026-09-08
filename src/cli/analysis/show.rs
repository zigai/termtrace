use std::{
    io::{self, Write as _},
    process::ExitCode,
};

use miette::{IntoDiagnostic as _, Result};
use termtrace_engine::observation::{
    observe_completed_with_formats, observe_validated_prefix, CaptureQuality, Observation,
};
use termtrace_model::{
    protocol::control::{ScreenPredicate, ScreenWaitQuality, StateHashComparison},
    terminal::observation::StatePoint,
};
use termtrace_sdk::{observe_session_with_options, wait_for_screen};
use termtrace_terminal::Format as GhosttyFormat;

use crate::cli::{
    analysis::args::{ShowArgs, ShowFormat},
    output::{stable_enum_label, write_json_line, write_stdout, ResponseWriter},
    session_helpers::{resolve_session_path, resolve_state_point},
};

pub(in crate::cli) fn run_show(
    response: &mut ResponseWriter,
    arguments: ShowArgs,
) -> Result<ExitCode> {
    let point = resolve_state_point(
        arguments.state,
        arguments.state_hash,
        arguments.event_seq,
        arguments.at,
        arguments.at_marker,
        Some(StatePoint::LatestPresented),
        "show",
    )?;

    let session = resolve_session_path(arguments.session)?;

    if arguments.recover_prefix {
        if !arguments.stable_for.is_zero()
            || matches!(arguments.format, ShowFormat::Vt | ShowFormat::Html)
        {
            return Err(miette::miette!(
                "prefix recovery supports text and JSON formats without live stability"
            ));
        }

        let observation = observe_validated_prefix(&session, point, arguments.include_hidden)
            .into_diagnostic()?;

        if arguments.metadata {
            write_observation_metadata(&observation)?;
        }

        return write_observation(response, &observation, None, arguments.format);
    }

    let mut stable_hash: Option<String> = None;
    if !arguments.stable_for.is_zero() && !session.join("completion.json").is_file() {
        if point != StatePoint::LatestPresented || arguments.include_hidden {
            return Err(miette::miette!(
                "live stability is valid only for latest-presented observations"
            ));
        }

        let stable_wait = wait_for_screen(
            &session,
            ScreenPredicate::StateHash {
                hash: "0".repeat(64),
                comparison: StateHashComparison::Changed,
            },
            arguments.stable_for,
            arguments.timeout,
        )
        .into_diagnostic()?;

        if !stable_wait.matched || stable_wait.quality != ScreenWaitQuality::Matched {
            return Err(miette::miette!(
                "live stability was not established before {:?}",
                stable_wait.quality
            ));
        }

        stable_hash = Some(stable_wait.frame.state_hash);
    }

    let engine_format = match arguments.format {
        ShowFormat::Vt => Some(GhosttyFormat::Vt),
        ShowFormat::Html => Some(GhosttyFormat::Html),
        _ => None,
    };

    let (mut observation, formatted) = match engine_format {
        Some(format) => {
            let (observation, rendered) = observe_completed_with_formats(
                &session,
                point,
                arguments.include_hidden,
                &[format],
            )
            .into_diagnostic()?;

            let formatted = rendered
                .into_iter()
                .next()
                .map(|(_, bytes)| return bytes)
                .ok_or_else(|| miette::miette!("selected formatter produced no output"))?;

            (observation, Some(formatted))
        }
        None => (
            observe_session_with_options(&session, point, arguments.include_hidden)
                .into_diagnostic()?,
            None,
        ),
    };
    if let Some(state_hash) = stable_hash {
        if observation.state_hash != state_hash {
            return Err(miette::miette!(
                "presented state changed between stability proof and observation; retry show"
            ));
        }

        observation.capture_quality = CaptureQuality::Stable;
    }

    if arguments.metadata {
        write_observation_metadata(&observation)?;
    }

    return write_observation(response, &observation, formatted, arguments.format);
}

fn write_observation(
    response: &mut ResponseWriter,
    observation: &Observation,
    formatted: Option<Vec<u8>>,
    format: ShowFormat,
) -> Result<ExitCode> {
    return match format {
        ShowFormat::Text => {
            write_stdout(response, format!("{}\n", observation.frame.viewport_text))
        }
        ShowFormat::Json => write_stdout(
            response,
            format!(
                "{}\n",
                serde_json::to_string_pretty(observation).into_diagnostic()?
            ),
        ),
        ShowFormat::JsonCompact => write_json_line(response, observation),
        ShowFormat::FrameJson => write_json_line(response, &observation.frame_envelope()),
        ShowFormat::Vt | ShowFormat::Html => write_stdout(
            response,
            formatted.ok_or_else(|| miette::miette!("selected formatter produced no output"))?,
        ),
    };
}

fn write_observation_metadata(observation: &Observation) -> Result<()> {
    writeln!(
        io::stderr(),
        "source={} resolution={} state_hash={} presented={} quality={} truth={}",
        stable_enum_label(&observation.source_kind)?,
        stable_enum_label(&observation.resolution)?,
        observation.state_hash,
        observation.presented,
        stable_enum_label(&observation.capture_quality)?,
        stable_enum_label(&observation.truth_layer)?,
    )
    .into_diagnostic()?;

    return Ok(());
}
